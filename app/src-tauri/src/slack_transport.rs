//! The sole Slack Socket Mode owner. Consumers never access the WebSocket.
//! Status reads never request a Socket ticket; only explicit App-token saves
//! and actual transport connections call apps.connections.open.
//! A Reaction envelope is ACKed before its recoverable Web API fetch; a
//! matching channel message is ACKed only after durable inbox staging.
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::inbound;
use crate::inbound_channel;
use crate::inbound_slack;
use crate::keychain::{self, Slot};
use crate::slack_api;
use crate::sync::LockRecover;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::AppHandle;
use tungstenite::Message;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SlackConnectionStatus {
    user_present: bool,
    bot_present: bool,
    app_present: bool,
    legacy_present: bool,
    user_valid: bool,
    bot_valid: bool,
    app_valid: bool,
    user_error: Option<&'static str>,
    bot_error: Option<&'static str>,
    app_error: Option<&'static str>,
    workspace_match: bool,
    workspace: Option<String>,
    connected: bool,
    reaction_enabled: bool,
    channel_enabled: bool,
    channel_rules: usize,
}

#[tauri::command]
pub(crate) async fn slack_connection_status() -> Result<SlackConnectionStatus, &'static str> {
    tauri::async_runtime::spawn_blocking(|| {
        let user_present = keychain::has(Slot::SlackUserToken);
        let bot_present = keychain::has(Slot::SlackBotToken);
        let app_present = keychain::has(Slot::SlackAppToken);
        let legacy_present =
            keychain::has(Slot::SlackChannelBotToken) || keychain::has(Slot::SlackChannelAppToken);
        let user = keychain::get(Slot::SlackUserToken)
            .map(|t| slack_api::verify(Slot::SlackUserToken, &t));
        let bot =
            keychain::get(Slot::SlackBotToken).map(|t| slack_api::verify(Slot::SlackBotToken, &t));
        // Presence, a successful save/open in this process, and an active
        // socket are separate facts. A restart cannot assert App validity
        // until the transport connects or the user saves the token again.
        let app_valid = app_valid_fact(
            app_present,
            APP_VERIFIED.load(Ordering::SeqCst),
            connected(),
        );
        let (user_valid, user_error) = match &user {
            Some(Ok(_)) => (true, None),
            Some(Err(e)) => (false, Some(*e)),
            None => (false, None),
        };
        let (bot_valid, bot_error) = match &bot {
            Some(Ok(_)) => (true, None),
            Some(Err(e)) => (false, Some(*e)),
            None => (false, None),
        };
        let app_error = None;
        let workspace_match = match (&user, &bot) {
            (Some(Ok(u)), Some(Ok(b))) => workspace_match(u, b),
            (Some(_), Some(_)) => false,
            _ => true,
        };
        let workspace = user
            .as_ref()
            .or(bot.as_ref())
            .and_then(|v| v.as_ref().ok())
            .and_then(|v| v.get("team"))
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 120)
            .map(str::to_string);
        let cfg = inbound::read_config();
        let channel = inbound_channel::read_config();
        SlackConnectionStatus {
            user_present,
            bot_present,
            app_present,
            legacy_present,
            user_valid,
            bot_valid,
            app_valid,
            user_error,
            bot_error,
            app_error,
            workspace_match,
            workspace,
            connected: connected(),
            reaction_enabled: cfg.slack_enabled,
            channel_enabled: channel.connection.enabled,
            channel_rules: channel.rules.len(),
        }
    })
    .await
    .map_err(|_| "worker")
}

// Closed, argument-free operation: neither the webview nor a caller may name
// another Keychain slot. Attempt both deletions even if the first fails.
#[tauri::command]
pub(crate) async fn slack_legacy_credentials_clear() -> Result<(), DeckError> {
    tauri::async_runtime::spawn_blocking(|| {
        clear_legacy_with(keychain::has, keychain::clear)
            .map_err(|_| DeckError::new(ErrorKind::Other, "keychain"))
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "credential worker failed"))?
}

fn clear_legacy_with(
    has: impl Fn(Slot) -> bool,
    clear: impl Fn(Slot) -> Result<(), DeckError>,
) -> Result<(), &'static str> {
    let slots = [Slot::SlackChannelBotToken, Slot::SlackChannelAppToken];
    let mut failed = false;
    for slot in slots {
        if has(slot) && clear(slot).is_err() {
            failed = true;
        }
    }
    // Re-read the actual remaining presence. A partial deletion stays visible
    // and the same command can be safely retried.
    if slots.into_iter().any(has) || failed {
        Err("keychain")
    } else {
        Ok(())
    }
}

static CONNECTED: AtomicBool = AtomicBool::new(false);
static APP_VERIFIED: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(1);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
const MAX_SOCKET_TEXT: usize = 1024 * 1024;

fn app_valid_fact(present: bool, verified_this_process: bool, connected: bool) -> bool {
    present && (verified_this_process || connected)
}

pub(crate) fn connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}
pub(crate) fn app_credential_saved(present: bool) {
    APP_VERIFIED.store(present, Ordering::SeqCst);
}
fn startup_plan(
    reaction_enabled: bool,
    badges: usize,
    user_present: bool,
    channel_enabled: bool,
    active_channel_rule: bool,
    bot_present: bool,
) -> (bool, bool) {
    (
        reaction_enabled && badges > 0 && user_present,
        channel_enabled && active_channel_rule && bot_present,
    )
}
pub(crate) fn wake() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
    let (flag, cv) = &WAKE;
    *flag.lock_or_recover() = true;
    cv.notify_all();
}
fn wait(d: Duration) {
    let (flag, cv) = &WAKE;
    let guard = flag.lock_or_recover();
    let (mut guard, _) = cv
        .wait_timeout_while(guard, d, |v| !*v)
        .unwrap_or_else(|e| e.into_inner());
    *guard = false;
}

pub(crate) fn confined_socket_url(raw: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    url.scheme() == "wss"
        && matches!(
            url.host_str(),
            Some("wss-primary.slack.com" | "wss-backup.slack.com")
        )
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
}
pub(crate) fn open_url(token: &str) -> Result<String, &'static str> {
    open_url_with(token, |candidate| {
        slack_api::call("apps.connections.open", candidate, &[])
    })
}
fn open_url_with(
    token: &str,
    call: impl FnOnce(&str) -> Result<Value, &'static str>,
) -> Result<String, &'static str> {
    let response = call(token)?;
    socket_url(&response)
}
fn socket_url(response: &Value) -> Result<String, &'static str> {
    let url = response.get("url").and_then(Value::as_str).ok_or("parse")?;
    if !confined_socket_url(url) {
        return Err("url");
    }
    Ok(url.to_string())
}
fn app_id(value: &Value) -> Option<&str> {
    value
        .pointer("/connection_info/app_id")
        .and_then(Value::as_str)
        .filter(|s| {
            s.len() >= 2
                && s.len() <= 64
                && s.starts_with('A')
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
}
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Reaction,
    Channel,
    Ignore,
}
fn route(value: &Value, expected_app: &str) -> Result<Route, &'static str> {
    if let Some(actual) = value.pointer("/payload/api_app_id").and_then(Value::as_str) {
        if actual != expected_app {
            return Err("app-id");
        }
    }
    if value.get("type").and_then(Value::as_str) != Some("events_api") {
        return Ok(Route::Ignore);
    }
    Ok(
        match value.pointer("/payload/event/type").and_then(Value::as_str) {
            Some("reaction_added") => Route::Reaction,
            Some("message") => Route::Channel,
            _ => Route::Ignore,
        },
    )
}
fn disconnect_requested(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("disconnect")
}
fn control_reply(message: Message) -> Option<Message> {
    match message {
        Message::Ping(payload) => Some(Message::Pong(payload)),
        _ => None,
    }
}
fn next_backoff(seconds: u64) -> u64 {
    seconds.saturating_mul(2).min(120)
}
fn envelope_id(value: &Value) -> Option<&str> {
    value
        .get("envelope_id")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}
fn parse_socket_text(text: &str) -> Result<Value, &'static str> {
    if text.len() > MAX_SOCKET_TEXT {
        return Err("oversize");
    }
    serde_json::from_str(text).map_err(|_| "parse")
}
/// Channel staging runs before this function returns; the caller is the
/// only place that may send the returned envelope's ACK.
fn stage_route(
    value: &Value,
    expected_app: &str,
    channel_wanted: bool,
    channel_identity_ready: bool,
    stage_channel: impl FnOnce() -> Result<(), &'static str>,
) -> Result<Route, &'static str> {
    let kind = route(value, expected_app)?;
    if kind == Route::Channel {
        if channel_wanted && !channel_identity_ready {
            return Err("channel-identity");
        }
        if channel_identity_ready {
            stage_channel()?;
        }
    }
    Ok(kind)
}
fn workspace_match(user: &Value, bot: &Value) -> bool {
    let team = user.get("team_id").and_then(Value::as_str);
    let bot_team = bot.get("team_id").and_then(Value::as_str);
    let enterprise = user.get("enterprise_id").and_then(Value::as_str);
    let bot_enterprise = bot.get("enterprise_id").and_then(Value::as_str);
    team.is_some()
        && team == bot_team
        && (enterprise.is_none() || bot_enterprise.is_none() || enterprise == bot_enterprise)
}
fn injected_channel_fault(active: bool) -> Option<&'static str> {
    if !active {
        return None;
    }
    if crate::smoke_faults::take("channel-network") {
        Some("network")
    } else if crate::smoke_faults::take("channel-scope") {
        Some("scope")
    } else {
        None
    }
}

pub(crate) fn spawn(app: AppHandle) {
    std::thread::spawn(move || socket_loop(app));
}
fn socket_loop(app: AppHandle) {
    use tungstenite::stream::MaybeTlsStream;
    let mut backoff = 1u64;
    loop {
        let epoch = EPOCH.load(Ordering::SeqCst);
        let cfg = inbound::read_config();
        let badges = cfg.badges("slack");
        let channel_cfg = inbound_channel::read_config();
        let (reaction_wanted, channel_wanted) = startup_plan(
            cfg.slack_enabled,
            badges.len(),
            keychain::has(Slot::SlackUserToken),
            channel_cfg.connection.enabled,
            inbound_channel::any_rule_active(&channel_cfg),
            keychain::has(Slot::SlackBotToken),
        );
        if !reaction_wanted && !channel_wanted {
            CONNECTED.store(false, Ordering::SeqCst);
            inbound_channel::transport_disabled();
            wait(Duration::from_secs(60));
            continue;
        }
        let Some(app_token) = keychain::get(Slot::SlackAppToken) else {
            CONNECTED.store(false, Ordering::SeqCst);
            if channel_wanted {
                inbound_channel::transport_disconnected("no-token");
            }
            wait(Duration::from_secs(60));
            continue;
        };
        let user_present = keychain::has(Slot::SlackUserToken);
        let user = if reaction_wanted || (channel_wanted && user_present) {
            keychain::get(Slot::SlackUserToken).and_then(|t| {
                slack_api::verify(Slot::SlackUserToken, &t)
                    .ok()
                    .map(|v| (t, v))
            })
        } else {
            None
        };
        let bot = if channel_wanted {
            keychain::get(Slot::SlackBotToken).and_then(|t| {
                slack_api::verify(Slot::SlackBotToken, &t)
                    .ok()
                    .map(|v| (t, v))
            })
        } else {
            None
        };
        let bot = match (&user, bot) {
            (None, Some(_)) if user_present => {
                applog("[slack] saved user credential could not be verified");
                None
            }
            (Some((_, u)), Some((_, b))) if !workspace_match(u, &b) => {
                applog("[slack] bot workspace mismatch");
                None
            }
            (_, b) => b,
        };
        let identity = bot
            .as_ref()
            .and_then(|(t, _)| inbound_channel::connection_identity(t).ok());
        if user.is_none() && identity.is_none() {
            wait(Duration::from_secs(60));
            continue;
        }
        let attempt = (|| -> Result<(), &'static str> {
            let url = open_url(&app_token)?;
            APP_VERIFIED.store(true, Ordering::SeqCst);
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }
            let (mut ws, _) = tungstenite::connect(url).map_err(|_| "socket")?;
            if let MaybeTlsStream::Rustls(s) = ws.get_mut() {
                let _ = s.get_mut().set_read_timeout(Some(Duration::from_secs(5)));
            }
            let mut expected_app: Option<String> = None;
            let mut idle = 0u32;
            loop {
                if EPOCH.load(Ordering::SeqCst) != epoch {
                    let _ = ws.close(None);
                    return Err("changed");
                }
                if let Some(code) = injected_channel_fault(channel_wanted) {
                    return Err(code);
                }
                match ws.read() {
                    Ok(Message::Text(text)) => {
                        idle = 0;
                        let value = parse_socket_text(&text)?;
                        if value.get("type").and_then(Value::as_str) == Some("hello") {
                            expected_app = Some(app_id(&value).ok_or("app-id")?.to_string());
                            CONNECTED.store(true, Ordering::SeqCst);
                            if identity.is_some() {
                                inbound_channel::transport_connected();
                            }
                            backoff = 1;
                            continue;
                        }
                        if disconnect_requested(&value) {
                            return Err("reconnect");
                        }
                        let expected = expected_app.as_deref().ok_or("app-id")?;
                        let id = envelope_id(&value).map(str::to_string);
                        let kind = stage_route(
                            &value,
                            expected,
                            channel_wanted,
                            identity.is_some(),
                            || {
                                inbound_channel::handle_message(
                                    &app,
                                    identity.as_ref().unwrap(),
                                    &text,
                                )
                            },
                        )?;
                        // Reaction ACK precedes its recoverable Web API fetch.
                        if let Some(id) = id {
                            ws.send(Message::Text(
                                serde_json::json!({"envelope_id":id}).to_string().into(),
                            ))
                            .map_err(|_| "socket")?;
                        }
                        if kind == Route::Reaction && reaction_wanted {
                            if let Some((_, user_identity)) = &user {
                                if let Some(self_id) =
                                    user_identity.get("user_id").and_then(Value::as_str)
                                {
                                    inbound_slack::handle_reaction(&app, &value, self_id, &badges);
                                }
                            }
                        }
                    }
                    Ok(message @ Message::Ping(_)) => {
                        ws.send(control_reply(message).ok_or("socket")?)
                            .map_err(|_| "socket")?;
                        idle = 0;
                    }
                    Ok(Message::Pong(_)) => {
                        idle = 0;
                    }
                    Ok(Message::Close(_)) => return Err("closed"),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) =>
                    {
                        idle += 1;
                        if idle > 2 {
                            return Err("stalled");
                        }
                        ws.send(Message::Ping(Vec::new().into()))
                            .map_err(|_| "socket")?;
                    }
                    Err(_) => return Err("socket"),
                }
            }
        })();
        CONNECTED.store(false, Ordering::SeqCst);
        let code = attempt.err().unwrap_or("socket");
        if identity.is_some() && code != "changed" {
            inbound_channel::transport_disconnected(code);
        }
        if code == "changed" {
            backoff = 1;
            continue;
        }
        applog(&format!(
            "[slack] socket dropped ({code}); retry in {backoff}s"
        ));
        wait(Duration::from_secs(backoff));
        backoff = next_backoff(backoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn status_does_not_open_socket_tickets_even_on_repeated_reads() {
        let source = include_str!("slack_transport.rs");
        let body = source
            .split("pub(crate) async fn slack_connection_status")
            .nth(1)
            .unwrap()
            .split("// Closed, argument-free operation")
            .next()
            .unwrap();
        assert!(!body.contains("open_url("));
        assert!(!body.contains("apps.connections.open"));
        for _ in 0..5 {
            assert!(!app_valid_fact(true, false, false));
            assert!(app_valid_fact(true, true, false));
            assert!(app_valid_fact(true, false, true));
            assert!(!app_valid_fact(false, true, true));
        }
    }

    #[test]
    fn app_ticket_verification_calls_once_and_confines_url() {
        let mut calls = 0;
        let url = open_url_with("xapp-candidate", |token| {
            calls += 1;
            assert_eq!(token, "xapp-candidate");
            Ok(json!({"url":"wss://wss-primary.slack.com/link/?ticket=one"}))
        })
        .unwrap();
        assert_eq!(calls, 1);
        assert!(confined_socket_url(&url));
        for result in [
            Err("auth"),
            Err("scope"),
            Err("network"),
            Err("slack"),
            Ok(json!({"url":"https://wss-primary.slack.com/link/"})),
        ] {
            let mut calls = 0;
            assert!(open_url_with("xapp-candidate", |_| {
                calls += 1;
                result
            })
            .is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn legacy_clear_is_idempotent_partial_and_canonical_safe() {
        for initial in [
            vec![],
            vec![Slot::SlackChannelBotToken],
            vec![Slot::SlackChannelAppToken],
            vec![Slot::SlackChannelBotToken, Slot::SlackChannelAppToken],
        ] {
            let state = Mutex::new(initial.into_iter().collect::<HashSet<_>>());
            let has = |slot| state.lock_or_recover().contains(&slot);
            let clear = |slot| {
                state.lock_or_recover().remove(&slot);
                Ok(())
            };
            assert_eq!(clear_legacy_with(has, clear), Ok(()));
            assert_eq!(clear_legacy_with(has, clear), Ok(()));
            assert!(state.lock_or_recover().is_empty());
        }
        let state = Mutex::new(
            [
                Slot::SlackChannelBotToken,
                Slot::SlackChannelAppToken,
                Slot::SlackUserToken,
                Slot::SlackBotToken,
                Slot::SlackAppToken,
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        );
        let has = |slot| state.lock_or_recover().contains(&slot);
        let clear = |slot| {
            if slot == Slot::SlackChannelAppToken {
                Err(DeckError::new(ErrorKind::Other, "keychain"))
            } else {
                state.lock_or_recover().remove(&slot);
                Ok(())
            }
        };
        assert_eq!(clear_legacy_with(has, clear), Err("keychain"));
        assert!(!has(Slot::SlackChannelBotToken));
        assert!(has(Slot::SlackChannelAppToken));
        for slot in [
            Slot::SlackUserToken,
            Slot::SlackBotToken,
            Slot::SlackAppToken,
        ] {
            assert!(has(slot));
        }
        assert_eq!(
            clear_legacy_with(has, |slot| {
                state.lock_or_recover().remove(&slot);
                Ok(())
            }),
            Ok(())
        );
        for slot in [
            Slot::SlackUserToken,
            Slot::SlackBotToken,
            Slot::SlackAppToken,
        ] {
            assert!(has(slot));
        }
    }
    #[test]
    fn startup_supports_reaction_only_channel_only_and_disabled_rules() {
        assert_eq!(
            startup_plan(true, 1, true, false, false, false),
            (true, false)
        );
        assert_eq!(
            startup_plan(false, 0, false, true, true, true),
            (false, true)
        );
        assert_eq!(startup_plan(true, 1, true, true, true, true), (true, true));
        assert_eq!(
            startup_plan(true, 0, true, true, false, true),
            (false, false)
        );
        // Legacy channel credentials are absent from the canonical plan.
        assert_eq!(
            startup_plan(true, 1, true, true, true, false),
            (true, false)
        );
    }
    #[test]
    fn shared_ack_contract_stages_channel_first_and_keeps_reaction_recoverable() {
        let reaction = json!({"type":"events_api","envelope_id":"e1","payload":{"api_app_id":"A123","event":{"type":"reaction_added"}}});
        let channel = json!({"type":"events_api","envelope_id":"e2","payload":{"api_app_id":"A123","event":{"type":"message"}}});
        let mut order = Vec::new();
        let r = stage_route(&reaction, "A123", true, true, || {
            order.push("stage");
            Ok(())
        })
        .unwrap();
        assert_eq!(r, Route::Reaction);
        order.push("ack");
        order.push("fetch");
        assert_eq!(order, ["ack", "fetch"]);
        order.clear();
        let r = stage_route(&channel, "A123", true, true, || {
            order.push("stage");
            Ok(())
        })
        .unwrap();
        assert_eq!(r, Route::Channel);
        order.push("ack");
        assert_eq!(order, ["stage", "ack"]);
        assert_eq!(
            stage_route(&channel, "A123", true, true, || Err("capacity")),
            Err("capacity")
        );
        assert_eq!(
            stage_route(&channel, "A123", true, false, || panic!("no identity")),
            Err("channel-identity")
        );
        assert_eq!(
            stage_route(
                &json!({"type":"events_api","payload":{"event":{"type":"unknown"}}}),
                "A123",
                true,
                true,
                || panic!("unknown staged")
            ),
            Ok(Route::Ignore)
        );
    }
    #[test]
    fn epoch_wake_retires_the_current_connection() {
        let before = EPOCH.load(Ordering::SeqCst);
        wake();
        assert!(EPOCH.load(Ordering::SeqCst) > before);
        wait(Duration::from_millis(1));
    }
    #[test]
    fn fake_socket_interleaves_consumers_and_never_acks_failed_stage() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.get_mut()
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            ws.send(Message::Text(
                json!({"type":"hello","connection_info":{"app_id":"A123"}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
            let mut acks = Vec::new();
            for (id, kind) in [
                ("e1", "reaction_added"),
                ("e2", "message"),
                ("e3", "message"),
            ] {
                ws.send(Message::Text(json!({"type":"events_api","envelope_id":id,"payload":{"api_app_id":"A123","event":{"type":kind}}}).to_string().into())).unwrap();
                if id != "e3" {
                    acks.push(ws.read().unwrap().into_text().unwrap().to_string());
                }
            }
            let next = ws.read();
            (
                acks,
                next.ok()
                    .and_then(|m| m.into_text().ok().map(|s| s.to_string())),
            )
        });
        let (mut ws, _) = tungstenite::connect(format!("ws://{address}")).unwrap();
        let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
        let app = app_id(&hello).unwrap();
        let mut stages = 0;
        for _ in 0..3 {
            let value: Value =
                serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
            let id = envelope_id(&value).unwrap();
            let decision = stage_route(&value, app, true, true, || {
                stages += 1;
                if stages == 2 {
                    Err("capacity")
                } else {
                    Ok(())
                }
            });
            if decision.is_err() {
                break;
            }
            ws.send(Message::Text(json!({"envelope_id":id}).to_string().into()))
                .unwrap();
        }
        ws.close(None).unwrap();
        let (acks, last) = server.join().unwrap();
        assert_eq!(acks.len(), 2);
        assert!(acks[0].contains("\"e1\"") && acks[1].contains("\"e2\""));
        assert!(
            last.as_deref()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .and_then(|value| value.get("envelope_id").cloned())
                .is_none(),
            "failed durable stage must not emit an ACK"
        );
    }
    #[test]
    fn one_stream_routes_interleaved_events_and_checks_app_identity() {
        let hello = json!({"type":"hello","connection_info":{"app_id":"A123"}});
        let expected = app_id(&hello).unwrap();
        let kinds = ["reaction_added", "message", "reaction_added", "message"];
        let routes: Vec<_> = kinds.iter().map(|kind| route(&json!({"type":"events_api","envelope_id":"e1","payload":{"api_app_id":"A123","event":{"type":kind}}}), expected).unwrap()).collect();
        assert_eq!(
            routes,
            vec![
                Route::Reaction,
                Route::Channel,
                Route::Reaction,
                Route::Channel
            ]
        );
        assert_eq!(
            route(
                &json!({"type":"events_api","payload":{"api_app_id":"A999","event":{"type":"message"}}}),
                expected
            ),
            Err("app-id")
        );
        assert_eq!(
            route(
                &json!({"type":"events_api","payload":{"event":{"type":"other"}}}),
                expected
            ),
            Ok(Route::Ignore)
        );
        assert!(app_id(&json!({"type":"hello","connection_info":{"app_id":"bad"}})).is_none());
    }
    #[test]
    fn workspace_binding_requires_matching_teams_and_reliable_enterprise() {
        assert!(workspace_match(
            &json!({"team_id":"T1","enterprise_id":"E1"}),
            &json!({"team_id":"T1","enterprise_id":"E1"})
        ));
        assert!(!workspace_match(
            &json!({"team_id":"T1"}),
            &json!({"team_id":"T2"})
        ));
        assert!(!workspace_match(
            &json!({"team_id":"T1","enterprise_id":"E1"}),
            &json!({"team_id":"T1","enterprise_id":"E2"})
        ));
    }
    #[test]
    fn ping_pong_disconnect_and_reconnect_backoff_are_bounded() {
        assert_eq!(
            control_reply(Message::Ping(vec![1, 2].into())),
            Some(Message::Pong(vec![1, 2].into()))
        );
        assert!(control_reply(Message::Pong(Vec::new().into())).is_none());
        assert!(disconnect_requested(
            &json!({"type":"disconnect","reason":"refresh_requested"})
        ));
        assert!(!disconnect_requested(&json!({"type":"events_api"})));
        let mut backoff = 1;
        for expected in [2, 4, 8, 16, 32, 64, 120, 120] {
            backoff = next_backoff(backoff);
            assert_eq!(backoff, expected);
        }
    }
    #[test]
    fn malformed_and_oversized_socket_frames_fail_closed() {
        assert_eq!(parse_socket_text("{"), Err("parse"));
        assert_eq!(
            parse_socket_text(&"x".repeat(MAX_SOCKET_TEXT + 1)),
            Err("oversize")
        );
        assert!(parse_socket_text("{\"type\":\"events_api\"}").is_ok());
    }
    #[test]
    fn wss_confinement_applies_to_shared_reaction_and_channel_socket() {
        assert!(confined_socket_url(
            "wss://wss-primary.slack.com/link/?ticket=x"
        ));
        for bad in [
            "https://wss-primary.slack.com/link/",
            "wss://wss-primary.slack.com.evil.invalid/x",
            "wss://user@wss-primary.slack.com/x",
            "wss://wss-primary.slack.com:444/x",
        ] {
            assert!(!confined_socket_url(bad));
        }
    }
    #[test]
    fn production_has_one_socket_owner_and_one_ack_site() {
        let transport = include_str!("slack_transport.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let reaction = include_str!("inbound_slack.rs");
        let channel = include_str!("inbound_channel.rs");
        assert_eq!(
            transport
                .matches("slack_api::call(\"apps.connections.open\"")
                .count(),
            1
        );
        assert_eq!(transport.matches("ws.send(Message::Text(").count(), 1);
        let runtime = transport.split("fn socket_loop(app:").nth(1).unwrap();
        assert!(
            runtime.find("open_url(&app_token)?").unwrap()
                < runtime.find("tungstenite::connect(url)").unwrap()
        );
        assert!(
            !reaction.contains("apps.connections.open")
                && !reaction.contains("tungstenite::connect")
        );
        assert!(
            !channel.contains("apps.connections.open") && !channel.contains("tungstenite::connect")
        );
    }
}
