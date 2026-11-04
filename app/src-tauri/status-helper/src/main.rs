//! deck-status-helper — invoked by agent hooks (Claude Code today) inside a
//! deck tmux pane. It forwards ONE closed status word plus the pane identity
//! it inherited from the environment to deck's local status socket, then
//! exits 0 no matter what: a hook helper must never break the agent that
//! invoked it, and it has nowhere safe to log.
//!
//! Privacy: the hook's stdin payload (prompt text, notification message,
//! tool input) never leaves this process and is never logged or stored.
//! ONE field may be read from it — the source's interaction id
//! (`identity_field`: Codex `turn_id`, Claude Code `prompt_id`, a TOP-LEVEL
//! key only, proven at runtime to be the source's own per-interaction id) —
//! and it is forwarded only as a validated lowercase UUID
//! (`interaction_from`); anything else, a duplicate key, a nested or
//! wrong-typed value, malformed or oversized JSON, invalid UTF-8, yields no
//! id and the message falls back to v1. The payload buffer is dropped right
//! after that one read. Every field that leaves this process is
//! charset-validated below, so the emitted JSON needs no escaping and cannot
//! carry content — with ONE exception, written into the hook by deck only
//! while the user's turn-summary switch is on: a Claude Code `turn-done`
//! hook run with `--reply` also reads the TOP-LEVEL string
//! `last_assistant_message` (Claude's final message of the turn, documented
//! for the Stop hook) and forwards its head and tail (`bounded_reply`) as
//! the JSON-escaped `reply` of a v3 line. Without the flag, for any other
//! source or word, or when the value is not a string or its key repeats,
//! nothing of it is read or sent.
//!
//! Wire: v1 `{v:1, source, state, socket, server_pid, pane}`; v2 adds
//! `interaction` (a source id, never synthesized); v3 adds `reply` (with or
//! without `interaction`). deck accepts all three; it takes `reply` off
//! before its own parser sees the line (`turn_summary::split`).
//!
//! Identity: the pane id in the message is a claim; deck proves it by asking
//! the kernel which process connected and walking that process's parents to
//! the pane's own process. The helper therefore keeps the connection open
//! until deck closes it (bounded by a 2 s read timeout) instead of exiting
//! right after the write.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

/// The largest hook payload the helper will inspect; beyond it no id is
/// read (the rest is still drained so the agent never sees EPIPE).
const MAX_PAYLOAD: usize = 1 << 20;

/// The argument deck adds to Claude Code's Stop hook while turn summaries
/// are on; it is honoured for `claude-code turn-done` only.
const REPLY_FLAG: &str = "--reply";
/// The one payload field the flag allows: Claude's final message.
const REPLY_FIELD: &str = "last_assistant_message";
/// The most of that message forwarded, in bytes: its head and its tail
/// (what was done tends to come first, a question last).
const REPLY_HEAD: usize = 2_000;
const REPLY_TAIL: usize = 4_000;
/// What joins the head and the tail of a reply that was cut.
const REPLY_CUT: &str = "\n…\n";
/// deck reads a first line of at most 8 KiB: a line with a reply stays
/// under this, the reply cut further when its escaping makes it longer.
const MAX_WIRE_LINE: usize = 8_000;

/// The one payload field each source may contribute: its own interaction
/// id, proven at runtime (Codex 0.157.0 `turn_id`: shared by start,
/// permission, interrupt and stop, new per turn; Claude Code 2.1.282
/// `prompt_id`: shared by start, notification and stop, new per prompt).
fn identity_field(source: &str) -> Option<&'static str> {
    match source {
        "codex" => Some("turn_id"),
        "claude-code" => Some("prompt_id"),
        _ => None,
    }
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase hex.
fn uuid_ok(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
}

/// What one pass over a payload keeps: the id field's string value and,
/// only when asked for, the reply field's.
#[derive(Debug, Default, PartialEq)]
struct Fields {
    id: Option<String>,
    reply: Option<String>,
}

/// Reads a JSON document whose top level is an object, keeping only the
/// string value of `id` and, when given, of `reply`; every other value is
/// skipped (and still validated) by serde's `IgnoredAny`. A second key of
/// either is an error, never first- or last-wins; a non-string id is an
/// error too, while a reply that is not a string is simply not kept.
struct TopLevel {
    id: &'static str,
    reply: Option<&'static str>,
}

impl<'de> Visitor<'de> for TopLevel {
    type Value = Fields;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = Fields::default();
        let (mut seen_id, mut seen_reply) = (false, false);
        while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
            if key == self.id {
                if seen_id {
                    return Err(serde::de::Error::custom("duplicate identity key"));
                }
                seen_id = true;
                // a non-string value is a type error, i.e. no id
                found.id = Some(map.next_value::<String>()?);
            } else if Some(key.as_ref()) == self.reply {
                if seen_reply {
                    return Err(serde::de::Error::custom("duplicate reply key"));
                }
                seen_reply = true;
                found.reply = match map.next_value::<serde_json::Value>()? {
                    serde_json::Value::String(text) => Some(text),
                    _ => None,
                };
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// Drain `input` to EOF (or a read error) so the agent that invoked the
/// hook never sees EPIPE, whatever it writes — an agent non-interference
/// rule, not only a privacy one. At most `MAX_PAYLOAD + 1` bytes are kept:
/// enough for `interaction_from` to tell an oversized payload from one that
/// fits, and nothing beyond that is retained.
fn read_payload(mut input: impl Read) -> Vec<u8> {
    let mut payload = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = (MAX_PAYLOAD + 1).saturating_sub(payload.len());
                payload.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    payload
}

/// The source's interaction id from a hook payload, or None. See the module
/// header: allowlisted top-level field, validated UUID, nothing else.
#[cfg(test)]
fn interaction_from(source: &str, payload: &[u8]) -> Option<String> {
    fields_from(source, payload, false).id
}

/// The id and, with `reply` (the `--reply` flag on Claude Code's
/// `turn-done`), the final message, from ONE pass over the payload. Any
/// failure of the pass yields neither.
fn fields_from(source: &str, payload: &[u8], reply: bool) -> Fields {
    let Some(field) = identity_field(source) else {
        return Fields::default();
    };
    if payload.len() > MAX_PAYLOAD {
        return Fields::default();
    }
    let Ok(text) = std::str::from_utf8(payload) else {
        return Fields::default();
    };
    let mut de = serde_json::Deserializer::from_str(text);
    let visitor = TopLevel {
        id: field,
        reply: reply.then_some(REPLY_FIELD),
    };
    let Ok(found) = (&mut de).deserialize_map(visitor) else {
        return Fields::default();
    };
    if de.end().is_err() {
        return Fields::default(); // trailing garbage: malformed
    }
    Fields {
        id: found.id.filter(|id| uuid_ok(id)),
        reply: found.reply.filter(|text| !text.trim().is_empty()),
    }
}

/// `text`, or its first `head` and last `tail` bytes (each cut at a
/// character boundary) joined by `REPLY_CUT` when it is longer.
fn bounded_reply(text: &str, head: usize, tail: usize) -> String {
    if text.len() <= head + tail {
        return text.to_string();
    }
    let mut end = head;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = text.len() - tail;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{}{REPLY_CUT}{}", &text[..end], &text[start..])
}

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

fn build_message(
    source: &str,
    state: &str,
    tmux: &str,
    pane: &str,
    interaction: Option<&str>,
) -> Option<String> {
    if !word_ok(source) || !word_ok(state) || !pane_ok(pane) {
        return None;
    }
    let (socket_name, server_pid) = parse_tmux_env(tmux)?;
    // every field is charset-validated above — plain format! is safe JSON
    Some(match interaction.filter(|id| uuid_ok(id)) {
        Some(id) => format!(
            "{{\"v\":2,\"source\":\"{source}\",\"state\":\"{state}\",\"socket\":\"{socket_name}\",\"server_pid\":{server_pid},\"pane\":\"{pane}\",\"interaction\":\"{id}\"}}\n"
        ),
        None => format!(
            "{{\"v\":1,\"source\":\"{source}\",\"state\":\"{state}\",\"socket\":\"{socket_name}\",\"server_pid\":{server_pid},\"pane\":\"{pane}\"}}\n"
        ),
    })
}

/// The v3 line: `build_message`'s line with `reply` (JSON-escaped by
/// serde_json, the only escaped field) added, cut further until the line
/// fits `MAX_WIRE_LINE`; the plain line when no cut of the reply fits.
fn build_reply_message(
    source: &str,
    state: &str,
    tmux: &str,
    pane: &str,
    interaction: Option<&str>,
    reply: &str,
) -> Option<String> {
    let plain = build_message(source, state, tmux, pane, interaction)?;
    let base = plain.strip_suffix("}\n")?;
    let base = base
        .strip_prefix("{\"v\":2,")
        .or_else(|| base.strip_prefix("{\"v\":1,"))?;
    for (head, tail) in [(REPLY_HEAD, REPLY_TAIL), (1_000, 2_000), (500, 1_000)] {
        let escaped = serde_json::to_string(&bounded_reply(reply, head, tail)).ok()?;
        let line = format!("{{\"v\":3,{base},\"reply\":{escaped}}}\n");
        if line.len() <= MAX_WIRE_LINE {
            return Some(line);
        }
    }
    Some(plain)
}

fn main() {
    // The ONE allowlisted id is read from this buffer below; it is dropped
    // right after.
    let payload = read_payload(std::io::stdin().lock());

    let args: Vec<String> = std::env::args().collect();
    let (Some(source), Some(state)) = (args.get(1), args.get(2)) else {
        return;
    };
    // the reply only where deck's switch put the flag, and only on
    // Claude Code's turn-done
    let wants_reply = args.get(3).map(String::as_str) == Some(REPLY_FLAG)
        && source == "claude-code"
        && state == "turn-done";
    let fields = fields_from(source, &payload, wants_reply);
    drop(payload);
    let (Ok(tmux), Ok(pane)) = (std::env::var("TMUX"), std::env::var("TMUX_PANE")) else {
        return; // not inside tmux — the agent runs outside deck; do nothing
    };
    let interaction = fields.id.as_deref();
    let message = match fields.reply.as_deref() {
        Some(reply) => build_reply_message(source, state, &tmux, &pane, interaction, reply),
        None => build_message(source, state, &tmux, &pane, interaction),
    };
    let Some(message) = message else {
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
    // Stay alive until deck closes the connection: deck binds the event to
    // this pane by walking this process's parent chain through the kernel,
    // which needs this process to still exist. deck closes as soon as it has
    // read the line and the chain (well under a millisecond); the timeout
    // only bounds a deck that stalls.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut ack = [0u8; 64];
    while let Ok(n) = stream.read(&mut ack) {
        if n == 0 {
            break;
        }
    }
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
                "%7",
                None
            )
            .unwrap(),
            "{\"v\":1,\"source\":\"claude-code\",\"state\":\"turn-done\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}\n"
        );
        assert!(build_message("claude-code", "turn-done", "/t/other,1,0", "%7", None).is_none());
        assert!(build_message("bad word", "turn-done", "/t/deck,1,0", "%7", None).is_none());
        assert!(build_message("claude-code", "turn-done", "/t/deck,1,0", "nope", None).is_none());
        // v2 carries exactly one extra, validated field
        assert_eq!(
            build_message("codex", "working", "/t/deck,9,0", "%1", Some(ID)).unwrap(),
            format!("{{\"v\":2,\"source\":\"codex\",\"state\":\"working\",\"socket\":\"deck\",\"server_pid\":9,\"pane\":\"%1\",\"interaction\":\"{ID}\"}}\n")
        );
        // an id that is not a lowercase UUID never reaches the wire
        for bad in [
            "",
            "not-a-uuid",
            "01999999-AAAA-7bbb-8ccc-dddddddddddd",
            "\"}, \"x\":\"1",
        ] {
            assert!(
                build_message("codex", "working", "/t/deck,9,0", "%1", Some(bad))
                    .unwrap()
                    .starts_with("{\"v\":1,")
            );
        }
    }

    /// A reader that counts what the helper consumed from it.
    struct Counted {
        left: usize,
        consumed: usize,
    }

    impl Read for Counted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.left).min(5000); // uneven chunks
            buf[..n].fill(b'a');
            self.left -= n;
            self.consumed += n;
            Ok(n)
        }
    }

    /// Non-interference: an oversized payload is consumed THROUGH EOF (the
    /// agent's write never meets a closed pipe), while no more than
    /// `MAX_PAYLOAD + 1` bytes are kept and no id is read from them.
    #[test]
    fn an_oversized_payload_is_drained_to_eof_but_not_kept() {
        for total in [
            0,
            10,
            MAX_PAYLOAD,
            MAX_PAYLOAD + 1,
            MAX_PAYLOAD + 2,
            3 * MAX_PAYLOAD + 7,
        ] {
            let mut input = Counted {
                left: total,
                consumed: 0,
            };
            let kept = read_payload(&mut input);
            assert_eq!(input.consumed, total, "drained to EOF: {total}");
            assert_eq!(kept.len(), total.min(MAX_PAYLOAD + 1), "kept: {total}");
        }
        // an oversized payload that STARTS with a valid id still yields none
        let mut big = format!(r#"{{"turn_id":"{ID}","prompt":""#).into_bytes();
        big.resize(MAX_PAYLOAD + 100, b'a');
        big.extend_from_slice(br#""}"#);
        let kept = read_payload(big.as_slice());
        assert_eq!(kept.len(), MAX_PAYLOAD + 1);
        assert_eq!(interaction_from("codex", &kept), None);
        // a read error ends the drain without panicking
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("closed"))
            }
        }
        assert!(read_payload(Broken).is_empty());
    }

    const ID: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
    const OTHER: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff9999";

    /// The reply is read only when asked for, only as a top-level string,
    /// never twice, and never at the cost of the id's own rules.
    #[test]
    fn the_reply_is_one_top_level_string_and_only_when_asked() {
        let payload = format!(
            r#"{{"prompt_id":"{ID}","last_assistant_message":"Fixed it.\nCommit?","message":"SENTINEL"}}"#
        );
        let asked = fields_from("claude-code", payload.as_bytes(), true);
        assert_eq!(asked.id.as_deref(), Some(ID));
        assert_eq!(asked.reply.as_deref(), Some("Fixed it.\nCommit?"));
        // not asked: the same payload yields the id alone
        assert_eq!(
            fields_from("claude-code", payload.as_bytes(), false),
            Fields {
                id: Some(ID.into()),
                reply: None
            }
        );
        // a reply that is not a string, or only space, is not kept; the id is
        for other in [
            r#"null"#,
            r#"42"#,
            r#"{"text":"x"}"#,
            r#"["x"]"#,
            r#""  \n ""#,
        ] {
            let payload = format!(r#"{{"prompt_id":"{ID}","last_assistant_message":{other}}}"#);
            let found = fields_from("claude-code", payload.as_bytes(), true);
            assert_eq!(
                found,
                Fields {
                    id: Some(ID.into()),
                    reply: None
                },
                "{other}"
            );
        }
        // a repeated reply key fails the whole pass, as a repeated id does
        let twice = format!(
            r#"{{"prompt_id":"{ID}","last_assistant_message":"a","last_assistant_message":"b"}}"#
        );
        assert_eq!(
            fields_from("claude-code", twice.as_bytes(), true),
            Fields::default()
        );
        // nested, it is somebody else's field
        let nested = format!(r#"{{"prompt_id":"{ID}","x":{{"last_assistant_message":"a"}}}}"#);
        assert_eq!(
            fields_from("claude-code", nested.as_bytes(), true).reply,
            None
        );
        // without an id the reply still travels (a v3 line without interaction)
        let alone = r#"{"last_assistant_message":"Done."}"#;
        assert_eq!(
            fields_from("claude-code", alone.as_bytes(), true),
            Fields {
                id: None,
                reply: Some("Done.".into())
            }
        );
    }

    #[test]
    fn a_long_reply_keeps_its_head_and_tail_on_character_boundaries() {
        assert_eq!(bounded_reply("short", 2, 3), "short");
        let text = format!(
            "{}{}{}",
            "头".repeat(1_000),
            "x".repeat(20_000),
            "尾".repeat(2_000)
        );
        let cut = bounded_reply(&text, REPLY_HEAD, REPLY_TAIL);
        assert!(cut.len() <= REPLY_HEAD + REPLY_TAIL + REPLY_CUT.len());
        assert!(cut.starts_with('头') && cut.ends_with('尾'));
        assert!(cut.contains(REPLY_CUT));
        // 3-byte characters: the cut never lands inside one
        let cut = bounded_reply(&"字".repeat(5_000), 1_000, 1_000);
        assert!(cut
            .split(REPLY_CUT)
            .all(|part| part.chars().all(|c| c == '字')));
    }

    #[test]
    fn a_reply_line_is_escaped_json_that_fits_deck_or_is_not_sent() {
        let line = build_reply_message(
            "claude-code",
            "turn-done",
            "/t/deck,9,0",
            "%1",
            Some(ID),
            "已修好 \"login\"\n要提交吗？\t\u{1}",
        )
        .unwrap();
        assert_eq!(
            line,
            format!("{{\"v\":3,\"source\":\"claude-code\",\"state\":\"turn-done\",\"socket\":\"deck\",\"server_pid\":9,\"pane\":\"%1\",\"interaction\":\"{ID}\",\"reply\":\"已修好 \\\"login\\\"\\n要提交吗？\\t\\u0001\"}}\n")
        );
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(parsed["reply"], "已修好 \"login\"\n要提交吗？\t\u{1}");
        // without an id: v3 all the same, and no interaction
        let line = build_reply_message("claude-code", "turn-done", "/t/deck,9,0", "%1", None, "ok")
            .unwrap();
        assert!(line.starts_with("{\"v\":3,") && !line.contains("interaction"));
        // a long reply is cut to fit the line deck reads
        let line = build_reply_message(
            "claude-code",
            "turn-done",
            "/t/deck,9,0",
            "%1",
            Some(ID),
            &"a".repeat(50_000),
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert!(line.len() <= MAX_WIRE_LINE);
        assert!(parsed["reply"].as_str().unwrap().contains(REPLY_CUT));
        // escaping that cannot fit even the smallest cut: the plain line
        let line = build_reply_message(
            "claude-code",
            "turn-done",
            "/t/deck,9,0",
            "%1",
            Some(ID),
            &"\u{1}".repeat(6_000),
        )
        .unwrap();
        assert!(line.starts_with("{\"v\":2,") && !line.contains("reply"));
        // nothing for an invalid pane, as the plain line
        assert!(
            build_reply_message("claude-code", "turn-done", "/t/deck,9,0", "nope", None, "x")
                .is_none()
        );
    }

    fn codex(payload: &str) -> Option<String> {
        interaction_from("codex", payload.as_bytes())
    }

    #[test]
    fn the_source_allowlist_names_one_top_level_field_each() {
        let turn =
            format!(r#"{{"hook_event_name":"Stop","turn_id":"{ID}","session_id":"{OTHER}"}}"#);
        assert_eq!(codex(&turn), Some(ID.into()));
        let prompt =
            format!(r#"{{"hook_event_name":"Stop","prompt_id":"{ID}","session_id":"{OTHER}"}}"#);
        assert_eq!(
            interaction_from("claude-code", prompt.as_bytes()),
            Some(ID.into())
        );
        // a valid UUID under the other source's field, or another field
        assert_eq!(interaction_from("claude-code", turn.as_bytes()), None);
        assert_eq!(codex(&prompt), None);
        assert_eq!(
            codex(&format!(r#"{{"session_id":"{ID}","tool_use_id":"{ID}"}}"#)),
            None
        );
        // an unknown source reads nothing
        assert_eq!(interaction_from("mystery", turn.as_bytes()), None);
    }

    /// Negative fixtures: content in every shape a payload can carry. None
    /// of it can become the id, and only the validated id can leave.
    #[test]
    fn nothing_but_the_validated_top_level_id_can_be_extracted() {
        // escaped quotes/backslashes and a fake identity inside a prompt
        let fake = format!(
            r#"{{"prompt":"say \"turn_id\":\"{OTHER}\" and \\ then","cwd":"/Users/me/secret","turn_id":"{ID}"}}"#
        );
        assert_eq!(codex(&fake), Some(ID.into()));
        let only_fake = format!(r#"{{"prompt":"{{\"turn_id\":\"{OTHER}\"}}","cwd":"/x"}}"#);
        assert_eq!(codex(&only_fake), None, "an id inside a string is text");
        // unicode escapes are decoded like any JSON string, then validated
        assert_eq!(
            codex(r#"{"turn_id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000"}"#),
            Some(ID.into())
        );
        assert_eq!(codex(r#"{"turn_id":"éé"}"#), None);
        assert_eq!(
            codex(r#"{"turn_id":"\ud800"}"#),
            None,
            "a lone surrogate is malformed"
        );
        // an escaped key is still that key: two spellings of it are a duplicate
        assert_eq!(
            codex(&format!(r#"{{"turn_id":"{ID}","turn_id":"{OTHER}"}}"#)),
            None
        );
        // same-named nested keys are not the top-level field
        let nested =
            format!(r#"{{"tool_input":{{"turn_id":"{OTHER}","list":[{{"turn_id":"{OTHER}"}}]}}}}"#);
        assert_eq!(codex(&nested), None);
        let both = format!(r#"{{"tool_input":{{"turn_id":"{OTHER}"}},"turn_id":"{ID}"}}"#);
        assert_eq!(codex(&both), Some(ID.into()));
        // duplicate top-level keys fail, whichever order
        assert_eq!(
            codex(&format!(r#"{{"turn_id":"{ID}","turn_id":"{OTHER}"}}"#)),
            None
        );
        assert_eq!(
            codex(&format!(r#"{{"turn_id":"{ID}","x":1,"turn_id":"{ID}"}}"#)),
            None
        );
        // wrong types and invalid ids
        for value in [
            "1",
            "null",
            "true",
            "[]",
            "{}",
            r#""TURN""#,
            r#""0199AAAA-BBBB-7CCC-8DDD-EEEEFFFF0000""#,
        ] {
            assert_eq!(codex(&format!(r#"{{"turn_id":{value}}}"#)), None, "{value}");
        }
        // malformed, truncated, trailing garbage, not an object
        for doc in [
            "",
            "{",
            "[1,2]",
            "\"x\"",
            "null",
            &format!(r#"{{"turn_id":"{ID}""#),
            &format!(r#"{{"turn_id":"{ID}"}} trailing"#),
            &format!(r#"{{"turn_id":"{ID}",}}"#),
        ] {
            assert_eq!(codex(doc), None, "{doc:?}");
        }
        // invalid UTF-8 anywhere
        let mut bytes = format!(r#"{{"prompt":"x","turn_id":"{ID}"}}"#).into_bytes();
        bytes[12] = 0xff;
        assert_eq!(interaction_from("codex", &bytes), None);
        // oversized stdin
        let big = format!(
            r#"{{"prompt":"{}","turn_id":"{ID}"}}"#,
            "a".repeat(MAX_PAYLOAD)
        );
        assert_eq!(codex(&big), None);
        // and the message never carries anything from the payload but the id
        let message = build_message(
            "codex",
            "turn-done",
            "/t/deck,9,0",
            "%1",
            codex(&fake).as_deref(),
        )
        .unwrap();
        for leaked in ["secret", "Users", "say", OTHER] {
            assert!(!message.contains(leaked), "{leaked} in {message}");
        }
    }
}
