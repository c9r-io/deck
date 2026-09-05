//! deck-status-helper — invoked by agent hooks (Claude Code today) inside a
//! deck tmux pane. It forwards ONE closed status word plus the pane identity
//! it inherited from the environment to deck's local status socket, then
//! exits 0 no matter what: a hook helper must never break the agent that
//! invoked it, and it has nowhere safe to log.
//!
//! Privacy: the hook's stdin payload (prompt text, notification message) is
//! drained and DISCARDED without being read into the message. Every field
//! that leaves this process is charset-validated below, so the emitted JSON
//! needs no escaping and cannot carry content.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// `[a-z0-9-]{1,32}` — the alphabet for source/state words. The closed SET of
/// accepted values is owned by the deck backend, so a new agent module needs
/// no helper change.
fn word_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// tmux socket basename: deck servers are `deck`, `deck-dev`, `deck-smoke*`.
fn socket_name_ok(s: &str) -> bool {
    s.starts_with("deck")
        && s.len() <= 48
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn pane_ok(s: &str) -> bool {
    s.len() >= 2
        && s.len() <= 10
        && s.starts_with('%')
        && s[1..].bytes().all(|b| b.is_ascii_digit())
}

/// The status socket to write to. The deck instance that owns this pane's
/// tmux server exports `DECK_STATUS_SOCK` into the server environment, so an
/// isolated/smoke instance receives its own events and a production pane
/// reaches the production socket. The env value is accepted only in the
/// exact expected shape; anything else falls back to the default path.
fn socket_path(env: Option<&str>, home: &std::path::Path) -> std::path::PathBuf {
    if let Some(path) = env {
        let p = std::path::Path::new(path);
        if p.is_absolute() && p.file_name().is_some_and(|n| n == "status.sock") {
            return p.to_path_buf();
        }
    }
    home.join(".deck").join("status.sock")
}

/// Parse `$TMUX` (`<socket path>,<server pid>,<session index>`) into the
/// socket basename + server pid. Not being inside tmux — or being inside a
/// non-deck tmux — is the common, silent case.
fn parse_tmux_env(tmux: &str) -> Option<(String, u32)> {
    let mut fields = tmux.split(',');
    let path = fields.next()?;
    let pid: u32 = fields.next()?.parse().ok()?;
    let name = path.rsplit('/').next()?;
    socket_name_ok(name).then(|| (name.to_string(), pid))
}

fn build_message(source: &str, state: &str, tmux: &str, pane: &str) -> Option<String> {
    if !word_ok(source) || !word_ok(state) || !pane_ok(pane) {
        return None;
    }
    let (socket_name, server_pid) = parse_tmux_env(tmux)?;
    // every field is charset-validated above — plain format! is safe JSON
    Some(format!(
        "{{\"v\":1,\"source\":\"{source}\",\"state\":\"{state}\",\"socket\":\"{socket_name}\",\"server_pid\":{server_pid},\"pane\":\"{pane}\"}}\n"
    ))
}

fn main() {
    // Drain the hook payload so the agent never sees EPIPE. The bytes are
    // discarded — deck's status model is content-free by construction.
    let mut sink = [0u8; 8192];
    let mut stdin = std::io::stdin().lock();
    let mut drained = 0usize;
    while let Ok(n) = stdin.read(&mut sink) {
        if n == 0 || drained > 1 << 20 {
            break;
        }
        drained += n;
    }

    let args: Vec<String> = std::env::args().collect();
    let (Some(source), Some(state)) = (args.get(1), args.get(2)) else {
        return;
    };
    let (Ok(tmux), Ok(pane)) = (std::env::var("TMUX"), std::env::var("TMUX_PANE")) else {
        return; // not inside tmux — the agent runs outside deck; do nothing
    };
    let Some(message) = build_message(source, state, &tmux, &pane) else {
        return;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let env_sock = std::env::var("DECK_STATUS_SOCK").ok();
    let path = socket_path(env_sock.as_deref(), std::path::Path::new(&home));
    // deck not running (or an old deck without the socket) — silently done
    let Ok(mut stream) = UnixStream::connect(path) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let _ = stream.write_all(message.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_accept_only_the_closed_alphabet() {
        assert!(word_ok("claude-code"));
        assert!(word_ok("needs-input"));
        assert!(!word_ok(""));
        assert!(!word_ok("Has-Upper"));
        assert!(!word_ok("space here"));
        assert!(!word_ok("path/../escape"));
        assert!(!word_ok(&"x".repeat(33)));
    }

    #[test]
    fn tmux_env_parses_only_deck_sockets() {
        assert_eq!(
            parse_tmux_env("/private/tmp/tmux-501/deck,4242,3"),
            Some(("deck".into(), 4242))
        );
        assert_eq!(
            parse_tmux_env("/private/tmp/tmux-501/deck-dev,17,0"),
            Some(("deck-dev".into(), 17))
        );
        assert_eq!(parse_tmux_env("/private/tmp/tmux-501/default,4242,3"), None);
        assert_eq!(parse_tmux_env("/private/tmp/tmux-501/deck"), None); // no pid
        assert_eq!(parse_tmux_env("/tmp/x/deck,notanumber,3"), None);
        assert_eq!(parse_tmux_env(""), None);
    }

    #[test]
    fn panes_are_percent_ids() {
        assert!(pane_ok("%0"));
        assert!(pane_ok("%123456789"));
        assert!(!pane_ok("%"));
        assert!(!pane_ok("5"));
        assert!(!pane_ok("%12a"));
        assert!(!pane_ok("%1234567890"));
    }

    #[test]
    fn socket_path_accepts_only_the_exact_env_shape() {
        let home = std::path::Path::new("/Users/u");
        let fallback = std::path::PathBuf::from("/Users/u/.deck/status.sock");
        assert_eq!(
            socket_path(Some("/tmp/deck-test/status.sock"), home),
            std::path::PathBuf::from("/tmp/deck-test/status.sock")
        );
        assert_eq!(socket_path(None, home), fallback);
        assert_eq!(socket_path(Some("relative/status.sock"), home), fallback);
        assert_eq!(socket_path(Some("/tmp/other.sock"), home), fallback);
        assert_eq!(socket_path(Some(""), home), fallback);
    }

    #[test]
    fn message_is_exact_validated_json() {
        assert_eq!(
            build_message(
                "claude-code",
                "turn-done",
                "/private/tmp/tmux-501/deck-dev,4242,0",
                "%7"
            )
            .unwrap(),
            "{\"v\":1,\"source\":\"claude-code\",\"state\":\"turn-done\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}\n"
        );
        assert!(build_message("claude-code", "turn-done", "/t/other,1,0", "%7").is_none());
        assert!(build_message("bad word", "turn-done", "/t/deck,1,0", "%7").is_none());
        assert!(build_message("claude-code", "turn-done", "/t/deck,1,0", "nope").is_none());
    }
}
