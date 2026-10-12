//! agent_pi.rs — the Pi coding agent's adapter: its registration, the closed
//! `pi-observer` wire, and (in the submodules) its one settings entry and
//! the bounded transport that serves its streams.
//!
//! Deck owns attention around work; the Agent owns the work. This adapter
//! observes two boundaries Pi itself declares — an agent run started, an
//! agent run settled — and, separately, which native session a Pi runtime is
//! in. It reads no prompt, message, title, tool call, result or transcript,
//! sends Pi nothing but acknowledgements, and starts, steers or resumes
//! nothing.
//!
//! # Contract
//! - **What is supported, and what is not.** Pi 1.1.x gives an extension a
//!   reliable settlement boundary (`agent_settled`, after retries,
//!   compaction recovery and queued continuations) and no reliable way to
//!   know that Pi waits for input: its UI prompt notifications carry no
//!   identity and can overtake one another, and its built-in dialogs emit
//!   none. So this adapter reports `working` and `turn-done` and NEVER
//!   `needs-input` (the closed `RunState` has no such word), Deck says so on
//!   the card, and no automatic prompt is ever sent to a recognized Pi
//!   (`scheduler/select.rs`, `Hold::PiInputUnverified`). Pi is never
//!   re-entered automatically: `PI.reentry` is `None`, because neither an
//!   npm install (the running image is Node) nor the official standalone
//!   build (ad-hoc signed) lets Deck prove whose code a client is.
//! - **One transport.** The observer is a Pi extension inside the signed
//!   Deck bundle (`resources/pi/`); Pi's settings name its path, nothing is
//!   written under the home directory but that one settings entry
//!   (`config`). In the Pi process it holds ONE stream on the existing
//!   status socket (`transport`). No helper is spawned per event, no second
//!   listener exists, and nothing here speaks to Pi by RPC, SDK or a
//!   wrapper.
//! - **Three identities that never stand in for one another.** The kernel
//!   says which process connected (pid and birth instant, the pane's
//!   foreground leader, its terminal) and that this process has not
//!   executed another image since (`procinfo::InstanceWatch`: an exec keeps
//!   the pid and the birth instant, so those two alone would let another
//!   program speak through an inherited connection). The backend names one
//!   accepted connection (a lease, never reused). The observer counts its
//!   runs inside that connection. Pi's native session id is a fourth thing,
//!   used only by the lifecycle capability and never as an interaction id.
//!   What a line SAYS about its pane, server, version or mode is a claim.
//!   That a process is still the instance that connected says nothing about
//!   WHOSE program it is: it is no product identity and admits no re-entry.
//! - **The wire** (`parse_hello`, `parse_frame`): one JSON object per line,
//!   at most `MAX_LINE` bytes, protocol `pi-observer`, version 1, source
//!   `pi`. Exactly six shapes, told apart by their exact key sets:
//!   `hello` (`socket`, `server_pid`, `pane`, `observerVersion`, `piVersion`,
//!   `mode`, `capabilities{signal,lifecycle}`), `signal` (`seq`, `run`,
//!   `state`), `reset` (`seq`, `reason`), `lifecycle` (`seq`, `event`,
//!   `nativeSessionId`, and `reason` for `start` and `shutdown` only),
//!   `unavailable` (`seq`, `unavailable`: the one closed word
//!   `unsupported-native-id`, sent INSTEAD of a lifecycle frame whose id
//!   does not fit a line — the id itself is never cut short, hashed or left
//!   out silently) and `ping` (`seq`). An unknown or repeated key, a missing
//!   one, a number that is negative, fractional or beyond 2^53-1, an unknown
//!   word, another protocol version or mode is refused; nothing is repaired.
//!   There is no field for a prompt, a title, a path, a model or free text.
//! - **What Deck says to Pi**, and nothing else: `accepted`; `ack` with the
//!   sequence number that was committed; `revoked` with the sequence number
//!   of a frame that was NOT committed because its capability was taken from
//!   the stream in the meantime (the stream goes on); `capabilities` with
//!   the two bits the stream may still use, sent whenever that became less
//!   than the observer last knew; and `refused` with a closed code, which
//!   ends the stream. The bits only ever narrow: nothing Deck sends gives a
//!   running observer a capability back.
//! - **Compatibility is not identity.** `piVersion` is Pi's own exported
//!   version, used only to decide whether this build knows the meaning of
//!   the events (`PiVersion::supported`: 1.1.x; 1.1.0 is what ran natively).
//!   It proves nothing about who connected and admits nothing by itself.
//! - **The hook path stays closed to Pi.** A wire v1 or v2 status line that
//!   names source `pi` is refused (`agent_status::parse_event`,
//!   `pi-observer-required`): Pi has no interaction id of its own, and an
//!   observer-made number must never look like one.
//!
//! Registered is not enabled, connected, trusted or authorized
//! (`extensions.rs`): the entry below earns nothing by being listed.

pub(crate) mod config;
pub(crate) mod transport;

use serde::de::{Deserializer, MapAccess, Visitor};

use crate::agent_status::{AgentAdapter, Install, SignalClass};

/// The protocol family word, and the adapter's wire source word (also the
/// Settings toggle's id).
pub(crate) const FAMILY: &str = "pi-observer";
pub(crate) const SOURCE: &str = "pi";
/// The file name Pi's launcher and standalone build carry. A name only ever
/// ADDS the scheduler's Pi hold: it proves nothing and is no delivery target.
pub(crate) const PROCESS: &str = "pi";
/// The longest line of this family, in bytes.
pub(crate) const MAX_LINE: usize = 1024;
/// The wire version and the observer's own version this build speaks.
const WIRE_VERSION: u64 = 1;
const OBSERVER_VERSION: u64 = 1;
/// The largest integer a JavaScript client can count to without a gap.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
/// The longest native session id this build keeps. A longer one is reported
/// as unsupported, never cut short and never hashed into something else.
pub(crate) const MAX_NATIVE_ID: usize = 256;

/// Pi's registration. No re-entry capability: Pi is never started,
/// re-entered or resumed by Deck.
pub(crate) const PI: AgentAdapter = AgentAdapter {
    source: SOURCE,
    class: SignalClass::Pi,
    install: Install::PiExtension,
    reentry: None,
};

/// One of the two things a Pi observer may do, each behind its own switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Capability {
    /// Run boundaries: `working` and `turn-done`.
    Signal,
    /// The native session a runtime is in.
    Lifecycle,
}

/// A set of capabilities: what an entry carries, what the settings enable,
/// what a stream may still do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub(crate) signal: bool,
    pub(crate) lifecycle: bool,
}

impl Capabilities {
    pub(crate) const NONE: Self = Self {
        signal: false,
        lifecycle: false,
    };

    pub(crate) fn any(self) -> bool {
        self.signal || self.lifecycle
    }

    pub(crate) fn has(self, capability: Capability) -> bool {
        match capability {
            Capability::Signal => self.signal,
            Capability::Lifecycle => self.lifecycle,
        }
    }

    /// What both sets hold.
    pub(crate) fn and(self, other: Self) -> Self {
        Self {
            signal: self.signal && other.signal,
            lifecycle: self.lifecycle && other.lifecycle,
        }
    }

    pub(crate) fn with(mut self, capability: Capability, on: bool) -> Self {
        match capability {
            Capability::Signal => self.signal = on,
            Capability::Lifecycle => self.lifecycle = on,
        }
        self
    }
}

/// The two words a Pi run may report. There is no third: Pi gives Deck no
/// reliable way to observe an input request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunState {
    Working,
    TurnDone,
}

/// Why an observer withdrew what its stream reported: Pi's own teardown
/// reasons, or the observer's own failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResetReason {
    New,
    Resume,
    Fork,
    Reload,
    Quit,
    Fault,
}

/// Pi's `session_start` reasons. `Startup` is also what a client started
/// with `--session` reports: it does not say "resumed".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartReason {
    Startup,
    Reload,
    New,
    Resume,
    Fork,
}

/// Pi's `session_shutdown` reasons. `Quit` is the runtime's teardown, not
/// proof that the process is gone; the others are not an exit at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShutdownReason {
    Quit,
    Reload,
    New,
    Resume,
    Fork,
}

/// One lifecycle observation as the wire carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    /// This runtime started in a session.
    Start(StartReason),
    /// This runtime is being torn down.
    Shutdown(ShutdownReason),
    /// What the API says now, read after a reconnect: neither a start nor a
    /// resume, and never reported as one.
    Snapshot,
}

/// A native Pi session id: Pi's own character set (`--session-id` accepts
/// more than a UUID), at most `MAX_NATIVE_ID` bytes. Never Claude's shape,
/// never a path, and never anything Deck made.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PiNativeId(String);

#[cfg(test)]
impl PiNativeId {
    /// The id as text, for a test that compares it with what Pi itself
    /// said. Production compares ids and never takes one out.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The id a lifecycle frame names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NativeId {
    Kept(PiNativeId),
    /// Well formed and longer than this build keeps.
    Unsupported,
}

/// `[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?`, as Pi itself validates a
/// session id.
fn native_id(text: &str) -> Option<NativeId> {
    let bytes = text.as_bytes();
    let edge = |b: &u8| b.is_ascii_alphanumeric();
    let shaped = bytes.first().is_some_and(edge)
        && bytes.last().is_some_and(edge)
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    shaped.then(|| {
        if bytes.len() <= MAX_NATIVE_ID {
            NativeId::Kept(PiNativeId(text.to_string()))
        } else {
            NativeId::Unsupported
        }
    })
}

/// Pi's exported version, as far as this build needs to know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PiVersion {
    /// A release whose event meanings this build knows.
    Supported,
    /// Any other release: the stream is refused and the card says so.
    Unsupported,
}

/// `text` as a version: at most 32 bytes of a version's characters, or it is
/// no version at all. Supported means three numeric parts and `1.1.x`.
fn pi_version(text: &str) -> Option<PiVersion> {
    if text.is_empty()
        || text.len() > 32
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
    {
        return None;
    }
    let parts: Vec<&str> = text.split('.').collect();
    let numeric = parts.len() == 3
        && parts
            .iter()
            .all(|part| (1..=5).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()));
    Some(if numeric && parts[0] == "1" && parts[1] == "1" {
        PiVersion::Supported
    } else {
        PiVersion::Unsupported
    })
}

/// The first line of a stream: what the observer claims about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hello {
    pub(crate) socket: String,
    pub(crate) server_pid: u32,
    pub(crate) pane: String,
    pub(crate) version: PiVersion,
    pub(crate) capabilities: Capabilities,
}

/// One frame after the hello.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Signal {
        seq: u64,
        run: u64,
        state: RunState,
    },
    Reset {
        seq: u64,
        reason: ResetReason,
    },
    Lifecycle {
        seq: u64,
        event: SessionEvent,
        native: NativeId,
    },
    /// The observer has a session id it cannot put on a line: it says so,
    /// in one closed word, instead of sending a lifecycle frame.
    Unavailable {
        seq: u64,
    },
    Ping {
        seq: u64,
    },
}

impl Frame {
    pub(crate) fn seq(&self) -> u64 {
        match self {
            Frame::Signal { seq, .. }
            | Frame::Reset { seq, .. }
            | Frame::Lifecycle { seq, .. }
            | Frame::Unavailable { seq }
            | Frame::Ping { seq } => *seq,
        }
    }

    /// The capability a frame belongs to; a reset and a ping to none.
    pub(crate) fn capability(&self) -> Option<Capability> {
        match self {
            Frame::Signal { .. } => Some(Capability::Signal),
            Frame::Lifecycle { .. } | Frame::Unavailable { .. } => Some(Capability::Lifecycle),
            Frame::Reset { .. } | Frame::Ping { .. } => None,
        }
    }
}

#[derive(Default)]
struct Fields {
    protocol: Option<String>,
    version: Option<u64>,
    source: Option<String>,
    socket: Option<String>,
    server_pid: Option<u64>,
    pane: Option<String>,
    observer_version: Option<u64>,
    pi_version: Option<String>,
    mode: Option<String>,
    capabilities: Option<Capabilities>,
    seq: Option<u64>,
    run: Option<u64>,
    state: Option<String>,
    reason: Option<String>,
    event: Option<String>,
    native_id: Option<String>,
    unavailable: Option<String>,
}

fn put<T, E: serde::de::Error>(slot: &mut Option<T>, value: T) -> Result<(), E> {
    if slot.replace(value).is_some() {
        return Err(E::custom("duplicate"));
    }
    Ok(())
}

struct CapabilitiesVisitor;

impl<'de> Visitor<'de> for CapabilitiesVisitor {
    type Value = Capabilities;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("the two capability bits")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Capabilities, A::Error> {
        use serde::de::Error;
        let (mut signal, mut lifecycle) = (None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "signal" => put(&mut signal, map.next_value::<bool>()?)?,
                "lifecycle" => put(&mut lifecycle, map.next_value::<bool>()?)?,
                _ => return Err(A::Error::custom("unknown key")),
            }
        }
        match (signal, lifecycle) {
            (Some(signal), Some(lifecycle)) => Ok(Capabilities { signal, lifecycle }),
            _ => Err(A::Error::custom("missing key")),
        }
    }
}

struct CapabilitiesSeed;

impl<'de> serde::de::DeserializeSeed<'de> for CapabilitiesSeed {
    type Value = Capabilities;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Capabilities, D::Error> {
        deserializer.deserialize_map(CapabilitiesVisitor)
    }
}

struct FieldsVisitor;

impl<'de> Visitor<'de> for FieldsVisitor {
    type Value = Fields;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a pi-observer object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
        use serde::de::Error;
        let mut fields = Fields::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "protocol" => put(&mut fields.protocol, map.next_value()?)?,
                "v" => put(&mut fields.version, map.next_value()?)?,
                "source" => put(&mut fields.source, map.next_value()?)?,
                "socket" => put(&mut fields.socket, map.next_value()?)?,
                "server_pid" => put(&mut fields.server_pid, map.next_value()?)?,
                "pane" => put(&mut fields.pane, map.next_value()?)?,
                "observerVersion" => put(&mut fields.observer_version, map.next_value()?)?,
                "piVersion" => put(&mut fields.pi_version, map.next_value()?)?,
                "mode" => put(&mut fields.mode, map.next_value()?)?,
                "capabilities" => put(
                    &mut fields.capabilities,
                    map.next_value_seed(CapabilitiesSeed)?,
                )?,
                "seq" => put(&mut fields.seq, map.next_value()?)?,
                "run" => put(&mut fields.run, map.next_value()?)?,
                "state" => put(&mut fields.state, map.next_value()?)?,
                "reason" => put(&mut fields.reason, map.next_value()?)?,
                "event" => put(&mut fields.event, map.next_value()?)?,
                "nativeSessionId" => put(&mut fields.native_id, map.next_value()?)?,
                "unavailable" => put(&mut fields.unavailable, map.next_value()?)?,
                _ => return Err(A::Error::custom("unknown key")),
            }
        }
        Ok(fields)
    }
}

/// Whether a line claims to be of this family (the listener's dispatch,
/// asked before the lifecycle family and the hook parser see it). A line
/// that is not a JSON object whose `protocol` is this family's word is not.
pub(crate) fn is_family(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .is_some_and(|value| value.get("protocol").and_then(|p| p.as_str()) == Some(FAMILY))
}

/// The fields of one line, with the family's common part checked.
fn fields(line: &str) -> Result<Fields, &'static str> {
    if line.len() > MAX_LINE {
        return Err("bad-frame");
    }
    let mut deserializer = serde_json::Deserializer::from_str(line);
    let fields = deserializer
        .deserialize_map(FieldsVisitor)
        .map_err(|_| "bad-frame")?;
    deserializer.end().map_err(|_| "bad-frame")?;
    if fields.protocol.as_deref() != Some(FAMILY) || fields.source.as_deref() != Some(SOURCE) {
        return Err("bad-frame");
    }
    if fields.version != Some(WIRE_VERSION) {
        return Err("bad-version");
    }
    Ok(fields)
}

/// A count a JavaScript client can hold exactly, from one.
fn count(value: Option<u64>) -> Result<u64, &'static str> {
    value
        .filter(|n| (1..=MAX_SAFE_INTEGER).contains(n))
        .ok_or("bad-frame")
}

/// The first line of a stream. The error is the closed refusal code.
pub(crate) fn parse_hello(line: &str) -> Result<Hello, &'static str> {
    let Fields {
        socket,
        server_pid,
        pane,
        observer_version,
        pi_version: version,
        mode,
        capabilities,
        seq: None,
        run: None,
        state: None,
        reason: None,
        event: None,
        native_id: None,
        unavailable: None,
        ..
    } = fields(line)?
    else {
        return Err("bad-frame");
    };
    if observer_version != Some(OBSERVER_VERSION) {
        return Err("bad-version");
    }
    let socket = socket
        .filter(|socket| {
            !socket.is_empty()
                && socket.len() <= 64
                && socket
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        .ok_or("bad-frame")?;
    let server_pid = server_pid
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .ok_or("bad-frame")?;
    let pane = pane
        .filter(|pane| {
            (2..=10).contains(&pane.len())
                && pane.starts_with('%')
                && pane[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or("bad-frame")?;
    let version = version.as_deref().and_then(pi_version).ok_or("bad-frame")?;
    // Only an interactive terminal session is observed. `hasUI` is true for
    // an RPC client as well, so the mode word is what is asked.
    match mode.as_deref() {
        Some("tui") => {}
        Some("rpc" | "json" | "print") => return Err("bad-mode"),
        _ => return Err("bad-frame"),
    }
    let capabilities = capabilities.filter(|c| c.any()).ok_or("bad-frame")?;
    Ok(Hello {
        socket,
        server_pid,
        pane,
        version,
        capabilities,
    })
}

/// One frame after the hello. The error is the closed refusal code.
pub(crate) fn parse_frame(line: &str) -> Result<Frame, &'static str> {
    let Fields {
        seq,
        run,
        state,
        reason,
        event,
        native_id: id,
        unavailable,
        socket: None,
        server_pid: None,
        pane: None,
        observer_version: None,
        pi_version: None,
        mode: None,
        capabilities: None,
        ..
    } = fields(line)?
    else {
        return Err("bad-frame");
    };
    let seq = count(seq)?;
    if let Some(word) = unavailable {
        return match (word.as_str(), run, state, reason, event, id) {
            ("unsupported-native-id", None, None, None, None, None) => {
                Ok(Frame::Unavailable { seq })
            }
            _ => Err("bad-frame"),
        };
    }
    match (run, state, reason, event, id) {
        (None, None, None, None, None) => Ok(Frame::Ping { seq }),
        (Some(run), Some(state), None, None, None) => Ok(Frame::Signal {
            seq,
            run: count(Some(run))?,
            state: match state.as_str() {
                "working" => RunState::Working,
                "turn-done" => RunState::TurnDone,
                _ => return Err("bad-frame"),
            },
        }),
        (None, None, Some(reason), None, None) => Ok(Frame::Reset {
            seq,
            reason: match reason.as_str() {
                "new" => ResetReason::New,
                "resume" => ResetReason::Resume,
                "fork" => ResetReason::Fork,
                "reload" => ResetReason::Reload,
                "quit" => ResetReason::Quit,
                "fault" => ResetReason::Fault,
                _ => return Err("bad-frame"),
            },
        }),
        (None, None, reason, Some(event), Some(id)) => {
            let event = match (event.as_str(), reason.as_deref()) {
                ("start", Some(reason)) => SessionEvent::Start(match reason {
                    "startup" => StartReason::Startup,
                    "reload" => StartReason::Reload,
                    "new" => StartReason::New,
                    "resume" => StartReason::Resume,
                    "fork" => StartReason::Fork,
                    _ => return Err("bad-frame"),
                }),
                ("shutdown", Some(reason)) => SessionEvent::Shutdown(match reason {
                    "quit" => ShutdownReason::Quit,
                    "reload" => ShutdownReason::Reload,
                    "new" => ShutdownReason::New,
                    "resume" => ShutdownReason::Resume,
                    "fork" => ShutdownReason::Fork,
                    _ => return Err("bad-frame"),
                }),
                ("snapshot", None) => SessionEvent::Snapshot,
                _ => return Err("bad-frame"),
            };
            Ok(Frame::Lifecycle {
                seq,
                event,
                native: native_id(&id).ok_or("bad-native-id")?,
            })
        }
        _ => Err("bad-frame"),
    }
}

/// The stream is bound: the observer may send frames.
pub(crate) fn accepted_line() -> String {
    format!("{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"accepted\":true}}\n")
}

/// Frame `seq` was verified and committed.
pub(crate) fn ack_line(seq: u64) -> String {
    format!("{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"ack\":{seq}}}\n")
}

/// Frame `seq` belongs to a capability this stream no longer has: it was
/// not committed, and the stream goes on.
pub(crate) fn revoked_line(seq: u64) -> String {
    format!("{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"revoked\":{seq}}}\n")
}

/// What the stream may still do. Only ever less than the observer knew.
pub(crate) fn capabilities_line(now: Capabilities) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"capabilities\":{{\"signal\":{},\"lifecycle\":{}}}}}\n",
        now.signal, now.lifecycle
    )
}

/// The stream is over, for a closed reason: one of `REFUSALS`, a plain word
/// that needs no escaping.
pub(crate) fn refused_line(code: &'static str) -> String {
    format!("{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"refused\":\"{code}\"}}\n")
}

/// The refusal codes of this family: what `refused_line` may carry and what
/// the drop diagnostics count (there, prefixed `pi-`).
pub(crate) const REFUSALS: &[&str] = &[
    "bad-frame",
    "bad-version",
    "bad-mode",
    "bad-native-id",
    "unsupported-pi-version",
    "other-server",
    "no-peer",
    "not-foreground",
    "no-such-pane",
    "foreign-pane",
    "terminal-discontinuity",
    "no-generation",
    "generation-mismatch",
    "no-instance",
    "peer-executed",
    "pane-changed",
    "duplicate-observer",
    "capability-disabled",
    "busy",
    "bad-seq",
    "bad-run",
    "bad-lifecycle",
    "late",
    "stale",
];

/// The drop-diagnostics name of a refusal code (`agent_status::DROP_REASONS`).
pub(crate) fn drop_reason(code: &'static str) -> &'static str {
    match code {
        "bad-frame" => "pi-bad-frame",
        "bad-version" => "pi-bad-version",
        "bad-mode" => "pi-bad-mode",
        "bad-native-id" => "pi-bad-native-id",
        "unsupported-pi-version" => "pi-unsupported-pi-version",
        "other-server" => "pi-other-server",
        "no-peer" => "pi-no-peer",
        "not-foreground" => "pi-not-foreground",
        "no-such-pane" => "pi-no-such-pane",
        "foreign-pane" => "pi-foreign-pane",
        "terminal-discontinuity" => "pi-terminal-discontinuity",
        "no-generation" => "pi-no-generation",
        "generation-mismatch" => "pi-generation-mismatch",
        "no-instance" => "pi-no-instance",
        "peer-executed" => "pi-peer-executed",
        "pane-changed" => "pi-pane-changed",
        "duplicate-observer" => "pi-duplicate-observer",
        "capability-disabled" => "pi-capability-disabled",
        "busy" => "pi-busy",
        "bad-seq" => "pi-bad-seq",
        "bad-run" => "pi-bad-run",
        "late" => "pi-late",
        "bad-lifecycle" => "pi-bad-lifecycle",
        "stale" => "pi-stale",
        _ => "other",
    }
}

#[cfg(test)]
mod tests;
