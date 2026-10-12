//! agent_pi/transport.rs — the bounded service that answers Pi observers.
//!
//! # Contract
//! - **One message, one connection.** An observer never keeps a connection.
//!   For every message it makes a new one, writes ONE line, reads Deck's one
//!   line of answer and is done; Deck answers and closes. The status
//!   listener reads that first line (`agent_status`, the listener section:
//!   an absolute deadline, a bounded number at a time) and hands line and
//!   connection over without waiting (`Service::hand`, which takes the
//!   kernel's peer pid of the connection there and then). Bytes behind the
//!   first line are no part of this family: the listener refuses the
//!   message (`trailing-bytes`) and nothing reaches this module.
//! - **Off the accept loop, and bounded.** ONE admission worker verifies,
//!   commits and answers the messages, one after the other; nothing here
//!   runs on the listener's threads, and no thread, task or timer exists per
//!   connection or per observer. At most `MAX_PENDING` messages wait for the
//!   worker (one more is refused `busy` at once); at most `MAX_LEASES`
//!   observers are bound and `MAX_CHALLENGES` nonces are open; a line is
//!   at most `agent_pi::MAX_LINE` bytes; an answer is one short line written
//!   within `WRITE_BUDGET`. The worker's one tmux call per batch has
//!   `LISTING_BUDGET`. The same worker, once a second, ends what went stale.
//! - **An answer follows the commit, in time or not at all.** A frame is
//!   answered `ack` only after it was verified and written. A message that
//!   is not ready to be written `ANSWER_BUDGET` after it was handed over —
//!   the wait for the worker and for tmux included — is `late`: the
//!   observer has stopped waiting (its own two seconds began earlier), so
//!   nothing is written for it and its lease is over here as well.
//! - **Who may bind.** The process that attests must be the leader of the
//!   foreground process group of the pane's terminal, a descendant of the
//!   pane's process with every process between them on that terminal, in a
//!   pane of Deck's own server that the line names and whose foreground is
//!   no shell. All of that is read from the kernel and from a pane listing
//!   taken after the line was read; the socket name, the server pid, the
//!   pane id, the versions and the mode in the line are claims: none of them
//!   is used as proof. Only after that is the generation RECOGNIZED as Pi
//!   (`agent_status::pi_observed`), whatever becomes of the attest: a release
//!   this build does not support and an observer none of whose capabilities
//!   the settings enable are refused after it, so the card can say which.
//! - **Whose bytes.** That a process is the pane's foreground program does
//!   not say that IT wrote what a connection carried: a child starts with a
//!   copy of its parent's descriptors and of its memory, and the kernel
//!   does not say which holder of a connection wrote a byte. Its peer pid
//!   (`LOCAL_PEERPID`) is the LAST process that connected, sent, received,
//!   polled or registered the peer socket — a parent that merely looks at a
//!   connection after its child wrote on it is named, and is named before
//!   its own bytes arrive (xnu `uipc_socket.c`; measured). So no single
//!   fact is taken for the author. A line is the bound process's own only
//!   when three things hold together. (a) The peer pid of its connection is
//!   that process. (b) It carries the nonce of Deck's last answer to that
//!   process: a nonce is made after the worker looked at the process, is
//!   given in that one answer, and is good for one message. (c) The kernel
//!   reported no fork of that process between that look and this one
//!   (`procinfo::InstanceWatch::forks`: the kernel posts the report before
//!   a child can run).
//!
//!   Why that is enough: whoever else wrote the line knew the nonce. A
//!   child made after the nonce reached its parent's memory is a fork after
//!   that look: (c) fails. A child made earlier can only have READ the
//!   nonce, from the connection that carried the answer — a connection it
//!   then held. An observer has ONE connection at a time (`observer.ts`,
//!   `line`; closing one is immediate on both runtimes Pi ships on) and one
//!   carries one message, so that child holds no connection of its parent's
//!   on which a line can still arrive, and a connection of its own names
//!   the child: (a) fails. Nothing in this depends on when a line is read,
//!   queued or committed.
//!
//!   When (b) or (c) does not hold for a peer that IS the bound process,
//!   the message is neither believed nor blamed: `again`, with a new nonce;
//!   nothing is written or ended, and the observer sends the same message
//!   on a new connection — Pi makes a child for every tool it runs, and
//!   that must cost a retry and nothing else. When (a) does not hold
//!   nothing of the lease is touched, not even its nonce
//!   (`sender-mismatch`).
//!
//!   What this does not cover is said plainly. A process that itself hands
//!   its connection, its nonce or its bytes to another, or whose memory
//!   another can read, is the author of what follows, exactly as an
//!   extension inside Pi that misreports is. And two observers inside ONE
//!   process are two connections at a time: that takes two of Deck's entry
//!   files in one Pi's settings, which Deck never writes and its Settings
//!   call a conflict; the second is refused (`duplicate-observer`) and
//!   stops asking by itself.
//! - **A hello is not believed; an attest is.** What reached the socket
//!   before Deck looked at a process says nothing about the program that
//!   runs in it now: an earlier image may have written it and executed
//!   another since. So a `hello` only asks: the worker starts watching the
//!   peer's execution instance and its forks, and answers with a nonce. The
//!   observer's claims come in an `attest` that carries that nonce, on a
//!   new connection: its bytes were written after the watch began, by the
//!   three facts above they are that process's own, and the watch says that
//!   this process executed nothing in between. One nonce is open per pid,
//!   good for one attest and for `CHALLENGE_BUDGET`. A nonce shows WHEN a
//!   line was written and never, by itself, by whom: a child that read it
//!   from the connection of the hello attests from a connection that names
//!   the child, and is refused like any stranger (`no-challenge`).
//! - **One execution instance.** A pid and a birth instant survive an
//!   `exec`. The watch that begins with the hello (`procinfo::InstanceWatch`:
//!   the kernel's own exec and exit reports) is asked at the bind, for every
//!   frame after the other kernel reads, by the poll's `reconcile`, by the
//!   worker's sweep and by every reader of what the lease proved
//!   (`agent_status`). A process that executed another image is
//!   `peer-executed`: its lease ends, what it reported is withdrawn, and
//!   what its attest proved is forgotten — the new program is nothing until
//!   it attests itself. A watch the kernel refused or cannot answer is
//!   `no-instance`: nothing is bound, or the lease ends; it is never read as
//!   "unchanged". A fork is not an exec (the lease stands; the child is
//!   another process), and a file replaced on disk changes nothing that runs.
//! - **A lease.** An accepted attest gets a number this process never uses
//!   again. It is bound to the pane (server pid, pane id, session id, pane
//!   process and its birth), to the peer's generation and to its execution
//!   instance. Every frame names its lease, and every commit is made under
//!   the registry lock that ends leases: a frame of an ended lease writes
//!   nothing, however long it waited, and is told why the lease ended. One
//!   foreground generation has at most one live lease: a second attest
//!   while one is live is `duplicate-observer`.
//! - **Capabilities narrow, one by one, and never widen.** A lease keeps
//!   what its runtime loaded (the attest's claim) apart from what it may
//!   still do (`usable`): the claim as far as the settings enable it, at the
//!   bind and whenever they enable less (`settings_changed`, and again at
//!   every commit). Taking one capability away leaves the other and the
//!   lease: turning Signal off withdraws that lease's word and episode and
//!   nothing else; turning session observation off forgets that lease's
//!   session and nothing else. Deck acts on a change at once; the observer
//!   hears of it in the answer to its next message — every answer to a
//!   frame, and the acceptance itself, carries the two bits as they are, so
//!   an observer bound with less than its runtime loaded knows before it
//!   sends anything — and a frame of a lost capability that was already on
//!   its way is answered `revoked`: its number is used up, nothing is
//!   written, the lease goes on. A frame of a capability the attest never
//!   claimed is a forgery and ends the lease (`capability-disabled`), as
//!   does a lease left with no capability at all. Nothing gives a running
//!   observer a capability back: that takes Pi loading its extensions again
//!   (`config`, `needs-reload`).
//! - **What is checked for every frame** (`commit`), in this order: the
//!   lease exists; the connection's peer pid is the lease's; the lease is
//!   live; the process is still the instance that attested; the frame
//!   carries the lease's nonce and no fork was counted since that nonce was
//!   made (`again` otherwise — and either way the lease has a new nonce
//!   before the answer is written); the line is one of the five frames and
//!   its answer is still due; for a frame that writes pane-bound state, the
//!   pane is read from a listing taken AFTER the frame was read; then the
//!   kernel is asked NOW — after that listing — for the peer's generation,
//!   the pane process, the terminal's foreground and the ancestry; then,
//!   under the lock, the lease again, what the settings enable (narrowing
//!   the lease when that is less), the sequence number (the next one, no
//!   gap, no repeat), the frame's capability and the run rules. Nothing
//!   before the sender is proven ends or changes a lease — but for its
//!   nonce, and for an exec or exit of its process, which is the kernel's
//!   word and nobody's line. A `ping` commits nothing pane-bound and needs
//!   no listing: an idle Pi causes no tmux call. Messages that were already
//!   waiting when a listing is taken share it; each still gets its own
//!   kernel reads.
//! - **Session observation.** A lifecycle frame is a capability of its own
//!   and of the settings, and is handed to `agent_reentry::lifecycle::
//!   observe_pi` with the lease it came on: an observation, kept apart from
//!   every re-entry receipt. A frame that contradicts what the lease already
//!   reported is refused (`bad-lifecycle`) and its record is gone. An id
//!   this build does not keep — longer than `MAX_NATIVE_ID`, or one the
//!   observer could not put on a line and reports with `unavailable` — is
//!   neither kept nor passed over: that lease's session observation becomes
//!   `unsupported`, visibly, and stays so whatever the lease says next
//!   (`lifecycle::pi_unsupported`); the frame is acknowledged and the
//!   lease's run boundaries go on.
//! - **Runs.** `working` opens run `n + 1` and only while no run is open;
//!   `turn-done` closes the open run and names it. Anything else — a run
//!   that goes back, a second start, an ending of another run — is refused:
//!   a late ending of one run can never close another.
//! - **Ending a lease withdraws its observation at once** (`end`): a reset,
//!   a refused frame of a proven sender, staleness (no committed frame for
//!   `STALE_AFTER`; the observer's heartbeat is every five seconds — this
//!   proves the observer gone, never that the agent is idle or done), the
//!   last capability turned off (`settings_changed`), another foreground,
//!   pane or execution instance (`reconcile`, the sweep). Nothing is
//!   reported in its place, nothing is replayed on a later lease, and a lost
//!   observer is never a turn that ended. A sender that never bound a lease
//!   changes nothing for any pane.
//! - **No authority.** Nothing here types, starts, resumes, moves or closes
//!   anything, and nothing a Pi observer says lifts a scheduler hold.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{Capabilities, Frame, Hello, Message, Nonce, PiVersion, ResetReason, RunState};
use crate::agent_origin::ORIGIN_HOPS;
use crate::agent_reentry::lifecycle::PiNoted;
use crate::agent_status::{ForegroundGeneration, PaneKey, PiCoverage};
use crate::applog::{applog, session_tag};
use crate::procinfo::{Continuity, InstanceWatch, ProcessInfo};
use crate::sync::LockRecover;
use crate::tmux::PaneRow;

/// The most observers that are bound at one time.
pub(crate) const MAX_LEASES: usize = 32;
/// The most messages that were handed over and are not yet answered.
pub(crate) const MAX_PENDING: usize = 128;
/// The most challenges that are open at one time.
pub(crate) const MAX_CHALLENGES: usize = 32;
/// How long a challenge waits for its attest.
pub(crate) const CHALLENGE_BUDGET: Duration = Duration::from_secs(5);
/// A lease without a committed frame for this long is over. The observer's
/// heartbeat is every five seconds; this proves the observer gone, never
/// that the agent is idle or done.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(15);
/// How long after a message was handed over its answer may be ready: the
/// observer waits two seconds, counted from before it connected.
pub(crate) const ANSWER_BUDGET: Duration = Duration::from_secs(2);
/// One pane listing of the admission worker.
const LISTING_BUDGET: Duration = Duration::from_secs(2);
/// How long writing one answer may take before its connection is closed.
const WRITE_BUDGET: Duration = Duration::from_millis(250);
/// The same, for a refusal written on the listener's dispatcher.
const BUSY_BUDGET: Duration = Duration::from_millis(50);
/// How often the worker looks for leases that went stale or whose process
/// is no longer what it was.
const SWEEP_EVERY: Duration = Duration::from_secs(1);
/// How long, and of how many, an ended lease is remembered, so that a frame
/// of it is told why it ended.
const ENDED_KEPT: Duration = Duration::from_secs(60);
const MAX_ENDED: usize = 64;

/// How this service counts a refused message in the status listener's
/// bounded drop diagnostics (`agent_status`): a closed reason, nothing else.
pub(crate) type Reporter = Arc<dyn Fn(&'static str) + Send + Sync>;

/// What the admission reads outside this process. Production is
/// `World::live`; a test describes a topology exactly.
pub(crate) struct World {
    /// One process as the kernel says it is now (`procinfo::process`).
    pub(crate) process: Box<dyn Fn(u32) -> Option<ProcessInfo> + Send + Sync>,
    /// Every pane of Deck's server, read now; `None` when that failed.
    pub(crate) listing: Box<dyn Fn() -> Option<Vec<PaneRow>> + Send + Sync>,
    /// The capabilities Pi's settings enable now (`config::enabled`).
    pub(crate) enabled: Box<dyn Fn() -> Capabilities + Send + Sync>,
    /// A watch of one process's execution instance and of its forks, from
    /// now on (`InstanceWatch::of`); `None` when the kernel gave none.
    pub(crate) watch: Box<dyn Fn(u32) -> Option<Arc<InstanceWatch>> + Send + Sync>,
    /// A nonce nobody has seen (`Nonce::fresh`).
    pub(crate) nonce: Box<dyn Fn() -> Option<Nonce> + Send + Sync>,
    /// The name of Deck's own tmux socket.
    pub(crate) socket: String,
}

impl World {
    fn live() -> Self {
        Self {
            process: Box::new(crate::procinfo::process),
            listing: Box::new(|| crate::tmux::list_panes().ok()),
            enabled: Box::new(super::config::enabled),
            watch: Box::new(InstanceWatch::of),
            nonce: Box::new(Nonce::fresh),
            socket: crate::tmux::socket().to_string(),
        }
    }
}

/// The time bounds of the service. Production's are the constants above; a
/// test of what happens when one passes does not wait that long.
#[derive(Clone, Copy)]
pub(crate) struct Bounds {
    pub(crate) answer: Duration,
    pub(crate) stale: Duration,
    pub(crate) challenge: Duration,
}

impl Bounds {
    const LIVE: Self = Self {
        answer: ANSWER_BUDGET,
        stale: STALE_AFTER,
        challenge: CHALLENGE_BUDGET,
    };
}

// ---------- leases and challenges ----------------------------------------------------

struct Lease {
    key: PaneKey,
    session: String,
    session_id: String,
    pane_pid: u32,
    pane_birth: (u64, u32),
    /// The peer: the pane's foreground leader when the attest was accepted.
    generation: ForegroundGeneration,
    /// The execution instance the peer has been since its hello.
    instance: Arc<InstanceWatch>,
    /// The nonce the lease's next message must carry, and the watch's fork
    /// count at the look after which that nonce was made (module header,
    /// "Whose bytes").
    nonce: Nonce,
    forks: u64,
    /// What the attest claimed: what this runtime loaded.
    loaded: Capabilities,
    /// What the lease may still do: `loaded` as far as the settings enabled
    /// it at the bind, less whenever they enable less. Never more.
    usable: Capabilities,
    /// The sequence number the next frame must carry.
    next_seq: u64,
    /// The highest run this lease opened, and whether it is still open.
    run: u64,
    run_open: bool,
    /// When a frame of this lease was last committed.
    valid: Instant,
    /// Why the lease is over, and since when. The entry stays for a while,
    /// so that a frame of it is told why and its number is not reused.
    over: Option<(&'static str, Instant)>,
}

/// What a `hello` was answered with, kept until its `attest`.
struct Challenge {
    nonce: Nonce,
    /// The watch that began with the hello.
    instance: Arc<InstanceWatch>,
    /// Its fork count at the look after which the nonce was made.
    forks: u64,
    until: Instant,
}

#[derive(Default)]
struct Registry {
    issued: u64,
    leases: HashMap<u64, Lease>,
    /// At most one open challenge per peer pid.
    challenges: HashMap<u32, Challenge>,
}

impl Registry {
    fn live(&self, lease: u64) -> Option<&Lease> {
        self.leases.get(&lease).filter(|lease| lease.over.is_none())
    }

    fn live_mut(&mut self, lease: u64) -> Option<&mut Lease> {
        self.leases
            .get_mut(&lease)
            .filter(|lease| lease.over.is_none())
    }

    fn alive(&self) -> usize {
        self.leases
            .values()
            .filter(|lease| lease.over.is_none())
            .count()
    }
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    f(REGISTRY
        .lock_or_recover()
        .get_or_insert_with(Registry::default))
}

/// How many messages were handed over and are not yet answered.
static PENDING: AtomicUsize = AtomicUsize::new(0);

/// End a lease: its observation is withdrawn at once, under the lock that
/// binds and commits. The first reason stands; ending an ended or unknown
/// lease does nothing.
fn end(lease: u64, code: &'static str) {
    let ended = with_registry(|registry| {
        let entry = registry.live_mut(lease)?;
        entry.over = Some((code, Instant::now()));
        crate::agent_status::pi_lost(&entry.key, entry.generation, lease);
        crate::agent_reentry::lifecycle::forget_pi(lease);
        // that observer stops for good when it hears this, before Pi reloads
        if code == "capability-disabled" {
            crate::agent_status::pi_waits_for_reload(&entry.key, entry.generation);
        }
        Some(entry.session.clone())
    });
    if let Some(session) = ended {
        #[cfg(test)]
        probe::ENDED.lock_or_recover().push((session.clone(), code));
        // closed words and a session tag: no id, pid, path or version
        applog(&format!(
            "[agent-pi] observer ended s={} ({code})",
            session_tag(&session)
        ));
    }
}

/// Whether a running Pi does something else than `configured` says and
/// will go on doing so until it loads its extensions again (`config::
/// status`, `needs-reload`): a live lease that may do other things than the
/// entry names — it loaded less, or lost a capability that has been turned
/// on again since — or a recognized Pi whose observer was ended for its
/// capabilities and does not come back by itself.
pub(crate) fn mismatched(configured: Capabilities) -> bool {
    with_registry(|registry| {
        registry
            .leases
            .values()
            .any(|lease| lease.over.is_none() && lease.usable != configured)
    }) || crate::agent_status::pi_reload_pending()
}

/// The settings entry changed in Deck: a lease loses what the settings no
/// longer enable now, not at its next frame, and keeps the rest.
pub(crate) fn settings_changed() {
    enabled_changed(super::config::enabled());
}

/// Take from one live lease what `enabled` no longer allows (module header,
/// "Capabilities narrow"). Called under the registry lock. `false` when
/// nothing would be left: the caller ends the lease.
fn narrowed(entry: &mut Lease, lease: u64, enabled: Capabilities) -> bool {
    let next = entry.usable.and(enabled);
    if next == entry.usable {
        return true;
    }
    if !next.any() {
        return false;
    }
    if entry.usable.signal && !next.signal {
        // that lease's word and episode, and nothing else; a run that was
        // open is not closed by anything this lease sends later
        entry.run_open = false;
        crate::agent_status::pi_lost(&entry.key, entry.generation, lease);
    }
    if entry.usable.lifecycle && !next.lifecycle {
        crate::agent_reentry::lifecycle::forget_pi(lease);
    }
    entry.usable = next;
    applog(&format!(
        "[agent-pi] observer narrowed s={} signal={} lifecycle={}",
        session_tag(&entry.session),
        u8::from(next.signal),
        u8::from(next.lifecycle)
    ));
    true
}

/// `settings_changed` for a given enabled set.
fn enabled_changed(enabled: Capabilities) {
    let ended: Vec<u64> = with_registry(|registry| {
        registry
            .leases
            .iter_mut()
            .filter(|(_, lease)| lease.over.is_none())
            .filter_map(|(id, lease)| (!narrowed(lease, *id, enabled)).then_some(*id))
            .collect()
    });
    for lease in ended {
        end(lease, "capability-disabled");
    }
}

/// Why a lease whose peer is no longer the execution instance it was bound
/// as is over; `None` while it is that instance.
fn replaced(instance: &InstanceWatch) -> Option<&'static str> {
    match instance.continuity() {
        Continuity::Same => None,
        Continuity::Executed => Some("peer-executed"),
        // the process is gone: its pid and birth name nothing that runs
        Continuity::Exited => Some("generation-mismatch"),
        Continuity::Unreadable => Some("no-instance"),
    }
}

/// The poll's pane listing against the leases: a lease whose pane now has
/// another process or session, whose peer executed another image, or whose
/// pane's foreground is no longer the peer, is over. The foreground is asked
/// NOW (`generation_now`), not taken from the poll's snapshot, so a lease
/// bound after that snapshot was made is judged by what is true and not by
/// what the snapshot missed.
pub(crate) fn reconcile_with(
    rows: &[PaneRow],
    generation_now: impl Fn(u32) -> Option<ForegroundGeneration>,
) {
    let ended: Vec<(u64, &'static str)> = with_registry(|registry| {
        registry
            .leases
            .iter()
            .filter(|(_, lease)| lease.over.is_none())
            .filter_map(|(id, lease)| {
                let moved = rows.iter().any(|row| {
                    lease.key.names(row)
                        && (row.pane_pid != lease.pane_pid || row.session_id != lease.session_id)
                });
                if moved {
                    Some((*id, "pane-changed"))
                } else if let Some(code) = replaced(&lease.instance) {
                    Some((*id, code))
                } else {
                    (generation_now(lease.pane_pid) != Some(lease.generation))
                        .then_some((*id, "generation-mismatch"))
                }
            })
            .collect()
    });
    for (lease, code) in ended {
        end(lease, code);
    }
}

/// `reconcile_with` on the live kernel (`agent_status::reconcile`).
pub(crate) fn reconcile(rows: &[PaneRow]) {
    reconcile_with(rows, crate::agent_status::live_generation);
}

/// The worker's own look, with no pane listing: a lease whose process is no
/// longer the instance that attested, or that committed nothing for
/// `stale`, is over; what ended long ago and challenges nobody answered are
/// forgotten.
fn sweep(bounds: &Bounds) {
    let now = Instant::now();
    let ended: Vec<(u64, &'static str)> = with_registry(|registry| {
        registry
            .challenges
            .retain(|_, challenge| challenge.until > now);
        registry.leases.retain(|_, lease| {
            lease
                .over
                .is_none_or(|(_, since)| now.duration_since(since) < ENDED_KEPT)
        });
        let mut gone: Vec<(Instant, u64)> = registry
            .leases
            .iter()
            .filter_map(|(id, lease)| lease.over.map(|(_, since)| (since, *id)))
            .collect();
        if gone.len() > MAX_ENDED {
            gone.sort();
            for (_, id) in &gone[..gone.len() - MAX_ENDED] {
                registry.leases.remove(id);
            }
        }
        registry
            .leases
            .iter()
            .filter(|(_, lease)| lease.over.is_none())
            .filter_map(|(id, lease)| {
                if let Some(code) = replaced(&lease.instance) {
                    Some((*id, code))
                } else {
                    (now.duration_since(lease.valid) >= bounds.stale).then_some((*id, "stale"))
                }
            })
            .collect()
    });
    for (lease, code) in ended {
        end(lease, code);
    }
}

// ---------- admission ----------------------------------------------------------------

/// One message as the listener handed it over.
struct Job {
    stream: UnixStream,
    line: String,
    /// The kernel's peer pid of the connection, read at the hand-over.
    peer: Option<u32>,
    /// When it was handed over: its answer is due `Bounds::answer` later.
    handed: Instant,
}

/// What Deck answers one message with, when it does not refuse it. Each
/// carries the nonce the observer's next message must carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// To a hello: the nonce its attest must carry, and nothing else.
    Challenge(Nonce),
    /// To an attest: the lease, and what it may do.
    Accepted {
        lease: u64,
        next: Nonce,
        usable: Capabilities,
    },
    /// The frame was written.
    Ack {
        seq: u64,
        next: Nonce,
        usable: Capabilities,
    },
    /// Its capability was taken from the lease before it arrived: its
    /// number is used up and nothing was written.
    Revoked {
        seq: u64,
        next: Nonce,
        usable: Capabilities,
    },
    /// The line could not be shown to be the bound process's own, through
    /// no fault of the message: nothing was read into anything (module
    /// header, "Whose bytes").
    Again(Nonce),
}

impl Answer {
    fn line(&self) -> String {
        match self {
            Answer::Challenge(next) => super::nonce_line(next),
            Answer::Accepted {
                lease,
                next,
                usable,
            } => super::accepted_line(*lease, next, *usable),
            Answer::Ack { seq, next, usable } => super::ack_line(*seq, next, *usable),
            Answer::Revoked { seq, next, usable } => super::revoked_line(*seq, next, *usable),
            Answer::Again(next) => super::again_line(next),
        }
    }
}

/// The pane listing of one batch: taken at most once, when a job first
/// needs it, under its own deadline.
struct Listing<'a> {
    world: &'a World,
    taken: Option<Option<Vec<PaneRow>>>,
}

impl Listing<'_> {
    fn rows(&mut self) -> Option<&[PaneRow]> {
        let world = self.world;
        self.taken
            .get_or_insert_with(|| {
                let _deadline =
                    crate::session_runtime::Deadline::until(Instant::now() + LISTING_BUDGET);
                (world.listing)()
            })
            .as_deref()
    }
}

/// One message, verified and answered (module header). `due` is when its
/// answer must be ready. `Err` is the closed code it is refused with.
fn exchange(
    world: &World,
    bounds: &Bounds,
    line: &str,
    peer: Option<u32>,
    due: Instant,
    listing: &mut Listing,
) -> Result<Answer, &'static str> {
    match super::parse_message(line)? {
        Message::Hello => challenge(world, bounds, peer),
        Message::Attest { nonce, hello } => bind(world, bounds, &hello, &nonce, peer, due, listing),
        Message::Frame {
            lease,
            nonce,
            frame,
        } => commit(world, lease, &nonce, &frame, peer, due, listing),
    }
}

/// Keep `nonce` as the one an attest of `peer` must carry. `forks` is the
/// watch's count at the look after which the nonce was made.
fn challenged(
    bounds: &Bounds,
    peer: u32,
    instance: Arc<InstanceWatch>,
    forks: u64,
    nonce: &Nonce,
) -> Result<(), &'static str> {
    let now = Instant::now();
    with_registry(|registry| {
        registry.challenges.retain(|_, open| open.until > now);
        // a second hello of one process replaces what its first was answered
        if registry.challenges.len() >= MAX_CHALLENGES && !registry.challenges.contains_key(&peer) {
            return Err("busy");
        }
        registry.challenges.insert(
            peer,
            Challenge {
                nonce: nonce.clone(),
                instance,
                forks,
                until: now + bounds.challenge,
            },
        );
        Ok(())
    })
}

/// Answer a hello (module header, "A hello is not believed"): from here on
/// the kernel reports what this process executes and whether it forks, and
/// the nonce — made after both — is what its attest must carry.
fn challenge(world: &World, bounds: &Bounds, peer: Option<u32>) -> Result<Answer, &'static str> {
    let peer = peer.ok_or("no-peer")?;
    let instance = (world.watch)(peer).ok_or("no-instance")?;
    let forks = instance.forks();
    let nonce = (world.nonce)().ok_or("busy")?;
    challenged(bounds, peer, instance, forks, &nonce)?;
    Ok(Answer::Challenge(nonce))
}

/// The kernel facts every commit needs, read now: the peer is still that
/// generation, the pane's process is alive, the pane terminal's foreground
/// group is led by the peer, the pane's process is the peer's ancestor with
/// every process between them on the pane's terminal, and — asked last, so
/// that it covers the reads before it — the peer has not executed another
/// image since its hello. Returns the pane process's birth instant.
fn verified(
    world: &World,
    pane_pid: u32,
    generation: ForegroundGeneration,
    instance: &InstanceWatch,
) -> Result<(u64, u32), &'static str> {
    let client = (world.process)(generation.pid())
        .filter(|client| (client.start_seconds, client.start_micros) == generation.birth())
        .ok_or("generation-mismatch")?;
    let pane = (world.process)(pane_pid)
        .filter(|pane| pane.start_seconds != 0)
        .ok_or("no-generation")?;
    if pane.tty == 0 || client.tty != pane.tty {
        return Err("foreign-pane");
    }
    if pane.tty_pgid != client.pid || client.pgid != client.pid {
        return Err("generation-mismatch");
    }
    let mut at = client;
    let mut hops = 0;
    while at.pid != pane_pid {
        if at.tty != pane.tty {
            return Err("terminal-discontinuity");
        }
        hops += 1;
        if hops > ORIGIN_HOPS || at.ppid <= 1 || at.ppid == at.pid {
            return Err("foreign-pane");
        }
        at = (world.process)(at.ppid).ok_or("foreign-pane")?;
    }
    match replaced(instance) {
        Some(code) => Err(code),
        None => Ok((pane.start_seconds, pane.start_micros)),
    }
}

/// Admit one attest (module header, "A hello is not believed" and "Who may
/// bind").
fn bind(
    world: &World,
    bounds: &Bounds,
    hello: &Hello,
    nonce: &Nonce,
    peer: Option<u32>,
    due: Instant,
    listing: &mut Listing,
) -> Result<Answer, &'static str> {
    let peer = peer.ok_or("no-peer")?;
    // The nonce this process was given: good for this one attest, whatever
    // becomes of it. A pid without one, a nonce that is not the one it was
    // given, and an attest that came too late are the same thing.
    let challenge = with_registry(|registry| registry.challenges.remove(&peer))
        .filter(|open| open.until > Instant::now() && open.nonce.same(nonce))
        .ok_or("no-challenge")?;
    let instance = challenge.instance;
    if let Some(code) = replaced(&instance) {
        return Err(code);
    }
    // the look, and after it the nonce of whatever is answered
    let forks = instance.forks();
    let next = (world.nonce)().ok_or("busy")?;
    // A child made since the hello was answered has a copy of that nonce
    // and may hold this connection: whose attest this is cannot be said.
    // The observer attests again, with the new nonce.
    if forks != challenge.forks {
        challenged(bounds, peer, instance, forks, &next)?;
        #[cfg(test)]
        probe::again("fork");
        return Ok(Answer::Again(next));
    }
    let client = (world.process)(peer)
        .filter(|client| client.start_seconds != 0)
        .ok_or("no-peer")?;
    let generation = ForegroundGeneration::of(peer, (client.start_seconds, client.start_micros));
    // the observer runs inside the terminal's foreground program itself
    if client.tty == 0 || client.pgid != client.pid || client.tty_pgid != client.pid {
        return Err("not-foreground");
    }
    // An earlier lease of this pid whose program has since executed another
    // image, or is gone, is over before it can stand in this one's way.
    let stale: Vec<(u64, &'static str)> = with_registry(|registry| {
        registry
            .leases
            .iter()
            .filter(|(_, lease)| lease.over.is_none() && lease.generation == generation)
            .filter_map(|(id, lease)| replaced(&lease.instance).map(|code| (*id, code)))
            .collect()
    });
    for (lease, code) in stale {
        end(lease, code);
    }
    let bound = |registry: &Registry| {
        registry
            .leases
            .values()
            .any(|lease| lease.over.is_none() && lease.generation == generation)
    };
    // asked before the listing, so a second observer of one Pi costs no tmux call
    if with_registry(|registry| bound(registry)) {
        return Err("duplicate-observer");
    }
    if hello.socket != world.socket {
        return Err("other-server");
    }
    let row = listing
        .rows()
        .ok_or("no-such-pane")?
        .iter()
        .find(|row| row.pane_id == hello.pane && row.server_pid == hello.server_pid)
        .filter(|row| crate::tmux::validate_session_name(&row.session_name).is_ok())
        .ok_or("no-such-pane")?
        .clone();
    // a plain shell in front has no Pi to bind an observation's lifetime to
    if crate::context::shell_process(Some(&row.command)) {
        return Err("not-foreground");
    }
    // the kernel, asked after the listing: what it says now is what counts
    let pane_birth = verified(world, row.pane_pid, generation, &instance)?;
    // Proven: this pane's foreground program is a Pi observer. That holds
    // for this execution instance whatever is decided about the attest below.
    if hello.version == PiVersion::Unsupported {
        let coverage = if hello.capabilities.signal {
            PiCoverage::Incompatible
        } else {
            PiCoverage::Unavailable
        };
        crate::agent_status::pi_observed(&row, generation, coverage, &instance);
        return Err("unsupported-pi-version");
    }
    // what this runtime loaded, as far as the settings enable it now: the
    // rest is not usable, and an observer with nothing left is refused
    let usable = hello.capabilities.and((world.enabled)());
    if !usable.any() {
        crate::agent_status::pi_observed(&row, generation, PiCoverage::Unavailable, &instance);
        crate::agent_status::pi_waits_for_reload(&PaneKey::of(&row), generation);
        return Err("capability-disabled");
    }
    // an observer that has stopped waiting for this answer holds no lease
    if Instant::now() > due {
        return Err("late");
    }
    let lease = with_registry(|registry| {
        if bound(registry) {
            return Err("duplicate-observer");
        }
        if registry.alive() >= MAX_LEASES {
            return Err("busy");
        }
        registry.issued += 1;
        let lease = registry.issued;
        registry.leases.insert(
            lease,
            Lease {
                key: PaneKey::of(&row),
                session: row.session_name.clone(),
                session_id: row.session_id.clone(),
                pane_pid: row.pane_pid,
                pane_birth,
                generation,
                instance: instance.clone(),
                nonce: next.clone(),
                forks,
                loaded: hello.capabilities,
                usable,
                next_seq: 1,
                run: 0,
                run_open: false,
                valid: Instant::now(),
                over: None,
            },
        );
        let coverage = if usable.signal {
            PiCoverage::Connected
        } else {
            PiCoverage::Unavailable
        };
        crate::agent_status::pi_observed(&row, generation, coverage, &instance);
        Ok(lease)
    })?;
    applog(&format!(
        "[agent-pi] observer bound s={} signal={} lifecycle={}",
        session_tag(&row.session_name),
        u8::from(usable.signal),
        u8::from(usable.lifecycle)
    ));
    Ok(Answer::Accepted {
        lease,
        next,
        usable,
    })
}

/// Verify and commit one frame of `lease` (module header). Nothing ends or
/// changes the lease before its sender is proven — but for its nonce, which
/// is replaced at every look, and for an exec or exit of its process, which
/// is the kernel's word and nobody's line. After that, a refusal ends it
/// here, before the observer hears of it.
fn commit(
    world: &World,
    lease: u64,
    nonce: &Nonce,
    frame: &Result<Frame, &'static str>,
    peer: Option<u32>,
    due: Instant,
    listing: &mut Listing,
) -> Result<Answer, &'static str> {
    let peer = peer.ok_or("no-peer")?;
    let known = with_registry(|registry| {
        registry.leases.get(&lease).map(|entry| {
            (
                entry.over.map(|(code, _)| code),
                entry.generation,
                entry.instance.clone(),
            )
        })
    });
    let Some((over, generation, instance)) = known else {
        return Err("late");
    };
    // a connection whose last user was another process is not this lease's
    if peer != generation.pid() {
        return Err("sender-mismatch");
    }
    if let Some(code) = over {
        // why it ended, when that is a word an observer knows
        return Err(if super::REFUSALS.contains(&code) {
            code
        } else {
            "late"
        });
    }
    // the process itself: whoever wrote, a program that executed another
    // image is not the one that attested
    if let Some(code) = replaced(&instance) {
        end(lease, code);
        return Err(code);
    }
    // The look, and after it the nonce of whatever is answered. The line is
    // the peer's own when it carries the nonce the lease had and no fork
    // was counted since that one was made.
    let forks = instance.forks();
    let next = (world.nonce)().ok_or("busy")?;
    let (alone, held) = with_registry(|registry| {
        let entry = registry.live_mut(lease)?;
        let proof = (entry.forks == forks, entry.nonce.same(nonce));
        entry.forks = forks;
        entry.nonce = next.clone();
        Some(proof)
    })
    .ok_or("late")?;
    if !(alone && held) {
        #[cfg(test)]
        probe::again(match (alone, held) {
            (false, true) => "fork",
            (true, false) => "nonce",
            _ => "fork and nonce",
        });
        return Ok(Answer::Again(next));
    }
    // From here on the line is the bound observer's own, and what is wrong
    // with it ends its lease: a line that is no frame, and one whose
    // observer has stopped waiting for the answer.
    let frame = match frame {
        Ok(frame) if Instant::now() <= due => frame,
        refused => {
            let code = refused.as_ref().err().copied().unwrap_or("late");
            end(lease, code);
            return Err(code);
        }
    };
    let verdict = committed(world, lease, frame, next, due, listing);
    match (&verdict, frame) {
        (Err(code), _) => end(lease, code),
        // a reset is acknowledged and is the end of what its lease reported
        (Ok(_), Frame::Reset { reason, .. }) => end(lease, reset_code(*reason)),
        _ => {}
    }
    #[cfg(test)]
    if let (Ok(Answer::Revoked { .. }), Some(capability)) = (&verdict, frame.capability()) {
        probe::revoked_frame(lease, capability);
    }
    verdict
}

/// The closed word a reset ends its lease with (the log's, never a refusal).
fn reset_code(reason: ResetReason) -> &'static str {
    match reason {
        ResetReason::New => "reset-new",
        ResetReason::Resume => "reset-resume",
        ResetReason::Fork => "reset-fork",
        ResetReason::Reload => "reset-reload",
        ResetReason::Quit => "reset-quit",
        ResetReason::Fault => "reset-fault",
    }
}

/// The closed word a session observation is logged with: what the runtime
/// said happened and Pi's own reason for it.
fn session_word(event: super::SessionEvent) -> &'static str {
    use super::{SessionEvent, ShutdownReason, StartReason};
    match event {
        SessionEvent::Start(StartReason::Startup) => "start-startup",
        SessionEvent::Start(StartReason::Reload) => "start-reload",
        SessionEvent::Start(StartReason::New) => "start-new",
        SessionEvent::Start(StartReason::Resume) => "start-resume",
        SessionEvent::Start(StartReason::Fork) => "start-fork",
        SessionEvent::Shutdown(ShutdownReason::Quit) => "shutdown-quit",
        SessionEvent::Shutdown(ShutdownReason::Reload) => "shutdown-reload",
        SessionEvent::Shutdown(ShutdownReason::New) => "shutdown-new",
        SessionEvent::Shutdown(ShutdownReason::Resume) => "shutdown-resume",
        SessionEvent::Shutdown(ShutdownReason::Fork) => "shutdown-fork",
        SessionEvent::Snapshot => "snapshot",
    }
}

/// One frame of a lease whose sender is proven: the pane, the kernel, and
/// then the write under the registry lock.
fn committed(
    world: &World,
    lease: u64,
    frame: &Frame,
    next: Nonce,
    due: Instant,
    listing: &mut Listing,
) -> Result<Answer, &'static str> {
    // what the lease is bound to: nothing is decided on this reading
    let (key, session_id, pane_pid, pane_birth, generation, instance) = with_registry(|registry| {
        registry.live(lease).map(|lease| {
            (
                lease.key.clone(),
                lease.session_id.clone(),
                lease.pane_pid,
                lease.pane_birth,
                lease.generation,
                lease.instance.clone(),
            )
        })
    })
    .ok_or("late")?;
    // the pane, for a frame that writes pane-bound state: a ping does not
    let pane = match frame {
        Frame::Ping { .. } => None,
        _ => {
            let rows = listing.rows().ok_or("no-such-pane")?;
            let row = rows
                .iter()
                .find(|row| key.names(row))
                .filter(|row| row.pane_pid == pane_pid && row.session_id == session_id)
                .filter(|row| crate::tmux::validate_session_name(&row.session_name).is_ok())
                .ok_or("pane-changed")?;
            Some((rows, row))
        }
    };
    // the kernel, now: after the listing, immediately before the write
    if verified(world, pane_pid, generation, &instance)? != pane_birth {
        return Err("pane-changed");
    }
    // the listing and the kernel took their time: nothing is written for
    // an observer that has stopped waiting
    if Instant::now() > due {
        return Err("late");
    }
    let enabled = (world.enabled)();
    with_registry(|registry| {
        let entry = registry.live_mut(lease).ok_or("late")?;
        // what the settings enable now: less is taken from the lease here,
        // and a lease left with nothing is over
        if !narrowed(entry, lease, enabled) {
            return Err("capability-disabled");
        }
        let seq = frame.seq();
        if seq != entry.next_seq {
            return Err("bad-seq");
        }
        if let Some(capability) = frame.capability() {
            // never claimed by the attest: no late frame, a forgery
            if !entry.loaded.has(capability) {
                return Err("capability-disabled");
            }
            // claimed, and taken from the lease since: the frame was on its
            // way; its number is used up and nothing is written
            if !entry.usable.has(capability) {
                entry.next_seq += 1;
                entry.valid = Instant::now();
                return Ok(Answer::Revoked {
                    seq,
                    next,
                    usable: entry.usable,
                });
            }
        }
        if let Frame::Signal { run, state, .. } = frame {
            match state {
                RunState::Working if !entry.run_open && *run == entry.run + 1 => {
                    entry.run = *run;
                    entry.run_open = true;
                }
                RunState::TurnDone if entry.run_open && *run == entry.run => {
                    entry.run_open = false;
                }
                _ => return Err("bad-run"),
            }
            let (rows, row) = pane.ok_or("pane-changed")?;
            crate::agent_status::pi_signal(rows, row, generation, lease, &instance, *run, *state);
        }
        // a session, or the observer's word that it has an id it cannot send
        let noted = match frame {
            Frame::Lifecycle { event, native, .. } => Some((
                crate::agent_reentry::lifecycle::observe_pi(
                    lease, &entry.key, generation, *event, native,
                )?,
                session_word(*event),
            )),
            Frame::Unavailable { .. } => Some((
                crate::agent_reentry::lifecycle::pi_unsupported(lease, &entry.key, generation),
                "unsupported-native-id",
            )),
            _ => None,
        };
        // closed words and a session tag: never the id, its length or a path
        match noted {
            Some((PiNoted::Observed, word)) => applog(&format!(
                "[agent-pi] session {word} s={}",
                session_tag(&entry.session)
            )),
            Some((PiNoted::Unsupported, _)) => applog(&format!(
                "[agent-pi] session unsupported-native-id s={}",
                session_tag(&entry.session)
            )),
            Some((PiNoted::Ignored, _)) | None => {}
        }
        entry.next_seq += 1;
        entry.valid = Instant::now();
        Ok(Answer::Ack {
            seq,
            next,
            usable: entry.usable,
        })
    })
}

/// Write one answer and close: a short line into the buffer of a client
/// that has read nothing yet, with a deadline for one that somehow does not
/// take it.
fn answer(mut stream: UnixStream, line: &str, budget: Duration) {
    use std::io::Write;
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(budget));
    let _ = stream.write_all(line.as_bytes());
}

fn worker(waiting: Receiver<Job>, world: World, report: Reporter, bounds: Bounds) {
    let every = SWEEP_EVERY
        .min(bounds.stale / 4)
        .max(Duration::from_millis(5));
    let mut swept = Instant::now();
    loop {
        let first = match waiting.recv_timeout(every) {
            Ok(job) => Some(job),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if let Some(first) = first {
            // Everything already waiting is one batch: a listing taken from
            // here on is newer than every line of it.
            let mut batch = vec![first];
            while batch.len() < MAX_PENDING {
                match waiting.try_recv() {
                    Ok(job) => batch.push(job),
                    Err(_) => break,
                }
            }
            let mut listing = Listing {
                world: &world,
                taken: None,
            };
            for job in batch {
                let due = job.handed + bounds.answer;
                let said = match exchange(&world, &bounds, &job.line, job.peer, due, &mut listing) {
                    Ok(answer) => answer.line(),
                    Err(code) => {
                        report(super::drop_reason(code));
                        super::refusal_of(&job.line, code)
                    }
                };
                PENDING.fetch_sub(1, Ordering::AcqRel);
                answer(job.stream, &said, WRITE_BUDGET);
            }
        }
        if swept.elapsed() >= every {
            sweep(&bounds);
            swept = Instant::now();
        }
    }
}

// ---------- the service ----------------------------------------------------------------

/// The service: the queue in front of the one admission worker.
pub(crate) struct Service {
    jobs: SyncSender<Job>,
    report: Reporter,
}

impl Service {
    /// Start the admission worker. A worker that cannot be started takes
    /// the queue's other end with it: every message is then refused `busy`.
    pub(crate) fn start(report: Reporter) -> Self {
        Self::start_in(report, World::live(), Bounds::LIVE)
    }

    pub(crate) fn start_in(report: Reporter, world: World, bounds: Bounds) -> Self {
        let (jobs, waiting) = std::sync::mpsc::sync_channel::<Job>(MAX_PENDING);
        let _ = std::thread::Builder::new()
            .name("deck-pi-admission".into())
            .spawn({
                let report = report.clone();
                move || worker(waiting, world, report, bounds)
            });
        Self { jobs, report }
    }

    /// Take one Pi message from the status listener. Never waits: the
    /// connection's peer is read from the kernel here, and the message is
    /// queued for the worker or refused at once.
    pub(crate) fn hand(&self, stream: UnixStream, line: String) {
        let peer = crate::procinfo::peer_pid(&stream);
        self.hand_as(stream, line, peer);
    }

    /// `hand` for a peer the caller names.
    pub(crate) fn hand_as(&self, stream: UnixStream, line: String, peer: Option<u32>) {
        PENDING.fetch_add(1, Ordering::AcqRel);
        let refused = match self.jobs.try_send(Job {
            stream,
            line,
            peer,
            handed: Instant::now(),
        }) {
            Ok(()) => return,
            Err(TrySendError::Full(job)) | Err(TrySendError::Disconnected(job)) => job,
        };
        PENDING.fetch_sub(1, Ordering::AcqRel);
        (self.report)(super::drop_reason("busy"));
        answer(refused.stream, &super::refused_line("busy"), BUSY_BUDGET);
    }
}

/// What a test may ask about the service's state.
#[cfg(test)]
pub(crate) mod probe {
    use super::*;

    /// Why each lease ended, in order: what production writes to app.log.
    pub(super) static ENDED: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

    /// (session, capability) of every frame that was answered `revoked`,
    /// in order: a frame of a capability its lease had lost on its way.
    static REVOKED: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

    /// Why each message that was answered `again` was: a fork was counted
    /// since its nonce was made, it did not carry the nonce, or both.
    static AGAIN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

    /// Forget every lease and challenge.
    pub(crate) fn reset() {
        with_registry(|registry| {
            registry.leases.clear();
            registry.challenges.clear();
        });
        ENDED.lock_or_recover().clear();
        REVOKED.lock_or_recover().clear();
        AGAIN.lock_or_recover().clear();
    }

    pub(super) fn again(why: &'static str) {
        AGAIN.lock_or_recover().push(why);
    }

    /// How many messages were answered `again` since `reset`.
    pub(crate) fn agains() -> usize {
        AGAIN.lock_or_recover().len()
    }

    /// Why each of them was, in order: `fork`, `nonce`, or `fork and nonce`.
    pub(crate) fn again_reasons() -> Vec<&'static str> {
        AGAIN.lock_or_recover().clone()
    }

    pub(super) fn revoked_frame(lease: u64, capability: crate::agent_pi::Capability) {
        let session = with_registry(|registry| {
            registry
                .leases
                .get(&lease)
                .map(|lease| lease.session.clone())
        });
        let word = match capability {
            crate::agent_pi::Capability::Signal => "signal",
            crate::agent_pi::Capability::Lifecycle => "lifecycle",
        };
        if let Some(session) = session {
            REVOKED.lock_or_recover().push((session, word));
        }
    }

    /// (session, capability) of every frame answered `revoked` since `reset`.
    pub(crate) fn revoked() -> Vec<(String, &'static str)> {
        REVOKED.lock_or_recover().clone()
    }

    /// (session, closed word) of every lease that ended since `reset`.
    pub(crate) fn ended() -> Vec<(String, &'static str)> {
        ENDED.lock_or_recover().clone()
    }

    pub(crate) fn live() -> usize {
        with_registry(|registry| registry.alive())
    }

    /// How many challenges are open.
    pub(crate) fn challenges() -> usize {
        with_registry(|registry| registry.challenges.len())
    }

    pub(crate) fn pending() -> usize {
        PENDING.load(Ordering::Acquire)
    }

    /// One live lease, as one reading of the registry.
    pub(crate) struct LeaseSeen {
        pub(crate) lease: u64,
        pub(crate) session: String,
        /// The highest run it opened, and whether that run is open.
        pub(crate) run: u64,
        pub(crate) open: bool,
        /// The sequence number it expects next.
        pub(crate) next_seq: u64,
        /// What the lease may still do.
        pub(crate) usable: Capabilities,
    }

    /// Every live lease, by its number.
    pub(crate) fn leases() -> Vec<LeaseSeen> {
        with_registry(|registry| {
            let mut leases: Vec<_> = registry
                .leases
                .iter()
                .filter(|(_, lease)| lease.over.is_none())
                .map(|(id, lease)| LeaseSeen {
                    lease: *id,
                    session: lease.session.clone(),
                    run: lease.run,
                    open: lease.run_open,
                    next_seq: lease.next_seq,
                    usable: lease.usable,
                })
                .collect();
            leases.sort_by_key(|seen| seen.lease);
            leases
        })
    }
}

#[cfg(test)]
mod tests;
