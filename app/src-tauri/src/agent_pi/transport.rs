//! The bounded service that owns every Pi observer stream.
//!
//! # Contract
//! - **Off the accept loop.** The status listener is every agent's Signal
//!   path. Of a Pi stream it reads the first line, takes the kernel's peer
//!   pid and hands the connection over without waiting (`Service::hand`);
//!   nothing below runs on that thread, and a full service refuses the
//!   stream at once. No listener, thread per connection or thread per frame
//!   is added: connections are tasks of the application's existing async
//!   runtime, and ONE admission worker verifies and commits their frames,
//!   one after the other.
//! - **Bounded.** At most `MAX_STREAMS` streams; at most `MAX_PENDING` lines
//!   read and not yet answered across all of them and `MAX_PENDING_PER_STREAM`
//!   of one; a line is at most `agent_pi::MAX_LINE` bytes, whole or partial.
//!   A stream over a bound is refused (`busy`, `bad-frame`) and its
//!   observation withdrawn. An answer takes at most `ANSWER_BUDGET` (the
//!   wait for the worker included) and so does writing it; a stream without
//!   a valid frame for `STALE_AFTER` is over. The worker's one tmux call per
//!   batch runs under `LISTING_BUDGET`. Nothing here is retried.
//! - **An acknowledgement follows the commit.** A frame is answered `ack`
//!   only after it was verified and written; a refusal ends the stream. The
//!   connection is never closed in place of an answer. Whole lines a client
//!   sent before it closed are still its frames (a quitting Pi writes its
//!   reset and goes): they are committed in order, at most the per-stream
//!   bound of them, and only then is the stream over; a line that did not
//!   end is nothing.
//! - **Who may bind** (`bind`). The peer the kernel names must be alive, the
//!   leader of its terminal's foreground process group (the observer runs
//!   INSIDE the interactive Pi: not a child, a helper, a background job or a
//!   wrapper's descendant), and have no other live stream. The hello's
//!   server and pane must be a pane of Deck's own server; that pane's own
//!   process must be the peer's ancestor, every process between them on the
//!   pane's terminal. `$TMUX_PANE`, the socket path, a version, a mode and
//!   every id in the line are claims: none of them is used as proof. Only
//!   after that is the generation RECOGNIZED as Pi (`agent_status::
//!   pi_observed`), whatever happens to the stream next: a release this
//!   build does not support and a capability the settings do not enable are
//!   refused after it, so the card can say which.
//! - **A lease.** An accepted stream gets a number this process never uses
//!   again. It is bound to the pane (server pid, pane id, session id, pane
//!   process and its birth) and to the peer's generation. Every commit names
//!   its lease and is made under the registry lock that ends leases: a frame
//!   of an ended lease writes nothing, however long it waited.
//! - **What is checked for every frame** (`commit`), in this order: the
//!   lease is live; for a frame that writes pane-bound state, the pane is
//!   read from a listing taken AFTER the frame was read; then the kernel is
//!   asked NOW — after that listing — for the peer's generation, the pane
//!   process, the terminal's foreground and the ancestry; then, under the
//!   lock, the lease again, what the settings enable, the sequence number
//!   (the next one, no gap, no repeat) and the run rules. A `ping` commits
//!   nothing pane-bound and needs no listing: an idle Pi causes no tmux
//!   call. Frames that were already waiting when a listing is taken share
//!   it; each still gets its own kernel reads.
//! - **Runs.** `working` opens run `n + 1` and only while no run is open;
//!   `turn-done` closes the open run and names it. Anything else — a run
//!   that goes back, a second start, an ending of another run — is refused:
//!   a late ending of one run can never close another.
//! - **Ending a lease withdraws its observation at once** (`end`): a reset,
//!   the end of the stream, a refusal, staleness, an overflow, a capability
//!   turned off (`settings_changed`), another foreground or pane
//!   (`reconcile`). Nothing is reported in its place, nothing is replayed on
//!   a later stream, and a lost observer is never a turn that ended. A
//!   sender that never bound a lease changes nothing for any pane.
//! - **No authority.** Nothing here types, starts, resumes, moves or closes
//!   anything, and nothing a Pi stream says lifts a scheduler hold.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use tokio::sync::{oneshot, Notify};

use super::{Capabilities, Frame, Hello, PiVersion, ResetReason, RunState};
use crate::agent_origin::ORIGIN_HOPS;
use crate::agent_status::{ForegroundGeneration, PaneKey, PiCoverage};
use crate::applog::{applog, session_tag};
use crate::procinfo::ProcessInfo;
use crate::sync::LockRecover;
use crate::tmux::PaneRow;

/// The most observer streams at one time.
pub(crate) const MAX_STREAMS: usize = 32;
/// The most lines read and not yet answered, across all streams.
pub(crate) const MAX_PENDING: usize = 128;
/// The most lines of one stream read and not yet answered.
pub(crate) const MAX_PENDING_PER_STREAM: usize = 32;
/// How long one frame may take from its submission to its answer, and how
/// long writing one answer may take. The observer gives up after the same
/// time, so a later answer would reach nobody.
pub(crate) const ANSWER_BUDGET: Duration = Duration::from_secs(2);
/// A stream without a valid frame for this long is over. The observer's
/// heartbeat is every five seconds; this proves the transport gone, never
/// that the agent is idle or done.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(15);
/// One pane listing of the admission worker.
const LISTING_BUDGET: Duration = Duration::from_secs(2);
/// How long a refusal may take to be written before the stream is closed.
const REFUSAL_BUDGET: Duration = Duration::from_millis(250);

/// How this service counts a refusal in the status listener's bounded drop
/// diagnostics (`agent_status`): a closed reason, nothing else.
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
    /// The name of Deck's own tmux socket.
    pub(crate) socket: String,
}

impl World {
    fn live() -> Self {
        Self {
            process: Box::new(crate::procinfo::process),
            listing: Box::new(|| crate::tmux::list_panes().ok()),
            enabled: Box::new(super::config::enabled),
            socket: crate::tmux::socket().to_string(),
        }
    }
}

// ---------- leases -----------------------------------------------------------------

struct Lease {
    key: PaneKey,
    session: String,
    session_id: String,
    pane_pid: u32,
    pane_birth: (u64, u32),
    /// The peer: the pane's foreground leader when the stream was bound.
    generation: ForegroundGeneration,
    /// What the hello claimed, all of it enabled when it was accepted.
    capabilities: Capabilities,
    /// The sequence number the next frame must carry.
    next_seq: u64,
    /// The highest run this stream opened, and whether it is still open.
    run: u64,
    run_open: bool,
    /// Why the lease is over. Its stream's task reads this, tells the
    /// client and closes; until that task releases it the entry stays, so
    /// the reason is not lost and the number is not reused.
    over: Option<&'static str>,
    /// Wakes the stream's task when the lease ends from elsewhere.
    wake: Arc<Notify>,
}

#[derive(Default)]
struct Registry {
    issued: u64,
    leases: HashMap<u64, Lease>,
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
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with_registry<R>(f: impl FnOnce(&mut Registry) -> R) -> R {
    f(REGISTRY
        .lock_or_recover()
        .get_or_insert_with(Registry::default))
}

/// How many streams this service holds, from the hand-over to the end of
/// the stream's task. An atomic of its own: the accept loop takes a slot
/// without ever waiting for the registry.
static STREAMS: AtomicUsize = AtomicUsize::new(0);
/// How many lines were read and are not yet answered.
static PENDING: AtomicUsize = AtomicUsize::new(0);

struct Slot;

impl Slot {
    fn take() -> Option<Self> {
        STREAMS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < MAX_STREAMS).then_some(held + 1)
            })
            .ok()
            .map(|_| Slot)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        STREAMS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The lines of one stream that were read and are not yet answered. Each is
/// counted in `PENDING` for as long as it is here.
#[derive(Default)]
struct Pending(VecDeque<String>);

impl Pending {
    fn push(&mut self, line: String) -> Result<(), &'static str> {
        if self.0.len() >= MAX_PENDING_PER_STREAM {
            return Err("busy");
        }
        PENDING
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < MAX_PENDING).then_some(held + 1)
            })
            .map_err(|_| "busy")?;
        self.0.push_back(line);
        Ok(())
    }

    fn pop(&mut self) -> Option<String> {
        let line = self.0.pop_front()?;
        PENDING.fetch_sub(1, Ordering::AcqRel);
        Some(line)
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.fetch_sub(self.0.len(), Ordering::AcqRel);
    }
}

/// End a lease: its observation is withdrawn at once, under the lock that
/// binds and commits, and its stream's task is woken. The first reason
/// stands; ending an ended or unknown lease does nothing.
fn end(lease: u64, code: &'static str) {
    let ended = with_registry(|registry| {
        let entry = registry.live_mut(lease)?;
        entry.over = Some(code);
        crate::agent_status::pi_lost(&entry.key, entry.generation, lease);
        entry.wake.notify_one();
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

/// The stream's task is over: the lease, ended if it was not, is forgotten.
fn release(lease: u64) {
    end(lease, "eof");
    with_registry(|registry| registry.leases.remove(&lease));
}

/// Why `lease` is over, when it is.
fn over(lease: u64) -> Option<&'static str> {
    with_registry(|registry| registry.leases.get(&lease).and_then(|lease| lease.over))
}

/// Whether a live stream has other capabilities than `configured`: that Pi
/// loaded its extensions before the settings entry changed (`config::
/// status`, `needs-reload`).
pub(crate) fn mismatched(configured: Capabilities) -> bool {
    with_registry(|registry| {
        registry
            .leases
            .values()
            .any(|lease| lease.over.is_none() && lease.capabilities != configured)
    })
}

/// The settings entry changed in Deck: a stream that has a capability the
/// settings no longer enable ends now, not at its next frame.
pub(crate) fn settings_changed() {
    enabled_changed(super::config::enabled());
}

/// `settings_changed` for a given enabled set.
fn enabled_changed(enabled: Capabilities) {
    let ended: Vec<u64> = with_registry(|registry| {
        registry
            .leases
            .iter()
            .filter(|(_, lease)| lease.over.is_none() && !lease.capabilities.within(enabled))
            .map(|(id, _)| *id)
            .collect()
    });
    for lease in ended {
        end(lease, "capability-disabled");
    }
}

/// The poll's pane listing against the leases: a lease whose pane now has
/// another process or session, or whose pane's foreground is no longer the
/// peer, is over. The foreground is asked NOW (`generation_now`), not taken
/// from the poll's snapshot, so a stream bound after that snapshot was made
/// is judged by what is true and not by what the snapshot missed.
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

// ---------- admission ----------------------------------------------------------------

/// A stream that was bound.
struct Bound {
    lease: u64,
    wake: Arc<Notify>,
}

enum Job {
    Hello {
        hello: Hello,
        peer: Option<u32>,
        reply: oneshot::Sender<Result<Bound, &'static str>>,
    },
    Frame {
        lease: u64,
        frame: Frame,
        reply: oneshot::Sender<Result<u64, &'static str>>,
    },
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

/// The kernel facts every commit needs, read now: the peer is still that
/// generation, the pane's process is alive, the pane terminal's foreground
/// group is led by the peer, and the pane's process is the peer's ancestor
/// with every process between them on the pane's terminal. Returns the pane
/// process's birth instant.
fn verified(
    world: &World,
    pane_pid: u32,
    generation: ForegroundGeneration,
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
    Ok((pane.start_seconds, pane.start_micros))
}

/// Admit one hello (module header, "Who may bind").
fn bind(
    world: &World,
    hello: &Hello,
    peer: Option<u32>,
    listing: &mut Listing,
) -> Result<Bound, &'static str> {
    let peer = peer.ok_or("no-peer")?;
    let client = (world.process)(peer)
        .filter(|client| client.start_seconds != 0)
        .ok_or("no-peer")?;
    let generation = ForegroundGeneration::of(peer, (client.start_seconds, client.start_micros));
    // the observer runs inside the terminal's foreground program itself
    if client.tty == 0 || client.pgid != client.pid || client.tty_pgid != client.pid {
        return Err("not-foreground");
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
    let pane_birth = verified(world, row.pane_pid, generation)?;
    // Proven: this pane's foreground program is a Pi observer. That holds
    // for the generation whatever is decided about the stream below.
    if hello.version == PiVersion::Unsupported {
        let coverage = if hello.capabilities.signal {
            PiCoverage::Incompatible
        } else {
            PiCoverage::Unavailable
        };
        crate::agent_status::pi_observed(&row, generation, coverage);
        return Err("unsupported-pi-version");
    }
    if !hello.capabilities.within((world.enabled)()) {
        crate::agent_status::pi_observed(&row, generation, PiCoverage::Unavailable);
        return Err("capability-disabled");
    }
    let wake = Arc::new(Notify::new());
    let lease = with_registry(|registry| {
        if bound(registry) {
            return Err("duplicate-observer");
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
                capabilities: hello.capabilities,
                next_seq: 1,
                run: 0,
                run_open: false,
                over: None,
                wake: wake.clone(),
            },
        );
        let coverage = if hello.capabilities.signal {
            PiCoverage::Connected
        } else {
            PiCoverage::Unavailable
        };
        crate::agent_status::pi_observed(&row, generation, coverage);
        Ok(lease)
    })?;
    applog(&format!(
        "[agent-pi] observer bound s={} signal={} lifecycle={}",
        session_tag(&row.session_name),
        u8::from(hello.capabilities.signal),
        u8::from(hello.capabilities.lifecycle)
    ));
    Ok(Bound { lease, wake })
}

/// Verify and commit one frame of `lease` (module header). A refusal ends
/// the lease here, before the stream's task hears of it.
fn commit(
    world: &World,
    lease: u64,
    frame: &Frame,
    listing: &mut Listing,
) -> Result<u64, &'static str> {
    let verdict = committed(world, lease, frame, listing);
    match (&verdict, frame) {
        (Err(code), _) => end(lease, code),
        // a reset is acknowledged and is the end of what its stream reported
        (Ok(_), Frame::Reset { reason, .. }) => end(lease, reset_code(*reason)),
        _ => {}
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

fn committed(
    world: &World,
    lease: u64,
    frame: &Frame,
    listing: &mut Listing,
) -> Result<u64, &'static str> {
    // what the lease is bound to: nothing is decided on this reading
    let (key, session_id, pane_pid, pane_birth, generation) = with_registry(|registry| {
        registry.live(lease).map(|lease| {
            (
                lease.key.clone(),
                lease.session_id.clone(),
                lease.pane_pid,
                lease.pane_birth,
                lease.generation,
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
    if verified(world, pane_pid, generation)? != pane_birth {
        return Err("pane-changed");
    }
    let enabled = (world.enabled)();
    with_registry(|registry| {
        let entry = registry.live_mut(lease).ok_or("late")?;
        if !entry.capabilities.within(enabled) {
            return Err("capability-disabled");
        }
        let seq = frame.seq();
        if seq != entry.next_seq {
            return Err("bad-seq");
        }
        if let Frame::Signal { run, state, .. } = frame {
            if !entry.capabilities.signal {
                return Err("capability-disabled");
            }
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
            crate::agent_status::pi_signal(rows, row, generation, lease, *run, *state);
        }
        entry.next_seq += 1;
        Ok(seq)
    })
}

fn worker(waiting: Receiver<Job>, world: World) {
    while let Ok(first) = waiting.recv() {
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
            match job {
                Job::Hello { hello, peer, reply } => {
                    let verdict = bind(&world, &hello, peer, &mut listing);
                    let lease = verdict.as_ref().ok().map(|bound| bound.lease);
                    // nobody waits for this stream any more: its lease is not kept
                    if let (Err(_), Some(lease)) = (reply.send(verdict), lease) {
                        release(lease);
                    }
                }
                Job::Frame {
                    lease,
                    frame,
                    reply,
                } => {
                    let _ = reply.send(commit(&world, lease, &frame, &mut listing));
                }
            }
        }
    }
}

// ---------- the service ----------------------------------------------------------------

/// The two time bounds of a stream. Production's are the constants above;
/// a test of what happens when one passes does not wait that long.
#[derive(Clone, Copy)]
pub(crate) struct Bounds {
    pub(crate) answer: Duration,
    pub(crate) stale: Duration,
}

impl Bounds {
    const LIVE: Self = Self {
        answer: ANSWER_BUDGET,
        stale: STALE_AFTER,
    };
}

/// The service: the queue in front of the one admission worker.
pub(crate) struct Service {
    jobs: SyncSender<Job>,
    report: Reporter,
    bounds: Bounds,
}

impl Service {
    /// Start the admission worker. A worker that cannot be started takes
    /// the queue's other end with it: every stream is then refused `busy`.
    pub(crate) fn start(report: Reporter) -> Self {
        Self::start_in(report, World::live(), Bounds::LIVE)
    }

    pub(crate) fn start_in(report: Reporter, world: World, bounds: Bounds) -> Self {
        let (jobs, waiting) = std::sync::mpsc::sync_channel::<Job>(MAX_PENDING);
        let _ = std::thread::Builder::new()
            .name("deck-pi-admission".into())
            .spawn(move || worker(waiting, world));
        Self {
            jobs,
            report,
            bounds,
        }
    }

    /// Take one Pi stream from the status listener. Never waits: a slot is
    /// taken or the stream is refused, the peer is read from the kernel
    /// while the client is certainly connected, and the rest happens in the
    /// stream's own task.
    pub(crate) fn hand(&self, stream: UnixStream, line: String) {
        let peer = crate::procinfo::peer_pid(&stream);
        self.hand_as(stream, line, peer);
    }

    /// `hand` for a peer the caller read from the kernel already.
    pub(crate) fn hand_as(&self, stream: UnixStream, line: String, peer: Option<u32>) {
        let Some(slot) = Slot::take() else {
            (self.report)(super::drop_reason("busy"));
            refuse_now(stream, "busy");
            return;
        };
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let (jobs, report, bounds) = (self.jobs.clone(), self.report.clone(), self.bounds);
        tauri::async_runtime::spawn(async move {
            serve(stream, line, peer, slot, jobs, report, bounds).await;
        });
    }
}

/// A refusal written on the calling thread, which must not wait: the reply
/// fits the socket buffer of a client that has read nothing yet, and a
/// short write timeout covers one that somehow does not take it.
fn refuse_now(mut stream: UnixStream, code: &'static str) {
    use std::io::Write;
    let _ = stream.set_write_timeout(Some(Duration::from_millis(50)));
    let _ = stream.write_all(super::refused_line(code).as_bytes());
}

/// The first of two futures to finish.
enum Either<A, B> {
    First(A),
    Second(B),
}

async fn first<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let mut a = std::pin::pin!(a);
    let mut b = std::pin::pin!(b);
    std::future::poll_fn(|cx| {
        if let Poll::Ready(value) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::First(value));
        }
        if let Poll::Ready(value) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Second(value));
        }
        Poll::Pending
    })
    .await
}

/// Write one line within `budget`. `Err` when it could not be written.
async fn write_line(
    stream: &tokio::net::UnixStream,
    line: &str,
    budget: Duration,
) -> Result<(), ()> {
    let written = async {
        let mut rest = line.as_bytes();
        while !rest.is_empty() {
            stream.writable().await.map_err(|_| ())?;
            match stream.try_write(rest) {
                Ok(0) => return Err(()),
                Ok(count) => rest = &rest[count..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(()),
            }
        }
        Ok(())
    };
    tokio::time::timeout(budget, written)
        .await
        .unwrap_or(Err(()))
}

/// Submit one job and wait for its verdict, no longer than the budget. A
/// full queue and a verdict that did not arrive in time are refusals.
async fn admit<T>(
    jobs: &SyncSender<Job>,
    budget: Duration,
    job: impl FnOnce(oneshot::Sender<Result<T, &'static str>>) -> Job,
) -> Result<T, &'static str> {
    let (reply, verdict) = oneshot::channel();
    jobs.try_send(job(reply)).map_err(|_| "busy")?;
    match tokio::time::timeout(budget, verdict).await {
        Ok(Ok(verdict)) => verdict,
        _ => Err("late"),
    }
}

/// Why reading a stream stopped.
enum Closed {
    /// The client closed its end, or the socket failed.
    Eof,
    /// The stream broke a bound or the wire: it is refused.
    Refused(&'static str),
}

/// Take what the socket holds right now into `pending`, whole lines only.
fn read_lines(
    stream: &tokio::net::UnixStream,
    buffer: &mut Vec<u8>,
    pending: &mut Pending,
) -> Result<(), Closed> {
    let mut chunk = [0u8; 2048];
    loop {
        match stream.try_read(&mut chunk) {
            // The client closed. Whole lines it sent before that are still
            // its frames (a quitting Pi writes its reset and goes): they are
            // answered first, and the next read finds the end again.
            Ok(0) if pending.0.is_empty() => return Err(Closed::Eof),
            Ok(0) => return Ok(()),
            Ok(count) => buffer.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(_) => return Err(Closed::Eof),
        }
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).take(end).collect();
            if line.len() > super::MAX_LINE {
                return Err(Closed::Refused("bad-frame"));
            }
            let line = String::from_utf8(line).map_err(|_| Closed::Refused("bad-frame"))?;
            pending.push(line).map_err(Closed::Refused)?;
        }
        // a line that does not end is bounded like one that does
        if buffer.len() > super::MAX_LINE {
            return Err(Closed::Refused("bad-frame"));
        }
    }
}

/// Releases the lease when the stream's task ends, however it ends.
struct Held(u64);

impl Drop for Held {
    fn drop(&mut self) {
        release(self.0);
    }
}

/// One stream, from its hello to its end.
async fn serve(
    stream: UnixStream,
    hello: String,
    peer: Option<u32>,
    _slot: Slot,
    jobs: SyncSender<Job>,
    report: Reporter,
    bounds: Bounds,
) {
    let Ok(stream) = tokio::net::UnixStream::from_std(stream) else {
        return;
    };
    let refuse = |code: &'static str| {
        report(super::drop_reason(code));
        super::refused_line(code)
    };
    let bound = match super::parse_hello(&hello) {
        Ok(hello) => {
            admit(&jobs, bounds.answer, |reply| Job::Hello {
                hello,
                peer,
                reply,
            })
            .await
        }
        Err(code) => Err(code),
    };
    let bound = match bound {
        Ok(bound) => bound,
        Err(code) => {
            let _ = write_line(&stream, &refuse(code), REFUSAL_BUDGET).await;
            return;
        }
    };
    let lease = Held(bound.lease);
    if write_line(&stream, &super::accepted_line(), bounds.answer)
        .await
        .is_err()
    {
        return;
    }
    let mut buffer = Vec::new();
    let mut pending = Pending::default();
    let mut valid = Instant::now();
    // the client no longer takes answers: what it sent before it left is
    // still committed in order, and then the stream is over
    let mut gone = false;
    let refusal = loop {
        // a lease that ended elsewhere is told why before anything else
        if let Some(code) = over(lease.0) {
            break Some(code);
        }
        if let Some(line) = pending.pop() {
            let frame = match super::parse_frame(&line) {
                Ok(frame) => frame,
                Err(code) => break Some(code),
            };
            let last = matches!(frame, Frame::Reset { .. });
            let seq = match admit(&jobs, bounds.answer, |reply| Job::Frame {
                lease: lease.0,
                frame,
                reply,
            })
            .await
            {
                Ok(seq) => seq,
                Err(code) => break Some(code),
            };
            valid = Instant::now();
            if !gone
                && write_line(&stream, &super::ack_line(seq), bounds.answer)
                    .await
                    .is_err()
            {
                gone = true;
            }
            if last {
                break None;
            }
            continue;
        }
        if gone {
            break None;
        }
        let stale = tokio::time::sleep_until((valid + bounds.stale).into());
        let woken = first(bound.wake.notified(), stale);
        match first(stream.readable(), woken).await {
            Either::First(Ok(())) => match read_lines(&stream, &mut buffer, &mut pending) {
                Ok(()) => {}
                Err(Closed::Eof) => break None,
                Err(Closed::Refused(code)) => break Some(code),
            },
            Either::First(Err(_)) => break None,
            // the lease ended elsewhere: the loop's first question answers
            Either::Second(Either::First(())) => {}
            Either::Second(Either::Second(())) => break Some("stale"),
        }
    };
    if let Some(code) = refusal {
        // ended before the client is told, so nothing more of it commits
        end(lease.0, code);
        // a reset's own ending is not a refusal: its stream is simply over
        if super::REFUSALS.contains(&code) {
            let _ = write_line(&stream, &refuse(code), REFUSAL_BUDGET).await;
        }
    }
}

/// What a test may ask about the service's state.
#[cfg(test)]
pub(crate) mod probe {
    use super::*;

    /// Why each lease ended, in order: what production writes to app.log.
    pub(super) static ENDED: Mutex<Vec<(String, &'static str)>> = Mutex::new(Vec::new());

    /// Forget every lease (the tasks of an earlier test release nothing).
    pub(crate) fn reset() {
        with_registry(|registry| registry.leases.clear());
        ENDED.lock_or_recover().clear();
    }

    /// (session, closed word) of every lease that ended since `reset`.
    pub(crate) fn ended() -> Vec<(String, &'static str)> {
        ENDED.lock_or_recover().clone()
    }

    pub(crate) fn live() -> usize {
        with_registry(|registry| {
            registry
                .leases
                .values()
                .filter(|lease| lease.over.is_none())
                .count()
        })
    }

    pub(crate) fn streams() -> usize {
        STREAMS.load(Ordering::Acquire)
    }

    pub(crate) fn pending() -> usize {
        PENDING.load(Ordering::Acquire)
    }

    /// Every live lease: its number, its session, the highest run it opened
    /// and whether that run is open, and the sequence number it expects next.
    pub(crate) fn leases() -> Vec<(u64, String, u64, bool, u64)> {
        with_registry(|registry| {
            let mut leases: Vec<_> = registry
                .leases
                .iter()
                .filter(|(_, lease)| lease.over.is_none())
                .map(|(id, lease)| {
                    (
                        *id,
                        lease.session.clone(),
                        lease.run,
                        lease.run_open,
                        lease.next_seq,
                    )
                })
                .collect();
            leases.sort();
            leases
        })
    }
}

#[cfg(test)]
mod tests;
