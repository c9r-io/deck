//! agent_pi.rs — the Pi coding agent's adapter: its registration, the closed
//! `pi-observer` wire, and (in the submodules) its one settings entry and
//! the bounded transport that answers its messages.
//!
//! Deck owns attention around work; the Agent owns the work. This adapter
//! observes two boundaries Pi itself declares — an agent run started, an
//! agent run settled — and, separately, which native session a Pi runtime is
//! in. It reads no prompt, message, title, tool call, result or transcript,
//! sends Pi nothing but the answers to its observer's own messages, and
//! starts, steers or resumes nothing.
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
//!   (`config`). In the Pi process it speaks to the existing status socket
//!   (`transport`), ONE message per connection: a line, Deck's answer, the
//!   end — as the other two families on that socket do. No helper is spawned
//!   per event, no second listener exists, no connection is kept, and
//!   nothing here speaks to Pi by RPC, SDK or a wrapper.
//! - **Three identities that never stand in for one another.** The kernel
//!   says which process last used a connection (its pid; with the birth
//!   instant, the pane's foreground leader and its terminal: `transport`,
//!   "Who may bind"), that this process has not executed another image since
//!   it was first watched (`procinfo::InstanceWatch`: an exec keeps the pid
//!   and the birth instant), and whether it made a child since Deck last
//!   looked — which, with a nonce Deck gave that process alone, is what says
//!   that a line is that process's own and no child's (`transport`, "Whose
//!   bytes"). The backend names one accepted observer (a lease, never
//!   reused). The observer counts its runs inside that lease. Pi's native
//!   session id is a fourth thing, used only by the lifecycle capability and
//!   never as an interaction id. What a line SAYS about its pane, server,
//!   version or mode is a claim. That a process is still the instance that
//!   attested says nothing about WHOSE program it is: it is no product
//!   identity and admits no re-entry.
//! - **The wire** (`parse_message`): one JSON object per line, at most
//!   `MAX_LINE` bytes, protocol `pi-observer`, version 2, source `pi`.
//!   Exactly seven shapes, told apart by their exact key sets: `hello` (no
//!   key of its own: it asks for a nonce and claims nothing), `attest`
//!   (`nonce`, `socket`, `server_pid`, `pane`, `observerVersion`,
//!   `piVersion`, `mode`, `capabilities{signal,lifecycle}`), and, each with
//!   the `lease` it belongs to, the `nonce` Deck's last answer gave and its
//!   `seq`: `signal` (`run`, `state`), `reset` (`reason`), `lifecycle`
//!   (`event`, `nativeSessionId`, and `reason` for `start` and `shutdown`
//!   only), `unavailable` (`unavailable`: the one closed word
//!   `unsupported-native-id`, sent INSTEAD of a lifecycle frame whose id
//!   does not fit a line — the id itself is never cut short, hashed or left
//!   out silently) and `ping` (nothing else). An unknown or repeated key, a
//!   missing one, a number that is negative, fractional or beyond 2^53-1, an
//!   unknown word, another protocol version or mode is refused; nothing is
//!   repaired. There is no field for a prompt, a title, a path, a model or
//!   free text. A line that names a lease and carries a nonce, and is none
//!   of the five frames, is refused as well — and ends that lease when its
//!   sender is proven, as any other frame Deck does not expect.
//! - **What Deck says to Pi**, one line per connection and nothing else.
//!   Every answer that is not a refusal carries a `nonce` no one has seen
//!   before: the one the observer's NEXT message must carry, good for that
//!   one message. To a `hello`, the nonce alone; `accepted` with the lease
//!   and the two capability bits it may use; `ack` with the sequence number
//!   that was committed, or `revoked` with the number of a frame that was
//!   NOT committed because its capability was taken from the lease in the
//!   meantime (the lease goes on) — both with the two bits as they are now;
//!   `again`, when the sender could not be proven for a reason that is
//!   nobody's fault (its process made a child, or an answer was lost):
//!   nothing was read into anything, and the same message is welcome on a
//!   new connection with the new nonce; and `refused` with a closed code.
//!   The bits only ever narrow: nothing Deck sends gives a running observer
//!   a capability back.
//! - **Versions.** Wire version 1 kept one connection per observer; it could
//!   not say whose bytes a frame was and is gone. A runtime that still holds
//!   a version-1 observer in memory (Pi loaded it before Deck was replaced)
//!   is answered `bad-version` in ITS framing, which that observer takes as
//!   final: it stops, nothing of it is recognized or shown, and Pi's next
//!   start or reload loads the observer this build ships. Nothing of version
//!   1 is accepted beside version 2. Such a Pi is still a Pi by its name
//!   (`PROCESS`), so its card says `unavailable` and the hold on automatic
//!   prompts stands; that its runtime needs a reload is not said anywhere,
//!   because nothing of a version-1 line is proven.
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
/// The wire version and the observer's own version this build speaks
/// (module header, "Versions").
const WIRE_VERSION: u64 = 2;
const OBSERVER_VERSION: u64 = 2;
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
/// what a lease may still do.
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

/// Why an observer withdrew what its lease reported: Pi's own teardown
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
    /// Any other release: the attest is refused and the card says so.
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

/// A nonce: 128 bits from the system's generator, given in one answer and
/// good for the one message that follows it. By itself it shows one thing:
/// that the line which carries it was written after it was made, by someone
/// who read that answer or holds a copy of the memory of one who did — never
/// which of them (`transport`, "Whose bytes").
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Nonce([u8; 16]);

impl Nonce {
    /// A nonce nobody has seen; `None` when the system has no randomness to
    /// give, and then nothing is answered with one.
    pub(crate) fn fresh() -> Option<Self> {
        use ring::rand::SecureRandom;
        let mut bytes = [0u8; 16];
        ring::rand::SystemRandom::new().fill(&mut bytes).ok()?;
        Some(Self(bytes))
    }

    /// Thirty-two lowercase hexadecimal digits, and nothing else.
    fn parse(text: &str) -> Option<Self> {
        let digits = text.as_bytes();
        if digits.len() != 32 {
            return None;
        }
        let value = |digit: u8| match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'a'..=b'f' => Some(digit - b'a' + 10),
            _ => None,
        };
        let mut bytes = [0u8; 16];
        for (at, pair) in digits.chunks(2).enumerate() {
            bytes[at] = value(pair[0])? << 4 | value(pair[1])?;
        }
        Some(Self(bytes))
    }

    fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// Whether two nonces are the same one, compared without a shortcut.
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |differ, (a, b)| differ | (a ^ b))
            == 0
    }

    #[cfg(test)]
    pub(crate) fn of(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for Nonce {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        // never printed: a log or a failed assertion must not carry one
        f.write_str("Nonce(..)")
    }
}

/// What an observer claims about itself and its pane, in its `attest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hello {
    pub(crate) socket: String,
    pub(crate) server_pid: u32,
    pub(crate) pane: String,
    pub(crate) version: PiVersion,
    pub(crate) capabilities: Capabilities,
}

/// One line of this family, as a connection carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Message {
    /// A request for a nonce. It claims nothing.
    Hello,
    /// The observer's claims, with the nonce its hello was answered with.
    Attest { nonce: Nonce, hello: Hello },
    /// One frame of the lease it names, with the nonce that lease's last
    /// answer gave. A frame that is not one of the five shapes is kept as
    /// its refusal: whose mistake that is can only be said once the sender
    /// is proven (`transport`, "What is checked for every frame").
    Frame {
        lease: u64,
        nonce: Nonce,
        frame: Result<Frame, &'static str>,
    },
}

/// One frame of a lease.
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
    nonce: Option<String>,
    lease: Option<u64>,
    /// A key this family does not have, or one that came twice: the line is
    /// refused, and whose line it was may still be asked (`Message::Frame`).
    malformed: bool,
}

fn put<T, E: serde::de::Error>(slot: &mut Option<T>, value: T) -> Result<(), E> {
    if slot.replace(value).is_some() {
        return Err(E::custom("duplicate"));
    }
    Ok(())
}

/// `put` for a key of the line itself: a second one is kept as what makes
/// the line malformed, not as a failure to read it.
fn keep<T>(slot: &mut Option<T>, value: T, malformed: &mut bool) {
    if slot.is_some() {
        *malformed = true;
    } else {
        *slot = Some(value);
    }
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
        let mut fields = Fields::default();
        let mut malformed = false;
        while let Some(key) = map.next_key::<String>()? {
            let bad = &mut malformed;
            match key.as_str() {
                "protocol" => keep(&mut fields.protocol, map.next_value()?, bad),
                "v" => keep(&mut fields.version, map.next_value()?, bad),
                "source" => keep(&mut fields.source, map.next_value()?, bad),
                "socket" => keep(&mut fields.socket, map.next_value()?, bad),
                "server_pid" => keep(&mut fields.server_pid, map.next_value()?, bad),
                "pane" => keep(&mut fields.pane, map.next_value()?, bad),
                "observerVersion" => keep(&mut fields.observer_version, map.next_value()?, bad),
                "piVersion" => keep(&mut fields.pi_version, map.next_value()?, bad),
                "mode" => keep(&mut fields.mode, map.next_value()?, bad),
                "capabilities" => keep(
                    &mut fields.capabilities,
                    map.next_value_seed(CapabilitiesSeed)?,
                    bad,
                ),
                "seq" => keep(&mut fields.seq, map.next_value()?, bad),
                "run" => keep(&mut fields.run, map.next_value()?, bad),
                "state" => keep(&mut fields.state, map.next_value()?, bad),
                "reason" => keep(&mut fields.reason, map.next_value()?, bad),
                "event" => keep(&mut fields.event, map.next_value()?, bad),
                "nativeSessionId" => keep(&mut fields.native_id, map.next_value()?, bad),
                "unavailable" => keep(&mut fields.unavailable, map.next_value()?, bad),
                "nonce" => keep(&mut fields.nonce, map.next_value()?, bad),
                "lease" => keep(&mut fields.lease, map.next_value()?, bad),
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                    malformed = true;
                }
            }
        }
        fields.malformed = malformed;
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

/// One line of this family. The error is the closed refusal code.
pub(crate) fn parse_message(line: &str) -> Result<Message, &'static str> {
    let fields = fields(line)?;
    let nonce = match fields.nonce.as_deref() {
        Some(nonce) => Some(Nonce::parse(nonce).ok_or("bad-frame")?),
        None => None,
    };
    if let Some(lease) = fields.lease {
        return Ok(Message::Frame {
            lease: count(Some(lease))?,
            nonce: nonce.ok_or("bad-frame")?,
            frame: if fields.malformed {
                Err("bad-frame")
            } else {
                frame(fields)
            },
        });
    }
    if fields.malformed {
        return Err("bad-frame");
    }
    if let Some(nonce) = nonce {
        return Ok(Message::Attest {
            nonce,
            hello: claims(fields)?,
        });
    }
    // a hello carries nothing of its own
    match fields {
        Fields {
            socket: None,
            server_pid: None,
            pane: None,
            observer_version: None,
            pi_version: None,
            mode: None,
            capabilities: None,
            seq: None,
            run: None,
            state: None,
            reason: None,
            event: None,
            native_id: None,
            unavailable: None,
            ..
        } => Ok(Message::Hello),
        _ => Err("bad-frame"),
    }
}

/// The claims of an `attest`.
fn claims(fields: Fields) -> Result<Hello, &'static str> {
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
        lease: None,
        ..
    } = fields
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

/// The frame of a line that names a lease.
fn frame(fields: Fields) -> Result<Frame, &'static str> {
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
    } = fields
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

fn bits(now: Capabilities) -> String {
    format!(
        "\"capabilities\":{{\"signal\":{},\"lifecycle\":{}}}",
        now.signal, now.lifecycle
    )
}

/// To a hello: the nonce the `attest` of this process must carry.
pub(crate) fn nonce_line(next: &Nonce) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"nonce\":\"{}\"}}\n",
        next.hex()
    )
}

/// The observer is bound: its lease, the nonce its first frame must carry,
/// and what that lease may do.
pub(crate) fn accepted_line(lease: u64, next: &Nonce, now: Capabilities) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"accepted\":{lease},\"nonce\":\"{}\",{}}}\n",
        next.hex(),
        bits(now)
    )
}

/// Frame `seq` was verified and committed; the nonce the next frame must
/// carry and what the lease may do now.
pub(crate) fn ack_line(seq: u64, next: &Nonce, now: Capabilities) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"ack\":{seq},\"nonce\":\"{}\",{}}}\n",
        next.hex(),
        bits(now)
    )
}

/// Frame `seq` belongs to a capability this lease no longer has: it was
/// not committed, and the lease goes on with what it may do now.
pub(crate) fn revoked_line(seq: u64, next: &Nonce, now: Capabilities) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"revoked\":{seq},\"nonce\":\"{}\",{}}}\n",
        next.hex(),
        bits(now)
    )
}

/// The sender of this connection's line could not be proven, through no
/// fault of the line: nothing was read into anything, and the same message
/// may come again on a new connection, with this nonce.
pub(crate) fn again_line(next: &Nonce) -> String {
    format!(
        "{{\"protocol\":\"{FAMILY}\",\"v\":{WIRE_VERSION},\"again\":true,\"nonce\":\"{}\"}}\n",
        next.hex()
    )
}

/// The message is refused, for a closed reason: one of `REFUSALS`, a plain
/// word that needs no escaping.
pub(crate) fn refused_line(code: &'static str) -> String {
    refused_as(code, WIRE_VERSION)
}

fn refused_as(code: &'static str, version: u64) -> String {
    format!("{{\"protocol\":\"{FAMILY}\",\"v\":{version},\"refused\":\"{code}\"}}\n")
}

/// The refusal of `line` for `code`. A line of another wire version is told
/// so in the framing it claims (module header, "Versions"): an observer
/// reads nothing that is not of its own version.
pub(crate) fn refusal_of(line: &str, code: &'static str) -> String {
    let claimed = (code == "bad-version")
        .then(|| serde_json::from_str::<serde_json::Value>(line).ok())
        .flatten()
        .and_then(|value| value.get("v").and_then(serde_json::Value::as_u64))
        .filter(|version| (1..=99).contains(version));
    refused_as(code, claimed.unwrap_or(WIRE_VERSION))
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
    "no-challenge",
    "sender-mismatch",
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
        "no-challenge" => "pi-no-challenge",
        "sender-mismatch" => "pi-sender-mismatch",
        _ => "other",
    }
}

#[cfg(test)]
mod tests;
