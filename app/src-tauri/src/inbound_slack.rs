//! Reaction consumer and search catch-up for the canonical Deck Slack app.
//! The shared transport ACKs routed reaction envelopes before invoking this
//! consumer's Web API fetch. Search remains independent and runs every poll,
//! recovering reactions missed during sleep, disconnect or fetch failure.
//! Only the current user's configured badges become inbound events.

use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

use crate::datadir::now_epoch as now_secs;
use crate::inbound::{self, Config, Event, Source, SourceStatus};
use crate::keychain::{self, Slot};
use crate::slack_api::call;
#[cfg(test)]
use crate::slack_api::{encode, last_slack_error, manifest, setup_url, verify, USER_SCOPES};
#[cfg(test)]
use crate::slack_api::{TEST_API, TEST_API_LOCK};

const SEARCH_PAGE: u32 = 100;
const SEARCH_MAX_PAGES: u32 = 3;
const MAX_TEXT: usize = 16 * 1024;

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

/* ---------- routed live Reaction consumer ---------- */

/// Platform ACK is emitted by slack_transport before this fetch. A failure
/// here is recovered by the unchanged search.messages catch-up path.
pub(crate) fn handle_reaction(app: &AppHandle, value: &Value, self_id: &str, badges: &[String]) {
    let (.., hit) = parse_envelope(&value.to_string(), self_id, badges);
    let Some((channel, ts, badge)) = hit else {
        return;
    };
    let Some(user) = keychain::get(Slot::SlackUserToken) else {
        return;
    };
    let Ok(text) = message_text(&user, &channel, &ts) else {
        return;
    };
    let mut names = Names::default();
    let ev = Event {
        source: "slack".into(),
        key: format!("{channel}/{ts}"),
        badge,
        text: clip(&text),
        from: user_name(&user, &mut names, self_id),
        where_: channel_label(&user, &mut names, &channel),
        link: permalink(&user, &channel, &ts),
    };
    let cfg = inbound::read_config();
    inbound::offer(app, &cfg, vec![ev], true);
}

/* ---------- Source impl ---------- */

#[derive(Default)]
pub(crate) struct Slack {
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

    fn poll(&mut self, _cfg: &Config, badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let token = keychain::get(Slot::SlackUserToken).ok_or("no-token")?;
        // Search is a separate recovery path, but it shares the same installed
        // scope gate as live delivery. A token with partial scopes is not ready.
        crate::slack_api::verify(Slot::SlackUserToken, &token)?;
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

    fn set_live(&mut self, _app: &AppHandle, _wanted: bool, _badges: &[String]) {}

    fn status(&self) -> SourceStatus {
        SourceStatus {
            live: crate::slack_transport::connected(),
            last_poll: self.last_poll,
            last_error: self.last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::LockRecover;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    /// Point `call` at a port nothing listens on for the duration of `f`.
    fn with_offline<T>(f: impl FnOnce() -> T) -> T {
        let _serial = TEST_API_LOCK.lock_or_recover();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let unused = listener.local_addr().unwrap();
        drop(listener);
        *TEST_API.lock_or_recover() = Some(format!("http://{unused}/"));
        let value = f();
        *TEST_API.lock_or_recover() = None;
        value
    }

    /// Answer exactly `responses.len()` requests from a local fake Web API
    /// while `f` runs; returns `f`'s value and the raw requests received.
    fn with_responses<T>(responses: Vec<(u16, &str)>, f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let _serial = TEST_API_LOCK.lock_or_recover();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let responses: Vec<(u16, String)> = responses
            .into_iter()
            .map(|(status, body)| (status, body.to_string()))
            .collect();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&raw[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if raw.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                requests.push(String::from_utf8(raw).unwrap());
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    _ => "Server Error",
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nx-oauth-scopes: search:read,reactions:read,channels:history,groups:history,im:history,mpim:history,users:read,channels:read,groups:read,im:read,mpim:read\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        *TEST_API.lock_or_recover() = Some(base);
        let value = f();
        *TEST_API.lock_or_recover() = None;
        (value, worker.join().unwrap())
    }

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
        assert!(m.get("features").is_some(), "one app includes the bot");
        assert_eq!(
            m.pointer("/oauth_config/scopes/bot"),
            Some(&json!(["channels:history", "groups:history"]))
        );
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

    #[test]
    fn web_api_transport_errors_pagination_and_live_helpers_are_closed() {
        // Exercise the async IPC entry through a real HTTP failure. Verification
        // must run off the async executor and never reach the real Keychain.
        let (error, requests) = with_responses(
            vec![(200, r#"{"ok":false,"error":"invalid_auth"}"#)],
            || {
                tauri::async_runtime::block_on(inbound::inbound_set_secret(
                    "slack-user-token".into(),
                    "xoxp-test-invalid".into(),
                ))
                .unwrap_err()
            },
        );
        assert_eq!(error.message(), "auth");
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));

        let (body, requests) = with_responses(vec![(200, r#"{"ok":true,"value":7}"#)], || {
            call("auth.test", "secret", &[("query", "a b"), ("badge", "+1")]).unwrap()
        });
        assert_eq!(body["value"], 7);
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));
        assert!(requests[0].contains("authorization: Bearer secret\r\n"));
        assert!(requests[0].ends_with("query=a%20b&badge=%2B1"));

        let (errors, _) = with_responses(
            vec![
                (200, r#"{"ok":false,"error":"invalid_auth"}"#),
                (200, r#"{"ok":false,"error":"missing_scope"}"#),
                (200, r#"{"ok":false,"error":"ratelimited"}"#),
                (200, r#"{"ok":false,"error":"strange.error-42"}"#),
                (429, "{}"),
                (500, "{}"),
                (200, "not json"),
            ],
            || {
                [
                    call("one", "t", &[]).unwrap_err(),
                    call("two", "t", &[]).unwrap_err(),
                    call("three", "t", &[]).unwrap_err(),
                    call("four", "t", &[]).unwrap_err(),
                    call("five", "t", &[]).unwrap_err(),
                    call("six", "t", &[]).unwrap_err(),
                    call("seven", "t", &[]).unwrap_err(),
                ]
            },
        );
        assert_eq!(
            errors,
            [
                "auth",
                "scope",
                "ratelimited",
                "slack",
                "ratelimited",
                "http",
                "parse"
            ]
        );
        assert_eq!(last_slack_error(), "strangeerror");

        assert_eq!(with_offline(|| call("offline", "t", &[])), Err("network"));

        let (verified, _) = with_responses(
            vec![
                (200, r#"{"ok":true,"team_id":"T1"}"#),
                (200, r#"{"ok":true,"team_id":"T1"}"#),
            ],
            || {
                (
                    verify(Slot::SlackUserToken, "xoxp-test"),
                    verify(Slot::SlackBotToken, "xoxb-test"),
                )
            },
        );
        assert!(verified.0.is_ok() && verified.1.is_ok());

        let page_one = r#"{"ok":true,"messages":{"matches":[{"ts":"1.0","text":"first","username":"alice","channel":{"id":"C1","name":"dev"}}],"pagination":{"page_count":2}}}"#;
        let page_two = r#"{"ok":true,"messages":{"matches":[{"ts":"2.0","text":"second","user":"U2","channel":{"id":"C2","name":"ops"}}],"pagination":{"page_count":2}}}"#;
        let (events, requests) = with_responses(vec![(200, page_one), (200, page_two)], || {
            search_badge("xoxp-test", "deck").unwrap()
        });
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].key, "C1/1.0");
        assert_eq!(events[1].key, "C2/2.0");
        assert!(requests[0].contains("page=1"));
        assert!(requests[1].contains("page=2"));
        assert!(requests[0].contains("query=hasmy%3A%3Adeck%3A%20after%3A"));

        let responses = vec![
            (
                200,
                r#"{"ok":true,"user":{"profile":{"display_name":""},"real_name":"Alice","name":"alice"}}"#,
            ),
            (200, r#"{"ok":true,"channel":{"id":"C1","name":"dev"}}"#),
            (
                200,
                r#"{"ok":true,"messages":[{"ts":"1.0","text":"root"}]}"#,
            ),
            (200, r#"{"ok":true,"messages":[]}"#),
            (
                200,
                r#"{"ok":true,"messages":[{"ts":"2.0","text":"reply"}],"response_metadata":{"next_cursor":""}}"#,
            ),
            (
                200,
                r#"{"ok":true,"permalink":"https://example.slack.com/p2"}"#,
            ),
        ];
        let (resolved, requests) = with_responses(responses, || {
            let mut names = Names::default();
            let user = user_name("xoxp-test", &mut names, "U1");
            let cached_user = user_name("no-network", &mut names, "U1");
            let channel = channel_label("xoxp-test", &mut names, "C1");
            let cached_channel = channel_label("no-network", &mut names, "C1");
            (
                user,
                cached_user,
                channel,
                cached_channel,
                message_text("xoxp-test", "C1", "1.0"),
                message_text("xoxp-test", "C1", "2.0"),
                permalink("xoxp-test", "C1", "2.0"),
            )
        });
        assert_eq!(resolved.0, "Alice");
        assert_eq!(resolved.1, "Alice");
        assert_eq!(resolved.2, "#dev");
        assert_eq!(resolved.3, "#dev");
        assert_eq!(resolved.4, Ok("root".into()));
        assert_eq!(resolved.5, Ok("reply".into()));
        assert_eq!(resolved.6, "https://example.slack.com/p2");
        assert_eq!(requests.len(), 6, "cached names issue no second request");

        assert_eq!(where_label(&json!({"is_im": true})), "DM");
        assert_eq!(where_label(&json!({"is_mpim": true})), "group DM");
        assert_eq!(where_label(&json!({"id": "C9"})), "C9");
        assert_eq!(where_label(&Value::Null), "?");

        let slack = Slack::default();
        assert_eq!(slack.id(), "slack");
        assert!(!slack.enabled(&Config::default()));
        let status = slack.status();
        assert!(!status.live);
        assert!(status.last_poll.is_none());
        assert!(status.last_error.is_none());
        assert!(now_secs() > 0);
    }

    #[test]
    fn verify_probes_each_slot_with_the_call_it_can_make() {
        let (verified, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"team_id":"T1"}"#),
                (200, r#"{"ok":true,"team_id":"T1"}"#),
            ],
            || {
                (
                    verify(Slot::SlackBotToken, "xoxb-test"),
                    verify(Slot::SlackUserToken, "xoxp-test"),
                )
            },
        );
        assert!(verified.0.is_ok() && verified.1.is_ok());
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));
        assert!(requests[0].contains("authorization: Bearer xoxb-test\r\n"));
        assert!(requests[1].starts_with("POST /auth.test HTTP/1.1\r\n"));
        // the Connector identity is not a Slack token: no request is made
        assert_eq!(
            with_offline(|| verify(Slot::ConnectorIdentity, "v1_identity")),
            Err("slot")
        );
    }

    #[test]
    fn live_helpers_fall_back_to_identifiers_and_bound_thread_paging() {
        // a reply that is in none of the first three thread pages is given up
        // on, with the cursor threaded through every follow-up request
        let page = |cursor: &str| {
            format!(
                r#"{{"ok":true,"messages":[{{"ts":"9.9","text":"other"}}],"response_metadata":{{"next_cursor":"{cursor}"}}}}"#
            )
        };
        let (pages, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"messages":[]}"#),
                (200, &page("c1")),
                (200, &page("c2")),
                (200, &page("c3")),
            ],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(pages, Err("slack"));
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("POST /conversations.history "));
        assert!(requests[1].starts_with("POST /conversations.replies "));
        assert!(
            !requests[1].contains("cursor="),
            "the first page has no cursor"
        );
        assert!(requests[2].ends_with("cursor=c1"));
        assert!(requests[3].ends_with("cursor=c2"));
        // an empty cursor ends the walk early, and a Slack error propagates
        let (short, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"messages":[]}"#),
                (200, r#"{"ok":true,"messages":[]}"#),
            ],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(short, Err("slack"));
        assert_eq!(requests.len(), 2);
        let (denied, _) = with_responses(
            vec![(200, r#"{"ok":false,"error":"missing_scope"}"#)],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(denied, Err("scope"));

        // name lookups degrade to the raw identifier, and are cached as such
        let (fallbacks, requests) = with_responses(
            vec![
                (500, "{}"),
                (200, r#"{"ok":true,"user":{"profile":{"display_name":""}}}"#),
                (200, r#"{"ok":false,"error":"channel_not_found"}"#),
                (200, r#"{"ok":false,"error":"invalid_auth"}"#),
            ],
            || {
                let mut names = Names::default();
                (
                    user_name("xoxp-test", &mut names, "U404"),
                    user_name("xoxp-test", &mut names, "U_NAMELESS"),
                    channel_label("xoxp-test", &mut names, "C404"),
                    permalink("xoxp-test", "C404", "1.0"),
                    user_name("xoxp-test", &mut names, "U404"),
                    channel_label("xoxp-test", &mut names, "C404"),
                )
            },
        );
        assert_eq!(fallbacks.0, "U404");
        assert_eq!(fallbacks.1, "U_NAMELESS", "no usable name field");
        assert_eq!(fallbacks.2, "C404");
        assert_eq!(
            fallbacks.3, "",
            "no permalink is an empty link, never an error"
        );
        assert_eq!(
            (fallbacks.4.as_str(), fallbacks.5.as_str()),
            ("U404", "C404")
        );
        assert_eq!(requests.len(), 4, "the fallback is cached like a hit");

        assert!(events_from_search(&json!({"ok": true}), "deck").is_empty());
        assert!(
            events_from_search(&json!({"ok": true, "messages": {"matches": []}}), "deck")
                .is_empty()
        );
        let no_item = json!({"envelope_id": "E9", "type": "events_api", "payload": {"event": {
            "type": "reaction_added", "user": "U_ME", "reaction": "deck",
            "item": {"type": "message", "ts": "1.2"}}}})
        .to_string();
        assert_eq!(
            parse_envelope(&no_item, "U_ME", &["deck".to_string()]),
            (Some("E9".into()), None),
            "a hit without a channel is acknowledged and dropped"
        );
        let no_event =
            json!({"envelope_id": "E8", "type": "events_api", "payload": {}}).to_string();
        assert_eq!(
            parse_envelope(&no_event, "U_ME", &["deck".to_string()]),
            (Some("E8".into()), None)
        );
        assert!(
            !Slack::default().polled_events_are_live(),
            "polled badges go through the first-poll baseline"
        );
    }
}
