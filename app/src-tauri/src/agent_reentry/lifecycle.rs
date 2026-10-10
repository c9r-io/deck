//! The lifecycle protocol family: closed wire parsing, kernel attribution of
//! a receipt to one exact client, and the mailbox of the one re-entry
//! operation that may be in flight.
//!
//! # Contract
//! `deck-lifecycle-helper` (bundled, its own closed family — NOT an
//! extension of `deck-status-helper`'s payload contract) writes one line to
//! the existing 0600 status socket:
//! `{protocol:"agent-lifecycle", v:1, source, event, nativeSessionId,
//! startSource | endReason, [finalPermission], socket, server_pid, pane}`.
//! `parse` accepts exactly that: a known family and version, one valid
//! lowercase UUID, closed words, no unknown key, no duplicate key, no
//! field of the other event, at most `MAX_LINE` bytes. There is no payload,
//! prompt, tool, transcript, path, model or free-form field to carry.
//!
//! A line is a CLAIM. `receive` turns it into a fact only for an armed
//! lease of the running operation, and only by what the kernel says while
//! the helper is still connected: the peer executes the bundled lifecycle
//! helper; its DIRECT parent is the lease's client (the frozen old pid and
//! birth instant for an exit lease; for a launch lease a child of the new
//! shell that is the pane's foreground leader and executes the frozen
//! executable); the pane's shell generation is in the ancestry; the chain is
//! terminal-continuous (`agent_origin`); the claimed server pid and pane are
//! the lease's. The WHOLE attribution — current foreground generation,
//! helper and client identity, ancestry, terminal continuity — is made a
//! second time on a fresh snapshot before the stream is closed (the close is
//! the helper's ACK), and the write requires the very arming that was
//! verified (`Lease::serial`): a lease sealed or re-armed meanwhile takes
//! nothing. A lease keeps at most `MAX_FACTS` facts; one more marks its
//! record incomplete (`Event::Overflow`), and an incomplete record names no
//! final session and confirms no entry. An event without a matching lease is
//! dropped: there is no session registry, no "latest id", and nothing here
//! reaches the Signal store, interactions, episodes, the scheduler, any
//! readiness or any authority. The three sources of trust stay separate:
//! the kernel proves which process reported, the Agent states its native
//! fact, and only the user's confirmation of this restart authorizes a
//! launch. Same-user code inside the client's own tree can still misreport;
//! that residual is accepted for this one user-approved side effect and
//! extends to nothing else.
//!
//! The listener thread calls `receive` and takes no restart guard: it does
//! bounded kernel reads and one short mailbox lock, never tmux, never a
//! shell, Board or queue call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use serde::de::{Deserializer, MapAccess, Visitor};

use super::claude::{EndReason, StartSource};
use crate::agent_status::ProcessTable;
use crate::procinfo::ExecutableRef;
use crate::sync::LockRecover;

pub(crate) const FAMILY: &str = "agent-lifecycle";
const MAX_LINE: usize = 1024;

type Generation = (u32, (u64, u32));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Start(StartSource),
    End(EndReason),
    /// Not a wire event: the lease's record reached its bound and at least
    /// one fact was not kept.
    Overflow,
}

/// One validated wire line: still only a claim.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Wire {
    pub(crate) event: Event,
    pub(crate) native_id: String,
    pub(crate) socket: String,
    pub(crate) server_pid: u32,
    pub(crate) pane: String,
}

/// One receipt attributed to a lease's client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fact {
    pub(crate) event: Event,
    pub(crate) native_id: String,
    pub(crate) client: Generation,
}

#[derive(Default)]
struct Fields {
    protocol: Option<String>,
    version: Option<u64>,
    source: Option<String>,
    event: Option<String>,
    native_id: Option<String>,
    start_source: Option<String>,
    end_reason: Option<String>,
    final_permission: Option<String>,
    socket: Option<String>,
    server_pid: Option<u64>,
    pane: Option<String>,
}

struct FieldsVisitor;

impl<'de> Visitor<'de> for FieldsVisitor {
    type Value = Fields;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a lifecycle object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields, A::Error> {
        use serde::de::Error;
        let mut fields = Fields::default();
        fn put<T, E: Error>(slot: &mut Option<T>, value: T) -> Result<(), E> {
            if slot.replace(value).is_some() {
                return Err(E::custom("duplicate"));
            }
            Ok(())
        }
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "protocol" => put(&mut fields.protocol, map.next_value()?)?,
                "v" => put(&mut fields.version, map.next_value()?)?,
                "source" => put(&mut fields.source, map.next_value()?)?,
                "event" => put(&mut fields.event, map.next_value()?)?,
                "nativeSessionId" => put(&mut fields.native_id, map.next_value()?)?,
                "startSource" => put(&mut fields.start_source, map.next_value()?)?,
                "endReason" => put(&mut fields.end_reason, map.next_value()?)?,
                "finalPermission" => put(&mut fields.final_permission, map.next_value()?)?,
                "socket" => put(&mut fields.socket, map.next_value()?)?,
                "server_pid" => put(&mut fields.server_pid, map.next_value()?)?,
                "pane" => put(&mut fields.pane, map.next_value()?)?,
                _ => return Err(A::Error::custom("unknown key")),
            }
        }
        Ok(fields)
    }
}

/// Whether a line claims to be of this family at all (the listener's
/// dispatch between the Signal parser and this one). A line that is not a
/// JSON object with a string `protocol` key is not.
pub(crate) fn is_family(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .is_some_and(|value| value.get("protocol").is_some())
}

/// Closed validation at the trust boundary; the error is a closed reason.
pub(crate) fn parse(line: &str, known_source: impl Fn(&str) -> bool) -> Result<Wire, &'static str> {
    if line.len() > MAX_LINE {
        return Err("lifecycle-oversize");
    }
    let mut deserializer = serde_json::Deserializer::from_str(line);
    let fields = deserializer
        .deserialize_map(FieldsVisitor)
        .map_err(|_| "lifecycle-bad-json")?;
    deserializer.end().map_err(|_| "lifecycle-bad-json")?;
    if fields.protocol.as_deref() != Some(FAMILY) {
        return Err("lifecycle-bad-family");
    }
    if fields.version != Some(1) {
        return Err("lifecycle-bad-version");
    }
    if !fields.source.as_deref().is_some_and(known_source) {
        return Err("lifecycle-unknown-source");
    }
    let native_id = fields
        .native_id
        .filter(|id| super::claude::native_id_ok(id))
        .ok_or("lifecycle-bad-id")?;
    // The optional final permission is a closed diagnostic word. It is
    // validated and then dropped: nothing in this build reads it.
    if fields
        .final_permission
        .as_deref()
        .is_some_and(|word| !matches!(word, "bypass" | "non-bypass" | "unknown"))
    {
        return Err("lifecycle-bad-field");
    }
    let event = match (
        fields.event.as_deref(),
        fields.start_source.as_deref(),
        fields.end_reason.as_deref(),
    ) {
        (Some("session-start"), Some(source), None) if fields.final_permission.is_none() => {
            Event::Start(super::claude::start_source(source).ok_or("lifecycle-bad-field")?)
        }
        (Some("session-end"), None, Some(reason)) => {
            Event::End(super::claude::end_reason(reason).ok_or("lifecycle-bad-field")?)
        }
        _ => return Err("lifecycle-bad-field"),
    };
    let socket = fields
        .socket
        .filter(|socket| {
            !socket.is_empty()
                && socket.len() <= 64
                && socket
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
        .ok_or("lifecycle-bad-field")?;
    let server_pid = fields
        .server_pid
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 1)
        .ok_or("lifecycle-bad-field")?;
    let pane = fields
        .pane
        .filter(|pane| {
            (2..=10).contains(&pane.len())
                && pane.starts_with('%')
                && pane[1..].bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or("lifecycle-bad-field")?;
    Ok(Wire {
        event,
        native_id,
        socket,
        server_pid,
        pane,
    })
}

// ---------- leases ---------------------------------------------------------------

#[derive(Clone, Debug)]
enum Phase {
    /// Armed before the first exit key: only the frozen old client reports.
    Exit { client: Generation },
    /// Armed before the launch transport: only a foreground child of the new
    /// shell that executes the frozen executable reports.
    Launch { executable: ExecutableRef },
}

#[derive(Clone, Debug)]
pub(crate) struct Lease {
    server_pid: u32,
    pane: String,
    shell: Generation,
    phase: Phase,
    /// Assigned when armed. A verification that started under one arming
    /// can never be written under a later one, even for the same entry,
    /// pane and shell.
    serial: u64,
}

impl Lease {
    pub(crate) fn exit(server_pid: u32, pane: &str, shell: Generation, client: Generation) -> Self {
        Self {
            server_pid,
            pane: pane.to_string(),
            shell,
            phase: Phase::Exit { client },
            serial: 0,
        }
    }

    pub(crate) fn launch(
        server_pid: u32,
        pane: &str,
        shell: Generation,
        executable: ExecutableRef,
    ) -> Self {
        Self {
            server_pid,
            pane: pane.to_string(),
            shell,
            phase: Phase::Launch { executable },
            serial: 0,
        }
    }
}

/// The most facts one lease keeps. One more makes the entry's record
/// incomplete, and an incomplete record confirms nothing.
const MAX_FACTS: usize = 64;

struct Mailbox {
    socket: String,
    helper: PathBuf,
    /// entry index → its one armed lease (re-armed for the launch phase)
    leases: HashMap<usize, Lease>,
    facts: HashMap<usize, Vec<Fact>>,
    next_serial: u64,
}

static MAILBOX: Mutex<Option<Mailbox>> = Mutex::new(None);
static ARRIVED: Condvar = Condvar::new();

/// Open the mailbox of one operation. Any earlier content is discarded.
pub(crate) fn begin(socket: &str, helper: &Path) {
    *MAILBOX.lock_or_recover() = Some(Mailbox {
        socket: socket.to_string(),
        helper: helper.to_path_buf(),
        leases: HashMap::new(),
        facts: HashMap::new(),
        next_serial: 1,
    });
}

/// Close the operation's mailbox: every lease and every native id is gone.
pub(crate) fn end() {
    *MAILBOX.lock_or_recover() = None;
}

/// Arm `entry`'s lease for a phase. Facts of an earlier phase are dropped:
/// an old generation's receipt can never count for the new one.
pub(crate) fn arm(entry: usize, mut lease: Lease) {
    if let Some(mailbox) = MAILBOX.lock_or_recover().as_mut() {
        lease.serial = mailbox.next_serial;
        mailbox.next_serial += 1;
        mailbox.facts.remove(&entry);
        mailbox.leases.insert(entry, lease);
    }
}

/// Seal `entry`: its lease is removed, so a later event updates nothing,
/// and the facts gathered so far are returned.
pub(crate) fn seal(entry: usize) -> Vec<Fact> {
    MAILBOX
        .lock_or_recover()
        .as_mut()
        .map(|mailbox| {
            mailbox.leases.remove(&entry);
            mailbox.facts.remove(&entry).unwrap_or_default()
        })
        .unwrap_or_default()
}

/// `entry`'s facts so far, leaving the lease armed.
pub(crate) fn peek(entry: usize) -> Vec<Fact> {
    MAILBOX
        .lock_or_recover()
        .as_ref()
        .and_then(|mailbox| mailbox.facts.get(&entry).cloned())
        .unwrap_or_default()
}

/// Record a fact for `entry` as if it had been attributed (tests of what the
/// coordinator does with facts; attribution itself is tested through
/// `receive`).
#[cfg(test)]
pub(crate) fn record_for_tests(entry: usize, fact: Fact) {
    if let Some(mailbox) = MAILBOX.lock_or_recover().as_mut() {
        mailbox.facts.entry(entry).or_default().push(fact);
    }
}

/// Whether any lease is armed (the listener's cheap first question).
pub(crate) fn armed() -> bool {
    MAILBOX
        .lock_or_recover()
        .as_ref()
        .is_some_and(|mailbox| !mailbox.leases.is_empty())
}

/// Sleep until a receipt arrives or `limit` passes.
pub(crate) fn wait(limit: Duration) {
    let guard = MAILBOX.lock_or_recover();
    drop(crate::sync::wait_timeout_or_recover(&ARRIVED, guard, limit));
}

fn generation(table: &ProcessTable, pid: u32) -> Option<Generation> {
    table
        .get(&pid)
        .filter(|info| info.start_seconds != 0)
        .map(|info| (pid, (info.start_seconds, info.start_micros)))
}

/// The kernel reads one attribution needs. Production is `Kernel::live`;
/// tests supply process tables that describe a topology exactly.
pub(crate) struct Kernel<'a> {
    /// The snapshot taken when the line was read, helper still connected.
    pub(crate) table: &'a ProcessTable,
    /// A fresh snapshot for the re-check immediately before the ACK.
    pub(crate) again: &'a dyn Fn() -> ProcessTable,
    /// The process executes the bundled lifecycle helper (file identity).
    pub(crate) runs_helper: &'a dyn Fn(u32, &Path) -> bool,
    /// The process generation executes exactly this frozen executable.
    pub(crate) runs_executable: &'a dyn Fn(Generation, &ExecutableRef) -> bool,
}

impl<'a> Kernel<'a> {
    pub(crate) fn live(table: &'a ProcessTable) -> Self {
        Self {
            table,
            again: &crate::procinfo::processes,
            runs_helper: &crate::procinfo::runs_file,
            runs_executable: &|client, executable| {
                crate::procinfo::runs_executable(client.0, client.1, executable)
            },
        }
    }
}

/// Everything that must hold for `peer` to be the lease's client's own
/// report, read from ONE table. Returns that client generation.
fn attribute(
    lease: &Lease,
    helper: &Path,
    peer: u32,
    table: &ProcessTable,
    kernel: &Kernel,
) -> Result<Generation, &'static str> {
    let reporter = table.get(&peer).ok_or("lifecycle-no-peer")?;
    if !(kernel.runs_helper)(peer, helper) {
        return Err("lifecycle-foreign-helper");
    }
    // The helper's direct parent is the client: a hook run by a tool shell,
    // a subagent or a wrapper has another parent and proves nothing here.
    let client = generation(table, reporter.ppid).ok_or("lifecycle-foreign-client")?;
    let chain = crate::procinfo::ancestry_in(table, peer, crate::agent_origin::ORIGIN_HOPS);
    if generation(table, lease.shell.0) != Some(lease.shell) || !chain.contains(&lease.shell.0) {
        return Err("lifecycle-foreign-pane");
    }
    // the client is the pane's CURRENT foreground generation
    let foreground = crate::agent_status::foreground_generation(table, lease.shell.0)
        .ok_or("lifecycle-foreign-client")?;
    if (foreground.pid(), foreground.birth()) != client {
        return Err("lifecycle-foreign-client");
    }
    match &lease.phase {
        Phase::Exit { client: frozen } => {
            if client != *frozen {
                return Err("lifecycle-foreign-client");
            }
        }
        Phase::Launch { executable } => {
            let direct = table
                .get(&client.0)
                .is_some_and(|info| info.ppid == lease.shell.0);
            if !direct || !(kernel.runs_executable)(client, executable) {
                return Err("lifecycle-foreign-client");
            }
        }
    }
    if !crate::agent_origin::terminal_continuous(table, &chain, client.0, lease.shell.0) {
        return Err("lifecycle-terminal-discontinuity");
    }
    Ok(client)
}

/// Attribute one claim to an armed lease by kernel facts, while the helper
/// is still connected, and record it. The error is a closed reason.
///
/// The whole attribution is made twice: on the snapshot taken with the
/// line, and again on a fresh snapshot immediately before the write (the
/// caller closes the stream — the helper's ACK — only after this returns).
/// Both must name the same client. The write then requires the very arming
/// that was verified: a lease sealed or re-armed in between takes nothing.
pub(crate) fn receive(wire: Wire, peer: Option<u32>, kernel: &Kernel) -> Result<(), &'static str> {
    let (entry, lease, helper) = {
        let mailbox = MAILBOX.lock_or_recover();
        let mailbox = mailbox.as_ref().ok_or("lifecycle-no-lease")?;
        if wire.socket != mailbox.socket {
            return Err("lifecycle-no-lease");
        }
        let (entry, lease) = mailbox
            .leases
            .iter()
            .find(|(_, lease)| lease.server_pid == wire.server_pid && lease.pane == wire.pane)
            .ok_or("lifecycle-no-lease")?;
        (*entry, lease.clone(), mailbox.helper.clone())
    };
    let peer = peer.ok_or("lifecycle-no-peer")?;
    let client = attribute(&lease, &helper, peer, kernel.table, kernel)?;
    // Before the ACK: the same attribution on what the kernel says NOW. A
    // client that lost the foreground, exited, or was replaced since the
    // line was read is late, whatever was true a moment ago.
    let now = (kernel.again)();
    if attribute(&lease, &helper, peer, &now, kernel) != Ok(client) {
        return Err("lifecycle-late");
    }
    let mut mailbox = MAILBOX.lock_or_recover();
    let mailbox = mailbox.as_mut().ok_or("lifecycle-no-lease")?;
    // the very arming that was verified, not a successor of it
    if mailbox.leases.get(&entry).map(|now| now.serial) != Some(lease.serial) {
        return Err("lifecycle-late");
    }
    let facts = mailbox.facts.entry(entry).or_default();
    let event = if facts.len() < MAX_FACTS {
        wire.event
    } else if facts.len() == MAX_FACTS {
        // One marker instead of the fact that did not fit: the record is
        // incomplete from here on and can no longer confirm anything.
        Event::Overflow
    } else {
        ARRIVED.notify_all();
        return Err("lifecycle-overflow");
    };
    let overflowed = event == Event::Overflow;
    facts.push(Fact {
        event,
        native_id: wire.native_id,
        client,
    });
    ARRIVED.notify_all();
    if overflowed {
        return Err("lifecycle-overflow");
    }
    Ok(())
}

fn complete(facts: &[Fact]) -> bool {
    !facts.iter().any(|fact| fact.event == Event::Overflow)
}

/// The final native session of an exited client: the id of its applicable
/// end (`prompt-input-exit`), which must be the LAST fact and the only such
/// id. `clear`, `resume`, `logout` and `other` ends switch or name nothing;
/// two different final ids never resolve to the later one, and an
/// incomplete record names nothing.
pub(crate) fn final_session(facts: &[Fact]) -> Option<String> {
    let last = facts.last()?;
    if !complete(facts) || last.event != Event::End(EndReason::PromptInputExit) {
        return None;
    }
    facts
        .iter()
        .filter(|fact| fact.event == Event::End(EndReason::PromptInputExit))
        .all(|fact| fact.native_id == last.native_id)
        .then(|| last.native_id.clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Receipt {
    Pending,
    /// Exactly one client reported, and all it reported is entering the
    /// wanted session through a resume. Carries that client generation so
    /// the coordinator can check it is still the pane's foreground.
    Entered(Generation),
    /// Another id, another source, an end, a second client or an
    /// incomplete record: not confirmed.
    Conflict,
}

/// What the launch-phase facts say about entering `wanted`.
pub(crate) fn reentry(facts: &[Fact], wanted: &str) -> Receipt {
    let Some(first) = facts.first() else {
        return Receipt::Pending;
    };
    let entered = complete(facts)
        && facts.iter().all(|fact| {
            fact.client == first.client
                && fact.native_id == wanted
                && fact.event == Event::Start(StartSource::Resume)
        });
    if entered {
        Receipt::Entered(first.client)
    } else {
        Receipt::Conflict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";
    const OTHER: &str = "8e8889c6-251a-4388-b3a3-4719ad442bce";
    const THIRD: &str = "d7ee403e-251a-4388-b3a3-4719ad442bce";

    fn start(source: &str) -> String {
        format!(
            r#"{{"protocol":"agent-lifecycle","v":1,"source":"claude-code","event":"session-start","nativeSessionId":"{ID}","startSource":"{source}","socket":"deck","server_pid":4242,"pane":"%7"}}"#
        )
    }

    fn end(reason: &str) -> String {
        format!(
            r#"{{"protocol":"agent-lifecycle","v":1,"source":"claude-code","event":"session-end","nativeSessionId":"{ID}","endReason":"{reason}","socket":"deck","server_pid":4242,"pane":"%7"}}"#
        )
    }

    fn fact(event: Event, id: &str) -> Fact {
        Fact {
            event,
            native_id: id.to_string(),
            client: (100, (1, 1)),
        }
    }

    #[test]
    fn the_wire_accepts_exactly_the_two_closed_shapes() {
        let wire = parse(&start("resume"), |source| source == "claude-code").unwrap();
        assert_eq!(wire.event, Event::Start(StartSource::Resume));
        assert_eq!(
            (
                wire.native_id.as_str(),
                wire.socket.as_str(),
                wire.server_pid,
                wire.pane.as_str()
            ),
            (ID, "deck", 4242, "%7")
        );
        assert_eq!(
            parse(&end("prompt-input-exit"), |source| source == "claude-code")
                .unwrap()
                .event,
            Event::End(EndReason::PromptInputExit)
        );
        let with_permission = end("clear").replace(
            r#""endReason":"clear""#,
            r#""endReason":"clear","finalPermission":"unknown""#,
        );
        assert!(parse(&with_permission, |source| source == "claude-code").is_ok());
        assert!(is_family(&start("resume")));
        assert!(!is_family(
            r#"{"v":1,"source":"claude-code","state":"working"}"#
        ));
        assert!(!is_family("not json"));
    }

    #[test]
    fn every_malformed_or_foreign_line_is_refused_with_a_closed_reason() {
        let good = start("resume");
        let cases: Vec<(String, &str)> = vec![
            (
                good.replace("agent-lifecycle", "agent-status"),
                "lifecycle-bad-family",
            ),
            (
                good.replace(r#""v":1"#, r#""v":2"#),
                "lifecycle-bad-version",
            ),
            (
                good.replace("claude-code", "codex"),
                "lifecycle-unknown-source",
            ),
            (good.replace(ID, &ID.to_uppercase()), "lifecycle-bad-id"),
            (good.replace(ID, "latest"), "lifecycle-bad-id"),
            (good.replace("resume", "fork"), "lifecycle-bad-field"),
            // a field of the other event
            (
                good.replace(r#""socket""#, r#""endReason":"clear","socket""#),
                "lifecycle-bad-field",
            ),
            (
                good.replace(r#""socket""#, r#""finalPermission":"bypass","socket""#),
                "lifecycle-bad-field",
            ),
            (
                end("clear").replace(
                    r#""socket""#,
                    r#""finalPermission":"bypassPermissions","socket""#,
                ),
                "lifecycle-bad-field",
            ),
            // an unknown key: no payload can ride along
            (
                good.replace(r#""socket""#, r#""prompt":"SENTINEL","socket""#),
                "lifecycle-bad-json",
            ),
            (
                good.replace(r#""socket""#, r#""transcript_path":"/x","socket""#),
                "lifecycle-bad-json",
            ),
            // a duplicate key never resolves to the later value
            (
                good.replace(
                    r#""socket""#,
                    &format!(r#""nativeSessionId":"{OTHER}","socket""#),
                ),
                "lifecycle-bad-json",
            ),
            (
                good.replace(r#""pane":"%7""#, r#""pane":"7""#),
                "lifecycle-bad-field",
            ),
            (good.replace("4242", "1"), "lifecycle-bad-field"),
            (
                good.replace(r#""server_pid":4242"#, r#""server_pid":"4242""#),
                "lifecycle-bad-json",
            ),
            (
                good.replace(r#""deck""#, r#""de ck""#),
                "lifecycle-bad-field",
            ),
            (format!("{good} {{}}"), "lifecycle-bad-json"),
            (format!("[{good}]"), "lifecycle-bad-json"),
            (
                good.replace(ID, &format!("{ID}{}", " ".repeat(MAX_LINE))),
                "lifecycle-oversize",
            ),
            ("\u{0}".to_string(), "lifecycle-bad-json"),
        ];
        for (line, reason) in cases {
            assert_eq!(
                parse(&line, |source| source == "claude-code").unwrap_err(),
                reason,
                "{line}"
            );
        }
    }

    /// One client switches A → clear → B → resume → C and exits at C: the
    /// final session is C, whatever the arrival order of the starts.
    #[test]
    fn the_final_session_is_the_exit_receipt_not_the_last_start() {
        use EndReason::{Logout, PromptInputExit};
        use StartSource::Startup;
        let facts = vec![
            fact(Event::Start(Startup), ID),
            fact(Event::End(EndReason::Clear), ID),
            fact(Event::Start(StartSource::Clear), OTHER),
            fact(Event::End(EndReason::Resume), OTHER),
            fact(Event::Start(StartSource::Resume), THIRD),
            fact(Event::End(PromptInputExit), THIRD),
        ];
        assert_eq!(final_session(&facts).as_deref(), Some(THIRD));
        // the same final fact twice is one fact
        let mut repeated = facts.clone();
        repeated.push(fact(Event::End(PromptInputExit), THIRD));
        assert_eq!(final_session(&repeated).as_deref(), Some(THIRD));
        // no applicable end: a clear, resume, logout or other end names nothing
        for reason in [
            EndReason::Clear,
            EndReason::Resume,
            Logout,
            EndReason::Other,
        ] {
            assert_eq!(final_session(&[fact(Event::End(reason), ID)]), None);
        }
        assert_eq!(final_session(&facts[..5]), None, "a start is not an exit");
        assert_eq!(final_session(&[]), None);
        // two different final ids never resolve to the later one
        let mut conflicting = facts.clone();
        conflicting.push(fact(Event::End(PromptInputExit), OTHER));
        assert_eq!(final_session(&conflicting), None);
        // something after the final end makes it not final
        let mut trailing = facts;
        trailing.push(fact(Event::Start(Startup), OTHER));
        assert_eq!(final_session(&trailing), None);
    }

    #[test]
    fn only_a_resume_of_the_same_id_by_one_client_is_an_entry() {
        use StartSource::*;
        let entered = fact(Event::Start(Resume), ID);
        assert_eq!(reentry(&[], ID), Receipt::Pending);
        assert_eq!(
            reentry(std::slice::from_ref(&entered), ID),
            Receipt::Entered((100, (1, 1)))
        );
        assert_eq!(
            reentry(&[entered.clone(), entered.clone()], ID),
            Receipt::Entered((100, (1, 1)))
        );
        for other in [
            fact(Event::Start(Resume), OTHER),
            fact(Event::Start(Startup), ID),
            fact(Event::Start(Clear), ID),
            fact(Event::Start(StartSource::Other), ID),
            fact(Event::End(EndReason::PromptInputExit), ID),
        ] {
            assert_eq!(reentry(std::slice::from_ref(&other), ID), Receipt::Conflict);
            // a switch or an exit after the entry makes it ambiguous
            assert_eq!(reentry(&[entered.clone(), other], ID), Receipt::Conflict);
        }
        let mut second_client = entered.clone();
        second_client.client = (101, (2, 2));
        assert_eq!(reentry(&[entered, second_client], ID), Receipt::Conflict);
    }

    #[test]
    fn without_an_operation_or_a_matching_lease_an_event_is_dropped() {
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        end_mailbox_for_test();
        let table = crate::procinfo::processes();
        let me = Some(std::process::id());
        let wire = || parse(&start("resume"), |source| source == "claude-code").unwrap();
        assert_eq!(
            receive(wire(), me, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-no-lease"
        );
        assert!(!armed());
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        assert_eq!(
            receive(wire(), me, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-no-lease"
        );
        // a lease for another server generation that reuses the pane id
        arm(0, Lease::exit(9999, "%7", (1, (1, 1)), (2, (2, 2))));
        assert!(armed());
        assert_eq!(
            receive(wire(), me, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-no-lease"
        );
        // the right pane, another socket
        arm(0, Lease::exit(4242, "%7", (1, (1, 1)), (2, (2, 2))));
        let mut foreign = wire();
        foreign.socket = "deck-dev".into();
        assert_eq!(
            receive(foreign, me, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-no-lease"
        );
        // the right claim from a process that is not the bundled helper
        assert_eq!(
            receive(wire(), None, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-no-peer"
        );
        assert_eq!(
            receive(wire(), me, &Kernel::live(&table)).unwrap_err(),
            "lifecycle-foreign-helper"
        );
        assert!(peek(0).is_empty());
        assert!(seal(0).is_empty());
        assert!(!armed(), "a sealed entry has no lease");
        end_mailbox_for_test();
    }

    fn end_mailbox_for_test() {
        super::end();
    }

    // ---------- attribution on exact process topologies ----------------------

    use crate::procinfo::ProcessInfo;

    const SHELL: Generation = (10, (100, 1));
    const CLIENT: Generation = (20, (200, 2));
    const TTY: u32 = 7;

    fn process(pid: u32, ppid: u32, pgid: u32, tty: u32, foreground: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            pgid,
            tty,
            tty_pgid: foreground,
            start_seconds: u64::from(pid) * 10,
            start_micros: pid / 10,
        }
    }

    /// The certified topology: the pane's shell, the client in the tty's
    /// foreground, and the helper as the client's direct child in its own
    /// terminal-less session.
    fn topology(foreground: u32) -> ProcessTable {
        [
            process(SHELL.0, 1, SHELL.0, TTY, foreground),
            process(CLIENT.0, SHELL.0, CLIENT.0, TTY, foreground),
            process(30, CLIENT.0, 30, 0, 0),
        ]
        .into_iter()
        .map(|info| (info.pid, info))
        .collect()
    }

    fn exit_lease() -> Lease {
        Lease::exit(4242, "%7", SHELL, CLIENT)
    }

    fn end_wire(id: &str) -> Wire {
        parse(&end("prompt-input-exit").replace(ID, id), |source| {
            source == "claude-code"
        })
        .unwrap()
    }

    fn resume_wire(id: &str) -> Wire {
        parse(&start("resume").replace(ID, id), |source| {
            source == "claude-code"
        })
        .unwrap()
    }

    /// `receive` on `table`, with `again` as the kernel's answer right
    /// before the ACK.
    fn attributed(
        wire: Wire,
        table: &ProcessTable,
        again: &dyn Fn() -> ProcessTable,
    ) -> Result<(), &'static str> {
        receive(
            wire,
            Some(30),
            &Kernel {
                table,
                again,
                runs_helper: &|_, _| true,
                runs_executable: &|_, _| true,
            },
        )
    }

    fn with_lease(lease: Lease, work: impl FnOnce()) {
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        end_mailbox_for_test();
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        arm(0, lease);
        work();
        end_mailbox_for_test();
    }

    #[test]
    fn the_certified_topology_is_attributed_and_recorded() {
        with_lease(exit_lease(), || {
            let table = topology(CLIENT.0);
            assert_eq!(
                attributed(end_wire(ID), &table, &|| topology(CLIENT.0)),
                Ok(())
            );
            assert_eq!(
                peek(0),
                [Fact {
                    event: Event::End(EndReason::PromptInputExit),
                    native_id: ID.into(),
                    client: CLIENT,
                }]
            );
            // a helper that is not the bundled one is nobody's report
            let foreign = receive(
                end_wire(ID),
                Some(30),
                &Kernel {
                    table: &table,
                    again: &|| topology(CLIENT.0),
                    runs_helper: &|_, _| false,
                    runs_executable: &|_, _| true,
                },
            );
            assert_eq!(foreign.unwrap_err(), "lifecycle-foreign-helper");
            assert_eq!(peek(0).len(), 1);
        });
    }

    /// The foreground changes between reading the line and the ACK: the
    /// claim was true a moment ago and is refused, with nothing recorded.
    #[test]
    fn a_foreground_that_changes_before_the_ack_is_late() {
        with_lease(exit_lease(), || {
            let table = topology(CLIENT.0);
            // the shell took the foreground back
            assert_eq!(
                attributed(end_wire(ID), &table, &|| topology(SHELL.0)).unwrap_err(),
                "lifecycle-late"
            );
            // the client is gone and its pid is not in the table any more
            let gone = || {
                let mut now = topology(SHELL.0);
                now.remove(&CLIENT.0);
                now
            };
            assert_eq!(
                attributed(end_wire(ID), &table, &gone).unwrap_err(),
                "lifecycle-late"
            );
            // another process generation under the client's pid
            let reused = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&CLIENT.0).unwrap().start_seconds += 1;
                now
            };
            assert_eq!(
                attributed(end_wire(ID), &table, &reused).unwrap_err(),
                "lifecycle-late"
            );
            assert!(peek(0).is_empty());
        });
    }

    /// A helper that reports after its client already returned the terminal
    /// to the shell is not the foreground client's report.
    #[test]
    fn a_report_after_the_shell_returned_is_refused() {
        with_lease(exit_lease(), || {
            let late = topology(SHELL.0);
            assert_eq!(
                attributed(end_wire(ID), &late, &|| topology(SHELL.0)).unwrap_err(),
                "lifecycle-foreign-client"
            );
            // its client exited: the helper was reparented
            let mut orphan = topology(SHELL.0);
            orphan.remove(&CLIENT.0);
            orphan.get_mut(&30).unwrap().ppid = 1;
            assert_eq!(
                attributed(end_wire(ID), &orphan, &|| topology(SHELL.0)).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert!(peek(0).is_empty());
        });
    }

    /// A lease sealed and armed again while the kernel was being read: the
    /// verification belongs to the earlier arming and writes nothing into
    /// its successor, even for the same entry, pane, shell and client.
    #[test]
    fn a_verification_never_lands_in_a_later_arming_of_the_lease() {
        with_lease(exit_lease(), || {
            let table = topology(CLIENT.0);
            let rearm = || {
                assert!(seal(0).is_empty());
                arm(0, exit_lease());
                topology(CLIENT.0)
            };
            assert_eq!(
                attributed(end_wire(ID), &table, &rearm).unwrap_err(),
                "lifecycle-late"
            );
            assert!(peek(0).is_empty(), "the successor lease took nothing");
            // sealed and not armed again: nothing to land on
            let sealed = || {
                seal(0);
                topology(CLIENT.0)
            };
            assert_eq!(
                attributed(end_wire(ID), &table, &sealed).unwrap_err(),
                "lifecycle-late"
            );
        });
    }

    /// A daemon between the client and the helper, a helper that shares a
    /// terminal-less group it does not lead, and a wrapper in the
    /// foreground: none is the certified direct topology.
    #[test]
    fn a_daemon_or_a_broken_terminal_chain_is_refused() {
        with_lease(exit_lease(), || {
            // helper → daemon (terminal-less, own group) → client
            let mut daemon = topology(CLIENT.0);
            daemon.insert(25, process(25, CLIENT.0, 25, 0, 0));
            daemon.get_mut(&30).unwrap().ppid = 25;
            assert_eq!(
                attributed(end_wire(ID), &daemon, &|| topology(CLIENT.0)).unwrap_err(),
                "lifecycle-foreign-client"
            );
            // the helper is the client's child but sits in a terminal-less
            // group led by some other process
            let mut shared = topology(CLIENT.0);
            shared.get_mut(&30).unwrap().pgid = 31;
            assert_eq!(
                attributed(end_wire(ID), &shared, &|| topology(CLIENT.0)).unwrap_err(),
                "lifecycle-terminal-discontinuity"
            );
            // the helper is on another terminal
            let mut elsewhere = topology(CLIENT.0);
            elsewhere.get_mut(&30).unwrap().tty = 9;
            assert_eq!(
                attributed(end_wire(ID), &elsewhere, &|| topology(CLIENT.0)).unwrap_err(),
                "lifecycle-terminal-discontinuity"
            );
            // the pane's shell generation is another one (a reused pid)
            let mut reused = topology(CLIENT.0);
            reused.get_mut(&SHELL.0).unwrap().start_seconds += 1;
            assert_eq!(
                attributed(end_wire(ID), &reused, &|| topology(CLIENT.0)).unwrap_err(),
                "lifecycle-foreign-pane"
            );
            assert!(peek(0).is_empty());
        });
    }

    /// The launch lease: only a direct child of the new shell that executes
    /// the frozen executable and holds the foreground reports.
    #[test]
    fn a_launch_lease_takes_only_the_new_shells_direct_foreground_client() {
        let executable =
            ExecutableRef::read(&std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
                .expect("own executable");
        with_lease(Lease::launch(4242, "%7", SHELL, executable), || {
            let table = topology(CLIENT.0);
            let kernel = |runs: bool| Kernel {
                table: &table,
                again: &|| topology(CLIENT.0),
                runs_helper: &|_, _| true,
                runs_executable: if runs { &|_, _| true } else { &|_, _| false },
            };
            assert_eq!(
                receive(resume_wire(ID), Some(30), &kernel(false)).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert_eq!(receive(resume_wire(ID), Some(30), &kernel(true)), Ok(()));
            assert_eq!(reentry(&peek(0), ID), Receipt::Entered(CLIENT));
            // a client that is the shell's grandchild is not a direct launch
            let mut wrapped = topology(CLIENT.0);
            wrapped.insert(15, process(15, SHELL.0, 15, TTY, CLIENT.0));
            wrapped.get_mut(&CLIENT.0).unwrap().ppid = 15;
            let nested = Kernel {
                table: &wrapped,
                again: &|| topology(CLIENT.0),
                runs_helper: &|_, _| true,
                runs_executable: &|_, _| true,
            };
            assert_eq!(
                receive(resume_wire(ID), Some(30), &nested).unwrap_err(),
                "lifecycle-foreign-client"
            );
        });
    }

    /// Sixty-four valid facts fit. The sixty-fifth — here a conflicting one —
    /// is not silently dropped behind a success: the record is marked
    /// incomplete, the report is refused, and nothing is confirmed.
    #[test]
    fn a_record_that_overflows_confirms_nothing() {
        let executable =
            ExecutableRef::read(&std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
                .expect("own executable");
        with_lease(Lease::launch(4242, "%7", SHELL, executable), || {
            let table = topology(CLIENT.0);
            let again = || topology(CLIENT.0);
            for _ in 0..MAX_FACTS {
                assert_eq!(attributed(resume_wire(ID), &table, &again), Ok(()));
            }
            assert_eq!(reentry(&peek(0), ID), Receipt::Entered(CLIENT));
            assert_eq!(
                attributed(resume_wire(OTHER), &table, &again).unwrap_err(),
                "lifecycle-overflow"
            );
            assert_eq!(reentry(&peek(0), ID), Receipt::Conflict);
            // and it stays that way, without growing
            assert_eq!(
                attributed(resume_wire(ID), &table, &again).unwrap_err(),
                "lifecycle-overflow"
            );
            let facts = seal(0);
            assert_eq!(facts.len(), MAX_FACTS + 1);
            assert_eq!(reentry(&facts, ID), Receipt::Conflict);
        });
        with_lease(exit_lease(), || {
            let table = topology(CLIENT.0);
            let again = || topology(CLIENT.0);
            for _ in 0..MAX_FACTS {
                assert_eq!(attributed(end_wire(ID), &table, &again), Ok(()));
            }
            assert_eq!(final_session(&peek(0)).as_deref(), Some(ID));
            assert!(attributed(end_wire(ID), &table, &again).is_err());
            assert_eq!(final_session(&seal(0)), None);
        });
    }
}
