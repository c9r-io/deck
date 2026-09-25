//! Slack Web API and the single Deck App manifest. Tokens remain in Keychain;
//! only bounded, closed errors and scope names cross this boundary.
use crate::keychain::Slot;
use crate::sync::LockRecover;
use serde_json::Value;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const API: &str = "https://slack.com/api/";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Test seam: the Web API base every `call` posts to. Set only while holding
/// `TEST_API_LOCK`, which the channel monitor's tests share.
#[cfg(test)]
pub(crate) static TEST_API: Mutex<Option<String>> = Mutex::new(None);
#[cfg(test)]
pub(crate) static TEST_API_LOCK: Mutex<()> = Mutex::new(());

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

fn endpoint(method: &str) -> String {
    #[cfg(test)]
    if let Some(base) = TEST_API.lock_or_recover().clone() {
        return format!("{base}{method}");
    }
    format!("{API}{method}")
}

/// Slack's own error names are a closed, lowercase vocabulary. Keep the
/// last one seen (bounded, charset-checked) so a verification failure can
/// name it in the log and the toast without ever carrying content.
static LAST_SLACK_ERROR: Mutex<String> = Mutex::new(String::new());

pub(crate) fn last_slack_error() -> String {
    LAST_SLACK_ERROR.lock_or_recover().clone()
}

fn note_slack_error(name: &str) {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_lowercase() || *c == '_')
        .take(48)
        .collect();
    *LAST_SLACK_ERROR.lock_or_recover() = clean;
}

/// One Slack Web API call — always POST with a form body, as every method
/// documents. `Err` is a closed code suitable for logs.
pub(crate) fn call(
    method: &str,
    token: &str,
    params: &[(&str, &str)],
) -> Result<Value, &'static str> {
    Ok(call_response(method, token, params)?.body)
}

pub(crate) struct ApiResponse {
    pub(crate) body: Value,
    pub(crate) scopes: Option<Vec<String>>,
}

pub(crate) fn call_response(
    method: &str,
    token: &str,
    params: &[(&str, &str)],
) -> Result<ApiResponse, &'static str> {
    let form: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect();
    let req = client()
        .post(endpoint(method))
        .bearer_auth(token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form.join("&"));
    let (body, scopes): (Value, Option<Vec<String>>) =
        tauri::async_runtime::block_on(async move {
            let resp =
                req.send()
                    .await
                    .map_err(|e| if e.is_timeout() { "timeout" } else { "network" })?;
            if resp.status().as_u16() == 429 {
                return Err("ratelimited");
            }
            if !resp.status().is_success() {
                return Err("http");
            }
            let scopes = match resp.headers().get("x-oauth-scopes") {
                Some(raw) => Some(parse_scopes(raw.as_bytes())?),
                None => None,
            };
            let body = resp.json::<Value>().await.map_err(|_| "parse")?;
            Ok::<_, &'static str>((body, scopes))
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
    Ok(ApiResponse { body, scopes })
}

/// Strictly bound header parser: missing/oversized/malformed scope metadata never
/// becomes a successful verification, and no raw header is retained.
fn parse_scopes(raw: &[u8]) -> Result<Vec<String>, &'static str> {
    if raw.len() > 2048 {
        return Err("scope");
    }
    let raw = std::str::from_utf8(raw).map_err(|_| "scope")?;
    let mut out = Vec::new();
    for item in raw.split(',') {
        let scope = item.trim();
        if scope.is_empty()
            || scope.len() > 64
            || !scope.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b':'
                    || b == b'_'
                    || b == b'.'
                    || b == b'-'
            })
        {
            return Err("scope");
        }
        if !out.iter().any(|s| s == scope) {
            out.push(scope.to_string());
        }
        if out.len() > 64 {
            return Err("scope");
        }
    }
    Ok(out)
}

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
pub(crate) const BOT_SCOPES: &[&str] = &["channels:history", "groups:history"];

pub(crate) fn manifest() -> Value {
    serde_json::json!({
        "display_information": {"name":"deck","description":"Slack badge automations and scoped channel monitoring in deck.","background_color":"#101318"},
        "features":{"bot_user":{"display_name":"deck monitor","always_online":false}},
        "oauth_config":{"scopes":{"user":USER_SCOPES,"bot":BOT_SCOPES}},
        "settings":{"socket_mode_enabled":true,"event_subscriptions":{"user_events":["reaction_added"],"bot_events":["message.channels","message.groups"]},"org_deploy_enabled":false,"token_rotation_enabled":false}
    })
}

pub(crate) fn setup_url() -> String {
    format!(
        "https://api.slack.com/apps?new_app=1&manifest_json={}",
        encode(&manifest().to_string())
    )
}

#[tauri::command]
pub(crate) fn slack_manifest() -> String {
    serde_json::to_string_pretty(&manifest()).unwrap_or_default()
}

pub(crate) fn verify(slot: Slot, value: &str) -> Result<Value, &'static str> {
    match slot {
        Slot::SlackUserToken | Slot::SlackBotToken => {
            let response = call_response("auth.test", value, &[])?;
            let granted = response.scopes.ok_or("scope")?;
            let required = if slot == Slot::SlackUserToken {
                USER_SCOPES
            } else {
                BOT_SCOPES
            };
            if !required.iter().all(|s| granted.iter().any(|g| g == s)) {
                return Err("scope");
            }
            if response
                .body
                .get("team_id")
                .and_then(Value::as_str)
                .is_none()
            {
                return Err("parse");
            }
            Ok(response.body)
        }
        Slot::SlackAppToken => Err("slot"),
        _ => Err("slot"),
    }
}

pub(crate) fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    fn fake(
        scopes: &str,
        body: &str,
        f: impl FnOnce() -> Result<Value, &'static str>,
    ) -> Result<Value, &'static str> {
        let _serial = TEST_API_LOCK.lock_or_recover();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let scopes = scopes.to_string();
        let body = body.to_string();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut raw = [0; 4096];
            let _ = stream.read(&mut raw).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nx-oauth-scopes: {scopes}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        *TEST_API.lock_or_recover() = Some(base);
        let result = f();
        *TEST_API.lock_or_recover() = None;
        worker.join().unwrap();
        result
    }
    #[test]
    fn installed_scope_header_gates_user_and_bot() {
        let all = USER_SCOPES.join(",");
        let identity = r#"{"ok":true,"team_id":"T1","user_id":"U1"}"#;
        assert!(fake(&all, identity, || verify(Slot::SlackUserToken, "xoxp-test")).is_ok());
        let missing = USER_SCOPES
            .iter()
            .filter(|s| **s != "search:read")
            .copied()
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            fake(&missing, identity, || verify(
                Slot::SlackUserToken,
                "xoxp-test"
            )),
            Err("scope")
        );
        for scopes in [
            "channels:history,groups:history",
            "groups:history",
            "channels:history",
        ] {
            let result = fake(scopes, identity, || {
                verify(Slot::SlackBotToken, "xoxb-test")
            });
            assert_eq!(
                result.is_ok(),
                scopes.contains(',') && scopes.contains("channels:history")
            );
        }
    }
    #[test]
    fn scope_parser_is_bounded_and_closed() {
        assert_eq!(
            parse_scopes(b"channels:history, groups:history"),
            Ok(vec!["channels:history".into(), "groups:history".into()])
        );
        for bad in [
            vec![b'x'; 2049],
            b"channels:history,,groups:history".to_vec(),
            b"channels:history,evil scope".to_vec(),
            vec![0xff],
        ] {
            assert_eq!(parse_scopes(&bad), Err("scope"));
        }
    }
    #[test]
    fn one_manifest_contains_only_required_scopes_and_events() {
        let m = manifest();
        assert_eq!(
            m.pointer("/oauth_config/scopes/user"),
            Some(&serde_json::json!(USER_SCOPES))
        );
        assert_eq!(
            m.pointer("/oauth_config/scopes/bot"),
            Some(&serde_json::json!(BOT_SCOPES))
        );
        assert_eq!(
            m.pointer("/settings/event_subscriptions/user_events"),
            Some(&serde_json::json!(["reaction_added"]))
        );
        assert_eq!(
            m.pointer("/settings/event_subscriptions/bot_events"),
            Some(&serde_json::json!(["message.channels", "message.groups"]))
        );
        assert_eq!(
            m.pointer("/settings/socket_mode_enabled"),
            Some(&Value::Bool(true))
        );
        assert!(m.pointer("/oauth_config/redirect_urls").is_none());
        let text = m.to_string();
        assert!(!text.contains("xoxp-") && !text.contains("xoxb-") && !text.contains("xapp-"));
    }
}
