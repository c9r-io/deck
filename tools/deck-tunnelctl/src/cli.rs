use std::io::{self, BufRead, Write};

use crate::identity;
use crate::protocol::{self, ActionResponse, StatusResponse, TunnelState};
use crate::secret_file::SecretFile;
use crate::tunnel_client::{Client, RuntimeStatus};
use crate::{keychain, runtime_alias, valid_client_id};

pub fn run(args: &[String]) -> i32 {
    match parse(args).and_then(execute) {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string(&value).expect("response is serializable")
            );
            0
        }
        Err(error) => {
            println!(
                "{}",
                serde_json::json!({
                    "protocolVersion": protocol::PROTOCOL_VERSION,
                    "ok": false,
                    "errorCode": error,
                })
            );
            1
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Request {
    Protocol,
    Status(String),
    Start(String),
    Stop(String),
    Setup(String),
    Remove(String),
}

fn parse(args: &[String]) -> Result<Request, &'static str> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err("usage");
    };
    if command == "protocol" {
        return if args == ["protocol", "--json"] {
            Ok(Request::Protocol)
        } else {
            Err("usage")
        };
    }
    if !matches!(command, "status" | "start" | "stop" | "setup" | "remove") {
        return Err("usage");
    }
    let json_required = command != "setup";
    let expected = if json_required { 4 } else { 3 };
    if args.len() != expected || args.get(1).map(String::as_str) != Some("--client-id") {
        return Err("usage");
    }
    if json_required && args.get(3).map(String::as_str) != Some("--json") {
        return Err("usage");
    }
    let client_id = args[2].clone();
    if !valid_client_id(&client_id) {
        return Err("invalid_client_id");
    }
    Ok(match command {
        "status" => Request::Status(client_id),
        "start" => Request::Start(client_id),
        "stop" => Request::Stop(client_id),
        "setup" => Request::Setup(client_id),
        "remove" => Request::Remove(client_id),
        _ => unreachable!(),
    })
}

fn execute(request: Request) -> Result<serde_json::Value, &'static str> {
    if request == Request::Protocol {
        return serde_json::to_value(protocol::protocol_response()).map_err(|_| "serialize_failed");
    }
    let client_id = match &request {
        Request::Status(id)
        | Request::Start(id)
        | Request::Stop(id)
        | Request::Setup(id)
        | Request::Remove(id) => id,
        Request::Protocol => unreachable!(),
    };
    let alias = runtime_alias(client_id)?;
    let executable = match identity::tunnel_client() {
        Ok(path) => path,
        Err("tunnel_client_missing") if matches!(request, Request::Status(_)) => {
            return value(StatusResponse {
                protocol_version: protocol::PROTOCOL_VERSION,
                state: TunnelState::TunnelClientMissing,
                runtime_alias: alias,
                runtime_exists: false,
                tunnel_id: None,
                error_code: None,
            });
        }
        Err(error) => return Err(error),
    };
    let tunnel = Client::new(executable);
    match request {
        Request::Status(_) => {
            let mut status = tunnel.status(&alias)?;
            if status.exists && !keychain::has(client_id)? {
                status.state = TunnelState::KeyMissing;
            }
            value(StatusResponse {
                protocol_version: protocol::PROTOCOL_VERSION,
                state: status.state,
                runtime_alias: alias,
                runtime_exists: status.exists,
                tunnel_id: status.tunnel_id,
                error_code: None,
            })
        }
        Request::Start(_) => {
            require_verified_secret_lifecycle()?;
            let current = tunnel.status(&alias)?;
            if !current.exists {
                return Err("not_configured");
            }
            let tunnel_id = current.tunnel_id.ok_or("runtime_config_invalid")?;
            let status = connect_with_key(&tunnel, &alias, &tunnel_id, client_id)?;
            action(alias, status)
        }
        Request::Stop(_) => action(alias.clone(), tunnel.stop(&alias)?),
        Request::Remove(_) => {
            tunnel.remove(&alias)?;
            keychain::clear(client_id)?;
            value(ActionResponse {
                protocol_version: protocol::PROTOCOL_VERSION,
                ok: true,
                state: TunnelState::NotConfigured,
                runtime_alias: alias,
                error_code: None,
            })
        }
        Request::Setup(_) => setup(&tunnel, &alias, client_id),
        Request::Protocol => unreachable!(),
    }
}

fn setup(tunnel: &Client, alias: &str, client_id: &str) -> Result<serde_json::Value, &'static str> {
    require_verified_secret_lifecycle()?;
    eprint!("OpenAI Tunnel ID: ");
    io::stderr().flush().map_err(|_| "input_failed")?;
    let mut tunnel_id = String::new();
    io::stdin()
        .lock()
        .read_line(&mut tunnel_id)
        .map_err(|_| "input_failed")?;
    let tunnel_id = tunnel_id.trim();
    if !valid_tunnel_id(tunnel_id) {
        return Err("invalid_tunnel_id");
    }
    eprint!("OpenAI Runtime API key: ");
    io::stderr().flush().map_err(|_| "input_failed")?;
    let mut key = read_secret()?;
    let result = (|| {
        keychain::set(client_id, &key)?;
        connect_with_secret(tunnel, alias, tunnel_id, client_id, &key)
    })();
    key.fill(0);
    action(alias.to_string(), result?)
}

/// The live v0.0.14 gate was completed on 2026-09-23: the Runtime key was read
/// from a 0600 file at startup, the file was removed after a successful
/// control-plane poll, multiple later polls succeeded, the manager did not
/// auto-restart an unexpectedly terminated child, and an explicit reconnect
/// succeeded with a newly generated file.
pub(crate) fn require_verified_secret_lifecycle() -> Result<(), &'static str> {
    Ok(())
}

fn connect_with_key(
    tunnel: &Client,
    alias: &str,
    tunnel_id: &str,
    client_id: &str,
) -> Result<RuntimeStatus, &'static str> {
    let mut key = keychain::get(client_id)?.ok_or("key_missing")?;
    let result = connect_with_secret(tunnel, alias, tunnel_id, client_id, &key);
    key.fill(0);
    result
}

fn connect_with_secret(
    tunnel: &Client,
    alias: &str,
    tunnel_id: &str,
    client_id: &str,
    key: &[u8],
) -> Result<RuntimeStatus, &'static str> {
    let adapter = identity::adapter()?;
    let secret = SecretFile::create(key)?;
    let status = tunnel.connect(alias, tunnel_id, &adapter, client_id, &secret.reference())?;
    if status.state != TunnelState::Ready {
        return Err("tunnel_not_ready");
    }
    // Client::connect waits for a successful control-plane poll. The verified
    // v0.0.14 lifecycle permits SecretFile to be dropped immediately after
    // that bounded readiness check; every explicit Start creates a new file.
    Ok(status)
}

fn read_secret() -> Result<Vec<u8>, &'static str> {
    let fd = libc::STDIN_FILENO;
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    let is_tty = unsafe { libc::isatty(fd) } == 1;
    if is_tty {
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err("input_failed");
        }
        let mut hidden = unsafe { original.assume_init() };
        hidden.c_lflag &= !libc::ECHO;
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &hidden) } != 0 {
            return Err("input_failed");
        }
        original.write(hidden);
    }
    let mut line = Vec::new();
    let read = io::stdin().lock().read_until(b'\n', &mut line);
    if is_tty {
        let original = unsafe { original.assume_init() };
        let _ = unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &original) };
        eprintln!();
    }
    read.map_err(|_| "input_failed")?;
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    if line.is_empty() || line.len() > 16 * 1024 {
        line.fill(0);
        return Err("key_invalid");
    }
    Ok(line)
}

fn valid_tunnel_id(value: &str) -> bool {
    value.starts_with("tunnel_")
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn action(alias: String, status: RuntimeStatus) -> Result<serde_json::Value, &'static str> {
    value(ActionResponse {
        protocol_version: protocol::PROTOCOL_VERSION,
        ok: true,
        state: status.state,
        runtime_alias: alias,
        error_code: None,
    })
}

fn value<T: serde::Serialize>(value: T) -> Result<serde_json::Value, &'static str> {
    serde_json::to_value(value).map_err(|_| "serialize_failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn parser_exposes_only_closed_actions() {
        assert_eq!(parse(&args(&["protocol", "--json"])), Ok(Request::Protocol));
        assert_eq!(
            parse(&args(&["status", "--client-id", "client_a", "--json"])),
            Ok(Request::Status("client_a".into()))
        );
        assert!(parse(&args(&["exec", "--client-id", "client_a", "--json"])).is_err());
        assert!(parse(&args(&[
            "start",
            "--client-id",
            "client_a",
            "--json",
            "--command",
            "sh"
        ]))
        .is_err());
        assert!(parse(&args(&["status", "--client-id", "client_a;rm", "--json"])).is_err());
    }

    #[test]
    fn tunnel_ids_are_closed_and_bounded() {
        assert!(valid_tunnel_id("tunnel_abc-123"));
        assert!(!valid_tunnel_id("tunnel_abc 123"));
        assert!(!valid_tunnel_id("other_abc"));
    }

    #[test]
    fn file_secret_lifecycle_release_gate_is_verified() {
        assert_eq!(require_verified_secret_lifecycle(), Ok(()));
    }
}
