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
//! `parse` accepts exactly that: a known family and version, closed keys, no
//! unknown key, no duplicate key, at most `MAX_LINE` bytes, and a source
//! that HAS a receipt contract. There is no payload, prompt, tool,
//! transcript, path, model or free-form field to carry.
//!
//! **What a receipt means is its source's, not this module's.** A source
//! that may send receipts declares a `ReceiptContract` on its re-entry
//! capability (`ReentryCapability::receipts`; today only Claude Code,
//! `claude.rs`): the pure decoding of its own words and its own native id
//! into its closed tag of `Event`, and the two reducers over a lease's
//! record (`final_session`, `entered`). This module owns the wire framing,
//! the origin and kernel attribution, the lease, the queue, the deadlines,
//! the mailbox and every fence below, and it keeps a record without reading
//! what its facts say: a source without a contract can send no receipt, and
//! one source's id or word is never judged by another's rule. Registering a
//! source, or giving it another kind of observation (`observe_pi` below),
//! gives it no contract.
//!
//! A line is a CLAIM. `receive` turns it into a fact only for an armed
//! lease of the running operation, and only by what the kernel says while
//! the helper is still connected: the peer executes the bundled lifecycle
//! helper; its DIRECT parent is the lease's client (the frozen old pid and
//! birth instant for an exit lease; for a launch lease a child of the new
//! shell that is the pane's foreground leader) and that client is BOUND to
//! the frozen executable — its running image, not its pid, its name or its
//! path alone (`procinfo::runs_executable`: kernel reads and one dynamic
//! validation, no file content read). An old client that replaced
//! its image under the same pid and birth instant reports nothing, and
//! neither does a started client that runs another build. The pane's shell
//! generation is in the ancestry; the chain is
//! terminal-continuous (`agent_origin`); the claimed server pid and pane are
//! the lease's.
//!
//! **Before the ACK means after the last call that can take time.** The
//! dynamic validation cannot be interrupted, and how long it takes is not
//! this module's to promise. So it is made ONCE, between two attributions,
//! and the second one — the helper's generation and file, the client's
//! generation, image path and foreground, the pane's shell, the ancestry,
//! the terminal chain — is read from a snapshot taken AFTER the validation
//! returned, immediately before the write. A snapshot older than that call
//! is never evidence that something still holds.
//!
//! **What a claim is bound to, and where that is checked.** A claim is taken
//! from one mailbox and one arming, and it can be written to nothing else.
//! Under the mailbox lock that writes the fact — not only when the claim was
//! taken or queued — all of this must hold:
//! - the mailbox is the very operation the claim was taken from
//!   (`Mailbox::operation`, a number this process gives each `begin` and
//!   never uses again). The next operation numbers its armings from one
//!   again, and may arm the same entry for the same pane, shell and client:
//!   a claim that outlived its operation in the queue or inside the
//!   validation is still refused there;
//! - the lease is the very arming that was claimed (`Lease::serial`, counted
//!   inside one operation: a lease sealed or armed again takes nothing);
//! - the time is inside BOTH limits: the receipt's own budget
//!   (`RECEIPT_BUDGET`, from the instant its line was read) and, for a launch
//!   lease, the end of the phase it was armed for (`Phase::Launch::closes`,
//!   the restart's post deadline). The second does not wait for anybody to
//!   seal: a lease whose phase is over takes nothing from that instant on,
//!   however late the coordinator gets to it, and whatever is left of the
//!   receipt's own budget.
//!
//! An answer that arrives after either limit is refused as late, a positive
//! one included. That the helper gives up waiting by itself is not what
//! refuses a late fact; this is. The coordinator, for its part, confirms
//! from a set it sealed for every entry at one instant (`seal_all`), before
//! its first check of a started client: no entry's record depends on how
//! long another entry's check takes.
//!
//! A lease keeps at most `MAX_FACTS` facts; one more marks its
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
//! **Who waits for the validation.** Not the status listener: that thread
//! serves every agent's Signal on the same socket, and of a lifecycle line
//! it does only `claim` — one short mailbox lock — before it hands the
//! connection to the one receipt worker and goes back to accepting
//! (`agent_status::Receipts`: one thread, one bounded queue; a full queue
//! drops the receipt). The worker calls `receive` under the receipt's
//! deadline and takes no restart guard, no Board fence and no queue lock:
//! kernel reads, that one validation and two short mailbox locks — never a
//! full read or hash of a file, never tmux, never a shell. A validation that
//! does not return holds that one worker and nothing else; receipts queued
//! behind it are refused as late, unread, when it does, and each is at worst
//! an entry that is not confirmed or a final session that is not named —
//! never a second request.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::de::{Deserializer, MapAccess, Visitor};

use crate::agent_status::ProcessTable;
use crate::procinfo::ExecutableRef;
use crate::sync::LockRecover;

pub(crate) const FAMILY: &str = "agent-lifecycle";
const MAX_LINE: usize = 1024;

type Generation = (u32, (u64, u32));

/// What one fact of a lease's record is. A receipt carries its source's own
/// closed tag — built and read only by that source's `ReceiptContract`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// A receipt of Claude Code, in Claude's own words (`claude.rs`).
    Claude(super::claude::Told),
    /// Not a wire event: the lease's record reached its bound and at least
    /// one fact was not kept.
    Overflow,
}

/// The source's own words of one line, as they were written: closed keys,
/// values its contract closes. Borrowed from the line for the decoding only.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Words<'a> {
    pub(crate) event: Option<&'a str>,
    pub(crate) native_id: Option<&'a str>,
    pub(crate) start_source: Option<&'a str>,
    pub(crate) end_reason: Option<&'a str>,
    pub(crate) final_permission: Option<&'a str>,
}

/// What one source's lifecycle receipts mean: static data of its
/// `ReentryCapability`, pure functions only. A source without one sends no
/// receipt (`parse`), whatever else Deck observes of it.
pub(crate) struct ReceiptContract {
    /// The event a line's words name and its native id, each by the
    /// source's own rule; the error is a closed reason.
    pub(crate) decode: fn(&Words) -> Result<(Event, String), &'static str>,
    /// The final native session of an exited client, from the record of its
    /// exit lease.
    pub(crate) final_session: fn(&[Fact]) -> Option<String>,
    /// What a launch lease's record says about entering the wanted session.
    pub(crate) entered: fn(&[Fact], &str) -> Receipt,
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
/// `contract` is the receipt contract of a source word, if that source has
/// one: the framing is checked here, the words and the id by that contract.
pub(crate) fn parse(
    line: &str,
    contract: impl Fn(&str) -> Option<&'static ReceiptContract>,
) -> Result<Wire, &'static str> {
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
    let contract = fields
        .source
        .as_deref()
        .and_then(contract)
        .ok_or("lifecycle-unknown-source")?;
    let (event, native_id) = (contract.decode)(&Words {
        event: fields.event.as_deref(),
        native_id: fields.native_id.as_deref(),
        start_source: fields.start_source.as_deref(),
        end_reason: fields.end_reason.as_deref(),
        final_permission: fields.final_permission.as_deref(),
    })?;
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
    /// Armed before the first exit key: only the frozen old client reports,
    /// and only while it still executes the frozen executable (an exec
    /// inside the same pid and birth instant is another program).
    Exit {
        client: Generation,
        executable: ExecutableRef,
    },
    /// Armed before the launch transport: only a foreground child of the new
    /// shell that executes the frozen executable reports, and only until
    /// `closes` — the end of the restart's post share. From that instant the
    /// lease takes nothing, sealed or not: what closes the phase is the
    /// clock, read under the lock that writes, and never how soon the
    /// coordinator gets to seal.
    Launch {
        executable: ExecutableRef,
        closes: Instant,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct Lease {
    server_pid: u32,
    pane: String,
    shell: Generation,
    phase: Phase,
    /// Assigned when armed, counted from one inside each operation. A
    /// verification that started under one arming can never be written
    /// under a later one, even for the same entry, pane and shell. It says
    /// nothing across operations: that is `Mailbox::operation`.
    serial: u64,
}

impl Lease {
    pub(crate) fn exit(
        server_pid: u32,
        pane: &str,
        shell: Generation,
        client: Generation,
        executable: ExecutableRef,
    ) -> Self {
        Self {
            server_pid,
            pane: pane.to_string(),
            shell,
            phase: Phase::Exit { client, executable },
            serial: 0,
        }
    }

    /// A launch lease that takes receipts until `closes`.
    pub(crate) fn launch(
        server_pid: u32,
        pane: &str,
        shell: Generation,
        executable: ExecutableRef,
        closes: Instant,
    ) -> Self {
        Self {
            server_pid,
            pane: pane.to_string(),
            shell,
            phase: Phase::Launch { executable, closes },
            serial: 0,
        }
    }

    /// The frozen executable this lease's client must be executing.
    fn executable(&self) -> &ExecutableRef {
        match &self.phase {
            Phase::Exit { executable, .. } | Phase::Launch { executable, .. } => executable,
        }
    }

    /// The instant this lease's phase is over, when the phase has one. An
    /// exit lease has none of its own: its client reports only while it is
    /// alive, the entry goes on only once that generation is gone
    /// (`Plan::seal_exits`), and the restart asks its own deadline after the
    /// seal, before anything is replaced.
    fn closes(&self) -> Option<Instant> {
        match &self.phase {
            Phase::Exit { .. } => None,
            Phase::Launch { closes, .. } => Some(*closes),
        }
    }
}

/// The most facts one lease keeps. One more makes the entry's record
/// incomplete, and an incomplete record confirms nothing.
const MAX_FACTS: usize = 64;

struct Mailbox {
    /// Which operation this mailbox is: `OPENED` when it was begun. No other
    /// mailbox of this process ever has the same number.
    operation: u64,
    socket: String,
    helper: PathBuf,
    /// entry index → its one armed lease (re-armed for the launch phase)
    leases: HashMap<usize, Lease>,
    facts: HashMap<usize, Vec<Fact>>,
    next_serial: u64,
}

static MAILBOX: Mutex<Option<Mailbox>> = Mutex::new(None);
static ARRIVED: Condvar = Condvar::new();
/// How many mailboxes this process has begun. In memory only: it names an
/// operation to the claims taken from it and to nothing else, and it is
/// neither a registry nor anything a later process reads.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// Open the mailbox of one operation. Any earlier content is discarded, and
/// every claim taken from an earlier mailbox is void from here on: this one
/// is another operation, whatever its leases turn out to be.
pub(crate) fn begin(socket: &str, helper: &Path) {
    *MAILBOX.lock_or_recover() = Some(Mailbox {
        operation: OPENED.fetch_add(1, Ordering::Relaxed) + 1,
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

/// Seal every entry at one instant: all leases are removed under one lock,
/// so from here on no receipt is written for any of them, and what each
/// gathered is returned by entry. The confirmation works on this set and on
/// nothing that arrives after it — one entry's check, however long it takes,
/// leaves no other entry's lease open behind it.
pub(crate) fn seal_all() -> HashMap<usize, Vec<Fact>> {
    MAILBOX
        .lock_or_recover()
        .as_mut()
        .map(|mailbox| {
            mailbox.leases.clear();
            std::mem::take(&mut mailbox.facts)
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

/// How long one receipt may take from the instant its line was read to the
/// instant its fact is written — the wait for the worker included. It is
/// shorter than the helper's own wait for the close (`SOCKET_BUDGET`, 500 ms,
/// in the lifecycle helper), so a fact is never written for a helper that
/// has stopped waiting; that is checked from the kernel as well, not assumed
/// from this number.
pub(crate) const RECEIPT_BUDGET: Duration = Duration::from_millis(400);

/// The kernel reads one attribution needs. Production is `Kernel::live`;
/// tests supply process tables that describe a topology exactly.
pub(crate) struct Kernel<'a> {
    /// A process-table snapshot, taken when asked.
    pub(crate) snapshot: &'a dyn Fn() -> ProcessTable,
    /// The process executes the bundled lifecycle helper (file identity).
    pub(crate) runs_helper: &'a dyn Fn(u32, &Path) -> bool,
    /// The process generation exists and its image path names this frozen
    /// executable's file state (`procinfo::at_executable`). Kernel reads
    /// only: it cannot stall, and it proves nothing about the image in
    /// memory.
    pub(crate) at_executable: &'a dyn Fn(Generation, &ExecutableRef) -> bool,
    /// The process generation executes exactly this frozen executable
    /// (`procinfo::runs_executable`: one dynamic validation of the running
    /// image, no file content read). This is the ONE call here that cannot
    /// be interrupted.
    pub(crate) runs_executable: &'a dyn Fn(Generation, &ExecutableRef) -> bool,
}

impl Kernel<'static> {
    pub(crate) fn live() -> Self {
        Self {
            snapshot: &crate::procinfo::processes,
            runs_helper: &crate::procinfo::runs_file,
            at_executable: &|client, executable| {
                crate::procinfo::at_executable(client.0, client.1, executable)
            },
            runs_executable: &|client, executable| {
                crate::procinfo::runs_executable(client.0, client.1, executable)
            },
        }
    }
}

/// Who reported, as one snapshot shows it: the helper's own generation and
/// its client's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    reporter: Generation,
    client: Generation,
}

/// Everything that must hold for `peer` to be the lease's client's own
/// report, as far as it can be read without waiting: ONE table, the helper's
/// file identity and the client's image path. Returns who reported.
fn attribute(
    lease: &Lease,
    helper: &Path,
    peer: u32,
    table: &ProcessTable,
    kernel: &Kernel,
) -> Result<Seen, &'static str> {
    // The helper as a generation, not a pid: the number alone could be
    // somebody else by the time it is read again.
    let reporter = generation(table, peer).ok_or("lifecycle-no-peer")?;
    if !(kernel.runs_helper)(peer, helper) {
        return Err("lifecycle-foreign-helper");
    }
    // The helper's direct parent is the client: a hook run by a tool shell,
    // a subagent or a wrapper has another parent and proves nothing here.
    let parent = table.get(&peer).map_or(0, |info| info.ppid);
    let client = generation(table, parent).ok_or("lifecycle-foreign-client")?;
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
    let expected = match &lease.phase {
        Phase::Exit { client: frozen, .. } => client == *frozen,
        Phase::Launch { .. } => table
            .get(&client.0)
            .is_some_and(|info| info.ppid == lease.shell.0),
    };
    if !expected || !(kernel.at_executable)(client, lease.executable()) {
        return Err("lifecycle-foreign-client");
    }
    if !crate::agent_origin::terminal_continuous(table, &chain, client.0, lease.shell.0) {
        return Err("lifecycle-terminal-discontinuity");
    }
    Ok(Seen { reporter, client })
}

/// One claim that names an armed lease of the operation in flight: the
/// lease as it was armed when the line was read, and the operation it was
/// armed in. Still only a claim, and a claim on that operation alone.
pub(crate) struct Claim {
    wire: Wire,
    /// The mailbox this claim was taken from (`Mailbox::operation`).
    operation: u64,
    entry: usize,
    lease: Lease,
    helper: PathBuf,
}

/// The lease `wire` names, if one is armed. One short mailbox lock and
/// nothing else: this is all of a lifecycle line the status listener itself
/// does (`agent_status::handle_stream`).
pub(crate) fn claim(wire: Wire) -> Result<Claim, &'static str> {
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
    Ok(Claim {
        operation: mailbox.operation,
        entry: *entry,
        lease: lease.clone(),
        helper: mailbox.helper.clone(),
        wire,
    })
}

/// Attribute one claim to its lease by kernel facts and record it, or refuse
/// it with a closed reason. `deadline` is the instant the fact must be
/// written by (`RECEIPT_BUDGET` after the line was read); the caller closes
/// the stream — the helper's ACK — only after this returns.
///
/// The one call that can take time is the dynamic validation of the client's
/// running image, and it cannot be interrupted. So the order is:
///
/// 1. who reported, from a snapshot taken now (`attribute`);
/// 2. that validation, ONCE;
/// 3. everything of step 1 AGAIN, from a snapshot taken after the validation
///    returned: the helper is the same generation, still alive and still the
///    bundled helper; its client is the same generation, still at the frozen
///    file, still the pane's foreground, in the same pane, shell and terminal
///    chain. What was true before a call of unknown length is not evidence
///    of what is true before the ACK;
/// 4. the write, under the mailbox lock, only into the very operation the
///    claim was taken from, only for the very arming that was claimed (a
///    lease sealed or armed again takes nothing) and only in time: inside
///    the receipt's budget AND before the lease's phase ended. All three are
///    asked under that lock, with the clock read there.
///
/// An answer that arrives after either limit is refused as late whatever it
/// says — also a positive one, and never as a client that is somebody
/// else's. Nothing is retried, and nothing here waits for anything but that
/// one validation.
pub(crate) fn receive(
    claim: Claim,
    peer: Option<u32>,
    deadline: Instant,
    kernel: &Kernel,
) -> Result<(), &'static str> {
    let Claim {
        wire,
        operation,
        entry,
        lease,
        helper,
    } = claim;
    let peer = peer.ok_or("lifecycle-no-peer")?;
    // Both limits hold for one receipt, and the earlier one decides: its own
    // budget, and the end of the phase its lease was armed for.
    let deadline = lease.closes().map_or(deadline, |phase| phase.min(deadline));
    let late = || Instant::now() >= deadline;
    // waited for the worker too long: nothing is read for it
    if late() {
        return Err("lifecycle-late");
    }
    let seen = attribute(&lease, &helper, peer, &(kernel.snapshot)(), kernel)?;
    let bound = (kernel.runs_executable)(seen.client, lease.executable());
    if late() {
        return Err("lifecycle-late");
    }
    if !bound {
        return Err("lifecycle-foreign-client");
    }
    // Before the ACK: the same attribution on what the kernel says NOW,
    // after the validation. A helper that left, a client that lost the
    // foreground, exited or was replaced meanwhile is late.
    if attribute(&lease, &helper, peer, &(kernel.snapshot)(), kernel) != Ok(seen) {
        return Err("lifecycle-late");
    }
    let client = seen.client;
    let mut mailbox = MAILBOX.lock_or_recover();
    // The operation this claim was taken from, and no later one: the next
    // operation counts its armings from one again and may arm this entry for
    // the same pane, shell and client. Its mailbox is not this claim's.
    let mailbox = mailbox
        .as_mut()
        .filter(|mailbox| mailbox.operation == operation)
        .ok_or("lifecycle-no-lease")?;
    // the very arming that was claimed, not a successor of it, and in time
    if late() || mailbox.leases.get(&entry).map(|now| now.serial) != Some(lease.serial) {
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

/// A record that kept every fact it was sent. An incomplete one names no
/// final session and confirms no entry, whatever its source's reducers
/// would read from the facts that were kept.
pub(crate) fn complete(facts: &[Fact]) -> bool {
    !facts.iter().any(|fact| fact.event == Event::Overflow)
}

/// What a launch lease's record says about entering the wanted session
/// (`ReceiptContract::entered`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Receipt {
    Pending,
    /// Exactly one client reported, and all it reported is entering the
    /// wanted session. Carries that client generation so the coordinator
    /// can check it is still the pane's foreground.
    Entered(Generation),
    /// Another id, another source, an end, a second client or an
    /// incomplete record: not confirmed.
    Conflict,
}

// ---------- Pi: an observed native session, never a receipt ------------------------
//
// Pi has no receipt contract and takes no part in anything above. A verified
// Pi observer stream that carries the lifecycle capability (`agent_pi`)
// says which native session its runtime is in. That is an OBSERVATION with
// the trust ceiling of Signal: the kernel proved which process said it, and
// nothing proves whose code that process runs. So it is kept apart by
// construction: its own type and its own store, no lease of an operation, no
// conversion into a `Fact`, nothing an operation's coordinator or a
// source's reducers ever read. It names no session to resume, confirms no
// entry and makes Pi no candidate for re-entry.
//
// One record per live observer lease, in memory only: the current native id
// (Pi's own shape, `agent_pi::PiNativeId`) and the last thing the runtime
// said of it — a start or a teardown with Pi's own reason, or a snapshot. It is gone with its lease — a reset, the end of the
// stream, another foreground, the capability turned off, a Deck restart —
// and is never rebuilt from disk. A runtime that contradicts itself (a
// second start, another id, or a teardown of a session it never reported)
// loses its record instead of having one of the two readings picked. The
// id is compared, never logged, shown, written or sent anywhere.

use crate::agent_pi::{PiNativeId, SessionEvent};
use crate::agent_status::{ForegroundGeneration, PaneKey};

struct PiObserved {
    key: PaneKey,
    generation: ForegroundGeneration,
    id: PiNativeId,
    /// The last thing this runtime said of its session. A snapshot is what
    /// the API said after a reconnect: neither a start nor a resume. A
    /// teardown is the runtime's own word — `quit` is not proof that the
    /// process is gone.
    last: SessionEvent,
}

impl PiObserved {
    fn torn_down(&self) -> bool {
        matches!(self.last, SessionEvent::Shutdown(_))
    }
}

static PI_SESSIONS: Mutex<Option<HashMap<u64, PiObserved>>> = Mutex::new(None);

fn with_pi<R>(f: impl FnOnce(&mut HashMap<u64, PiObserved>) -> R) -> R {
    f(PI_SESSIONS
        .lock_or_recover()
        .get_or_insert_with(HashMap::new))
}

/// One verified lifecycle frame of the observer lease `lease`, bound to its
/// pane and foreground generation by the transport that calls this. `Err`
/// when the frame contradicts what that lease reported before: the record
/// is then removed, and the transport ends the stream.
pub(crate) fn observe_pi(
    lease: u64,
    key: &PaneKey,
    generation: ForegroundGeneration,
    event: SessionEvent,
    id: &PiNativeId,
) -> Result<(), &'static str> {
    with_pi(|sessions| {
        let known = sessions.get_mut(&lease);
        let consistent = match (event, known) {
            // the first thing a runtime says of its session
            (SessionEvent::Start(_) | SessionEvent::Snapshot, None) => {
                sessions.insert(
                    lease,
                    PiObserved {
                        key: key.clone(),
                        generation,
                        id: id.clone(),
                        last: event,
                    },
                );
                true
            }
            // what the API says again, for the session already known
            (SessionEvent::Snapshot, Some(known)) => known.id == *id && !known.torn_down(),
            (SessionEvent::Shutdown(_), Some(known)) if known.id == *id && !known.torn_down() => {
                known.last = event;
                true
            }
            // a second start in one runtime, a teardown of another session
            // or of none: neither reading is picked
            _ => false,
        };
        if consistent {
            Ok(())
        } else {
            sessions.remove(&lease);
            Err("bad-lifecycle")
        }
    })
}

/// The lease is over, or may no longer observe sessions: its record goes.
pub(crate) fn forget_pi(lease: u64) {
    with_pi(|sessions| sessions.remove(&lease));
}

/// What a card may say about its Pi's native session: one closed word, and
/// only while a live observer of the pane's CURRENT foreground generation
/// reported a session and no teardown. The id never leaves this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PiSession {
    Observed,
}

pub(crate) fn pi_session(key: &PaneKey, generation: ForegroundGeneration) -> Option<PiSession> {
    with_pi(|sessions| {
        sessions
            .values()
            .any(|known| known.key == *key && known.generation == generation && !known.torn_down())
            .then_some(PiSession::Observed)
    })
}

/// What a test may ask about the records: (lease, id, the last event).
#[cfg(test)]
pub(crate) fn pi_sessions_for_tests() -> Vec<(u64, String, SessionEvent)> {
    with_pi(|sessions| {
        let mut all: Vec<_> = sessions
            .iter()
            .map(|(lease, known)| (*lease, known.id.as_str().to_owned(), known.last))
            .collect();
        all.sort_by_key(|record| record.0);
        all
    })
}

#[cfg(test)]
pub(crate) fn reset_pi_for_tests() {
    with_pi(|sessions| sessions.clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    // Claude Code's contract is the one the record is read by here
    use crate::agent_reentry::claude::{
        self, entered as reentry, final_session, EndReason, StartSource,
    };

    /// The contract lookup of a table that registers Claude Code alone.
    fn claude_only(source: &str) -> Option<&'static ReceiptContract> {
        (source == "claude-code").then_some(&claude::RECEIPTS)
    }

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
        let wire = parse(&start("resume"), claude_only).unwrap();
        assert_eq!(wire.event, claude::started(StartSource::Resume));
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
            parse(&end("prompt-input-exit"), claude_only).unwrap().event,
            claude::ended(EndReason::PromptInputExit)
        );
        let with_permission = end("clear").replace(
            r#""endReason":"clear""#,
            r#""endReason":"clear","finalPermission":"unknown""#,
        );
        assert!(parse(&with_permission, claude_only).is_ok());
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
            assert_eq!(parse(&line, claude_only).unwrap_err(), reason, "{line}");
        }
    }

    /// One client switches A → clear → B → resume → C and exits at C: the
    /// final session is C, whatever the arrival order of the starts.
    #[test]
    fn the_final_session_is_the_exit_receipt_not_the_last_start() {
        use EndReason::{Logout, PromptInputExit};
        use StartSource::Startup;
        let facts = vec![
            fact(claude::started(Startup), ID),
            fact(claude::ended(EndReason::Clear), ID),
            fact(claude::started(StartSource::Clear), OTHER),
            fact(claude::ended(EndReason::Resume), OTHER),
            fact(claude::started(StartSource::Resume), THIRD),
            fact(claude::ended(PromptInputExit), THIRD),
        ];
        assert_eq!(final_session(&facts).as_deref(), Some(THIRD));
        // the same final fact twice is one fact
        let mut repeated = facts.clone();
        repeated.push(fact(claude::ended(PromptInputExit), THIRD));
        assert_eq!(final_session(&repeated).as_deref(), Some(THIRD));
        // no applicable end: a clear, resume, logout or other end names nothing
        for reason in [
            EndReason::Clear,
            EndReason::Resume,
            Logout,
            EndReason::Other,
        ] {
            assert_eq!(final_session(&[fact(claude::ended(reason), ID)]), None);
        }
        assert_eq!(final_session(&facts[..5]), None, "a start is not an exit");
        assert_eq!(final_session(&[]), None);
        // two different final ids never resolve to the later one
        let mut conflicting = facts.clone();
        conflicting.push(fact(claude::ended(PromptInputExit), OTHER));
        assert_eq!(final_session(&conflicting), None);
        // something after the final end makes it not final
        let mut trailing = facts;
        trailing.push(fact(claude::started(Startup), OTHER));
        assert_eq!(final_session(&trailing), None);
    }

    #[test]
    fn only_a_resume_of_the_same_id_by_one_client_is_an_entry() {
        use StartSource::*;
        let entered = fact(claude::started(Resume), ID);
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
            fact(claude::started(Resume), OTHER),
            fact(claude::started(Startup), ID),
            fact(claude::started(Clear), ID),
            fact(claude::started(StartSource::Other), ID),
            fact(claude::ended(EndReason::PromptInputExit), ID),
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
        let me = Some(std::process::id());
        let wire = || parse(&start("resume"), claude_only).unwrap();
        assert_eq!(
            received(wire(), me, &Kernel::live()).unwrap_err(),
            "lifecycle-no-lease"
        );
        assert!(!armed());
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        assert_eq!(
            received(wire(), me, &Kernel::live()).unwrap_err(),
            "lifecycle-no-lease"
        );
        // a lease for another server generation that reuses the pane id
        let own = ExecutableRef::of_this_test;
        arm(0, Lease::exit(9999, "%7", (1, (1, 1)), (2, (2, 2)), own()));
        assert!(armed());
        assert_eq!(
            received(wire(), me, &Kernel::live()).unwrap_err(),
            "lifecycle-no-lease"
        );
        // the right pane, another socket
        arm(0, Lease::exit(4242, "%7", (1, (1, 1)), (2, (2, 2)), own()));
        let mut foreign = wire();
        foreign.socket = "deck-dev".into();
        assert_eq!(
            received(foreign, me, &Kernel::live()).unwrap_err(),
            "lifecycle-no-lease"
        );
        // the right claim from a process that is not the bundled helper
        assert_eq!(
            received(wire(), None, &Kernel::live()).unwrap_err(),
            "lifecycle-no-peer"
        );
        assert_eq!(
            received(wire(), me, &Kernel::live()).unwrap_err(),
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

    /// `claim` and `receive` in one step, with a budget no test here reaches
    /// unless it means to.
    fn received(wire: Wire, peer: Option<u32>, kernel: &Kernel) -> Result<(), &'static str> {
        receive(
            claim(wire)?,
            peer,
            Instant::now() + Duration::from_secs(30),
            kernel,
        )
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
        Lease::exit(4242, "%7", SHELL, CLIENT, ExecutableRef::of_this_test())
    }

    fn end_wire(id: &str) -> Wire {
        parse(&end("prompt-input-exit").replace(ID, id), claude_only).unwrap()
    }

    fn resume_wire(id: &str) -> Wire {
        parse(&start("resume").replace(ID, id), claude_only).unwrap()
    }

    /// A kernel whose first snapshot is `table` and whose every later one
    /// is `again()` — what it says after the validation, right before the
    /// ACK. The helper is the bundled one and the client is at the frozen
    /// file and bound to it, unless a test says otherwise.
    struct Scripted<'a> {
        table: &'a ProcessTable,
        again: &'a dyn Fn() -> ProcessTable,
        snapshots: std::cell::Cell<usize>,
    }

    impl<'a> Scripted<'a> {
        fn new(table: &'a ProcessTable, again: &'a dyn Fn() -> ProcessTable) -> Self {
            Self {
                table,
                again,
                snapshots: std::cell::Cell::new(0),
            }
        }

        fn snapshot(&self) -> ProcessTable {
            self.snapshots.set(self.snapshots.get() + 1);
            if self.snapshots.get() == 1 {
                self.table.clone()
            } else {
                (self.again)()
            }
        }
    }

    /// `receive` on `table`, with `again` as the kernel's answer right
    /// before the ACK.
    fn attributed(
        wire: Wire,
        table: &ProcessTable,
        again: &dyn Fn() -> ProcessTable,
    ) -> Result<(), &'static str> {
        let script = Scripted::new(table, again);
        received(
            wire,
            Some(30),
            &Kernel {
                snapshot: &|| script.snapshot(),
                runs_helper: &|_, _| true,
                at_executable: &|_, _| true,
                runs_executable: &|_, _| true,
            },
        )
    }

    fn with_lease(lease: Lease, work: impl FnOnce()) {
        with_mailbox(|| {
            arm(0, lease);
            work();
        });
    }

    /// One operation's mailbox, with nothing armed, for the time `work`
    /// runs. A test whose lease carries an instant of its own arms it in
    /// here: the wait for the tests' lock is then not part of that time.
    fn with_mailbox(work: impl FnOnce()) {
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        end_mailbox_for_test();
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
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
                    event: claude::ended(EndReason::PromptInputExit),
                    native_id: ID.into(),
                    client: CLIENT,
                }]
            );
            // a helper that is not the bundled one is nobody's report
            let again = || topology(CLIENT.0);
            let script = Scripted::new(&table, &again);
            let foreign = received(
                end_wire(ID),
                Some(30),
                &Kernel {
                    snapshot: &|| script.snapshot(),
                    runs_helper: &|_, _| false,
                    at_executable: &|_, _| true,
                    runs_executable: &|_, _| true,
                },
            );
            assert_eq!(foreign.unwrap_err(), "lifecycle-foreign-helper");
            assert_eq!(peek(0).len(), 1);
        });
    }

    /// The leased old client is still that pid and birth instant, but it no
    /// longer executes the frozen executable (an exec in place, or a file
    /// that is not what runs): its exit report names nothing — when it is
    /// not at the frozen file, when its running image does not validate, or
    /// when it has left that file by the time the validation returned. The
    /// validation is asked ONCE per receipt, and not at all for a client
    /// that is positively somewhere else.
    #[test]
    fn an_old_client_that_no_longer_executes_the_frozen_executable_reports_nothing() {
        with_lease(exit_lease(), || {
            let table = topology(CLIENT.0);
            let again = || topology(CLIENT.0);
            let (validated, placed) = (std::cell::Cell::new(0), std::cell::Cell::new(0));
            let report = |at: &dyn Fn(usize) -> bool, runs: bool| {
                validated.set(0);
                placed.set(0);
                let script = Scripted::new(&table, &again);
                received(
                    end_wire(ID),
                    Some(30),
                    &Kernel {
                        snapshot: &|| script.snapshot(),
                        runs_helper: &|_, _| true,
                        at_executable: &|client, _| {
                            assert_eq!(client, CLIENT, "asked of the leased generation");
                            placed.set(placed.get() + 1);
                            at(placed.get())
                        },
                        runs_executable: &|client, _| {
                            assert_eq!(client, CLIENT, "asked of the leased generation");
                            validated.set(validated.get() + 1);
                            runs
                        },
                    },
                )
            };
            // another file at its path: refused before any validation
            assert_eq!(
                report(&|_| false, true).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert_eq!((placed.get(), validated.get()), (1, 0));
            // at the frozen file, and its running image is not that code
            assert_eq!(
                report(&|_| true, false).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert_eq!((placed.get(), validated.get()), (1, 1));
            // bound when the line was read, elsewhere before the ACK
            assert_eq!(report(&|nth| nth == 1, true).unwrap_err(), "lifecycle-late");
            assert_eq!(
                (placed.get(), validated.get()),
                (2, 1),
                "placed at both readings, validated once"
            );
            assert!(peek(0).is_empty());
            // bound, and still there after the validation: recorded
            assert_eq!(report(&|_| true, true), Ok(()));
            assert_eq!((placed.get(), validated.get()), (2, 1));
            assert_eq!(final_session(&peek(0)).as_deref(), Some(ID));
        });
    }

    /// What `receive` is handed while its one validation runs: a kernel
    /// whose answers change DURING that call, and a clock that may run out
    /// in it. Each case is positive when the line is read and positive in
    /// the validation's own answer; none may leave a fact behind.
    struct During<'a> {
        /// What the kernel says after the validation returned.
        after: &'a dyn Fn() -> ProcessTable,
        /// Run inside the validation, before it answers.
        meanwhile: &'a dyn Fn(),
        /// The helper is still the bundled one after the validation.
        helper_after: bool,
    }

    fn during(case: During, deadline: Instant) -> (Result<(), &'static str>, usize) {
        let table = topology(CLIENT.0);
        let validations = std::cell::Cell::new(0);
        let returned = std::cell::Cell::new(false);
        let result = claim(end_wire(ID)).and_then(|claim| {
            receive(
                claim,
                Some(30),
                deadline,
                &Kernel {
                    snapshot: &|| {
                        if returned.get() {
                            (case.after)()
                        } else {
                            table.clone()
                        }
                    },
                    runs_helper: &|_, _| !returned.get() || case.helper_after,
                    at_executable: &|_, _| true,
                    runs_executable: &|_, _| {
                        validations.set(validations.get() + 1);
                        (case.meanwhile)();
                        returned.set(true);
                        true
                    },
                },
            )
        });
        (result, validations.get())
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    /// The helper stops waiting — exits, or its pid is another process by
    /// then — while the client's code is being validated. The validation
    /// answers yes; the fact is not written, because nobody is waiting for
    /// the ACK it would be.
    #[test]
    fn a_helper_that_leaves_during_the_validation_reports_nothing() {
        with_lease(exit_lease(), || {
            let nothing = || {};
            // exited: the pid is in no table any more
            let exited = || {
                let mut now = topology(CLIENT.0);
                now.remove(&30);
                now
            };
            // exited and its pid given to another child of the same client
            let reused = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&30).unwrap().start_seconds += 1;
                now
            };
            // exited and reparented, the client still in the foreground
            let orphaned = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&30).unwrap().ppid = 1;
                now
            };
            for (what, after) in [
                ("exited", &exited as &dyn Fn() -> ProcessTable),
                ("reused", &reused),
                ("orphaned", &orphaned),
            ] {
                let (result, validations) = during(
                    During {
                        after,
                        meanwhile: &nothing,
                        helper_after: true,
                    },
                    far(),
                );
                assert_eq!(result.unwrap_err(), "lifecycle-late", "{what}");
                assert_eq!(validations, 1, "{what}: validated once, never again");
            }
            // the pid now runs something that is not the bundled helper
            let same = || topology(CLIENT.0);
            let (result, _) = during(
                During {
                    after: &same,
                    meanwhile: &nothing,
                    helper_after: false,
                },
                far(),
            );
            assert_eq!(result.unwrap_err(), "lifecycle-late");
            assert!(peek(0).is_empty(), "no late fact");
            // the same receipt with nothing changing is a fact
            let (result, validations) = during(
                During {
                    after: &same,
                    meanwhile: &nothing,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!((result, validations), (Ok(()), 1));
            assert_eq!(final_session(&peek(0)).as_deref(), Some(ID));
        });
    }

    /// The client loses the foreground, exits, or is another generation
    /// under its pid while its code is being validated: the answer was
    /// about a moment that is over.
    #[test]
    fn a_foreground_or_generation_that_changes_during_the_validation_reports_nothing() {
        with_lease(exit_lease(), || {
            let nothing = || {};
            let shell_took_over = || topology(SHELL.0);
            let client_gone = || {
                let mut now = topology(SHELL.0);
                now.remove(&CLIENT.0);
                now
            };
            let client_replaced = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&CLIENT.0).unwrap().start_seconds += 1;
                now
            };
            let shell_replaced = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&SHELL.0).unwrap().start_seconds += 1;
                now
            };
            // another program between the helper and the terminal
            let chain_broken = || {
                let mut now = topology(CLIENT.0);
                now.get_mut(&30).unwrap().tty = 9;
                now
            };
            for (what, after) in [
                (
                    "shell foreground",
                    &shell_took_over as &dyn Fn() -> ProcessTable,
                ),
                ("client gone", &client_gone),
                ("client replaced", &client_replaced),
                ("shell replaced", &shell_replaced),
                ("chain broken", &chain_broken),
            ] {
                let (result, validations) = during(
                    During {
                        after,
                        meanwhile: &nothing,
                        helper_after: true,
                    },
                    far(),
                );
                assert_eq!(result.unwrap_err(), "lifecycle-late", "{what}");
                assert_eq!(validations, 1, "{what}");
            }
            assert!(peek(0).is_empty(), "no late fact");
        });
    }

    /// The lease is sealed, or sealed and armed again, while the client's
    /// code is being validated: the answer belongs to an arming that is
    /// over. Nothing is written — not into the sealed entry, and not into
    /// the successor, which is the same entry, pane, shell and client.
    #[test]
    fn a_lease_sealed_or_armed_again_during_the_validation_takes_nothing() {
        with_lease(exit_lease(), || {
            let same = || topology(CLIENT.0);
            let rearm = || {
                assert!(seal(0).is_empty());
                arm(0, exit_lease());
            };
            let (result, validations) = during(
                During {
                    after: &same,
                    meanwhile: &rearm,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!((result, validations), (Err("lifecycle-late"), 1));
            assert!(armed(), "the successor is armed");
            assert!(peek(0).is_empty(), "and took nothing");
            let sealed = || {
                assert!(seal(0).is_empty());
            };
            let (result, _) = during(
                During {
                    after: &same,
                    meanwhile: &sealed,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!(result.unwrap_err(), "lifecycle-late");
            assert!(!armed());
            assert!(seal(0).is_empty(), "nothing to seal a second time");
            // the whole operation ended meanwhile: no mailbox to write to
            arm(0, exit_lease());
            let ended = || super::end();
            let (result, _) = during(
                During {
                    after: &same,
                    meanwhile: &ended,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!(result.unwrap_err(), "lifecycle-no-lease");
            begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        });
    }

    /// The validation answers yes — after the receipt's budget. A late yes
    /// is refused like every other late answer, and a late no is called
    /// late, not "somebody else's client". A receipt whose budget is already
    /// over when it is looked at costs no kernel read at all.
    #[test]
    fn a_validation_that_answers_after_the_budget_is_refused_whatever_it_says() {
        with_lease(exit_lease(), || {
            let same = || topology(CLIENT.0);
            let slow = || std::thread::sleep(Duration::from_millis(60));
            let (result, validations) = during(
                During {
                    after: &same,
                    meanwhile: &slow,
                    helper_after: true,
                },
                Instant::now() + Duration::from_millis(20),
            );
            assert_eq!((result, validations), (Err("lifecycle-late"), 1));
            assert!(peek(0).is_empty(), "a late yes writes nothing");
            // a late no
            let table = topology(CLIENT.0);
            let script = Scripted::new(&table, &same);
            let late_no = receive(
                claim(end_wire(ID)).unwrap(),
                Some(30),
                Instant::now() + Duration::from_millis(20),
                &Kernel {
                    snapshot: &|| script.snapshot(),
                    runs_helper: &|_, _| true,
                    at_executable: &|_, _| true,
                    runs_executable: &|_, _| {
                        slow();
                        false
                    },
                },
            );
            assert_eq!(late_no.unwrap_err(), "lifecycle-late");
            // already over when the worker reaches it: nothing is asked
            let asked = std::cell::Cell::new(0);
            let count = || asked.set(asked.get() + 1);
            let stale = receive(
                claim(end_wire(ID)).unwrap(),
                Some(30),
                Instant::now(),
                &Kernel {
                    snapshot: &|| {
                        count();
                        topology(CLIENT.0)
                    },
                    runs_helper: &|_, _| {
                        count();
                        true
                    },
                    at_executable: &|_, _| {
                        count();
                        true
                    },
                    runs_executable: &|_, _| {
                        count();
                        true
                    },
                },
            );
            assert_eq!(stale.unwrap_err(), "lifecycle-late");
            assert_eq!(asked.get(), 0, "an expired receipt reads nothing");
            assert!(peek(0).is_empty());
            // in time, the same receipt is a fact
            let (result, _) = during(
                During {
                    after: &same,
                    meanwhile: &|| {},
                    helper_after: true,
                },
                far(),
            );
            assert_eq!(result, Ok(()));
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
        let executable = ExecutableRef::of_this_test();
        with_lease(Lease::launch(4242, "%7", SHELL, executable, far()), || {
            let table = topology(CLIENT.0);
            let again = || topology(CLIENT.0);
            let report = |table: &ProcessTable, at: bool, runs: bool| {
                let script = Scripted::new(table, &again);
                received(
                    resume_wire(ID),
                    Some(30),
                    &Kernel {
                        snapshot: &|| script.snapshot(),
                        runs_helper: &|_, _| true,
                        at_executable: if at { &|_, _| true } else { &|_, _| false },
                        runs_executable: if runs { &|_, _| true } else { &|_, _| false },
                    },
                )
            };
            // a client started from another file, or running another image
            assert_eq!(
                report(&table, false, true).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert_eq!(
                report(&table, true, false).unwrap_err(),
                "lifecycle-foreign-client"
            );
            assert!(peek(0).is_empty());
            assert_eq!(report(&table, true, true), Ok(()));
            assert_eq!(reentry(&peek(0), ID), Receipt::Entered(CLIENT));
            // a client that is the shell's grandchild is not a direct launch
            let mut wrapped = topology(CLIENT.0);
            wrapped.insert(15, process(15, SHELL.0, 15, TTY, CLIENT.0));
            wrapped.get_mut(&CLIENT.0).unwrap().ppid = 15;
            assert_eq!(
                report(&wrapped, true, true).unwrap_err(),
                "lifecycle-foreign-client"
            );
        });
    }

    /// Sixty-four valid facts fit. The sixty-fifth — here a conflicting one —
    /// is not silently dropped behind a success: the record is marked
    /// incomplete, the report is refused, and nothing is confirmed.
    #[test]
    fn a_record_that_overflows_confirms_nothing() {
        let executable = ExecutableRef::of_this_test();
        with_lease(Lease::launch(4242, "%7", SHELL, executable, far()), || {
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

    // ---------- a claim is its own operation's, and its phase's ---------------

    /// The next operation, begun exactly as the last one was: the same
    /// socket and helper, and the same entry armed with `lease` — which is
    /// its first arming, so its serial is the one the last operation's first
    /// arming had.
    fn next_operation(lease: Lease) {
        super::end();
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        arm(0, lease);
    }

    /// A kernel on the certified topology in which nothing changes and
    /// every answer is yes; `validated` counts the validations.
    fn settled(claim: Claim, validated: &std::cell::Cell<usize>) -> Result<(), &'static str> {
        receive(
            claim,
            Some(30),
            far(),
            &Kernel {
                snapshot: &|| topology(CLIENT.0),
                runs_helper: &|_, _| true,
                at_executable: &|_, _| true,
                runs_executable: &|_, _| {
                    validated.set(validated.get() + 1);
                    true
                },
            },
        )
    }

    /// A claim waits for the worker while its operation ends and the next
    /// one begins and arms the same entry for the same pane, shell and
    /// client — with the same serial, because every operation counts from
    /// one. Everything the kernel says about the old claim is still true and
    /// its budget is far from over. It is refused all the same, under the
    /// lock that writes: the next operation's mailbox takes nothing from it,
    /// and takes its own claim as before.
    #[test]
    fn a_claim_that_waited_across_operations_is_refused_by_the_next_ones_mailbox() {
        with_lease(exit_lease(), || {
            let validated = std::cell::Cell::new(0);
            let waiting = claim(end_wire(ID)).expect("the armed lease");
            let (operation, serial) = (waiting.operation, waiting.lease.serial);
            next_operation(exit_lease());
            let own = claim(end_wire(OTHER)).expect("the next operation's lease");
            assert_eq!(
                (own.entry, own.lease.serial),
                (waiting.entry, serial),
                "the same entry and the same local serial"
            );
            assert_ne!(own.operation, operation, "another operation");
            assert_eq!(settled(waiting, &validated), Err("lifecycle-no-lease"));
            assert_eq!(validated.get(), 1, "verified in full, and still refused");
            assert!(armed(), "the next operation's lease is untouched");
            assert!(peek(0).is_empty(), "and its mailbox took nothing");
            // the next operation's own claim is a fact, as ever
            assert_eq!(settled(own, &validated), Ok(()));
            assert_eq!(final_session(&peek(0)).as_deref(), Some(OTHER));
            assert_eq!(peek(0).len(), 1);
            // and a claim of an operation that is simply over has no mailbox
            let last = claim(end_wire(ID)).expect("armed");
            super::end();
            assert_eq!(settled(last, &validated), Err("lifecycle-no-lease"));
        });
    }

    /// The same, with the old claim INSIDE its one validation while the
    /// operations change: the validation answers yes, the kernel still says
    /// what it said, and the write finds another operation's mailbox.
    #[test]
    fn a_claim_validated_across_operations_is_refused_by_the_next_ones_mailbox() {
        with_lease(exit_lease(), || {
            let same = || topology(CLIENT.0);
            let replaced = || next_operation(exit_lease());
            let (result, validations) = during(
                During {
                    after: &same,
                    meanwhile: &replaced,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!((result, validations), (Err("lifecycle-no-lease"), 1));
            assert!(armed(), "the next operation is armed");
            assert!(peek(0).is_empty(), "and took nothing");
            // armed again INSIDE the next operation: the old claim's serial
            // is behind, the operation is still not its own
            let rearmed = || {
                next_operation(exit_lease());
                assert!(seal(0).is_empty());
                arm(0, exit_lease());
            };
            let (result, _) = during(
                During {
                    after: &same,
                    meanwhile: &rearmed,
                    helper_after: true,
                },
                far(),
            );
            assert_eq!(result.unwrap_err(), "lifecycle-no-lease");
            assert!(peek(0).is_empty());
            // the operation in flight takes its own claim
            let (result, validations) = during(
                During {
                    after: &same,
                    meanwhile: &|| {},
                    helper_after: true,
                },
                far(),
            );
            assert_eq!((result, validations), (Ok(()), 1));
            assert_eq!(final_session(&peek(0)).as_deref(), Some(ID));
        });
    }

    fn launch_lease(closes: Instant) -> Lease {
        Lease::launch(4242, "%7", SHELL, ExecutableRef::of_this_test(), closes)
    }

    /// A launch lease stops taking facts when its phase ends, by the clock
    /// and whoever has or has not sealed it. The line was read in time and
    /// the receipt's own budget is far from over; the validation answers
    /// yes after the phase ended: nothing is written. And in the window
    /// between the end of the phase and the seal — the lease still armed —
    /// a receipt with all of its own budget left is refused without a
    /// single kernel read.
    #[test]
    fn a_launch_lease_takes_nothing_after_its_phase_whatever_is_left_of_the_receipts_budget() {
        with_mailbox(|| {
            // read before the phase's end is fixed: the first reference a
            // test process makes is a full read of its own executable
            let executable = ExecutableRef::of_this_test();
            let closes = Instant::now() + Duration::from_millis(300);
            arm(0, Lease::launch(4242, "%7", SHELL, executable, closes));
            let validated = std::cell::Cell::new(0);
            let claimed = claim(resume_wire(ID)).expect("armed, inside its phase");
            let result = receive(
                claimed,
                Some(30),
                far(),
                &Kernel {
                    snapshot: &|| topology(CLIENT.0),
                    runs_helper: &|_, _| true,
                    at_executable: &|_, _| true,
                    runs_executable: &|_, _| {
                        validated.set(validated.get() + 1);
                        assert!(Instant::now() < closes, "asked inside the phase");
                        // answers yes, after the phase ended
                        while Instant::now() < closes {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        true
                    },
                },
            );
            assert_eq!(result.unwrap_err(), "lifecycle-late");
            assert_eq!(validated.get(), 1);
            assert!(armed(), "nobody sealed it: the lease is still there");
            assert!(peek(0).is_empty(), "and it took nothing");
            // the window before the seal: the phase is over, the lease armed
            let asked = std::cell::Cell::new(0);
            let count = || asked.set(asked.get() + 1);
            let after = receive(
                claim(resume_wire(ID)).expect("still armed"),
                Some(30),
                far(),
                &Kernel {
                    snapshot: &|| {
                        count();
                        topology(CLIENT.0)
                    },
                    runs_helper: &|_, _| {
                        count();
                        true
                    },
                    at_executable: &|_, _| {
                        count();
                        true
                    },
                    runs_executable: &|_, _| {
                        count();
                        true
                    },
                },
            );
            assert_eq!(after.unwrap_err(), "lifecycle-late");
            assert_eq!(asked.get(), 0, "a closed phase reads nothing");
            assert!(seal_all().is_empty(), "what is sealed then holds nothing");
        });
        // inside its phase, the same receipt is a fact
        with_lease(launch_lease(far()), || {
            let validated = std::cell::Cell::new(0);
            let claimed = claim(resume_wire(ID)).expect("armed");
            assert_eq!(settled(claimed, &validated), Ok(()));
            assert_eq!(reentry(&peek(0), ID), Receipt::Entered(CLIENT));
        });
        // an exit lease has no phase of its own to end
        assert_eq!(exit_lease().closes(), None);
        let closes = far();
        assert_eq!(launch_lease(closes).closes(), Some(closes));
    }

    /// Every entry is sealed at one instant. What each gathered until then
    /// is handed out by entry; a claim that was already taken — its receipt
    /// waiting, or inside its validation — writes nothing afterwards, a new
    /// line names no lease, and there is nothing to seal a second time.
    #[test]
    fn sealing_every_entry_closes_all_of_them_at_once() {
        let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
        end_mailbox_for_test();
        begin("deck", Path::new("/nonexistent/deck-lifecycle-helper"));
        arm(0, launch_lease(far()));
        // a second entry: another pane of the same server
        arm(
            1,
            Lease::launch(4242, "%8", SHELL, ExecutableRef::of_this_test(), far()),
        );
        let other_pane = |id: &str| {
            let mut wire = resume_wire(id);
            wire.pane = "%8".into();
            wire
        };
        let validated = std::cell::Cell::new(0);
        assert_eq!(settled(claim(resume_wire(ID)).unwrap(), &validated), Ok(()));
        // taken before the seal, settled after it
        let waiting = claim(other_pane(OTHER)).expect("the second entry's lease");
        assert_eq!(waiting.entry, 1);
        // inside its validation when the seal comes
        let table = topology(CLIENT.0);
        let sealed = std::cell::RefCell::new(None);
        let during_validation = receive(
            claim(other_pane(THIRD)).expect("the second entry's lease"),
            Some(30),
            far(),
            &Kernel {
                snapshot: &|| table.clone(),
                runs_helper: &|_, _| true,
                at_executable: &|_, _| true,
                runs_executable: &|_, _| {
                    *sealed.borrow_mut() = Some(seal_all());
                    true
                },
            },
        );
        assert_eq!(during_validation.unwrap_err(), "lifecycle-late");
        let sealed = sealed.into_inner().expect("sealed inside the validation");
        assert_eq!(sealed.len(), 1, "one entry had gathered anything");
        assert_eq!(reentry(&sealed[&0], ID), Receipt::Entered(CLIENT));
        assert!(!armed(), "no lease is left");
        assert_eq!(settled(waiting, &validated), Err("lifecycle-late"));
        assert_eq!(
            claim(resume_wire(ID)).err(),
            Some("lifecycle-no-lease"),
            "a line that arrives after the seal names nothing"
        );
        assert!(seal_all().is_empty(), "nothing was written after the seal");
        assert!(peek(0).is_empty() && peek(1).is_empty());
        end_mailbox_for_test();
    }

    // ---------- Pi: an observed session is never a receipt ----------

    mod pi {
        use super::super::*;
        use crate::agent_pi::{ShutdownReason, StartReason};

        fn pane(id: &str) -> PaneKey {
            PaneKey::of(&crate::tmux::PaneRow {
                server_pid: 42,
                pane_id: id.into(),
                ..crate::tmux::PaneRow::default()
            })
        }

        fn generation(pid: u32) -> ForegroundGeneration {
            ForegroundGeneration::of(pid, (2000, 0))
        }

        fn id(text: &str) -> PiNativeId {
            match crate::agent_pi::parse_frame(&format!(
                r#"{{"protocol":"pi-observer","v":1,"source":"pi","seq":1,"event":"snapshot","nativeSessionId":"{text}"}}"#
            )) {
                Ok(crate::agent_pi::Frame::Lifecycle {
                    native: crate::agent_pi::NativeId::Kept(id),
                    ..
                }) => id,
                other => panic!("{other:?}"),
            }
        }

        /// What a lease's runtime says is kept per lease, read back only as
        /// one closed word for the pane's CURRENT generation, and forgotten
        /// with the lease. An id Claude's rule would refuse is kept: it is
        /// Pi's, judged by Pi's rule, and it never meets Claude's.
        #[test]
        fn one_record_per_lease_read_as_one_word_and_gone_with_the_lease() {
            let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
            reset_pi_for_tests();
            let (a, b) = (pane("%1"), pane("%2"));
            let native = id("fixture.custom-ID_123");
            assert!(!crate::agent_reentry::claude::native_id_ok(native.as_str()));
            assert_eq!(pi_session(&a, generation(310)), None);
            assert_eq!(
                observe_pi(
                    7,
                    &a,
                    generation(310),
                    SessionEvent::Start(StartReason::Startup),
                    &native
                ),
                Ok(())
            );
            assert_eq!(pi_session(&a, generation(310)), Some(PiSession::Observed));
            // another generation of that pane, and another pane, read nothing
            assert_eq!(pi_session(&a, generation(311)), None);
            assert_eq!(pi_session(&b, generation(310)), None);
            // the same session in a second pane is a second, independent record
            assert_eq!(
                observe_pi(8, &b, generation(410), SessionEvent::Snapshot, &native),
                Ok(())
            );
            assert_eq!(
                pi_sessions_for_tests(),
                [
                    (
                        7,
                        "fixture.custom-ID_123".to_string(),
                        SessionEvent::Start(StartReason::Startup)
                    ),
                    (
                        8,
                        "fixture.custom-ID_123".to_string(),
                        SessionEvent::Snapshot
                    ),
                ]
            );
            // a snapshot of the session already known changes nothing
            assert_eq!(
                observe_pi(7, &a, generation(310), SessionEvent::Snapshot, &native),
                Ok(())
            );
            // the teardown is the runtime's word: no session is observed after it
            assert_eq!(
                observe_pi(
                    7,
                    &a,
                    generation(310),
                    SessionEvent::Shutdown(ShutdownReason::Quit),
                    &native
                ),
                Ok(())
            );
            assert_eq!(pi_session(&a, generation(310)), None);
            assert_eq!(
                pi_sessions_for_tests()[0].2,
                SessionEvent::Shutdown(ShutdownReason::Quit)
            );
            assert_eq!(
                pi_session(&b, generation(410)),
                Some(PiSession::Observed),
                "B is not A"
            );
            forget_pi(7);
            forget_pi(8);
            forget_pi(8);
            assert!(pi_sessions_for_tests().is_empty());
            assert_eq!(
                serde_json::to_value(PiSession::Observed).unwrap(),
                "observed"
            );
        }

        /// A runtime that contradicts itself loses its record: neither of two
        /// readings is picked.
        #[test]
        fn a_runtime_that_contradicts_itself_loses_its_record() {
            let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
            reset_pi_for_tests();
            let a = pane("%1");
            let (first, other) = (id("session-one"), id("session-two"));
            let start = SessionEvent::Start(StartReason::Startup);
            let quit = SessionEvent::Shutdown(ShutdownReason::Quit);
            let cases: [(&str, Vec<(SessionEvent, &PiNativeId)>); 6] = [
                (
                    "a second start without a teardown",
                    vec![
                        (start, &first),
                        (SessionEvent::Start(StartReason::Resume), &other),
                    ],
                ),
                (
                    "the same start twice",
                    vec![(start, &first), (start, &first)],
                ),
                (
                    "a snapshot of another session",
                    vec![(start, &first), (SessionEvent::Snapshot, &other)],
                ),
                (
                    "a teardown of another session",
                    vec![(start, &first), (quit, &other)],
                ),
                ("a teardown of none", vec![(quit, &first)]),
                (
                    "a second teardown",
                    vec![(start, &first), (quit, &first), (quit, &first)],
                ),
            ];
            for (what, frames) in cases {
                reset_pi_for_tests();
                let (last, before) = frames.split_last().unwrap();
                for (event, native) in before {
                    assert_eq!(
                        observe_pi(7, &a, generation(310), *event, native),
                        Ok(()),
                        "{what}"
                    );
                }
                assert_eq!(
                    observe_pi(7, &a, generation(310), last.0, last.1),
                    Err("bad-lifecycle"),
                    "{what}"
                );
                assert!(
                    pi_sessions_for_tests().is_empty(),
                    "{what}: the record is gone"
                );
                assert_eq!(pi_session(&a, generation(310)), None, "{what}");
            }
            // a start after a contradiction is a first start again
            assert_eq!(observe_pi(7, &a, generation(310), start, &other), Ok(()));
            reset_pi_for_tests();
        }

        /// An observation is no receipt. With an operation in flight and a
        /// lease armed for the very same pane, what Pi's observer reports
        /// reaches no mailbox, and a receipt line that names source `pi` is
        /// refused: Pi has no contract. The reverse holds too: a receipt
        /// writes no observation.
        #[test]
        fn an_observation_reaches_no_mailbox_and_pi_has_no_receipt_contract() {
            let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
            reset_pi_for_tests();
            begin("deck", Path::new("/nonexistent/helper"));
            let executable = ExecutableRef::of_this_test();
            arm(
                0,
                Lease::exit(42, "%1", (300, (1000, 0)), (310, (2000, 0)), executable),
            );
            assert!(armed());
            let native = id("731c5908-251a-4388-b3a3-4719ad442bce");
            for event in [
                SessionEvent::Start(StartReason::Resume),
                SessionEvent::Shutdown(ShutdownReason::Quit),
            ] {
                assert_eq!(
                    observe_pi(7, &pane("%1"), generation(310), event, &native),
                    Ok(())
                );
            }
            assert!(
                peek(0).is_empty(),
                "the armed lease of that very pane holds nothing"
            );
            assert!(seal_all().values().all(Vec::is_empty));
            // the production lookup: only a source whose adapter has a re-entry
            // capability has a contract, and Pi's adapter has none
            let contract = |source: &str| {
                crate::extensions::agent(source)
                    .and_then(|adapter| adapter.reentry)
                    .map(|capability| capability.receipts)
            };
            assert!(contract("claude-code").is_some());
            assert!(contract("pi").is_none() && contract("codex").is_none());
            let line = r#"{"protocol":"agent-lifecycle","v":1,"source":"pi","event":"session-start","nativeSessionId":"731c5908-251a-4388-b3a3-4719ad442bce","startSource":"resume","socket":"deck","server_pid":42,"pane":"%1"}"#;
            assert_eq!(
                parse(line, contract).unwrap_err(),
                "lifecycle-unknown-source"
            );
            assert!(parse(&line.replace("\"pi\"", "\"claude-code\""), contract).is_ok());
            end();
            // the store of observations is not the mailbox's: an operation's end leaves it
            assert_eq!(pi_sessions_for_tests().len(), 1);
            reset_pi_for_tests();
            // and the two are separate in the source: the Pi section names
            // neither the mailbox nor a fact, and nothing above it names Pi's store
            let source = include_str!("lifecycle.rs");
            let production = &source[..source.find("\n#[cfg(test)]\nmod tests {").unwrap()];
            let (receipts, observation) = production
                .split_once("// ---------- Pi: an observed native session, never a receipt")
                .expect("the Pi section");
            let code = |text: &str| -> String {
                text.lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            for word in [
                "MAILBOX", "Fact", "Claim", "Lease", "Wire", "Receipt", "ARRIVED", "facts",
            ] {
                assert!(
                    !code(observation).contains(word),
                    "the Pi section uses {word}"
                );
            }
            for word in [
                "PI_SESSIONS",
                "PiObserved",
                "observe_pi",
                "forget_pi",
                "PiNativeId",
            ] {
                assert!(
                    !code(receipts).contains(word),
                    "the receipt half uses {word}"
                );
            }
        }
    }
}
