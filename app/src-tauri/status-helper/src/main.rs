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
//! while the user's turn-summary switch is on (or replies are forwarded for a
//! reader): a Claude Code or Codex `turn-done` hook run with `--reply` also
//! reads the TOP-LEVEL string `last_assistant_message` (the final message of
//! the turn: documented for Claude Code's Stop hook, present in Codex's
//! Stop hook input) and forwards its head and tail (`bounded_reply`) as
//! the JSON-escaped `reply` of a v3 line. Without the flag, for any other
//! source or word, or when the value is not a string or its key repeats,
//! nothing of it is read or sent.
//!
//! A second, narrower exception, also written into the hook by deck: Claude
//! Code's PostToolUse hook runs `claude-code working --after-tool`. For it
//! the helper also notes whether the TOP-LEVEL key `agent_id` is present
//! (a subagent's tool; its value is never read). A subagent's tool, or a
//! payload without a valid id, sends nothing; otherwise the v2 line carries
//! `"after_tool":true`, which tells deck the word may only end that same
//! interaction's input request (`agent_status.rs`, "After a tool").
//!
//! A third, for agy (Google Antigravity CLI), whose `PreInvocation` hook runs
//! before EVERY model call of a turn: deck's entry runs
//! `agy working --turn-start`, and the helper reads the TOP-LEVEL number
//! `invocationNum` and sends only when it is exactly 0 (the first call of a
//! turn); any other value, a missing or repeated key, or a payload that does
//! not parse sends nothing. agy has no per-turn id, so its lines are v1.
//!
//! A fourth, for Claude Code's `StopFailure` hook (an API error ended the
//! turn; it fires INSTEAD of `Stop`): deck's entry runs
//! `claude-code turn-failed`, never with `--reply`. The helper reads, in the
//! same one pass, the prompt id, the presence of `agent_id` (a subagent's:
//! nothing is sent) and the TOP-LEVEL string `error`, Claude Code's own
//! error kind. The kind leaves this process only as one word of the closed
//! `FAILURE_KINDS` list (anything else, a non-string or a missing value is
//! `unknown`), as the line's `failure`; without a valid prompt id nothing is
//! sent. `last_assistant_message` (there the rendered error text) and
//! `error_details` are never read: the failure is no final message, no
//! summary and no worker reply.
//!
//! Wire: v1 `{v:1, source, state, socket, server_pid, pane}`; v2 adds
//! `interaction` (a source id, never synthesized) and, on a `turn-failed`
//! only, `failure`; v3 adds `reply` (with or without `interaction`). deck accepts all three; it takes `reply` off
//! before its own parser sees the line (`turn_summary::split`).
//!
//! Identity: the pane id in the message is a claim; deck proves it by asking
//! the kernel which process connected and walking that process's parents to
//! the pane's own process. The helper therefore keeps the connection open
//! until deck closes it (bounded by a 2 s read timeout) instead of exiting
//! right after the write.
//!
//! A fourth mode, `--quota <source> [--quiet]`, is deck's statusLine entry
//! for Claude Code and agy (`quota.rs` owns the contract). The agent hands
//! its statusLine command the session JSON on stdin; the helper is a TEE:
//! it copies stdin to stdout unchanged (into the user's own statusLine
//! command, which deck's entry pipes it to, or nowhere with `--quiet`),
//! closes stdout before anything else, and only then reads the usage
//! windows out of what it kept: Claude Code's top-level `rate_limits`
//! (every member with `used_percentage` and `resets_at`) or agy's `quota`
//! (each bucket's `remaining_fraction` and `reset_in_seconds`), plus the
//! account the windows belong to — Claude Code's config folder (this
//! process's own `CLAUDE_CONFIG_DIR`, the default when unset) or agy's
//! signed-in `email` and `plan_tier`. Nothing else of the payload is read;
//! the line (`protocol: deck-quota`) carries names, numbers, the folder and
//! the account, and deck turns the account into an opaque key at once. It
//! never spawns anything and never runs the user's command: the agent's
//! own shell runs the pipe.

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

/// The sources whose Stop payload carries `last_assistant_message`: Claude
/// Code (documented) and Codex (its Stop hook input, checked on 0.169).
const REPLY_SOURCES: [&str; 2] = ["claude-code", "codex"];

/// The reply only where deck put the flag, and only on the turn-done of a
/// source that has a final message.
/// The argument deck adds to Claude Code's PostToolUse hook; honoured for
/// `claude-code working` only.
const AFTER_TOOL_FLAG: &str = "--after-tool";

/// The top-level key a subagent's hook payload carries (Claude Code
/// 2.1.330: present on a subagent's PreToolUse/PostToolUse, absent on the
/// main thread's and on the permission Notification). Only its presence is
/// read.
const SUBAGENT_FIELD: &str = "agent_id";

/// The argument deck adds to agy's PreInvocation hook; honoured for
/// `agy working` only.
const TURN_START_FLAG: &str = "--turn-start";

/// The top-level key of agy's PreInvocation payload that numbers the model
/// calls of one turn (agy 1.5.14: 0 for the first).
const INVOCATION_FIELD: &str = "invocationNum";

/// Whether this is the PreInvocation hook deck wrote for agy.
fn turn_start(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("agy")
        && args.get(2).map(String::as_str) == Some("working")
        && args.get(3).map(String::as_str) == Some(TURN_START_FLAG)
        && args.len() == 4
}

/// Whether agy's payload says this model call is the first of its turn:
/// a top-level `invocationNum` that is exactly the number 0, read in one
/// strict pass (a repeated key, malformed JSON or anything else: no).
fn first_invocation(payload: &[u8]) -> bool {
    struct Visit;
    impl<'de> Visitor<'de> for Visit {
        type Value = Option<bool>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut found = None;
            while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                if key == INVOCATION_FIELD {
                    if found.is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    found = Some(map.next_value::<serde_json::Value>()? == serde_json::json!(0));
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            Ok(found)
        }
    }
    if payload.len() > MAX_PAYLOAD {
        return false;
    }
    let Ok(text) = std::str::from_utf8(payload) else {
        return false;
    };
    let mut de = serde_json::Deserializer::from_str(text);
    let Ok(found) = (&mut de).deserialize_map(Visit) else {
        return false;
    };
    de.end().is_ok() && found == Some(true)
}

/// The word deck's StopFailure entry runs for Claude Code (`hooks.rs`).
const FAILED_WORD: &str = "turn-failed";

/// The top-level key of Claude Code's StopFailure payload that names the
/// error kind (2.1.332; the hook's matcher values).
const ERROR_FIELD: &str = "error";

/// The error kinds a `turn-failed` line may carry: Claude Code 2.1.332's
/// StopFailure matcher values, the backend's `FAILURE_KINDS` the same list
/// (`agent_status.rs` pins the two together). Anything else is `unknown`.
const FAILURE_KINDS: [&str; 13] = [
    "rate_limit",
    "overloaded",
    "authentication_failed",
    "oauth_org_not_allowed",
    "account_on_hold",
    "verification_required",
    "billing_error",
    "invalid_request",
    "model_not_found",
    "server_error",
    "max_output_tokens",
    "cloud_credential_error",
    "unknown",
];

/// Whether this is the StopFailure hook deck wrote.
fn turn_failed(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("claude-code")
        && args.get(2).map(String::as_str) == Some(FAILED_WORD)
        && args.len() == 3
}

/// The closed word for a payload's error kind.
fn failure_kind(kind: Option<&str>) -> &'static str {
    kind.and_then(|kind| FAILURE_KINDS.iter().find(|k| **k == kind))
        .copied()
        .unwrap_or("unknown")
}

/// Whether this is the PostToolUse hook deck wrote.
fn after_tool(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("claude-code")
        && args.get(2).map(String::as_str) == Some("working")
        && args.get(3).map(String::as_str) == Some(AFTER_TOOL_FLAG)
        && args.len() == 4
}

fn wants_reply(args: &[String]) -> bool {
    args.get(3).map(String::as_str) == Some(REPLY_FLAG)
        && args
            .get(1)
            .is_some_and(|s| REPLY_SOURCES.contains(&s.as_str()))
        && args.get(2).map(String::as_str) == Some("turn-done")
}
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
    /// The presence key was there (its value skipped, never kept).
    present: bool,
    /// The kind field's string value, only when asked for
    /// (`ERROR_FIELD` on a `turn-failed`).
    kind: Option<String>,
}

/// Reads a JSON document whose top level is an object, keeping only the
/// string value of `id` and, when given, of `reply`; every other value is
/// skipped (and still validated) by serde's `IgnoredAny`. A second key of
/// either is an error, never first- or last-wins; a non-string id is an
/// error too, while a reply that is not a string is simply not kept.
struct TopLevel {
    id: &'static str,
    reply: Option<&'static str>,
    /// A key whose presence alone is noted (`SUBAGENT_FIELD`).
    presence: Option<&'static str>,
    /// A key whose string value is kept (`ERROR_FIELD`); a second one is an
    /// error, a value of another type is simply not kept.
    kind: Option<&'static str>,
}

impl<'de> Visitor<'de> for TopLevel {
    type Value = Fields;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = Fields::default();
        let (mut seen_id, mut seen_reply, mut seen_kind) = (false, false, false);
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
            } else if Some(key.as_ref()) == self.kind {
                if seen_kind {
                    return Err(serde::de::Error::custom("duplicate kind key"));
                }
                seen_kind = true;
                found.kind = match map.next_value::<serde_json::Value>()? {
                    serde_json::Value::String(text) => Some(text),
                    _ => None,
                };
            } else {
                if Some(key.as_ref()) == self.presence {
                    found.present = true;
                }
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

/// `fields_with` without a presence key (the tests' shorthand).
#[cfg(test)]
fn fields_from(source: &str, payload: &[u8], reply: bool) -> Fields {
    fields_with(source, payload, reply, None, None)
}

/// The id, with `reply` (the `--reply` flag on Claude Code's `turn-done`)
/// the final message, and with `presence` (`--after-tool`) whether that
/// top-level key is there, and with `kind` (`turn-failed`) that key's
/// string, from ONE pass over the payload. Any failure of the pass yields
/// none of them.
fn fields_with(
    source: &str,
    payload: &[u8],
    reply: bool,
    presence: Option<&'static str>,
    kind: Option<&'static str>,
) -> Fields {
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
        presence,
        kind,
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
        present: found.present,
        kind: found.kind,
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

/// The PostToolUse line: the v2 line with `"after_tool":true`. Nothing for
/// a subagent's tool or without a valid id: such an event could never end
/// an input request deck can pair it with.
fn build_after_tool_message(tmux: &str, pane: &str, fields: &Fields) -> Option<String> {
    if fields.present {
        return None;
    }
    let id = fields.id.as_deref().filter(|id| uuid_ok(id))?;
    let plain = build_message("claude-code", "working", tmux, pane, Some(id))?;
    let base = plain.strip_suffix("}\n")?;
    Some(format!("{base},\"after_tool\":true}}\n"))
}

/// The StopFailure line: the v2 `turn-failed` line with `"failure"`, one
/// word of `FAILURE_KINDS`. Nothing for a subagent's payload or without a
/// valid id: deck pairs a failed turn with its interaction or not at all.
fn build_failed_message(tmux: &str, pane: &str, fields: &Fields) -> Option<String> {
    if fields.present {
        return None;
    }
    let id = fields.id.as_deref().filter(|id| uuid_ok(id))?;
    let plain = build_message("claude-code", FAILED_WORD, tmux, pane, Some(id))?;
    let base = plain.strip_suffix("}\n")?;
    let kind = failure_kind(fields.kind.as_deref());
    // a word of the closed list: plain format! is safe JSON
    Some(format!("{base},\"failure\":\"{kind}\"}}\n"))
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

/// The argument that selects the statusLine tee (module header).
const QUOTA_FLAG: &str = "--quota";
/// The tee's flag for "there is no user command after me": print nothing.
const QUIET_FLAG: &str = "--quiet";
/// The sources whose statusLine payload deck reads.
const QUOTA_SOURCES: [&str; 2] = ["claude-code", "agy"];
/// The most windows one line carries.
const MAX_WINDOWS: usize = 8;

/// `--quota <source>` or `--quota <source> --quiet`: (source, quiet).
fn quota_mode(args: &[String]) -> Option<(&str, bool)> {
    if args.get(1).map(String::as_str) != Some(QUOTA_FLAG) {
        return None;
    }
    let source = args.get(2)?.as_str();
    match (args.len(), args.get(3).map(String::as_str)) {
        (3, None) => Some((source, false)),
        (4, Some(QUIET_FLAG)) => Some((source, true)),
        _ => None,
    }
}

/// One usage window as the line carries it: a name, the percent used and
/// the reset instant (epoch seconds).
#[derive(Debug, PartialEq)]
struct Window {
    name: String,
    used: f64,
    resets_at: u64,
}

/// What the tee keeps of a statusLine payload.
#[derive(Debug, Default, PartialEq)]
struct Usage {
    windows: Vec<Window>,
    account: Option<String>,
    plan: Option<String>,
}

/// `[a-z0-9_-]{1,40}`: a window or bucket name.
fn window_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A signed-in account's email as agy reports it: printable ASCII without
/// quotes or spaces, one `@`, at most 254 bytes. Anything else: none.
fn email_ok(s: &str) -> bool {
    s.len() <= 254
        && s.bytes().filter(|b| *b == b'@').count() == 1
        && s.bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

fn percent(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 100.0))
}

/// The windows and account of one statusLine payload, `now` the epoch
/// seconds agy's relative reset is counted from. A payload that does not
/// parse, or has no windows, yields an empty `Usage`.
fn usage_from(source: &str, payload: &[u8], now: u64) -> Usage {
    if payload.len() > MAX_PAYLOAD {
        return Usage::default();
    }
    let Ok(root) = serde_json::from_slice::<serde_json::Value>(payload) else {
        return Usage::default();
    };
    let mut usage = Usage::default();
    match source {
        "claude-code" => {
            let Some(limits) = root.get("rate_limits").and_then(|v| v.as_object()) else {
                return usage;
            };
            for (name, window) in limits {
                let (Some(used), Some(resets_at)) = (
                    window.get("used_percentage").and_then(percent),
                    window.get("resets_at").and_then(|v| v.as_u64()),
                ) else {
                    continue;
                };
                if window_name_ok(name) && resets_at > 0 {
                    usage.windows.push(Window {
                        name: name.clone(),
                        used,
                        resets_at,
                    });
                }
            }
        }
        "agy" => {
            let Some(buckets) = root.get("quota").and_then(|v| v.as_object()) else {
                return usage;
            };
            for (name, bucket) in buckets {
                if bucket.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
                    continue;
                }
                let (Some(remaining), Some(seconds)) = (
                    bucket
                        .get("remaining_fraction")
                        .and_then(|v| v.as_f64())
                        .filter(|v| v.is_finite()),
                    bucket.get("reset_in_seconds").and_then(|v| v.as_u64()),
                ) else {
                    continue;
                };
                if window_name_ok(name) {
                    usage.windows.push(Window {
                        name: name.clone(),
                        used: ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).clamp(0.0, 100.0),
                        resets_at: now.saturating_add(seconds),
                    });
                }
            }
            usage.account = root
                .get("email")
                .and_then(|v| v.as_str())
                .filter(|s| email_ok(s))
                .map(str::to_owned);
            usage.plan = root
                .get("plan_tier")
                .and_then(|v| v.as_str())
                .filter(|s| window_name_ok(&s.to_ascii_lowercase()))
                .map(str::to_owned);
        }
        _ => {}
    }
    usage.windows.truncate(MAX_WINDOWS);
    usage
}

/// An absolute config folder without `..` or control bytes, at most 512
/// bytes: the only `CLAUDE_CONFIG_DIR` the line carries.
fn root_ok(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 512
        && !s.split('/').any(|part| part == "..")
        && !s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

/// The quota line: `{protocol: deck-quota, v: 1, source, windows, root?,
/// account?, plan?, socket?, server_pid?, pane?}`; None without windows.
fn build_quota_message(
    source: &str,
    usage: &Usage,
    root: Option<&str>,
    tmux: Option<(&str, &str)>,
) -> Option<String> {
    if usage.windows.is_empty() || !QUOTA_SOURCES.contains(&source) {
        return None;
    }
    let windows: Vec<serde_json::Value> = usage
        .windows
        .iter()
        .map(|w| serde_json::json!({"name": w.name, "used": w.used, "resets_at": w.resets_at}))
        .collect();
    let mut line = serde_json::json!({
        "protocol": "deck-quota",
        "v": 1,
        "source": source,
        "windows": windows,
    });
    let object = line.as_object_mut()?;
    if let Some(root) = root.filter(|r| root_ok(r) && source == "claude-code") {
        object.insert("root".into(), root.into());
    }
    if let Some(account) = &usage.account {
        object.insert("account".into(), account.as_str().into());
    }
    if let Some(plan) = &usage.plan {
        object.insert("plan".into(), plan.as_str().into());
    }
    if let Some((tmux, pane)) = tmux {
        if let (Some((socket, server_pid)), true) = (parse_tmux_env(tmux), pane_ok(pane)) {
            object.insert("socket".into(), socket.into());
            object.insert("server_pid".into(), server_pid.into());
            object.insert("pane".into(), pane.into());
        }
    }
    let mut text = serde_json::to_string(&line).ok()?;
    text.push('\n');
    (text.len() <= MAX_WIRE_LINE).then_some(text)
}

/// Copy `input` to `output` (unless `quiet`), keeping at most
/// `MAX_PAYLOAD + 1` bytes; a failed write stops the copying, never the
/// draining.
fn tee(mut input: impl Read, mut output: impl Write, quiet: bool) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut writing = !quiet;
    loop {
        match input.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if writing && output.write_all(&chunk[..n]).is_err() {
                    writing = false;
                }
                let room = (MAX_PAYLOAD + 1).saturating_sub(kept.len());
                kept.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let _ = output.flush();
    kept
}

/// Close stdout so the user's command behind the pipe sees its end now,
/// and put /dev/null in its place so fd 1 is never another file.
fn close_stdout() {
    use std::os::fd::FromRawFd;
    let _ = std::io::stdout().flush();
    // SAFETY: fd 1 is this process's stdout, which nothing writes after
    // this point; the std handle is flushed and never used again.
    drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(1) });
    // the lowest free descriptor is 1 again: /dev/null takes it
    std::mem::forget(std::fs::File::open("/dev/null"));
}

fn quota_main(source: &str, quiet: bool) {
    let payload = tee(std::io::stdin().lock(), std::io::stdout().lock(), quiet);
    close_stdout();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let usage = usage_from(source, &payload, now);
    drop(payload);
    let root = std::env::var("CLAUDE_CONFIG_DIR").ok();
    let (tmux, pane) = (std::env::var("TMUX").ok(), std::env::var("TMUX_PANE").ok());
    let Some(message) = build_quota_message(
        source,
        &usage,
        root.as_deref(),
        tmux.as_deref().zip(pane.as_deref()),
    ) else {
        return;
    };
    send(
        &message,
        Duration::from_millis(300),
        Duration::from_millis(800),
    );
}

/// Write one line to deck's socket and wait for deck to close it (the
/// kernel proof of the sender needs this process alive), within `write`
/// and `read` bounds. Deck not running: nothing.
fn send(message: &str, write: Duration, read: Duration) {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let env_sock = std::env::var("DECK_STATUS_SOCK").ok();
    let path = socket_path(env_sock.as_deref(), std::path::Path::new(&home));
    let Ok(mut stream) = UnixStream::connect(path) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(write));
    let _ = stream.write_all(message.as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(read));
    let mut ack = [0u8; 64];
    while let Ok(n) = stream.read(&mut ack) {
        if n == 0 {
            break;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some((source, quiet)) = quota_mode(&args) {
        return quota_main(source, quiet);
    }
    if args.get(1).map(String::as_str) == Some(QUOTA_FLAG) {
        // a statusLine entry deck does not recognize: still never break it
        let _ = tee(std::io::stdin().lock(), std::io::stdout().lock(), false);
        return;
    }
    hook_main(args);
}

fn hook_main(args: Vec<String>) {
    // The ONE allowlisted id is read from this buffer below; it is dropped
    // right after.
    let payload = read_payload(std::io::stdin().lock());

    let (Some(source), Some(state)) = (args.get(1), args.get(2)) else {
        return;
    };
    // agy: only the first model call of a turn says the turn started
    if turn_start(&args) && !first_invocation(&payload) {
        return;
    }
    let after_tool = after_tool(&args);
    let failed = turn_failed(&args);
    let wants_reply = wants_reply(&args);
    let fields = fields_with(
        source,
        &payload,
        wants_reply,
        (after_tool || failed).then_some(SUBAGENT_FIELD),
        failed.then_some(ERROR_FIELD),
    );
    drop(payload);
    let (Ok(tmux), Ok(pane)) = (std::env::var("TMUX"), std::env::var("TMUX_PANE")) else {
        return; // not inside tmux — the agent runs outside deck; do nothing
    };
    let interaction = fields.id.as_deref();
    let message = match fields.reply.as_deref() {
        _ if after_tool => build_after_tool_message(&tmux, &pane, &fields),
        _ if failed => build_failed_message(&tmux, &pane, &fields),
        Some(reply) => build_reply_message(source, state, &tmux, &pane, interaction, reply),
        None => build_message(source, state, &tmux, &pane, interaction),
    };
    let Some(message) = message else {
        return;
    };
    // Stay alive until deck closes the connection: deck binds the event to
    // this pane by walking this process's parent chain through the kernel,
    // which needs this process to still exist. deck closes as soon as it has
    // read the line and the chain (well under a millisecond); the timeout
    // only bounds a deck that stalls. Deck not running: silently done.
    send(&message, Duration::from_secs(1), Duration::from_secs(2));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quota_mode_takes_only_the_two_forms() {
        let h = "deck-status-helper";
        assert_eq!(
            quota_mode(&words(&[h, "--quota", "claude-code"])),
            Some(("claude-code", false))
        );
        assert_eq!(
            quota_mode(&words(&[h, "--quota", "agy", "--quiet"])),
            Some(("agy", true))
        );
        assert_eq!(quota_mode(&words(&[h, "--quota"])), None);
        assert_eq!(quota_mode(&words(&[h, "--quota", "agy", "--loud"])), None);
        assert_eq!(quota_mode(&words(&[h, "claude-code", "working"])), None);
    }

    #[test]
    fn the_tee_passes_every_byte_and_keeps_a_bounded_copy() {
        let input: Vec<u8> = (0..(MAX_PAYLOAD + 5000)).map(|i| (i % 251) as u8).collect();
        let mut out = Vec::new();
        let kept = tee(&input[..], &mut out, false);
        assert_eq!(
            out, input,
            "the user's command sees the payload byte for byte"
        );
        assert_eq!(kept.len(), MAX_PAYLOAD + 1);
        let mut quiet = Vec::new();
        let kept = tee(&b"{}"[..], &mut quiet, true);
        assert!(quiet.is_empty());
        assert_eq!(kept, b"{}");
    }

    #[test]
    fn a_failed_write_still_drains_the_input() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(tee(&b"abc"[..], Broken, false), b"abc");
    }

    #[test]
    fn claude_windows_are_read_generically_and_nothing_else() {
        let payload = br#"{"session_id":"s","cwd":"/secret","cost":{"total_cost_usd":3},
            "rate_limits":{"five_hour":{"used_percentage":23.5,"resets_at":1738425600},
                           "seven_day":{"used_percentage":141,"resets_at":1738857600},
                           "Bad Name":{"used_percentage":1,"resets_at":1},
                           "no_reset":{"used_percentage":1}}}"#;
        let usage = usage_from("claude-code", payload, 0);
        assert_eq!(
            usage.windows,
            vec![
                Window {
                    name: "five_hour".into(),
                    used: 23.5,
                    resets_at: 1738425600
                },
                Window {
                    name: "seven_day".into(),
                    used: 100.0,
                    resets_at: 1738857600
                },
            ]
        );
        assert_eq!(usage.account, None);
        let line = build_quota_message("claude-code", &usage, Some("/Users/u/.claude-work"), None)
            .expect("a line");
        assert!(!line.contains("secret") && !line.contains("session") && !line.contains("cost"));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["protocol"], "deck-quota");
        assert_eq!(value["root"], "/Users/u/.claude-work");
        assert!(value.get("pane").is_none());
    }

    #[test]
    fn agy_buckets_become_used_percent_and_absolute_resets() {
        let payload = br#"{"email":"dev@example.com","plan_tier":"Pro","transcript_path":"/t",
            "quota":{"gemini-weekly":{"remaining_fraction":0.75,"reset_time":"2026-07-06T07:50:32Z","reset_in_seconds":100},
                     "off":{"remaining_fraction":0.1,"reset_in_seconds":5,"disabled":true},
                     "amount":{"remaining_amount":3,"reset_in_seconds":5}}}"#;
        let usage = usage_from("agy", payload, 1000);
        assert_eq!(
            usage.windows,
            vec![Window {
                name: "gemini-weekly".into(),
                used: 25.0,
                resets_at: 1100
            }]
        );
        assert_eq!(usage.account.as_deref(), Some("dev@example.com"));
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        let line = build_quota_message(
            "agy",
            &usage,
            Some("/Users/u/.claude"),
            Some(("/private/tmp/tmux-501/deck,4242,0", "%3")),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(
            value.get("root").is_none(),
            "a Claude folder is never agy's"
        );
        assert_eq!(value["pane"], "%3");
        assert_eq!(value["server_pid"], 4242);
        assert!(!line.contains("transcript"));
    }

    #[test]
    fn no_windows_or_bad_input_send_nothing() {
        for payload in [&b"not json"[..], b"{}", b"{\"rate_limits\":3}", b"[]"] {
            let usage = usage_from("claude-code", payload, 0);
            assert!(build_quota_message("claude-code", &usage, None, None).is_none());
        }
        let usage = usage_from(
            "claude-code",
            br#"{"rate_limits":{"five_hour":{"used_percentage":1,"resets_at":2}}}"#,
            0,
        );
        assert!(build_quota_message("codex", &usage, None, None).is_none());
        assert!(!root_ok("relative/.claude"));
        assert!(!root_ok("/Users/u/../etc"));
        assert!(!email_ok("a b@c"));
        assert!(!email_ok("no-at"));
    }

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
                ..Fields::default()
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
                    ..Fields::default()
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
                reply: Some("Done.".into()),
                ..Fields::default()
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

    #[test]
    fn a_reply_is_wanted_only_with_the_flag_on_a_turn_ending_of_a_source_that_has_one() {
        let args = |words: &[&str]| -> Vec<String> {
            std::iter::once("helper")
                .chain(words.iter().copied())
                .map(String::from)
                .collect()
        };
        assert!(wants_reply(&args(&[
            "claude-code",
            "turn-done",
            "--reply",
            "workers"
        ])));
        assert!(wants_reply(&args(&[
            "codex",
            "turn-done",
            "--reply",
            "workers"
        ])));
        assert!(!wants_reply(&args(&["codex", "turn-done"])));
        assert!(!wants_reply(&args(&["codex", "working", "--reply"])));
        assert!(!wants_reply(&args(&["pi", "turn-done", "--reply"])));
        assert!(!wants_reply(&args(&[
            "codex",
            "turn-done",
            "workers",
            "--reply"
        ])));
    }

    #[test]
    fn after_tool_only_on_the_main_thread_of_claude_code_with_an_id() {
        let args = |words: &[&str]| -> Vec<String> {
            std::iter::once("helper")
                .chain(words.iter().copied())
                .map(String::from)
                .collect()
        };
        assert!(after_tool(&args(&[
            "claude-code",
            "working",
            "--after-tool"
        ])));
        assert!(!after_tool(&args(&["claude-code", "working"])));
        assert!(!after_tool(&args(&["codex", "working", "--after-tool"])));
        assert!(!after_tool(&args(&[
            "claude-code",
            "needs-input",
            "--after-tool"
        ])));
        assert!(!after_tool(&args(&[
            "claude-code",
            "turn-done",
            "--after-tool"
        ])));
        assert!(!after_tool(&args(&[
            "claude-code",
            "working",
            "--after-tool",
            "x"
        ])));

        let main_thread = format!(
            r#"{{"prompt_id":"{ID}","tool_name":"Bash","tool_input":{{"command":"secret"}},"tool_response":"say"}}"#
        );
        let found = fields_with(
            "claude-code",
            main_thread.as_bytes(),
            false,
            Some(SUBAGENT_FIELD),
            None,
        );
        assert_eq!(found.id.as_deref(), Some(ID));
        assert!(!found.present);
        let line = build_after_tool_message("/t/deck,9,0", "%1", &found).unwrap();
        assert_eq!(
            line,
            format!(
                "{{\"v\":2,\"source\":\"claude-code\",\"state\":\"working\",\"socket\":\"deck\",\"server_pid\":9,\"pane\":\"%1\",\"interaction\":\"{ID}\",\"after_tool\":true}}\n"
            )
        );
        for leaked in ["secret", "say", "Bash"] {
            assert!(!line.contains(leaked), "{leaked} in {line}");
        }

        // a subagent's tool: its presence alone stops the line
        let subagent = format!(r#"{{"prompt_id":"{ID}","agent_id":"a1701cd0d377b085d"}}"#);
        let found = fields_with(
            "claude-code",
            subagent.as_bytes(),
            false,
            Some(SUBAGENT_FIELD),
            None,
        );
        assert!(found.present);
        assert_eq!(build_after_tool_message("/t/deck,9,0", "%1", &found), None);
        // whatever its value, and only at the top level
        let odd = format!(r#"{{"prompt_id":"{ID}","agent_id":null}}"#);
        assert!(
            fields_with(
                "claude-code",
                odd.as_bytes(),
                false,
                Some(SUBAGENT_FIELD),
                None
            )
            .present
        );
        let nested = format!(r#"{{"prompt_id":"{ID}","x":{{"agent_id":"a"}}}}"#);
        assert!(
            !fields_with(
                "claude-code",
                nested.as_bytes(),
                false,
                Some(SUBAGENT_FIELD),
                None
            )
            .present
        );
        // without an id there is nothing to pair with: no line, no v1 fallback
        let no_id = r#"{"tool_name":"Bash"}"#;
        let found = fields_with(
            "claude-code",
            no_id.as_bytes(),
            false,
            Some(SUBAGENT_FIELD),
            None,
        );
        assert_eq!(build_after_tool_message("/t/deck,9,0", "%1", &found), None);
        // presence is not read unless asked
        assert!(!fields_from("claude-code", subagent.as_bytes(), false).present);
    }

    #[test]
    fn a_failed_turn_carries_its_id_and_a_closed_kind_never_its_text() {
        let args = |words: &[&str]| -> Vec<String> {
            std::iter::once("helper")
                .chain(words.iter().copied())
                .map(String::from)
                .collect()
        };
        assert!(turn_failed(&args(&["claude-code", "turn-failed"])));
        assert!(!turn_failed(&args(&["codex", "turn-failed"])));
        assert!(!turn_failed(&args(&["claude-code", "turn-done"])));
        assert!(!turn_failed(&args(&[
            "claude-code",
            "turn-failed",
            "--reply"
        ])));
        // the StopFailure entry never asks for the message, even if a
        // hand-written entry added the flag
        assert!(!wants_reply(&args(&[
            "claude-code",
            "turn-failed",
            "--reply"
        ])));

        // the payload shape Claude Code 2.1.332 sent in the isolated probe
        let payload = format!(
            r#"{{"session_id":"a0670b33-95b1-43ba-ab22-aaefb876e58b","transcript_path":"/x.jsonl","cwd":"/p","prompt_id":"{ID}","hook_event_name":"StopFailure","error":"rate_limit","error_details":"SECRET-DETAIL","last_assistant_message":"API Error: SECRET-TEXT"}}"#
        );
        let read = |payload: &str| {
            fields_with(
                "claude-code",
                payload.as_bytes(),
                false,
                Some(SUBAGENT_FIELD),
                Some(ERROR_FIELD),
            )
        };
        let found = read(&payload);
        assert_eq!(found.id.as_deref(), Some(ID));
        assert_eq!(found.reply, None);
        assert_eq!(found.kind.as_deref(), Some("rate_limit"));
        let line = build_failed_message("/t/deck,9,0", "%1", &found).unwrap();
        assert_eq!(
            line,
            format!(
                "{{\"v\":2,\"source\":\"claude-code\",\"state\":\"turn-failed\",\"socket\":\"deck\",\"server_pid\":9,\"pane\":\"%1\",\"interaction\":\"{ID}\",\"failure\":\"rate_limit\"}}\n"
            )
        );
        for leaked in ["SECRET", "API Error", "a0670b33", "/x.jsonl"] {
            assert!(!line.contains(leaked), "{leaked} in {line}");
        }

        // a kind outside the closed list, a non-string or none: `unknown`
        for (error, kind) in [
            (r#""brand_new_kind""#, "unknown"),
            (r#""rate_limit\"}""#, "unknown"),
            (r#"{"type":"x"}"#, "unknown"),
            (r#"null"#, "unknown"),
            (r#""server_error""#, "server_error"),
        ] {
            let payload = format!(r#"{{"prompt_id":"{ID}","error":{error}}}"#);
            let line = build_failed_message("/t/deck,9,0", "%1", &read(&payload)).unwrap();
            assert!(
                line.ends_with(&format!(",\"failure\":\"{kind}\"}}\n")),
                "{line}"
            );
        }
        let no_kind = format!(r#"{{"prompt_id":"{ID}"}}"#);
        assert!(build_failed_message("/t/deck,9,0", "%1", &read(&no_kind))
            .unwrap()
            .contains("\"failure\":\"unknown\""));
        // a repeated kind fails the whole pass: no id, nothing sent
        let twice = format!(r#"{{"prompt_id":"{ID}","error":"a","error":"b"}}"#);
        assert_eq!(
            build_failed_message("/t/deck,9,0", "%1", &read(&twice)),
            None
        );
        // a subagent's payload, or one without a valid id, sends nothing
        let subagent = format!(r#"{{"prompt_id":"{ID}","agent_id":"a1","error":"overloaded"}}"#);
        assert_eq!(
            build_failed_message("/t/deck,9,0", "%1", &read(&subagent)),
            None
        );
        let no_id = r#"{"error":"overloaded"}"#;
        assert_eq!(
            build_failed_message("/t/deck,9,0", "%1", &read(no_id)),
            None
        );
        // the kind is not read unless asked
        assert_eq!(
            fields_from("claude-code", payload.as_bytes(), false).kind,
            None
        );
        // every closed word fits the wire alphabet deck accepts
        for kind in FAILURE_KINDS {
            assert!(kind.len() <= 32 && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'));
        }
    }

    #[test]
    fn agy_sends_working_only_for_the_first_model_call_of_a_turn() {
        let args = |words: &[&str]| -> Vec<String> {
            std::iter::once("helper")
                .chain(words.iter().copied())
                .map(String::from)
                .collect()
        };
        assert!(turn_start(&args(&["agy", "working", "--turn-start"])));
        assert!(!turn_start(&args(&["agy", "working"])));
        assert!(!turn_start(&args(&["agy", "turn-done", "--turn-start"])));
        assert!(!turn_start(&args(&[
            "claude-code",
            "working",
            "--turn-start"
        ])));
        assert!(!turn_start(&args(&["agy", "working", "--turn-start", "x"])));

        assert!(first_invocation(
            br#"{"conversationId":"x","invocationNum":0,"lastUserInput":"secret"}"#
        ));
        for no in [
            &br#"{"invocationNum":1}"#[..],
            br#"{"invocationNum":"0"}"#,
            br#"{"invocationNum":0.5}"#,
            br#"{"invocationNum":null}"#,
            br#"{"conversationId":"x"}"#,
            br#"{"invocationNum":0,"invocationNum":0}"#,
            br#"{"x":{"invocationNum":0}}"#,
            br#"[0]"#,
            b"not json",
            br#"{"invocationNum":0} trailing"#,
        ] {
            assert!(!first_invocation(no), "{}", String::from_utf8_lossy(no));
        }
        // agy has no interaction id: its line is v1
        assert_eq!(identity_field("agy"), None);
    }
}
