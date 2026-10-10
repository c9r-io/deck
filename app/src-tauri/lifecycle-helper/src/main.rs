//! deck-lifecycle-helper — invoked by an agent's session lifecycle hooks
//! (Claude Code `SessionStart` / `SessionEnd` today) inside a deck tmux pane.
//! It forwards ONE closed lifecycle fact plus the pane identity it inherited
//! from the environment to deck's local status socket, then exits 0 no matter
//! what: a hook helper must never break the agent that invoked it, and it has
//! nowhere safe to log. No stdout, no stderr, no log, no file.
//!
//! CLI: `deck-lifecycle-helper <source> <event>` with source exactly
//! `claude-code` and event exactly `session-start` or `session-end`. Any other
//! argv sends nothing.
//!
//! A separate closed protocol family. This helper never extends
//! `deck-status-helper`'s payload contract (that helper still reads exactly
//! one interaction id and speaks status v1/v2); the two share the 0600 status
//! socket and nothing else. Every line written here names its family
//! (`"protocol":"agent-lifecycle"`), so a host that does not know the family
//! refuses the line and this helper still exits 0.
//!
//! Privacy: the hook's stdin payload never leaves this process and is never
//! logged or stored. Only these TOP-LEVEL keys are read, every other key and
//! value is skipped (and still validated) by serde's `IgnoredAny` — prompt,
//! tool data, `transcript_path`, `cwd`, `model`, `permission_mode` and the
//! rest are never copied into owned memory or forwarded:
//!
//! - `session_id`: a string that is a lowercase UUID, forwarded as
//!   `nativeSessionId`.
//! - session-start only, `source`: mapped to the closed word
//!   `startup | resume | clear | compact`, any other string to `other`.
//! - session-end only, `reason`: mapped to `clear | resume | logout`,
//!   `prompt_input_exit` to `prompt-input-exit`, any other string to `other`.
//!
//! The native strings are compared in place and only the mapped static word
//! is kept, so an unknown value can never reach the wire. A missing, nested,
//! wrong-typed or invalid field, a duplicate of ANY allowlisted key,
//! malformed JSON, a top level that is not an object, invalid UTF-8, trailing
//! JSON and an oversized payload all send nothing. The payload buffer is
//! dropped right after that extraction. This helper version never sends a
//! `finalPermission` field.
//!
//! Wire: exactly one JSON line of at most `MAX_LINE` bytes, fixed key order,
//! built only from validated fields (so it needs no escaping):
//!
//! ```text
//! {"protocol":"agent-lifecycle","v":1,"source":"claude-code","event":"session-start","nativeSessionId":"<uuid>","startSource":"<word>","socket":"<name>","server_pid":<n>,"pane":"%<n>"}
//! {"protocol":"agent-lifecycle","v":1,"source":"claude-code","event":"session-end","nativeSessionId":"<uuid>","endReason":"<word>","socket":"<name>","server_pid":<n>,"pane":"%<n>"}
//! ```
//!
//! Identity: `socket`, `server_pid` and `pane` are claims inherited from
//! `$TMUX` / `$TMUX_PANE`, validated for shape only; deck proves them by
//! asking the kernel which process connected and walking its parents. The
//! helper therefore keeps the connection open until deck closes it. Routing
//! is the status helper's: `DECK_STATUS_SOCK` in the exact expected shape,
//! else `~/.deck/status.sock`. Outside a deck tmux pane nothing is sent.
//!
//! Bounds: at most `MAX_PAYLOAD` bytes of stdin are inspected; stdin as a
//! whole gets `STDIN_BUDGET`, and a writer that never closes it yields no
//! message. Connect, write and wait-for-close share ONE `SOCKET_BUDGET`
//! deadline. Both are enforced from the main thread over a channel, so a
//! blocked read or connect cannot outlive its budget; a timeout reports
//! nothing.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};

/// The largest hook payload the helper will inspect; beyond it nothing is
/// read from it (the rest is still drained, within the stdin budget, so the
/// agent does not meet a closed pipe).
const MAX_PAYLOAD: usize = 1 << 20;

/// The longest line this helper will write.
const MAX_LINE: usize = 1024;

/// Total time stdin may take to reach EOF.
const STDIN_BUDGET: Duration = Duration::from_millis(300);

/// One deadline for connect + write + wait-for-close.
const SOCKET_BUDGET: Duration = Duration::from_millis(500);

/// The only agent source this helper speaks for.
const SOURCE: &str = "claude-code";

/// The lifecycle event named by argv.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    SessionStart,
    SessionEnd,
}

/// `<program> <source> <event>`, exact spellings and nothing after them.
fn parse_args(args: &[String]) -> Option<Event> {
    let [_, source, event] = args else {
        return None;
    };
    if source != SOURCE {
        return None;
    }
    match event.as_str() {
        "session-start" => Some(Event::SessionStart),
        "session-end" => Some(Event::SessionEnd),
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

/// Claude Code's SessionStart `source`, as the closed wire word.
fn start_source_word(native: &str) -> &'static str {
    match native {
        "startup" => "startup",
        "resume" => "resume",
        "clear" => "clear",
        "compact" => "compact",
        _ => "other",
    }
}

/// Claude Code's SessionEnd `reason`, as the closed wire word.
fn end_reason_word(native: &str) -> &'static str {
    match native {
        "clear" => "clear",
        "resume" => "resume",
        "logout" => "logout",
        "prompt_input_exit" => "prompt-input-exit",
        _ => "other",
    }
}

/// The closed set a detail word must belong to before it is written.
fn detail_word_ok(event: Event, word: &str) -> bool {
    match event {
        Event::SessionStart => {
            matches!(word, "startup" | "resume" | "clear" | "compact" | "other")
        }
        Event::SessionEnd => matches!(
            word,
            "clear" | "resume" | "logout" | "prompt-input-exit" | "other"
        ),
    }
}

/// Everything a payload may contribute to the wire.
#[derive(Debug, PartialEq, Eq)]
struct Fields {
    /// Validated lowercase UUID.
    session_id: String,
    /// The mapped start source or end reason, always a static closed word.
    detail: &'static str,
}

/// Deserializes one JSON string by handing it, still in the parser's
/// buffer, to `0`; only what `0` returns is kept. Any other JSON type is a
/// type error.
struct Str<F>(F);

impl<'de, T, F: FnOnce(&str) -> T> DeserializeSeed<'de> for Str<F> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<T, D::Error> {
        deserializer.deserialize_str(self)
    }
}

impl<'de, T, F: FnOnce(&str) -> T> Visitor<'de> for Str<F> {
    type Value = T;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON string")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<T, E> {
        Ok((self.0)(v))
    }
}

/// The top-level keys this helper tells apart; every other key is `Other`.
#[derive(Clone, Copy)]
enum Key {
    SessionId,
    Source,
    Reason,
    Other,
}

fn key_of(name: &str) -> Key {
    match name {
        "session_id" => Key::SessionId,
        "source" => Key::Source,
        "reason" => Key::Reason,
        _ => Key::Other,
    }
}

/// Reads a JSON document whose top level is an object, keeping only the
/// allowlisted fields of `0`; every other value is skipped (and still
/// validated) by `IgnoredAny`. A second occurrence of any allowlisted key —
/// also the one the event does not use — is an error, never first- or
/// last-wins.
struct TopLevel(Event);

impl<'de> Visitor<'de> for TopLevel {
    type Value = Option<Fields>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let duplicate = || serde::de::Error::custom("duplicate allowlisted key");
        let mut session_id: Option<String> = None;
        let mut detail: Option<&'static str> = None;
        let (mut seen_id, mut seen_source, mut seen_reason) = (false, false, false);
        while let Some(key) = map.next_key_seed(Str(key_of))? {
            let seen = match key {
                Key::SessionId => &mut seen_id,
                Key::Source => &mut seen_source,
                Key::Reason => &mut seen_reason,
                Key::Other => {
                    map.next_value::<IgnoredAny>()?;
                    continue;
                }
            };
            if std::mem::replace(seen, true) {
                return Err(duplicate());
            }
            match (key, self.0) {
                (Key::SessionId, _) => {
                    // copied only once it is known to be a UUID
                    session_id = Some(
                        map.next_value_seed(Str(|v: &str| uuid_ok(v).then(|| v.to_string())))?
                            .ok_or_else(|| serde::de::Error::custom("invalid session id"))?,
                    );
                }
                (Key::Source, Event::SessionStart) => {
                    detail = Some(map.next_value_seed(Str(start_source_word))?);
                }
                (Key::Reason, Event::SessionEnd) => {
                    detail = Some(map.next_value_seed(Str(end_reason_word))?);
                }
                // the other event's key: counted for duplicates, never read
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(match (session_id, detail) {
            (Some(session_id), Some(detail)) => Some(Fields { session_id, detail }),
            _ => None,
        })
    }
}

/// The lifecycle fields of a hook payload, or None. See the module header:
/// allowlisted top-level keys, validated UUID, closed words, nothing else.
fn extract(payload: &[u8], event: Event) -> Option<Fields> {
    if payload.len() > MAX_PAYLOAD {
        return None;
    }
    let text = std::str::from_utf8(payload).ok()?;
    let mut de = serde_json::Deserializer::from_str(text);
    let fields = (&mut de).deserialize_map(TopLevel(event)).ok()??;
    de.end().ok()?; // trailing JSON: malformed
    Some(fields)
}

/// Drain `input` to EOF (or a read error) so the agent that invoked the hook
/// does not see EPIPE for a payload it closes in time. At most
/// `MAX_PAYLOAD + 1` bytes are kept: enough for `extract` to tell an
/// oversized payload from one that fits, and nothing beyond that is retained.
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

/// `read_payload` under a total budget. The read runs on its own thread and
/// the caller waits on a channel, so an input that never reaches EOF costs
/// `budget` and yields None; the reader thread ends with the process.
fn read_payload_within(input: impl Read + Send + 'static, budget: Duration) -> Option<Vec<u8>> {
    let (done, wait) = mpsc::channel();
    std::thread::Builder::new()
        .spawn(move || {
            let _ = done.send(read_payload(input));
        })
        .ok()?;
    wait.recv_timeout(budget).ok()
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
fn socket_path(env: Option<&str>, home: &Path) -> PathBuf {
    if let Some(path) = env {
        let p = Path::new(path);
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

/// The one wire line for `event`, or None when any field is not in its
/// validated shape. Every field is re-checked here, so plain `format!` is
/// safe JSON whatever the caller passed.
fn build_line(event: Event, fields: &Fields, tmux: &str, pane: &str) -> Option<String> {
    if !uuid_ok(&fields.session_id) || !detail_word_ok(event, fields.detail) || !pane_ok(pane) {
        return None;
    }
    let (socket_name, server_pid) = parse_tmux_env(tmux)?;
    let (event_word, detail_key) = match event {
        Event::SessionStart => ("session-start", "startSource"),
        Event::SessionEnd => ("session-end", "endReason"),
    };
    let line = format!(
        "{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"{SOURCE}\",\"event\":\"{event_word}\",\"nativeSessionId\":\"{id}\",\"{detail_key}\":\"{detail}\",\"socket\":\"{socket_name}\",\"server_pid\":{server_pid},\"pane\":\"{pane}\"}}\n",
        id = fields.session_id,
        detail = fields.detail,
    );
    (line.len() <= MAX_LINE).then_some(line)
}

/// The line this invocation would send, from argv, the payload and the
/// inherited tmux claims — or None, the silent case.
fn message_for(
    args: &[String],
    payload: &[u8],
    tmux: Option<&str>,
    pane: Option<&str>,
) -> Option<String> {
    let event = parse_args(args)?;
    let fields = extract(payload, event)?;
    build_line(event, &fields, tmux?, pane?)
}

/// Time left before `deadline`, None once it has passed.
fn remaining(deadline: Instant) -> Option<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    (!left.is_zero()).then_some(left)
}

/// Connect, write the line, then stay connected until deck closes: deck
/// binds the event to this pane by walking this process's parent chain
/// through the kernel, which needs this process to still exist. Every step
/// stops at `deadline`.
fn exchange(path: &Path, line: &[u8], deadline: Instant) {
    // deck not running (or an old deck without the socket) — silently done
    let Ok(mut stream) = UnixStream::connect(path) else {
        return;
    };
    let Some(left) = remaining(deadline) else {
        return;
    };
    if stream.set_write_timeout(Some(left)).is_err() || stream.write_all(line).is_err() {
        return;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut ack = [0u8; 64];
    loop {
        let Some(left) = remaining(deadline) else {
            return;
        };
        if stream.set_read_timeout(Some(left)).is_err() {
            return;
        }
        match stream.read(&mut ack) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

/// `exchange` under ONE total deadline that also covers the connect: the
/// exchange runs on its own thread and the caller stops waiting at the
/// deadline whatever that thread is blocked in.
fn deliver(path: PathBuf, line: String, budget: Duration) {
    let deadline = Instant::now() + budget;
    let (done, wait) = mpsc::channel();
    let started = std::thread::Builder::new().spawn(move || {
        exchange(&path, line.as_bytes(), deadline);
        let _ = done.send(());
    });
    if started.is_ok() {
        let _ = wait.recv_timeout(budget);
    }
}

fn main() {
    // Stdin is drained whatever argv says, within its budget; the allowlisted
    // fields are read from this buffer below and it is dropped right after.
    let Some(payload) = read_payload_within(std::io::stdin(), STDIN_BUDGET) else {
        return; // a writer that never closes stdin: no message
    };
    let args: Vec<String> = std::env::args().collect();
    let tmux = std::env::var("TMUX").ok();
    let pane = std::env::var("TMUX_PANE").ok();
    // None outside tmux too: the agent runs outside deck; do nothing
    let message = message_for(&args, &payload, tmux.as_deref(), pane.as_deref());
    drop(payload);
    let Some(message) = message else {
        return;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let env_sock = std::env::var("DECK_STATUS_SOCK").ok();
    let path = socket_path(env_sock.as_deref(), Path::new(&home));
    deliver(path, message, SOCKET_BUDGET);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
    const OTHER: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff9999";
    const TMUX: &str = "/private/tmp/tmux-501/deck-dev,4242,0";

    fn start(payload: &str) -> Option<Fields> {
        extract(payload.as_bytes(), Event::SessionStart)
    }

    fn end(payload: &str) -> Option<Fields> {
        extract(payload.as_bytes(), Event::SessionEnd)
    }

    fn fields(detail: &'static str) -> Fields {
        Fields {
            session_id: ID.into(),
            detail,
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn argv_accepts_only_the_two_exact_invocations() {
        assert_eq!(
            parse_args(&args(&["h", "claude-code", "session-start"])),
            Some(Event::SessionStart)
        );
        assert_eq!(
            parse_args(&args(&["h", "claude-code", "session-end"])),
            Some(Event::SessionEnd)
        );
        for bad in [
            &["h"][..],
            &["h", "claude-code"],
            &["h", "codex", "session-start"],
            &["h", "Claude-Code", "session-start"],
            &["h", "claude-code", "SessionStart"],
            &["h", "claude-code", "session_start"],
            &["h", "claude-code", "turn-done"],
            &["h", "claude-code", "session-start", "extra"],
            &["h", "session-start", "claude-code"],
            &["h", "claude-code", ""],
        ] {
            assert_eq!(parse_args(&args(bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn an_unknown_argv_produces_no_message_from_a_valid_payload() {
        let payload = format!(r#"{{"session_id":"{ID}","source":"startup","reason":"clear"}}"#);
        for bad in [
            &["h"][..],
            &["h", "codex", "session-start"],
            &["h", "claude-code", "needs-input"],
            &["h", "claude-code", "session-end", "x"],
        ] {
            assert_eq!(
                message_for(&args(bad), payload.as_bytes(), Some(TMUX), Some("%7")),
                None,
                "{bad:?}"
            );
        }
        // the same payload does produce one for a known argv
        assert!(message_for(
            &args(&["h", "claude-code", "session-start"]),
            payload.as_bytes(),
            Some(TMUX),
            Some("%7")
        )
        .is_some());
    }

    #[test]
    fn outside_tmux_nothing_is_built() {
        let payload = format!(r#"{{"session_id":"{ID}","source":"startup"}}"#);
        let argv = args(&["h", "claude-code", "session-start"]);
        assert_eq!(
            message_for(&argv, payload.as_bytes(), None, Some("%7")),
            None
        );
        assert_eq!(
            message_for(&argv, payload.as_bytes(), Some(TMUX), None),
            None
        );
        assert_eq!(
            message_for(
                &argv,
                payload.as_bytes(),
                Some("/private/tmp/tmux-501/default,4242,0"),
                Some("%7")
            ),
            None,
            "a tmux server that is not deck's"
        );
    }

    #[test]
    fn a_valid_start_payload_yields_the_exact_line() {
        let payload = format!(
            r#"{{"hook_event_name":"SessionStart","session_id":"{ID}","source":"resume"}}"#
        );
        assert_eq!(start(&payload), Some(fields("resume")));
        assert_eq!(
            message_for(
                &args(&["h", "claude-code", "session-start"]),
                payload.as_bytes(),
                Some(TMUX),
                Some("%7")
            )
            .unwrap(),
            format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-start\",\"nativeSessionId\":\"{ID}\",\"startSource\":\"resume\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}}\n")
        );
    }

    #[test]
    fn a_valid_end_payload_yields_the_exact_line() {
        let payload = format!(
            r#"{{"reason":"prompt_input_exit","hook_event_name":"SessionEnd","session_id":"{ID}"}}"#
        );
        assert_eq!(end(&payload), Some(fields("prompt-input-exit")));
        assert_eq!(
            message_for(
                &args(&["h", "claude-code", "session-end"]),
                payload.as_bytes(),
                Some("/t/deck,9,3"),
                Some("%0")
            )
            .unwrap(),
            format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-end\",\"nativeSessionId\":\"{ID}\",\"endReason\":\"prompt-input-exit\",\"socket\":\"deck\",\"server_pid\":9,\"pane\":\"%0\"}}\n")
        );
    }

    #[test]
    fn every_start_source_maps_to_its_closed_word() {
        for (native, word) in [
            ("startup", "startup"),
            ("resume", "resume"),
            ("clear", "clear"),
            ("compact", "compact"),
            ("fork", "other"),
            ("Resume", "other"),
            ("resume ", "other"),
            ("", "other"),
            ("prompt_input_exit", "other"),
            ("logout", "other"),
        ] {
            assert_eq!(start_source_word(native), word, "{native:?}");
            let payload = format!(r#"{{"session_id":"{ID}","source":"{native}"}}"#);
            assert_eq!(start(&payload), Some(fields(word)), "{native:?}");
            assert!(detail_word_ok(Event::SessionStart, word));
        }
    }

    #[test]
    fn every_end_reason_maps_to_its_closed_word() {
        for (native, word) in [
            ("clear", "clear"),
            ("resume", "resume"),
            ("logout", "logout"),
            ("prompt_input_exit", "prompt-input-exit"),
            ("prompt-input-exit", "other"),
            ("bypass_permissions_disabled", "other"),
            ("other", "other"),
            ("startup", "other"),
            ("compact", "other"),
            ("", "other"),
        ] {
            assert_eq!(end_reason_word(native), word, "{native:?}");
            let payload = format!(r#"{{"session_id":"{ID}","reason":"{native}"}}"#);
            assert_eq!(end(&payload), Some(fields(word)), "{native:?}");
            assert!(detail_word_ok(Event::SessionEnd, word));
        }
    }

    #[test]
    fn an_unknown_native_value_never_reaches_the_wire() {
        let hostile = r#"x\",\"pane\":\"%9"#;
        let payload = format!(r#"{{"session_id":"{ID}","source":"{hostile}"}}"#);
        let line = message_for(
            &args(&["h", "claude-code", "session-start"]),
            payload.as_bytes(),
            Some(TMUX),
            Some("%7"),
        )
        .unwrap();
        assert!(line.contains("\"startSource\":\"other\""));
        assert!(!line.contains("%9"));
    }

    #[test]
    fn each_event_reads_only_its_own_detail_key() {
        // the other event's key is not a substitute
        assert_eq!(
            start(&format!(r#"{{"session_id":"{ID}","reason":"clear"}}"#)),
            None
        );
        assert_eq!(
            end(&format!(r#"{{"session_id":"{ID}","source":"resume"}}"#)),
            None
        );
        // present beside the right one, it is skipped whatever its type
        assert_eq!(
            start(&format!(
                r#"{{"session_id":"{ID}","reason":{{"a":[1]}},"source":"clear"}}"#
            )),
            Some(fields("clear"))
        );
        assert_eq!(
            end(&format!(
                r#"{{"source":7,"session_id":"{ID}","reason":"logout"}}"#
            )),
            Some(fields("logout"))
        );
    }

    #[test]
    fn a_missing_field_sends_nothing() {
        assert_eq!(start(&format!(r#"{{"session_id":"{ID}"}}"#)), None);
        assert_eq!(end(&format!(r#"{{"session_id":"{ID}"}}"#)), None);
        assert_eq!(start(r#"{"source":"startup"}"#), None);
        assert_eq!(end(r#"{"reason":"logout"}"#), None);
        assert_eq!(start("{}"), None);
    }

    #[test]
    fn a_session_id_that_is_not_a_lowercase_uuid_sends_nothing() {
        for bad in [
            "0199AAAA-BBBB-7CCC-8DDD-EEEEFFFF0000",
            "0199aaaa-bbbb-7ccc-8ddd-eeeeffff000",
            "0199aaaa-bbbb-7ccc-8ddd-eeeeffff00000",
            "0199aaaabbbb7ccc8dddeeeeffff0000",
            "0199aaaa_bbbb_7ccc_8ddd_eeeeffff0000",
            "0199aaaa-bbbb-7ccc-8ddd-eeeeffff000g",
            " 0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "",
            "session",
        ] {
            assert!(!uuid_ok(bad), "{bad:?}");
            assert_eq!(
                start(&format!(r#"{{"session_id":"{bad}","source":"startup"}}"#)),
                None,
                "{bad:?}"
            );
            assert_eq!(
                end(&format!(r#"{{"reason":"logout","session_id":"{bad}"}}"#)),
                None,
                "{bad:?}"
            );
        }
        assert!(uuid_ok(ID));
        // unicode escapes are decoded like any JSON string, then validated
        assert_eq!(
            start(&format!(
                r#"{{"session_id":"0{}","source":"startup"}}"#,
                &ID[1..]
            )),
            Some(fields("startup"))
        );
    }

    #[test]
    fn a_duplicate_of_any_allowlisted_key_sends_nothing() {
        for doc in [
            format!(r#"{{"session_id":"{ID}","session_id":"{ID}","source":"startup"}}"#),
            format!(r#"{{"session_id":"{ID}","source":"startup","session_id":"{OTHER}"}}"#),
            format!(r#"{{"session_id":"{ID}","source":"startup","source":"startup"}}"#),
            format!(r#"{{"source":"resume","x":1,"session_id":"{ID}","source":"startup"}}"#),
            // the key the event does not use is allowlisted too
            format!(r#"{{"session_id":"{ID}","source":"startup","reason":1,"reason":2}}"#),
            // an escaped spelling is still that key
            format!(r#"{{"session_id":"{ID}","source":"startup","session_id":"{ID}"}}"#),
        ] {
            assert_eq!(start(&doc), None, "{doc}");
        }
        for doc in [
            format!(r#"{{"session_id":"{ID}","reason":"logout","reason":"clear"}}"#),
            format!(r#"{{"session_id":"{ID}","reason":"logout","session_id":"{ID}"}}"#),
            format!(r#"{{"session_id":"{ID}","reason":"logout","source":"a","source":"b"}}"#),
        ] {
            assert_eq!(end(&doc), None, "{doc}");
        }
        // an ordinary duplicate outside the allowlist is not this rule
        assert_eq!(
            start(&format!(
                r#"{{"cwd":"/a","cwd":"/b","session_id":"{ID}","source":"startup"}}"#
            )),
            Some(fields("startup"))
        );
    }

    #[test]
    fn a_nested_key_is_not_the_top_level_field() {
        assert_eq!(
            start(&format!(
                r#"{{"hook":{{"session_id":"{ID}","source":"startup"}}}}"#
            )),
            None
        );
        assert_eq!(
            start(&format!(
                r#"{{"tool_input":{{"session_id":"{OTHER}"}},"source":"startup"}}"#
            )),
            None
        );
        assert_eq!(
            end(&format!(
                r#"{{"list":[{{"session_id":"{ID}","reason":"logout"}}],"reason":"logout"}}"#
            )),
            None
        );
        // nested copies beside real top-level fields change nothing
        assert_eq!(
            start(&format!(
                r#"{{"tool_input":{{"session_id":"{OTHER}","source":"resume"}},"session_id":"{ID}","source":"clear"}}"#
            )),
            Some(fields("clear"))
        );
        // a key inside a string is text
        assert_eq!(
            start(&format!(
                r#"{{"prompt":"{{\"session_id\":\"{ID}\",\"source\":\"startup\"}}"}}"#
            )),
            None
        );
    }

    #[test]
    fn a_wrong_typed_field_sends_nothing() {
        for value in ["1", "null", "true", "[]", "{}", r#"["startup"]"#] {
            assert_eq!(
                start(&format!(r#"{{"session_id":"{ID}","source":{value}}}"#)),
                None,
                "source {value}"
            );
            assert_eq!(
                end(&format!(r#"{{"session_id":"{ID}","reason":{value}}}"#)),
                None,
                "reason {value}"
            );
            assert_eq!(
                start(&format!(r#"{{"session_id":{value},"source":"startup"}}"#)),
                None,
                "session_id {value}"
            );
        }
        assert_eq!(
            start(&format!(r#"{{"session_id":["{ID}"],"source":"startup"}}"#)),
            None
        );
    }

    #[test]
    fn malformed_documents_send_nothing() {
        let good = format!(r#"{{"session_id":"{ID}","source":"startup"}}"#);
        assert!(start(&good).is_some());
        for doc in [
            String::new(),
            "{".into(),
            "null".into(),
            "\"x\"".into(),
            "[1,2]".into(),
            format!("[{good}]"),
            format!(r#"{{"session_id":"{ID}","source":"startup""#),
            format!(r#"{{"session_id":"{ID}","source":"startup",}}"#),
            // trailing JSON, whatever follows
            format!("{good} trailing"),
            format!("{good}{good}"),
            format!("{good}\n{{}}"),
            format!("{good}]"),
            // a lone surrogate is malformed
            format!(r#"{{"session_id":"{ID}","source":"\ud800"}}"#),
        ] {
            assert_eq!(start(&doc), None, "{doc:?}");
        }
        // trailing whitespace is not trailing JSON
        assert!(start(&format!("{good}\n  ")).is_some());
    }

    #[test]
    fn invalid_utf8_anywhere_sends_nothing() {
        let good = format!(r#"{{"prompt":"x","session_id":"{ID}","source":"startup"}}"#);
        assert!(extract(good.as_bytes(), Event::SessionStart).is_some());
        let mut bytes = good.into_bytes();
        bytes[11] = 0xff; // inside the skipped prompt value
        assert_eq!(extract(&bytes, Event::SessionStart), None);
    }

    #[test]
    fn an_oversized_payload_sends_nothing() {
        let fits = format!(
            r#"{{"session_id":"{ID}","source":"startup","prompt":"{}"}}"#,
            "a".repeat(1000)
        );
        assert!(start(&fits).is_some());
        // valid fields first, then padding past the limit
        let big = format!(
            r#"{{"session_id":"{ID}","source":"startup","prompt":"{}"}}"#,
            "a".repeat(MAX_PAYLOAD)
        );
        assert!(big.len() > MAX_PAYLOAD);
        assert_eq!(start(&big), None);
        // and through the bounded read: capped, then refused
        let kept = read_payload(big.as_bytes());
        assert_eq!(kept.len(), MAX_PAYLOAD + 1);
        assert_eq!(extract(&kept, Event::SessionStart), None);
        // exactly at the limit is still inspected
        let overhead = format!(r#"{{"session_id":"{ID}","source":"startup","prompt":""}}"#).len();
        let edge = format!(
            r#"{{"session_id":"{ID}","source":"startup","prompt":"{}"}}"#,
            "a".repeat(MAX_PAYLOAD - overhead)
        );
        assert_eq!(edge.len(), MAX_PAYLOAD);
        assert_eq!(start(&edge), Some(fields("startup")));
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

    #[test]
    fn stdin_is_drained_to_eof_but_never_kept_past_the_limit() {
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
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("closed"))
            }
        }
        assert!(read_payload(Broken).is_empty());
    }

    #[test]
    fn a_reader_that_never_reaches_eof_costs_only_the_budget() {
        // the write end stays open, so the read end never sees EOF
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer
            .write_all(format!(r#"{{"session_id":"{ID}","source":"startup"}}"#).as_bytes())
            .unwrap();
        let started = Instant::now();
        assert_eq!(
            read_payload_within(reader, Duration::from_millis(100)),
            None
        );
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(100), "{took:?}");
        assert!(took < Duration::from_secs(2), "{took:?}");
        drop(writer);
        // a closed writer is read in full well inside the budget
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"{}").unwrap();
        drop(writer);
        assert_eq!(
            read_payload_within(reader, Duration::from_secs(5)),
            Some(b"{}".to_vec())
        );
    }

    /// Every field a real payload carries besides the allowlisted ones,
    /// stuffed with sentinels: the line is exactly the allowed keys and
    /// values, and no sentinel is anywhere in it.
    #[test]
    fn a_payload_full_of_content_yields_only_the_allowed_keys() {
        let start_payload = format!(
            r#"{{
                "hook_event_name": "SENTINEL_EVENT",
                "prompt": "SENTINEL_PROMPT says \"session_id\":\"{OTHER}\" and \\ more",
                "tool_name": "SENTINEL_TOOL",
                "tool_input": {{"command": "SENTINEL_COMMAND", "session_id": "{OTHER}", "source": "resume"}},
                "tool_response": ["SENTINEL_OUTPUT", {{"reason": "SENTINEL_NESTED"}}],
                "transcript_path": "/Users/SENTINEL_USER/.claude/projects/SENTINEL_TRANSCRIPT.jsonl",
                "cwd": "/Users/SENTINEL_USER/SENTINEL_CWD",
                "model": "SENTINEL_MODEL",
                "permission_mode": "SENTINEL_PERMISSION",
                "agent_type": "SENTINEL_AGENT",
                "title": "SENTINEL_TITLE",
                "reason": "SENTINEL_REASON",
                "session_id": "{ID}",
                "source": "startup",
                "message": "SENTINEL_MESSAGE"
            }}"#
        );
        let line = message_for(
            &args(&["h", "claude-code", "session-start"]),
            start_payload.as_bytes(),
            Some(TMUX),
            Some("%7"),
        )
        .unwrap();
        assert_eq!(
            line,
            format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-start\",\"nativeSessionId\":\"{ID}\",\"startSource\":\"startup\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}}\n")
        );
        let end_payload = start_payload.replace(
            r#""reason": "SENTINEL_REASON""#,
            r#""reason": "SENTINEL_REASON_UNKNOWN""#,
        );
        let end_line = message_for(
            &args(&["h", "claude-code", "session-end"]),
            end_payload.as_bytes(),
            Some(TMUX),
            Some("%7"),
        )
        .unwrap();
        assert_eq!(
            end_line,
            format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-end\",\"nativeSessionId\":\"{ID}\",\"endReason\":\"other\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}}\n")
        );
        for line in [&line, &end_line] {
            for leaked in [
                "SENTINEL",
                "Users",
                "transcript",
                "cwd",
                "model",
                "permission",
                "finalPermission",
                "prompt\"",
                OTHER,
            ] {
                assert!(!line.contains(leaked), "{leaked} in {line}");
            }
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            let keys: Vec<&str> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let detail_key = if line.contains("session-start") {
                "startSource"
            } else {
                "endReason"
            };
            let mut allowed = vec![
                "protocol",
                "v",
                "source",
                "event",
                "nativeSessionId",
                detail_key,
                "socket",
                "server_pid",
                "pane",
            ];
            allowed.sort_unstable();
            let mut keys_sorted = keys.clone();
            keys_sorted.sort_unstable();
            assert_eq!(keys_sorted, allowed);
        }
    }

    #[test]
    fn the_line_is_one_line_within_the_limit() {
        // the longest value every field can validly take
        let tmux = format!(
            "/private/tmp/tmux-501/deck{},{},0",
            "x".repeat(44),
            u32::MAX
        );
        for (event, detail) in [
            (Event::SessionStart, "startup"),
            (Event::SessionEnd, "prompt-input-exit"),
        ] {
            let line = build_line(event, &fields(detail), &tmux, "%123456789").unwrap();
            assert!(line.len() <= MAX_LINE, "{}", line.len());
            assert!(line.ends_with("}\n"));
            assert_eq!(line.matches('\n').count(), 1);
            assert!(serde_json::from_str::<serde_json::Value>(&line).is_ok());
        }
    }

    #[test]
    fn the_line_is_built_only_from_validated_fields() {
        let good = fields("startup");
        assert!(build_line(Event::SessionStart, &good, TMUX, "%7").is_some());
        // a detail word of the other event, or outside both sets
        for (event, detail) in [
            (Event::SessionStart, "logout"),
            (Event::SessionStart, "prompt-input-exit"),
            (Event::SessionEnd, "startup"),
            (Event::SessionEnd, "compact"),
            (Event::SessionStart, "x\",\"y\":\""),
        ] {
            assert_eq!(
                build_line(event, &fields(detail), TMUX, "%7"),
                None,
                "{detail}"
            );
        }
        let bad_id = Fields {
            session_id: "\",\"pane\":\"%9".into(),
            detail: "startup",
        };
        assert_eq!(build_line(Event::SessionStart, &bad_id, TMUX, "%7"), None);
        for pane in ["", "%", "7", "%7a", "%1234567890", "%7\""] {
            assert_eq!(
                build_line(Event::SessionStart, &good, TMUX, pane),
                None,
                "{pane:?}"
            );
        }
        for tmux in ["", "/t/deck", "/t/other,1,0", "/t/deck,x,0", "/t/deck,-1,0"] {
            assert_eq!(
                build_line(Event::SessionStart, &good, tmux, "%7"),
                None,
                "{tmux:?}"
            );
        }
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
        assert_eq!(parse_tmux_env("/tmp/x/deck\",1,3"), None);
        assert_eq!(
            parse_tmux_env(&format!("/t/deck{},1,0", "x".repeat(45))),
            None
        );
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
        let home = Path::new("/Users/u");
        let fallback = PathBuf::from("/Users/u/.deck/status.sock");
        assert_eq!(
            socket_path(Some("/tmp/deck-test/status.sock"), home),
            PathBuf::from("/tmp/deck-test/status.sock")
        );
        assert_eq!(socket_path(None, home), fallback);
        assert_eq!(socket_path(Some("relative/status.sock"), home), fallback);
        assert_eq!(socket_path(Some("/tmp/other.sock"), home), fallback);
        assert_eq!(socket_path(Some(""), home), fallback);
    }

    #[test]
    fn the_deadline_helper_reports_none_once_passed() {
        assert!(remaining(Instant::now() + Duration::from_secs(5)).is_some());
        assert_eq!(remaining(Instant::now()), None);
    }

    #[test]
    fn the_budgets_fit_inside_a_one_second_hook_timeout() {
        assert!(STDIN_BUDGET + SOCKET_BUDGET < Duration::from_secs(1));
    }
}
