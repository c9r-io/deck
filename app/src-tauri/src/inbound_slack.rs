// inbound_slack.rs — the Slack source for `inbound`.
//
// Two paths, one token, one Event:
// - catch-up: every poll runs `search.messages` with `hasmy::<badge>:` for
//   each badge a rule names. The result is exactly the set of messages the
//   user reacted to with that emoji — never their other reactions — inside
//   a bounded lookback window. Slack's search index lags a fresh reaction by
//   about a minute, which is why this path is the safety net, not the ear.
// - live: a Socket Mode connection (app-level token) subscribed to the USER
//   event `reaction_added`. Reactions by anyone in the user's channels
//   arrive; only the user's own reactions with a ruled badge are fetched
//   and turned into events. Slack retries an undelivered event only a few
//   times, so anything missed while disconnected is left to the catch-up.
//
// Credentials come from the Keychain per request and are never cached in
// a struct field, logged or placed in an error string. Every failure maps
// to a closed code; the raw Slack error and any URL stay inside this module.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

use crate::inbound::{self, Config, Event, Source, SourceStatus};
use crate::keychain::{self, Slot};
use crate::storage::applog;

const API: &str = "https://slack.com/api/";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const SEARCH_PAGE: u32 = 100;
const SEARCH_MAX_PAGES: u32 = 3;
const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_TEXT: usize = 16 * 1024;

/* ---------- HTTP ---------- */

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // reqwest is built with `rustls-no-provider`; the updater installs
        // ring lazily too. Installing twice is harmless (the second fails).
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent("deck")
            .build()
            .expect("reqwest client")
    })
}

/// Slack's own error names are a closed, lowercase vocabulary. Keep the
/// last one seen (bounded, charset-checked) so a verification failure can
/// name it in the log and the toast without ever carrying content.
static LAST_SLACK_ERROR: Mutex<String> = Mutex::new(String::new());

pub(crate) fn last_slack_error() -> String {
    LAST_SLACK_ERROR
        .lock()
        .map(|e| e.clone())
        .unwrap_or_default()
}

fn note_slack_error(name: &str) {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == '_')
        .take(48)
        .collect();
    if let Ok(mut e) = LAST_SLACK_ERROR.lock() {
        *e = clean;
    }
}

/// One Slack Web API call — always POST with a form body, as every method
/// documents. `Err` is a closed code suitable for logs.
fn call(method: &str, token: &str, params: &[(&str, &str)]) -> Result<Value, &'static str> {
    let form: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    let req = client()
        .post(format!("{API}{method}"))
        .bearer_auth(token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form.join("&"));
    let body: Value = tauri::async_runtime::block_on(async move {
        let resp = req
            .send()
            .await
            .map_err(|e| if e.is_timeout() { "timeout" } else { "network" })?;
        if resp.status().as_u16() == 429 {
            return Err("ratelimited");
        }
        if !resp.status().is_success() {
            return Err("http");
        }
        resp.json::<Value>().await.map_err(|_| "parse")
    })?;
    if body.get("ok").and_then(Value::as_bool) != Some(true) {
        let name = body.get("error").and_then(Value::as_str).unwrap_or("");
        note_slack_error(name);
        return Err(match name {
            "invalid_auth" | "not_authed" | "token_revoked" | "token_expired"
            | "account_inactive" => "auth",
            "missing_scope" => "scope",
            "ratelimited" => "ratelimited",
            _ => "slack",
        });
    }
    Ok(body)
}

/// RFC 3986 percent-encoding of one query component (unreserved bytes pass).
pub(crate) fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `YYYY-MM-DD` for Slack's `after:` modifier, `days` back from now (UTC).
pub(crate) fn after_date(now: u64, days: u64) -> String {
    let secs = now.saturating_sub(days * 86_400);
    let days_since_epoch = (secs / 86_400) as i64;
    // civil-from-days (Howard Hinnant), no chrono dependency
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Slack mrkdwn → plain text a prompt can carry: `<@U1|alice>` → `@alice`,
/// `<#C1|dev>` → `#dev`, `<https://x|label>` → `label (https://x)`,
/// `<https://x>` → `https://x`, HTML entities unescaped. Nothing else is
/// interpreted; formatting marks stay as typed.
pub(crate) fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) => (t, Some(l)),
            None => (inner, None),
        };
        if let Some(id) = target.strip_prefix('@') {
            out.push('@');
            out.push_str(label.unwrap_or(id));
        } else if let Some(id) = target.strip_prefix('#') {
            out.push('#');
            out.push_str(label.unwrap_or(id));
        } else if let Some(id) = target.strip_prefix('!') {
            out.push('@');
            out.push_str(label.unwrap_or(id));
        } else if target.contains("://") || target.starts_with("mailto:") {
            match label {
                Some(l) if !l.is_empty() && l != target => {
                    out.push_str(l);
                    out.push_str(" (");
                    out.push_str(target);
                    out.push(')');
                }
                _ => out.push_str(target),
            }
        } else {
            out.push('<');
            out.push_str(inner);
            out.push('>');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn clip(s: &str) -> String {
    let s: String = plain_text(s)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    if s.len() <= MAX_TEXT {
        return s;
    }
    let mut end = MAX_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// A reaction name as the API spells it, minus any skin-tone suffix.
pub(crate) fn plain_badge(reaction: &str) -> &str {
    reaction.split("::").next().unwrap_or(reaction)
}

fn where_label(channel: &Value) -> String {
    let is_im = channel
        .get("is_im")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_mpim = channel
        .get("is_mpim")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = channel.get("name").and_then(Value::as_str).unwrap_or("");
    if is_im
        || name.starts_with('D')
            && name.len() > 8
            && name.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        "DM".to_string()
    } else if is_mpim || name.starts_with("mpdm-") {
        "group DM".to_string()
    } else if name.is_empty() {
        channel
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    } else {
        format!("#{name}")
    }
}

/// Build the events one `search.messages` response page yields.
pub(crate) fn events_from_search(body: &Value, badge: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let Some(matches) = body.pointer("/messages/matches").and_then(Value::as_array) else {
        return out;
    };
    for m in matches {
        let (Some(ts), Some(channel)) = (
            m.get("ts").and_then(Value::as_str),
            m.pointer("/channel/id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        let from = m
            .get("username")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| m.get("user").and_then(Value::as_str))
            .unwrap_or("?");
        out.push(Event {
            source: "slack".into(),
            key: format!("{channel}/{ts}"),
            badge: badge.to_string(),
            text: clip(text),
            from: from.to_string(),
            where_: where_label(m.get("channel").unwrap_or(&Value::Null)),
            link: m
                .get("permalink")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    out
}

fn search_badge(token: &str, badge: &str) -> Result<Vec<Event>, &'static str> {
    let query = format!(
        "hasmy::{badge}: after:{}",
        after_date(now_secs(), inbound::LOOKBACK_DAYS)
    );
    let count = SEARCH_PAGE.to_string();
    let mut events = Vec::new();
    let mut page = 1u32;
    loop {
        let p = page.to_string();
        let body = call(
            "search.messages",
            token,
            &[
                ("query", &query),
                ("sort", "timestamp"),
                ("sort_dir", "desc"),
                ("count", &count),
                ("page", &p),
            ],
        )?;
        events.extend(events_from_search(&body, badge));
        let pages = body
            .pointer("/messages/pagination/page_count")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        page += 1;
        if page > pages || page > SEARCH_MAX_PAGES {
            break;
        }
    }
    Ok(events)
}

/* ---------- setup: one prefilled "Create an app" page ---------- */

/// The user scopes both paths need. Kept in one place so the manifest, the
/// Settings hint and the docs cannot drift apart.
pub(crate) const USER_SCOPES: &[&str] = &[
    "search:read",
    "reactions:read",
    "channels:history",
    "groups:history",
    "im:history",
    "mpim:history",
    "users:read",
    "channels:read",
    "groups:read",
    "im:read",
    "mpim:read",
];

/// Slack's manifest for a personal, user-scoped, Socket Mode app: no bot
/// user, no public URL, one user event. `apps?new_app=1&manifest_json=…`
/// opens the Create page prefilled; the user only picks a workspace.
/// Tokens still have to be copied back by hand — Slack offers no OAuth
/// redirect to a local app and no API that mints app-level tokens.
pub(crate) fn manifest() -> Value {
    serde_json::json!({
        "display_information": {
            "name": "deck",
            "description": "Badges you add in Slack start sessions in deck on your Mac.",
            "background_color": "#101318"
        },
        "oauth_config": { "scopes": { "user": USER_SCOPES } },
        "settings": {
            "socket_mode_enabled": true,
            "event_subscriptions": { "user_events": ["reaction_added"] },
            "org_deploy_enabled": false,
            "token_rotation_enabled": false
        }
    })
}

pub(crate) fn setup_url() -> String {
    format!(
        "https://api.slack.com/apps?new_app=1&manifest_json={}",
        encode(&manifest().to_string())
    )
}

/// Prove a pasted token is the right kind and alive before it is stored:
/// `auth.test` for the user token, `apps.connections.open` for the
/// app-level token (the only call it can make). Returns a closed code.
pub(crate) fn verify(slot: Slot, value: &str) -> Result<(), &'static str> {
    match slot {
        Slot::SlackUserToken => call("auth.test", value, &[]).map(|_| ()),
        Slot::SlackAppToken => call("apps.connections.open", value, &[]).map(|_| ()),
    }
}

/* ---------- live path helpers (need the user token) ---------- */

#[derive(Default)]
struct Names {
    users: HashMap<String, String>,
    channels: HashMap<String, String>,
}

fn user_name(token: &str, names: &mut Names, id: &str) -> String {
    if let Some(n) = names.users.get(id) {
        return n.clone();
    }
    let name = call("users.info", token, &[("user", id)])
        .ok()
        .and_then(|b| {
            let u = b.get("user")?;
            let pick = |p: &str| {
                u.pointer(p)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            pick("/profile/display_name")
                .or_else(|| pick("/real_name"))
                .or_else(|| pick("/name"))
        })
        .unwrap_or_else(|| id.to_string());
    names.users.insert(id.to_string(), name.clone());
    name
}

fn channel_label(token: &str, names: &mut Names, id: &str) -> String {
    if let Some(n) = names.channels.get(id) {
        return n.clone();
    }
    let label = call("conversations.info", token, &[("channel", id)])
        .ok()
        .and_then(|b| b.get("channel").map(where_label))
        .unwrap_or_else(|| id.to_string());
    names.channels.insert(id.to_string(), label.clone());
    label
}

fn message_text(token: &str, channel: &str, ts: &str) -> Result<String, &'static str> {
    let body = call(
        "conversations.history",
        token,
        &[
            ("channel", channel),
            ("latest", ts),
            ("oldest", ts),
            ("inclusive", "true"),
            ("limit", "1"),
        ],
    )?;
    if let Some(m) = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|m| m.get("ts").and_then(Value::as_str) == Some(ts))
        })
    {
        return Ok(m
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string());
    }
    // A thread reply is only reachable through the thread.
    let mut cursor = String::new();
    for _ in 0..3 {
        let mut params: Vec<(&str, &str)> =
            vec![("channel", channel), ("ts", ts), ("limit", "200")];
        if !cursor.is_empty() {
            params.push(("cursor", &cursor));
        }
        let body = call("conversations.replies", token, &params)?;
        if let Some(m) = body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|m| m.get("ts").and_then(Value::as_str) == Some(ts))
            })
        {
            return Ok(m
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string());
        }
        cursor = body
            .pointer("/response_metadata/next_cursor")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if cursor.is_empty() {
            break;
        }
    }
    Err("slack")
}

fn permalink(token: &str, channel: &str, ts: &str) -> String {
    call(
        "chat.getPermalink",
        token,
        &[("channel", channel), ("message_ts", ts)],
    )
    .ok()
    .and_then(|b| {
        b.get("permalink")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
    .unwrap_or_default()
}

/// Parse one Socket Mode envelope. Returns (envelope_id, own reaction hit).
pub(crate) fn parse_envelope(
    text: &str,
    self_id: &str,
    badges: &[String],
) -> (Option<String>, Option<(String, String, String)>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return (None, None);
    };
    let envelope = v
        .get("envelope_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if v.get("type").and_then(Value::as_str) != Some("events_api") {
        return (envelope, None);
    }
    let Some(ev) = v.pointer("/payload/event") else {
        return (envelope, None);
    };
    if ev.get("type").and_then(Value::as_str) != Some("reaction_added") {
        return (envelope, None);
    }
    if ev.get("user").and_then(Value::as_str) != Some(self_id) {
        return (envelope, None);
    }
    if ev.pointer("/item/type").and_then(Value::as_str) != Some("message") {
        return (envelope, None);
    }
    let badge = plain_badge(ev.get("reaction").and_then(Value::as_str).unwrap_or(""));
    if !badges.iter().any(|b| b == badge) {
        return (envelope, None);
    }
    let (Some(channel), Some(ts)) = (
        ev.pointer("/item/channel").and_then(Value::as_str),
        ev.pointer("/item/ts").and_then(Value::as_str),
    ) else {
        return (envelope, None);
    };
    (
        envelope,
        Some((channel.to_string(), ts.to_string(), badge.to_string())),
    )
}

/* ---------- the socket thread ---------- */

struct Live {
    /// Bumped to retire a running thread; the thread exits when it no
    /// longer matches. Cheap, lock-free, and survives a stuck read because
    /// the read has a timeout.
    epoch: Arc<AtomicU64>,
    badges: Arc<Mutex<Vec<String>>>,
    connected: Arc<Mutex<bool>>,
    running: bool,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            epoch: Arc::new(AtomicU64::new(0)),
            badges: Arc::new(Mutex::new(Vec::new())),
            connected: Arc::new(Mutex::new(false)),
            running: false,
        }
    }
}

fn socket_loop(
    app: AppHandle,
    epoch: Arc<AtomicU64>,
    my_epoch: u64,
    badges: Arc<Mutex<Vec<String>>>,
    connected: Arc<Mutex<bool>>,
) {
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::Message;
    let mut backoff = 1u64;
    let mut names = Names::default();
    let mut failures = 0u32;
    while epoch.load(Ordering::SeqCst) == my_epoch {
        let (Some(user), Some(app_token)) = (
            keychain::get(Slot::SlackUserToken),
            keychain::get(Slot::SlackAppToken),
        ) else {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        };
        let attempt = (|| -> Result<(), &'static str> {
            let self_id = call("auth.test", &user, &[])?
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or("parse")?;
            let url = call("apps.connections.open", &app_token, &[])?
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or("parse")?;
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
            let (mut ws, _) = tungstenite::connect(url.as_str()).map_err(|_| "socket")?;
            if let MaybeTlsStream::Rustls(s) = ws.get_mut() {
                let _ = s.get_mut().set_read_timeout(Some(SOCKET_READ_TIMEOUT));
            }
            *connected.lock().unwrap_or_else(|p| p.into_inner()) = true;
            applog("[inbound] slack live connected");
            backoff = 1;
            let mut idle = 0u32;
            let result = loop {
                if epoch.load(Ordering::SeqCst) != my_epoch {
                    let _ = ws.close(None);
                    break Ok(());
                }
                match ws.read() {
                    Ok(Message::Text(t)) => {
                        idle = 0;
                        let current = badges.lock().unwrap_or_else(|p| p.into_inner()).clone();
                        let (envelope, hit) = parse_envelope(&t, &self_id, &current);
                        if let Some(id) = envelope {
                            let ack = serde_json::json!({ "envelope_id": id }).to_string();
                            if ws.send(Message::Text(ack.into())).is_err() {
                                break Err("socket");
                            }
                        }
                        if t.contains("\"disconnect\"") && t.contains("\"type\"") {
                            if let Ok(v) = serde_json::from_str::<Value>(&t) {
                                if v.get("type").and_then(Value::as_str) == Some("disconnect") {
                                    break Err("reconnect");
                                }
                            }
                        }
                        if let Some((channel, ts, badge)) = hit {
                            match message_text(&user, &channel, &ts) {
                                Ok(text) => {
                                    let ev = Event {
                                        source: "slack".into(),
                                        key: format!("{channel}/{ts}"),
                                        badge,
                                        text: clip(&text),
                                        from: user_name(&user, &mut names, &self_id),
                                        where_: channel_label(&user, &mut names, &channel),
                                        link: permalink(&user, &channel, &ts),
                                    };
                                    let cfg = inbound::read_config();
                                    inbound::offer(&app, &cfg, vec![ev], true);
                                }
                                Err(code) => {
                                    failures = failures.saturating_add(1);
                                    if failures <= 20 {
                                        applog(&format!(
                                            "[inbound] slack live fetch FAILED ({code})"
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                        idle = 0;
                        let _ = ws.flush();
                    }
                    Ok(Message::Close(_)) => break Err("closed"),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        idle += 1;
                        if idle > 1 || ws.send(Message::Ping(Vec::new().into())).is_err() {
                            break Err("stalled");
                        }
                    }
                    Err(_) => break Err("socket"),
                }
            };
            *connected.lock().unwrap_or_else(|p| p.into_inner()) = false;
            result
        })();
        *connected.lock().unwrap_or_else(|p| p.into_inner()) = false;
        match attempt {
            Ok(()) => {}
            Err(code) => {
                if code != "reconnect" {
                    applog(&format!(
                        "[inbound] slack live dropped ({code}); retry in {backoff}s"
                    ));
                }
                if epoch.load(Ordering::SeqCst) != my_epoch {
                    break;
                }
                let wait = if code == "reconnect" { 1 } else { backoff };
                std::thread::sleep(Duration::from_secs(wait));
                backoff = (backoff * 2).min(120);
            }
        }
    }
    applog("[inbound] slack live stopped");
}

/* ---------- Source impl ---------- */

#[derive(Default)]
pub(crate) struct Slack {
    live: Live,
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
}

impl Source for Slack {
    fn id(&self) -> &'static str {
        "slack"
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.slack_enabled && keychain::has(Slot::SlackUserToken)
    }

    fn poll(&mut self, badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let token = keychain::get(Slot::SlackUserToken).ok_or("no-token")?;
        let mut all = Vec::new();
        let mut result = Ok(());
        for badge in badges {
            match search_badge(&token, badge) {
                Ok(events) => all.extend(events),
                Err(code) => {
                    result = Err(code);
                    if code == "auth" || code == "ratelimited" {
                        break;
                    }
                }
            }
        }
        self.last_poll = Some(now_secs());
        self.last_error = result.err();
        result.map(|_| all)
    }

    fn set_live(&mut self, app: &AppHandle, wanted: bool, badges: &[String]) {
        *self.live.badges.lock().unwrap_or_else(|p| p.into_inner()) = badges.to_vec();
        let can = wanted && keychain::has(Slot::SlackAppToken);
        if can && !self.live.running {
            let my_epoch = self.live.epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let (app, epoch, badges, connected) = (
                app.clone(),
                self.live.epoch.clone(),
                self.live.badges.clone(),
                self.live.connected.clone(),
            );
            std::thread::spawn(move || socket_loop(app, epoch, my_epoch, badges, connected));
            self.live.running = true;
        } else if !can && self.live.running {
            self.live.epoch.fetch_add(1, Ordering::SeqCst);
            self.live.running = false;
        }
    }

    fn status(&self) -> SourceStatus {
        SourceStatus {
            live: *self
                .live
                .connected
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            last_poll: self.last_poll,
            last_error: self.last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn after_date_is_civil_utc() {
        // 2026-09-04 07:00 UTC
        assert_eq!(after_date(1_788_505_200, 0), "2026-09-04");
        assert_eq!(after_date(1_788_505_200, 30), "2026-08-05");
        assert_eq!(after_date(0, 0), "1970-01-01");
        assert_eq!(after_date(951_782_400, 0), "2000-02-29");
    }

    #[test]
    fn query_components_are_percent_encoded() {
        assert_eq!(
            encode("hasmy::deck: after:2026-08-05"),
            "hasmy%3A%3Adeck%3A%20after%3A2026-08-05"
        );
        assert_eq!(encode("+1"), "%2B1");
        assert_eq!(encode("a_b-c.d~e"), "a_b-c.d~e");
    }

    #[test]
    fn setup_link_carries_the_whole_manifest_and_nothing_secret() {
        let url = setup_url();
        assert!(url.starts_with("https://api.slack.com/apps?new_app=1&manifest_json=%7B"));
        let encoded = url.split("manifest_json=").nth(1).unwrap();
        let decoded: String = {
            let bytes = encoded.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    out.push(
                        u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16)
                            .unwrap(),
                    );
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        };
        let m: Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(m, manifest());
        assert_eq!(
            m.pointer("/settings/socket_mode_enabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            m.pointer("/settings/event_subscriptions/user_events/0")
                .and_then(Value::as_str),
            Some("reaction_added")
        );
        let scopes = m
            .pointer("/oauth_config/scopes/user")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(scopes.len(), USER_SCOPES.len());
        assert!(m.get("features").is_none(), "no bot user");
        assert!(m.pointer("/oauth_config/scopes/bot").is_none());
        assert!(m.pointer("/oauth_config/redirect_urls").is_none());
    }

    #[test]
    fn badge_names_drop_skin_tone() {
        assert_eq!(plain_badge("+1::skin-tone-2"), "+1");
        assert_eq!(plain_badge("deck"), "deck");
    }

    #[test]
    fn search_matches_become_events_with_source_neutral_fields() {
        let body = json!({"ok": true, "messages": {"matches": [
            {"ts": "1.5", "text": "fix the login flicker\nplease", "user": "U1", "username": "alice",
             "channel": {"id": "C1", "name": "frontend"}, "permalink": "https://x.slack.com/archives/C1/p15"},
            {"ts": "2.5", "text": "hi", "user": "U2", "channel": {"id": "D9", "name": "D9ABCDEFGH", "is_im": true}},
            {"text": "no ts"},
            {"ts": "3.5", "text": "grp", "channel": {"id": "G1", "name": "mpdm-a--b--c-1", "is_mpim": true}}
        ]}});
        let ev = events_from_search(&body, "deck");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0].key, "C1/1.5");
        assert_eq!(ev[0].badge, "deck");
        assert_eq!(ev[0].from, "alice");
        assert_eq!(ev[0].where_, "#frontend");
        assert_eq!(ev[0].link, "https://x.slack.com/archives/C1/p15");
        assert_eq!(ev[0].text, "fix the login flicker\nplease");
        assert_eq!(ev[1].from, "U2", "falls back to the user id");
        assert_eq!(ev[1].where_, "DM");
        assert_eq!(ev[2].where_, "group DM");
        let json = serde_json::to_value(&ev[0]).unwrap();
        assert!(json.get("where").is_some() && json.get("where_").is_none());
    }

    #[test]
    fn mrkdwn_becomes_plain_text() {
        assert_eq!(
            plain_text("<@U1|alice> see <#C1|dev> and <https://x.y/z|the doc> or <https://a.b>"),
            "@alice see #dev and the doc (https://x.y/z) or https://a.b"
        );
        assert_eq!(plain_text("<@U1>"), "@U1");
        assert_eq!(plain_text("<!here> a &lt;b&gt; &amp; c"), "@here a <b> & c");
        assert_eq!(plain_text("x < y > z"), "x < y > z");
        assert_eq!(plain_text("unterminated <@U1"), "unterminated <@U1");
    }

    #[test]
    fn text_is_clipped_and_stripped_of_control_bytes() {
        assert_eq!(clip("a\u{7}b\nc\td"), "ab\nc\td");
        let long = "é".repeat(MAX_TEXT);
        let c = clip(&long);
        assert!(c.len() <= MAX_TEXT && c.chars().all(|ch| ch == 'é'));
    }

    #[test]
    fn envelopes_are_filtered_to_own_ruled_message_reactions() {
        let badges = vec!["deck".to_string()];
        let mk = |user: &str, reaction: &str, item_type: &str| {
            json!({"envelope_id": "E1", "type": "events_api", "payload": {"event": {
                "type": "reaction_added", "user": user, "reaction": reaction,
                "item": {"type": item_type, "channel": "C1", "ts": "1.2"}}}})
            .to_string()
        };
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck", "message"), "U_ME", &badges),
            (
                Some("E1".into()),
                Some(("C1".into(), "1.2".into(), "deck".into()))
            )
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck::skin-tone-3", "message"), "U_ME", &badges)
                .1
                .map(|h| h.2),
            Some("deck".into())
        );
        assert_eq!(
            parse_envelope(&mk("U_OTHER", "deck", "message"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "eyes", "message"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck", "file"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(r#"{"type":"hello"}"#, "U_ME", &badges),
            (None, None)
        );
        assert_eq!(
            parse_envelope(
                r#"{"envelope_id":"E2","type":"slash_commands"}"#,
                "U_ME",
                &badges
            ),
            (Some("E2".into()), None)
        );
        assert_eq!(parse_envelope("not json", "U_ME", &badges), (None, None));
    }
}
