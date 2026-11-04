//! model_api.rs — OpenAI and Anthropic API keys: the backend of the "Model
//! APIs" Settings contribution (`ui/js/extension-model-api.js`).
//!
//! # Contract
//! - Two providers, a closed list (`Provider`): OpenAI and Anthropic. Each
//!   key lives only in the login Keychain, in its own closed slot
//!   (`keychain::Slot::OpenAiApiKey`, `AnthropicApiKey`). It never returns to
//!   the webview and never enters a log line, an error or `~/.deck`.
//! - Nothing here runs by itself: no read at startup, no polling, no call in
//!   the background. A key is sent only when the user saves or checks it,
//!   and only to that provider's model list (`GET /v1/models`), which proves
//!   the key without spending tokens. No prompt, pane text, file or other
//!   content leaves the Mac through this module; a feature that sends any
//!   arrives with its own consent and data boundary, not through this one.
//! - A save proves the candidate first and stores it only when the provider
//!   accepted it, as Slack's tokens are (`slack.rs`); an empty value removes
//!   the key. Errors are closed words: `provider`, `shape`, `missing`,
//!   `auth` (401), `permission` (403), `busy` (429, 5xx), `network` (not
//!   delivered, or the answer cut off mid-way), `response` (an answer that
//!   arrived whole but is not a model list), `keychain`, `worker`; never
//!   the provider's own text.
//! - The last check of each provider is kept in memory only (outcome and
//!   time) and describes the stored key: a candidate that failed to save
//!   leaves it as it was. Status reads only whether a key exists, which
//!   never opens the Keychain access prompt (`keychain::has`).

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::keychain::{self, Slot};
use crate::sync::LockRecover;
use serde_json::{json, Value};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The Messages API version Anthropic requires on every request.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Test seam: the base every model-list request goes to.
#[cfg(test)]
static TEST_BASE: Mutex<Option<String>> = Mutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    OpenAi,
    Anthropic,
}

impl Provider {
    #[cfg(test)]
    const ALL: [Provider; 2] = [Provider::OpenAi, Provider::Anthropic];

    fn parse(id: &str) -> Option<Provider> {
        match id {
            "openai" => Some(Provider::OpenAi),
            "anthropic" => Some(Provider::Anthropic),
            _ => None,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Provider::OpenAi => "openai",
            Provider::Anthropic => "anthropic",
        }
    }

    fn slot(self) -> Slot {
        match self {
            Provider::OpenAi => Slot::OpenAiApiKey,
            Provider::Anthropic => Slot::AnthropicApiKey,
        }
    }

    fn base(self) -> &'static str {
        match self {
            Provider::OpenAi => "https://api.openai.com/",
            Provider::Anthropic => "https://api.anthropic.com/",
        }
    }

    fn index(self) -> usize {
        match self {
            Provider::OpenAi => 0,
            Provider::Anthropic => 1,
        }
    }
}

/// The last check of each provider's stored key, in this process only.
#[derive(Clone, Copy)]
struct Check {
    ok: bool,
    code: Option<&'static str>,
    at: u64,
}

static LAST: Mutex<[Option<Check>; 2]> = Mutex::new([None, None]);

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // reqwest is built with `rustls-no-provider` (see slack_api.rs);
        // installing ring a second time is harmless.
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

fn models_url(provider: Provider) -> String {
    #[cfg(test)]
    if let Some(base) = TEST_BASE.lock_or_recover().clone() {
        return format!("{base}v1/models");
    }
    format!("{}v1/models", provider.base())
}

/// Asks the provider for its model list with `key`: `Ok` when it accepted
/// the key, otherwise a closed word.
fn verify(provider: Provider, key: &str) -> Result<(), &'static str> {
    let request = match provider {
        Provider::OpenAi => client().get(models_url(provider)).bearer_auth(key),
        Provider::Anthropic => client()
            .get(models_url(provider))
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION),
    };
    tauri::async_runtime::block_on(async move {
        let response = request.send().await.map_err(|_| "network")?;
        match response.status().as_u16() {
            200..=299 => {}
            401 => return Err("auth"),
            403 => return Err("permission"),
            429 | 500..=599 => return Err("busy"),
            _ => return Err("response"),
        }
        // a body cut off mid-way is the network's failure, not the provider's
        let bytes = response.bytes().await.map_err(|_| "network")?;
        let body = serde_json::from_slice::<Value>(&bytes).map_err(|_| "response")?;
        if body.get("data").is_some_and(Value::is_array) {
            Ok(())
        } else {
            Err("response")
        }
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn record(provider: Provider, outcome: Result<(), &'static str>) {
    LAST.lock_or_recover()[provider.index()] = Some(Check {
        ok: outcome.is_ok(),
        code: outcome.err(),
        at: now_secs(),
    });
}

fn fail(code: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Other, code)
}

/// Whether each provider has a key, and the last check of it this run.
fn status() -> Value {
    let last = *LAST.lock_or_recover();
    let entry = |provider: Provider| {
        let check = last[provider.index()]
            .map(|check| json!({ "ok": check.ok, "code": check.code, "at": check.at }));
        json!({ "saved": keychain::has(provider.slot()), "check": check })
    };
    json!({ "openai": entry(Provider::OpenAi), "anthropic": entry(Provider::Anthropic) })
}

fn set_key(provider: &str, value: &str) -> Result<(), DeckError> {
    let provider = Provider::parse(provider).ok_or_else(|| fail("provider"))?;
    let value = value.trim();
    if value.is_empty() {
        keychain::clear(provider.slot()).map_err(|_| fail("keychain"))?;
        LAST.lock_or_recover()[provider.index()] = None;
        applog(&format!("[model-api] {} key removed", provider.id()));
        return Ok(());
    }
    if !keychain::accepts(provider.slot(), value) {
        return Err(fail("shape"));
    }
    // proven before it is stored: a refused candidate changes nothing
    verify(provider, value).map_err(|code| {
        applog(&format!(
            "[model-api] {} key refused: {code}",
            provider.id()
        ));
        fail(code)
    })?;
    keychain::set(provider.slot(), value).map_err(|_| fail("keychain"))?;
    record(provider, Ok(()));
    applog(&format!("[model-api] {} key stored", provider.id()));
    Ok(())
}

fn check(provider: &str) -> Result<(), DeckError> {
    let provider = Provider::parse(provider).ok_or_else(|| fail("provider"))?;
    let key = keychain::get_checked(provider.slot())
        .map_err(|_| fail("keychain"))?
        .ok_or_else(|| fail("missing"))?;
    let outcome = verify(provider, &key);
    record(provider, outcome);
    applog(&format!(
        "[model-api] {} check {}",
        provider.id(),
        outcome.err().unwrap_or("ok")
    ));
    outcome.map_err(fail)
}

#[tauri::command]
pub(crate) fn model_api_status() -> Value {
    status()
}

/// Save (after proving) or, with an empty value, remove one provider's key.
#[tauri::command]
pub(crate) async fn model_api_set_key(provider: String, value: String) -> Result<(), DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        static WRITES: Mutex<()> = Mutex::new(());
        let _write = WRITES.lock_or_recover();
        set_key(&provider, &value)
    })
    .await
    .map_err(|_| fail("worker"))?
}

/// Check the stored key of one provider against its model list now.
#[tauri::command]
pub(crate) async fn model_api_check(provider: String) -> Result<(), DeckError> {
    tauri::async_runtime::spawn_blocking(move || check(&provider))
        .await
        .map_err(|_| fail("worker"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// One test at a time owns the base override, the last checks and the
    /// two slots.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Serves one request with `status` and `body`; returns what was asked.
    fn serve(status: &str, body: &str, f: impl FnOnce()) -> String {
        serve_raw(&format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()), f)
    }

    /// Answers one request with exactly `response`, then closes; returns
    /// what was asked.
    fn serve_raw(response: &str, f: impl FnOnce()) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        *TEST_BASE.lock_or_recover() = Some(format!("http://{}/", listener.local_addr().unwrap()));
        let response = response.to_string();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut raw = [0; 8192];
            let read = stream.read(&mut raw).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&raw[..read]).to_lowercase()
        });
        f();
        let request = worker.join().unwrap();
        *TEST_BASE.lock_or_recover() = None;
        request
    }

    fn reset() {
        for provider in Provider::ALL {
            keychain::clear(provider.slot()).unwrap();
        }
        *LAST.lock_or_recover() = [None, None];
    }

    #[test]
    fn each_provider_is_asked_for_its_model_list_with_its_own_credentials() {
        let _serial = SERIAL.lock_or_recover();
        let request = serve("200 OK", r#"{"object":"list","data":[{"id":"m"}]}"#, || {
            assert_eq!(verify(Provider::OpenAi, "sk-proj-test1"), Ok(()));
        });
        assert!(request.starts_with("get /v1/models "), "{request}");
        assert!(request.contains("authorization: bearer sk-proj-test1"));
        assert!(!request.contains("x-api-key"));
        let request = serve("200 OK", r#"{"data":[],"has_more":false}"#, || {
            assert_eq!(verify(Provider::Anthropic, "sk-ant-api03-test1"), Ok(()));
        });
        assert!(request.starts_with("get /v1/models "), "{request}");
        assert!(request.contains("x-api-key: sk-ant-api03-test1"));
        assert!(request.contains("anthropic-version: 2023-06-01"));
        assert!(!request.contains("authorization:"));
    }

    #[test]
    fn a_refusal_is_a_closed_word_never_the_providers_text() {
        let _serial = SERIAL.lock_or_recover();
        for (status, body, expected) in [
            (
                "401 Unauthorized",
                r#"{"error":{"message":"bad key sk-x"}}"#,
                "auth",
            ),
            ("403 Forbidden", "{}", "permission"),
            ("429 Too Many Requests", "{}", "busy"),
            ("500 Internal Server Error", "{}", "busy"),
            ("529 Overloaded", "{}", "busy"),
            ("404 Not Found", "{}", "response"),
            ("200 OK", "not json", "response"),
            ("200 OK", r#"{"models":[]}"#, "response"),
        ] {
            serve(status, body, || {
                assert_eq!(
                    verify(Provider::OpenAi, "sk-test"),
                    Err(expected),
                    "{status} {body}"
                );
            });
        }
        // the answer stops 10 bytes into a body announced as 300: the network
        serve_raw("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 300\r\nConnection: close\r\n\r\n{\"data\":[", || {
            assert_eq!(verify(Provider::OpenAi, "sk-test"), Err("network"));
        });
        // nothing listens there: the request cannot be delivered
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        *TEST_BASE.lock_or_recover() = Some(format!("http://{port}/"));
        assert_eq!(verify(Provider::Anthropic, "sk-ant-test"), Err("network"));
        *TEST_BASE.lock_or_recover() = None;
    }

    #[test]
    fn a_key_is_stored_only_after_its_provider_accepted_it() {
        let _serial = SERIAL.lock_or_recover();
        reset();
        // the wrong provider's shape never reaches the network
        assert_eq!(
            set_key("openai", "sk-ant-api03-x").unwrap_err().to_string(),
            "shape"
        );
        assert_eq!(
            set_key("chatgpt", "sk-x").unwrap_err().to_string(),
            "provider"
        );
        // refused by the provider: nothing stored, no check recorded
        serve("401 Unauthorized", "{}", || {
            assert_eq!(
                set_key("anthropic", " sk-ant-api03-bad ")
                    .unwrap_err()
                    .to_string(),
                "auth"
            );
        });
        assert!(!keychain::has(Slot::AnthropicApiKey));
        assert_eq!(
            status()["anthropic"],
            json!({ "saved": false, "check": null })
        );
        // accepted: stored, trimmed, and its check recorded as ok
        serve("200 OK", r#"{"data":[]}"#, || {
            set_key("anthropic", " sk-ant-api03-good ").unwrap()
        });
        assert_eq!(
            keychain::get(Slot::AnthropicApiKey).as_deref(),
            Some("sk-ant-api03-good")
        );
        let entry = status()["anthropic"].clone();
        assert_eq!(entry["saved"], json!(true));
        assert_eq!(entry["check"]["ok"], json!(true));
        assert_eq!(entry["check"]["code"], Value::Null);
        // a later candidate the provider refuses leaves the stored key and its check
        serve("401 Unauthorized", "{}", || {
            assert!(set_key("anthropic", "sk-ant-api03-other").is_err());
        });
        assert_eq!(
            keychain::get(Slot::AnthropicApiKey).as_deref(),
            Some("sk-ant-api03-good")
        );
        assert_eq!(status()["anthropic"]["check"]["ok"], json!(true));
        assert_eq!(status()["openai"], json!({ "saved": false, "check": null }));
        // an empty value removes the key and forgets its check
        set_key("anthropic", "  ").unwrap();
        assert!(!keychain::has(Slot::AnthropicApiKey));
        assert_eq!(
            status()["anthropic"],
            json!({ "saved": false, "check": null })
        );
        reset();
    }

    #[test]
    fn a_check_reads_the_stored_key_and_records_what_the_provider_said() {
        let _serial = SERIAL.lock_or_recover();
        reset();
        assert_eq!(check("openai").unwrap_err().to_string(), "missing");
        assert_eq!(check("other").unwrap_err().to_string(), "provider");
        serve("200 OK", r#"{"data":[]}"#, || {
            set_key("openai", "sk-proj-stored").unwrap()
        });
        let request = serve("401 Unauthorized", "{}", || {
            assert_eq!(check("openai").unwrap_err().to_string(), "auth");
        });
        assert!(request.contains("authorization: bearer sk-proj-stored"));
        let entry = status()["openai"].clone();
        assert_eq!(entry["saved"], json!(true));
        assert_eq!(entry["check"]["ok"], json!(false));
        assert_eq!(entry["check"]["code"], json!("auth"));
        assert!(entry["check"]["at"].as_u64().unwrap() > 0);
        serve("200 OK", r#"{"data":[]}"#, || check("openai").unwrap());
        assert_eq!(status()["openai"]["check"]["ok"], json!(true));
        reset();
    }
}
