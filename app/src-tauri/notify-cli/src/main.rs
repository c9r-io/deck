//! `deck-notify`: hand the running Deck a notification from any program of
//! the same macOS user, over Deck's private local socket.
//!
//! # Contract
//! - Commands: `publish` (the notification as one JSON object on stdin;
//!   `--json-stdin` names that explicitly), `status --producer P --event-id
//!   E`, `socket` (print the socket path it would use), `help`, `--version`.
//!   `--socket PATH` (or `DECK_NOTIFY_SOCKET`) points it at another Deck's
//!   data directory: absolute, and the file must be named `notify.sock`;
//!   `--timeout SECS` (1..=60, default 10) bounds the whole exchange.
//! - stdin is read as bytes up to `MAX_INPUT_BYTES` and parsed with a real
//!   JSON parser; nothing is ever evaluated or passed to a shell. stdout gets
//!   exactly one JSON line — Deck's receipt, or an error object of the same
//!   shape (`status: "error"`, `code`, `message`, `retryable`) — and stderr a
//!   one-line diagnostic for people. Deck validates everything again; the
//!   checks here only give a caller its answer without a round trip.
//! - Exit codes: 0 accepted, duplicate or found; 1 not found (`status`);
//!   2 usage; 3 invalid input (also unsupported version, too large);
//!   4 conflict (this event_id was used for other content); 5 unavailable
//!   now (Deck not running, local notifications off, capacity, rate limit,
//!   busy, inbox error) — retry later with the same event_id; 6 no receipt
//!   (the request may have been received) — retry with the SAME event_id or
//!   ask `status`; 7 protocol mismatch (use the deck-notify inside the
//!   running Deck.app).
//! - One request per connection: a newline-terminated line of at most
//!   `MAX_REQUEST_BYTES`; the wire protocol is `PROTOCOL` and is this
//!   program's, not a public interface.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

const PROTOCOL: u64 = 1;
const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// The notification itself; the request wraps it in a few more bytes.
const MAX_INPUT_BYTES: usize = 60 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

const USAGE: &str = "usage:
  deck-notify publish [--json-stdin] < notification.json
  deck-notify status --producer <producer> --event-id <event_id>
  deck-notify socket
options: --socket <path>/notify.sock   --timeout <seconds, 1-60>
exit: 0 ok · 1 not found · 2 usage · 3 invalid · 4 conflict · 5 unavailable (retry) · 6 no receipt (retry same event_id) · 7 protocol mismatch
docs: docs/local-notifications.md in the Deck repository";

#[derive(Debug, PartialEq)]
enum Command {
    Publish,
    Status { producer: String, event_id: String },
    Socket,
    Help,
    Version,
}

#[derive(Debug, PartialEq)]
struct Args {
    command: Command,
    socket: Option<String>,
    timeout: u64,
}

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut command = None;
    let mut socket = None;
    let mut timeout = 10;
    let mut producer = None;
    let mut event_id = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "publish" | "status" | "socket" | "help" if command.is_none() => {
                command = Some(arg.clone())
            }
            "--help" | "-h" => command = Some("help".into()),
            "--version" | "-V" => command = Some("version".into()),
            "--json-stdin" => {}
            "--socket" => socket = Some(value("--socket")?),
            "--timeout" => {
                timeout = value("--timeout")?
                    .parse::<u64>()
                    .ok()
                    .filter(|t| (1..=60).contains(t))
                    .ok_or("--timeout is 1 to 60 seconds")?
            }
            "--producer" => producer = Some(value("--producer")?),
            "--event-id" => event_id = Some(value("--event-id")?),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let status_args = producer.is_some() || event_id.is_some();
    let command = match command.as_deref() {
        Some("publish") => Command::Publish,
        Some("status") => Command::Status {
            producer: producer.ok_or("status needs --producer")?,
            event_id: event_id.ok_or("status needs --event-id")?,
        },
        Some("socket") => Command::Socket,
        Some("version") => Command::Version,
        Some("help") | None => Command::Help,
        Some(_) => unreachable!("only listed commands are kept"),
    };
    if !matches!(command, Command::Status { .. }) && status_args {
        return Err("--producer and --event-id belong to status".into());
    }
    Ok(Args {
        command,
        socket,
        timeout,
    })
}

/// The socket to use: `--socket`, else `DECK_NOTIFY_SOCKET`, else the
/// default under the home directory. An override must be absolute and name
/// `notify.sock`.
fn socket_path(flag: Option<&str>, env: Option<&str>, home: &Path) -> Result<PathBuf, String> {
    match flag.or(env) {
        Some(path) => {
            let p = Path::new(path);
            if p.is_absolute() && p.file_name().is_some_and(|n| n == "notify.sock") {
                Ok(p.to_path_buf())
            } else {
                Err("the socket path must be absolute and end in notify.sock".into())
            }
        }
        None => Ok(home.join(".deck").join("notify.sock")),
    }
}

/// An error object in the receipt's shape, written by this program.
fn local_error(code: &str, message: &str, retryable: bool) -> Value {
    json!({"protocol": PROTOCOL, "status": "error", "code": code,
        "message": message, "retryable": retryable})
}

fn exit_code(response: &Value) -> i32 {
    match response["status"].as_str() {
        Some("accepted" | "duplicate" | "found") => 0,
        Some("not-found") => 1,
        _ => match response["code"].as_str() {
            Some("USAGE") => 2,
            Some("INVALID_INPUT" | "UNSUPPORTED_VERSION" | "TOO_LARGE") => 3,
            Some("CONFLICT") => 4,
            Some("NO_RECEIPT") => 6,
            Some("UNSUPPORTED_PROTOCOL") => 7,
            _ => 5,
        },
    }
}

/// Read the notification from `input`: one JSON object, bounded.
fn read_notification(input: &mut impl Read) -> Result<Value, Value> {
    let mut bytes = Vec::new();
    input
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| local_error("INVALID_INPUT", "stdin could not be read", false))?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(local_error(
            "TOO_LARGE",
            "a notification is at most 60 KiB of JSON",
            false,
        ));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        local_error(
            "INVALID_INPUT",
            "stdin is not one JSON object (UTF-8)",
            false,
        )
    })?;
    if !value.is_object() {
        return Err(local_error(
            "INVALID_INPUT",
            "a notification is a JSON object",
            false,
        ));
    }
    Ok(value)
}

/// One request, one response. Connection failure is `UNAVAILABLE` (nothing
/// was sent); a failure after the request may have reached Deck is
/// `NO_RECEIPT`.
fn exchange(socket: &Path, request: &Value, timeout: Duration) -> Value {
    let mut line = match serde_json::to_vec(request) {
        Ok(bytes) => bytes,
        Err(_) => return local_error("INVALID_INPUT", "the request could not be encoded", false),
    };
    line.push(b'\n');
    if line.len() > MAX_REQUEST_BYTES {
        return local_error("TOO_LARGE", "the request is larger than 64 KiB", false);
    }
    let mut stream =
        match UnixStream::connect(socket) {
            Ok(stream) => stream,
            Err(_) => return local_error(
                "UNAVAILABLE",
                "Deck is not running, or Notifications > Accept local notifications is off in Deck",
                true,
            ),
        };
    let _ = stream.set_write_timeout(Some(timeout));
    let _ = stream.set_read_timeout(Some(timeout));
    let no_receipt = || {
        local_error(
            "NO_RECEIPT",
            "no answer from Deck; the notification may have been received — retry with the same event_id, or ask with status",
            true,
        )
    };
    if stream
        .write_all(&line)
        .and_then(|_| stream.flush())
        .is_err()
    {
        return no_receipt();
    }
    let mut response = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buffer[..n]);
                if response.contains(&b'\n') || response.len() > MAX_RESPONSE_BYTES {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return no_receipt(),
        }
    }
    let end = response
        .iter()
        .position(|b| *b == b'\n')
        .unwrap_or(response.len());
    match serde_json::from_slice::<Value>(&response[..end]) {
        Ok(value) if value.is_object() => value,
        _ => no_receipt(),
    }
}

fn diagnostic(response: &Value) -> String {
    match response["status"].as_str() {
        Some("accepted") if response["applied"] == false => format!(
            "accepted, not applied ({})",
            response["reason"].as_str().unwrap_or("unknown")
        ),
        Some("accepted") => "accepted".into(),
        Some("duplicate") => "duplicate: Deck already has this event".into(),
        Some("found") => "found".into(),
        Some("not-found") => "not found: Deck has no record of this event".into(),
        _ => format!(
            "{}: {}",
            response["code"].as_str().unwrap_or("ERROR"),
            response["message"].as_str().unwrap_or("")
        ),
    }
}

fn run(argv: &[String], stdin: &mut impl Read, env: Option<&str>, home: &Path) -> (Value, String) {
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(message) => {
            return (
                local_error("USAGE", &message, false),
                format!("{message}\n{USAGE}"),
            )
        }
    };
    let socket = match socket_path(args.socket.as_deref(), env, home) {
        Ok(path) => path,
        Err(message) => return (local_error("USAGE", &message, false), message),
    };
    let timeout = Duration::from_secs(args.timeout);
    let response = match args.command {
        Command::Help => return (Value::Null, USAGE.into()),
        Command::Version => {
            return (
                json!({"protocol": PROTOCOL, "deck_notify": env!("CARGO_PKG_VERSION")}),
                String::new(),
            )
        }
        Command::Socket => {
            return (
                json!({"protocol": PROTOCOL, "socket": socket.to_string_lossy()}),
                String::new(),
            )
        }
        Command::Publish => match read_notification(stdin) {
            Ok(notification) => exchange(
                &socket,
                &json!({"protocol": PROTOCOL, "op": "publish", "notification": notification}),
                timeout,
            ),
            Err(error) => error,
        },
        Command::Status { producer, event_id } => exchange(
            &socket,
            &json!({"protocol": PROTOCOL, "op": "status", "producer": producer, "event_id": event_id}),
            timeout,
        ),
    };
    let note = diagnostic(&response);
    (response, note)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let env = std::env::var("DECK_NOTIFY_SOCKET").ok();
    let (response, note) = run(&argv, &mut std::io::stdin().lock(), env.as_deref(), &home);
    if !response.is_null() {
        println!("{response}");
    }
    if !note.is_empty() {
        eprintln!("deck-notify: {note}");
    }
    let informational = response.is_null()
        || response.get("socket").is_some()
        || response.get("deck_notify").is_some();
    let code = if informational {
        0
    } else {
        exit_code(&response)
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_are_a_closed_list() {
        assert_eq!(
            parse_args(&args(&["publish", "--json-stdin"]))
                .unwrap()
                .command,
            Command::Publish
        );
        assert_eq!(
            parse_args(&args(&["status", "--producer", "ci", "--event-id", "e1"]))
                .unwrap()
                .command,
            Command::Status {
                producer: "ci".into(),
                event_id: "e1".into()
            }
        );
        assert!(parse_args(&args(&["status", "--producer", "ci"])).is_err());
        assert!(parse_args(&args(&["publish", "--target", "x"])).is_err());
        assert!(parse_args(&args(&["publish", "--producer", "ci"])).is_err());
        assert!(parse_args(&args(&["publish", "--timeout", "0"])).is_err());
        assert_eq!(parse_args(&args(&[])).unwrap().command, Command::Help);
    }

    #[test]
    fn socket_override_must_name_notify_sock() {
        let home = Path::new("/Users/u");
        assert_eq!(
            socket_path(None, None, home).unwrap(),
            PathBuf::from("/Users/u/.deck/notify.sock")
        );
        assert_eq!(
            socket_path(Some("/tmp/x/notify.sock"), None, home).unwrap(),
            PathBuf::from("/tmp/x/notify.sock")
        );
        assert!(socket_path(Some("relative/notify.sock"), None, home).is_err());
        assert!(socket_path(None, Some("/tmp/x/mcp-control.sock"), home).is_err());
    }

    #[test]
    fn stdin_is_one_bounded_json_object() {
        assert!(read_notification(&mut &b"{\"version\":1}"[..]).is_ok());
        let not_object = read_notification(&mut &b"[1]"[..]).unwrap_err();
        assert_eq!(exit_code(&not_object), 3);
        let garbage = read_notification(&mut &b"{"[..]).unwrap_err();
        assert_eq!(garbage["code"], "INVALID_INPUT");
        let big = vec![b' '; MAX_INPUT_BYTES + 10];
        assert_eq!(
            read_notification(&mut &big[..]).unwrap_err()["code"],
            "TOO_LARGE"
        );
    }

    #[test]
    fn exit_codes_follow_the_contract() {
        for (status, code, want) in [
            ("accepted", "", 0),
            ("duplicate", "", 0),
            ("found", "", 0),
            ("not-found", "", 1),
            ("error", "USAGE", 2),
            ("error", "INVALID_INPUT", 3),
            ("error", "UNSUPPORTED_VERSION", 3),
            ("error", "TOO_LARGE", 3),
            ("error", "CONFLICT", 4),
            ("error", "UNAVAILABLE", 5),
            ("error", "DISABLED", 5),
            ("error", "CAPACITY", 5),
            ("error", "RATE_LIMITED", 5),
            ("error", "INTERNAL", 5),
            ("error", "NO_RECEIPT", 6),
            ("error", "UNSUPPORTED_PROTOCOL", 7),
        ] {
            assert_eq!(
                exit_code(&json!({"status": status, "code": code})),
                want,
                "{status} {code}"
            );
        }
    }

    #[test]
    fn no_socket_is_unavailable_and_retryable() {
        let dir = std::env::temp_dir().join(format!("deck-notify-test-{}", std::process::id()));
        let socket = dir.join("notify.sock");
        let out = exchange(&socket, &json!({"protocol": 1}), Duration::from_secs(1));
        assert_eq!(out["code"], "UNAVAILABLE");
        assert_eq!(out["retryable"], true);
        assert_eq!(exit_code(&out), 5);
    }

    #[test]
    fn a_server_that_closes_without_answering_is_no_receipt() {
        let dir = std::env::temp_dir().join(format!("dn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("notify.sock");
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            while stream.read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
                line.push(byte[0]);
            }
            line
        });
        let out = exchange(
            &socket,
            &json!({"protocol": 1, "op": "publish", "notification": {"title": "a\nb \"q\" 日本"}}),
            Duration::from_secs(2),
        );
        let line = server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(out["code"], "NO_RECEIPT");
        assert_eq!(exit_code(&out), 6);
        // the request is one JSON line: newlines and quotes stay escaped
        let sent: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(sent["notification"]["title"], "a\nb \"q\" 日本");
    }
}
