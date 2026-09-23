use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::process;
use crate::protocol::TunnelState;

const SHORT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const READY_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub state: TunnelState,
    pub exists: bool,
    pub tunnel_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StatusProjection {
    process_running: bool,
    healthy: bool,
    ready: bool,
    stale: bool,
    tunnel_id: Option<String>,
    health_url_file: Option<String>,
}

pub struct Client {
    executable: PathBuf,
}

impl Client {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    pub fn status(&self, alias: &str) -> Result<RuntimeStatus, &'static str> {
        let output = self.run(["runtimes", "status", alias, "--json"], SHORT_TIMEOUT)?;
        if !output.status.success() {
            if !self.alias_exists(alias)? {
                return Ok(RuntimeStatus {
                    state: TunnelState::NotConfigured,
                    exists: false,
                    tunnel_id: None,
                });
            }
            return Err("tunnel_client_status_failed");
        }
        let (mut status, health_url_file) = parse_status(&output.stdout)?;
        if status.state == TunnelState::Ready {
            let health_url_file = health_url_file.ok_or("tunnel_client_invalid_json")?;
            if !self.control_plane_poll_ok(&health_url_file)? {
                status.state = TunnelState::Starting;
            }
        }
        Ok(status)
    }

    pub fn connect(
        &self,
        alias: &str,
        tunnel_id: &str,
        adapter: &Path,
        client_id: &str,
        secret_ref: &str,
    ) -> Result<RuntimeStatus, &'static str> {
        let command_string =
            encode_command(&[adapter.to_string_lossy().as_ref(), "--client-id", client_id]);
        let output = process::output(
            Command::new(&self.executable).args([
                "runtimes",
                "connect",
                "--alias",
                alias,
                "--mcp-command",
                &command_string,
                "--runtime-api-key",
                secret_ref,
                "--tunnel-id",
                tunnel_id,
                "--json",
            ]),
            CONNECT_TIMEOUT,
        )
        .map_err(map_io)?;
        if !output.status.success() {
            return Err("tunnel_client_connect_failed");
        }
        self.wait_ready(alias)
    }

    pub fn stop(&self, alias: &str) -> Result<RuntimeStatus, &'static str> {
        let output = self.run(["runtimes", "stop", alias, "--json"], SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err("tunnel_client_stop_failed");
        }
        self.status(alias)
    }

    pub fn remove(&self, alias: &str) -> Result<(), &'static str> {
        let output = self.run(["runtimes", "rm", alias, "--json"], SHORT_TIMEOUT)?;
        if output.status.success() {
            Ok(())
        } else {
            Err("tunnel_client_remove_failed")
        }
    }

    fn run<const N: usize>(
        &self,
        args: [&str; N],
        timeout: Duration,
    ) -> Result<std::process::Output, &'static str> {
        process::output(Command::new(&self.executable).args(args), timeout).map_err(map_io)
    }

    fn alias_exists(&self, alias: &str) -> Result<bool, &'static str> {
        let output = self.run(["runtimes", "list", "--json"], SHORT_TIMEOUT)?;
        if !output.status.success() {
            return Err("tunnel_client_list_failed");
        }
        let value: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| "tunnel_client_invalid_json")?;
        let aliases = value
            .get("aliases")
            .and_then(Value::as_array)
            .ok_or("tunnel_client_invalid_json")?;
        Ok(aliases
            .iter()
            .any(|entry| entry.get("alias").and_then(Value::as_str) == Some(alias)))
    }

    fn control_plane_poll_ok(&self, health_url_file: &str) -> Result<bool, &'static str> {
        let output = self.run(
            [
                "health",
                "--url-file",
                health_url_file,
                "--require-control-plane-poll",
                "--json",
            ],
            SHORT_TIMEOUT,
        )?;
        let value: Value =
            serde_json::from_slice(&output.stdout).map_err(|_| "tunnel_client_invalid_json")?;
        value
            .pointer("/control_plane_poll/ok")
            .and_then(Value::as_bool)
            .ok_or("tunnel_client_invalid_json")
    }

    fn wait_ready(&self, alias: &str) -> Result<RuntimeStatus, &'static str> {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            let status = self.status(alias)?;
            if status.state == TunnelState::Ready {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("tunnel_client_readiness_timeout");
            }
            std::thread::sleep(READY_POLL_INTERVAL);
        }
    }
}

fn map_io(error: std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::TimedOut => "tunnel_client_timeout",
        std::io::ErrorKind::InvalidData => "tunnel_client_output_too_large",
        _ => "tunnel_client_failed",
    }
}

fn parse_status(bytes: &[u8]) -> Result<(RuntimeStatus, Option<String>), &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "tunnel_client_invalid_json")?;
    let projection_value = serde_json::json!({
        "process_running": value.get("process_running").and_then(Value::as_bool).ok_or("tunnel_client_invalid_json")?,
        "healthy": value.get("healthy").and_then(Value::as_bool).ok_or("tunnel_client_invalid_json")?,
        "ready": value.get("ready").and_then(Value::as_bool).ok_or("tunnel_client_invalid_json")?,
        "stale": value.get("stale").and_then(Value::as_bool).ok_or("tunnel_client_invalid_json")?,
        "tunnel_id": value.get("tunnel_id").and_then(Value::as_str),
        "health_url_file": value.get("health_url_file").and_then(Value::as_str),
    });
    let status: StatusProjection =
        serde_json::from_value(projection_value).map_err(|_| "tunnel_client_invalid_json")?;
    let state = if status.stale {
        TunnelState::Stale
    } else if !status.process_running {
        TunnelState::Stopped
    } else if status.healthy && status.ready {
        TunnelState::Ready
    } else if !status.healthy {
        TunnelState::Starting
    } else {
        TunnelState::Unhealthy
    };
    Ok((
        RuntimeStatus {
            state,
            exists: true,
            tunnel_id: status.tunnel_id,
        },
        status.health_url_file,
    ))
}

/// tunnel-client v0.0.14 parses this field with its own argv lexer, not a shell.
/// Every argument is single-quoted; an apostrophe becomes a quoted backslash
/// escape outside the surrounding quotes. No user-supplied command is accepted.
pub fn encode_command(args: &[&str]) -> String {
    args.iter()
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_file::SecretFile;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn status_mapping_requires_both_health_and_readiness() {
        let status = |running, healthy, ready, stale| {
            parse_status(
                serde_json::json!({
                    "process_running": running, "healthy": healthy, "ready": ready,
                    "stale": stale, "tunnel_id": "tunnel_test", "ignored": {"log": "secret"}
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap()
            .0
            .state
        };
        assert_eq!(status(false, false, false, false), TunnelState::Stopped);
        assert_eq!(status(true, false, false, false), TunnelState::Starting);
        assert_eq!(status(true, true, false, false), TunnelState::Unhealthy);
        assert_eq!(status(true, true, true, false), TunnelState::Ready);
        assert_eq!(status(true, true, true, true), TunnelState::Stale);
    }

    #[test]
    fn command_encoder_contains_no_unquoted_user_fragments() {
        let encoded = encode_command(&[
            "/Applications/Deck Tunnel Helper's Test/deck-mcp",
            "--client-id",
            "client_space $HOME; touch /tmp/nope | & \" unicode-雪",
        ]);
        assert_eq!(encoded, "'/Applications/Deck Tunnel Helper'\\''s Test/deck-mcp' '--client-id' 'client_space $HOME; touch /tmp/nope | & \" unicode-雪'");
        assert!(!encoded.contains("sh -c"));
    }

    #[test]
    fn malformed_or_incomplete_status_is_rejected() {
        assert_eq!(parse_status(b"not-json"), Err("tunnel_client_invalid_json"));
        assert_eq!(
            parse_status(br#"{"process_running":false}"#),
            Err("tunnel_client_invalid_json")
        );
    }

    #[test]
    fn connect_uses_exact_argv_and_never_places_the_secret_in_argv() {
        let directory = tempfile::tempdir().unwrap();
        let fake = directory.path().join("fake-tunnel-client");
        fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/fake-tunnel-client.sh"),
            &fake,
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let log = directory.path().join("argv.log");
        std::env::set_var("DECK_TUNNELCTL_FAKE_LOG", &log);

        let secret_value = b"runtime-secret-must-never-appear";
        let secret = SecretFile::create(secret_value).unwrap();
        let secret_path = secret.path().to_path_buf();
        let client = Client::new(fake);
        let status = client
            .connect(
                "deck-0123456789abcdef0123456789abcdef",
                "tunnel_fixture",
                std::path::Path::new("/Applications/Deck Test's App/deck-mcp"),
                "client_fixture",
                &secret.reference(),
            )
            .unwrap();
        assert_eq!(status.tunnel_id.as_deref(), Some("tunnel_fixture"));

        let argv = fs::read_to_string(&log).unwrap();
        assert!(argv.contains("runtimes\nconnect\n"));
        assert!(argv.contains("--runtime-api-key\nfile:"));
        assert!(argv.contains("'--client-id' 'client_fixture'"));
        assert!(!argv.contains(std::str::from_utf8(secret_value).unwrap()));
        drop(secret);
        assert!(!secret_path.exists());
    }
}
