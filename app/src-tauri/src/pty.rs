//! PTY bridge: `tmux attach` inside a portable-pty, bytes streamed to the
//! webview as base64 `pty-data` events. Detach kills only the tmux *client*.
//! Exit events also carry their attachment generation so a queued old exit
//! cannot invalidate a newly attached pane or its read receipt.
//! Reader threads carry a generation counter so a stale thread never removes
//! a newer attachment.
//! Every webview write first passes the MCP control fence: MCP-owned managed
//! panes refuse keyboard/paste bytes until the local user takes control.
//! Every write names the attachment generation its input was bound to;
//! `pty_write` checks it under the same map lock that selects the writer and
//! refuses a replaced attachment (`stale-attachment`), so bytes accepted for
//! one attachment never reach its successor. Missing/null generations are
//! refused too; early frontend input waits for its own attach request.
//!
//! # Contract
//! Flow control is END-TO-END. Every event carries the attachment's
//! generation (`gen`) and a monotonically increasing sequence number (`seq`);
//! the webview ACKs each sequence (`pty_ack`) only after xterm's write
//! callback has actually written the bytes. The emitter never has more than
//! MAX_INFLIGHT_BATCHES (4 × ≤256KB) un-ACKed events outstanding — past that
//! it WAITS on the attachment's AckGate instead of calling app.emit, so a
//! slow or wedged webview stalls the emitter → the bounded channel fills →
//! the reader blocks → the kernel PTY buffer fills → the tmux client stalls,
//! with memory bounded at ~1.5MB per attachment (stalls are logged). Bytes
//! are never dropped or reordered.
//!
//! The gate tracks an emitted HIGH-WATER mark: an ACK is honored only for
//! `acked < seq <= emitted` on an open gate, so a buggy or hostile webview
//! ACKing sequences that were never sent cannot widen the window. A failed
//! app.emit ends the pump and closes the gate (the webview can never ACK an
//! event it never received); sequence overflow ends the stream cleanly.
//!
//! The frontend drops (without ACKing) events whose gen is older than the
//! current attachment and accepts+adopts a NEWER gen (the first event can
//! beat the attach invoke's resolution). A stale generation's gate is closed
//! by the detach/re-attach that replaced it, which is also what releases a
//! waiting emitter.
//!
//! The resize/ACK/detach commands are thin: each delegates to the `PtyState`
//! method of the same name, which the unit test drives over an unspawned pty
//! pair; `pty_write` stays the reviewed input site.
//!
//! # Nothing here waits on a tty or a tmux client on the IPC thread
//! (2026-11-26 freeze investigation, `docs/evidence/deck-freeze-20261126/`).
//! Ordinary commands run on the window's main thread, so:
//! - The master is NON-BLOCKING. A macOS pty master takes only 1022 unread
//!   input bytes; a blocking write past that parked the main thread, holding
//!   the map lock, while the tty was not being read.
//! - `pty_write` checks the generation under the map lock and QUEUES the
//!   bytes on that attachment's own input pump (`PtyInput`), which writes
//!   them in call order, waiting for the tty with `poll`. The queue is
//!   bounded (`INPUT_QUEUE_BYTES`); past it the call is refused whole
//!   (`input-busy`). A pump belongs to one generation: when its attachment
//!   ends, bytes the tty still refuses are dropped and logged, never moved to
//!   a successor and never resent. Ok means "queued for this attachment",
//!   not "delivered" (terminal-input.js already treats an attempt so).
//! - Detach, replacement and `detach_all` remove the entry under the lock
//!   and `retire` it outside: the gate closes, the pump is told to give up,
//!   and a short reaper thread kills and REAPS that attachment's own tmux
//!   client and closes its master — once the client is gone, or 300 ms
//!   after the kill if it is not: a session leader exits only after its
//!   tty's output drained, and nothing reads this master any more (the
//!   kill's own grace is 200 ms). portable-pty's writer is never taken (`master_input`): its
//!   drop blocks on a full tty and types Enter and Ctrl-D into a live one.
//!   Ending an attachment types nothing.
//! - `pty_resize` takes the terminal operation lock, which a command waiting
//!   on tmux may hold, so it runs off the IPC thread under a budget.
//! - Per generation, app.log carries: `attached ... gen=N client started`,
//!   `first read`, `first emit ... (emit Xms)`; the webview adds `pty-first`
//!   when xterm consumed that generation's first frame.
//!
//! # Attach lifecycle (2026-11-27 freeze, `docs/evidence/deck-freeze-20261127/`)
//! `attach_session` is async: openpty, starting this attachment's own tmux
//! client (`pane_client.rs`: its spawn returns once tmux is running, or with
//! exec's error), the entry and its reader/emitter run on a pool
//! thread, and the caller is answered within `ATTACH_BUDGET` whatever
//! process creation does (`attach_within`). Each request has ONE life,
//! every transition under the table lock (`Table`: the attachments and the
//! one attach still in flight per session):
//! - `InFlight` → `Delivered`: its worker's `deliver` inserted it, still
//!   wanted. Only then do its reader and emitter start: nothing of an attach
//!   nobody wanted ever emits (the frontend adopts a NEWER generation from
//!   events, so a late emit would hijack the pane).
//! - `InFlight` → `Abandoned`: the caller's budget ran out first; it is
//!   answered `terminal-timeout` and the request leaves the table.
//! - `InFlight` → `Superseded`: a newer attach of the same session, a
//!   detach, or `detach_all` replaced it (`attach … superseded`).
//!
//! A worker that ends Abandoned or Superseded RETIRES what it made — kills
//! and reaps its own client, closes its master — and inserts nothing,
//! emits nothing, changes no current generation (`late attach … retired`).
//! A worker that delivered before the caller decided makes the caller
//! succeed, with exactly the inserted generation. At most
//! `MAX_ATTACH_WORKERS` workers run (one a stalled spawn holds keeps
//! counting); past that an attach is refused `terminal-busy`. `detach`
//! waits for nothing. Generations are assigned when a request BEGINS, so a
//! request that never delivers only burns a number; input bound to it names
//! a generation that was never current and is refused (`stale-attachment`,
//! never moved to the successor). The activity guard is held for a whole
//! worker: an attach never races a server replacement, and while a stalled
//! spawn holds a worker a restart is refused as busy (as during a delivery).
//! `attached()` does NOT count an attach in flight: its pane shows no
//! terminal yet (see `PtyState::attached`). app.log adds: `attach … still
//! spawning after Nms (attach workers=K)`, `late attach … retired`,
//! `attach … superseded`, `attach … refused`, `no first read from … gen=N
//! within Nms`, `client not reaped … (reapers=N)` / `client reaped after`.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use crate::tmux::{session_target, socket, tmux_conf, tmux_program};

// ---------- ACK window ---------------------------------------------------------

/// Max un-ACKed pty-data events in flight to the webview. With batches
/// coalesced to ≤EMIT_COALESCE_MAX this bounds webview-side queueing at
/// ~1MB per attachment (plus the 512KB reader channel behind it).
pub(crate) const MAX_INFLIGHT_BATCHES: u64 = 4;

#[derive(Default)]
struct AckInner {
    acked: u64,
    /// high-water mark: the last sequence actually handed to the webview.
    /// ACKs are only valid up to here — the window can never be widened by
    /// acknowledging events that were never sent.
    emitted: u64,
    closed: bool,
}

/// The emitter⇄webview flow-control gate for ONE attachment. `ack` and
/// `close` are cheap (their own mutex + condvar, never the PtyState map
/// lock), so acknowledging can never block input, resize or detach.
pub(crate) struct AckGate {
    name: String,
    inner: Mutex<AckInner>,
    cv: Condvar,
}

impl AckGate {
    pub(crate) fn new(name: String) -> Arc<Self> {
        Arc::new(AckGate {
            name,
            inner: Mutex::new(AckInner::default()),
            cv: Condvar::new(),
        })
    }

    /// Frontend confirmed everything up to `seq` was written to xterm.
    /// Valid only when `acked < seq <= emitted` on an open gate: a FUTURE
    /// ack (buggy or hostile webview acking a seq that was never emitted),
    /// a regressed ack, a duplicate, or an ack after close is ignored and
    /// can never expand the in-flight window past MAX_INFLIGHT_BATCHES.
    pub(crate) fn ack(&self, seq: u64) {
        let mut g = self.inner.lock_or_recover();
        if !g.closed && seq > g.acked && seq <= g.emitted {
            g.acked = seq;
            self.cv.notify_all();
        }
    }

    /// Register `seq` as handed to the webview. MUST run before the event
    /// becomes visible to the frontend, so a fast ACK is never rejected as
    /// "future" — pump_gated calls this between admit() and the emit.
    pub(crate) fn mark_emitted(&self, seq: u64) {
        let mut g = self.inner.lock_or_recover();
        if seq > g.emitted {
            g.emitted = seq;
        }
    }

    /// (acked, emitted, closed) — test observability.
    #[cfg(test)]
    pub(crate) fn state(&self) -> (u64, u64, bool) {
        let g = self.inner.lock_or_recover();
        (g.acked, g.emitted, g.closed)
    }

    /// Release any waiting emitter and mark the attachment finished —
    /// called on detach, on replacement by a newer attachment, and is the
    /// answer to "the webview will never ACK again".
    pub(crate) fn close(&self) {
        let mut g = self.inner.lock_or_recover();
        g.closed = true;
        self.cv.notify_all();
    }

    /// Block until `seq` fits in the un-ACKed window (true) or the gate is
    /// closed (false). Logs once per stall episode so a wedged webview is
    /// visible in app.log while memory stays bounded.
    pub(crate) fn admit(&self, seq: u64, window: u64) -> bool {
        let mut g = self.inner.lock_or_recover();
        let mut stalled = false;
        loop {
            if g.closed {
                return false;
            }
            if seq <= g.acked.saturating_add(window) {
                return true;
            }
            if !stalled {
                stalled = true;
                applog(&format!(
                    "[pty] ack stall for {}: seq {seq} waiting on ack {} (webview not consuming)",
                    crate::applog::session_tag(&self.name),
                    g.acked
                ));
            }
            g = crate::sync::wait_or_recover(&self.cv, g);
        }
    }
}

// ---------- PTY bridge (the open session) -------------------------------------

pub(crate) struct PtyEntry {
    input: PtyInput,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
    gate: Arc<AckGate>,
}

impl PtyEntry {
    /// End this attachment off the caller's thread: release a waiting
    /// emitter, tell the input pump to give up on bytes the tty will not
    /// take, and let a short-lived reaper kill and reap this attachment's
    /// own tmux client and then close its master. Nothing here waits on the
    /// tty or on tmux, so detach, replacement and restart never block the
    /// IPC thread (the client's kill grants it up to 200 ms).
    fn retire(self) {
        self.gate.close();
        self.input.close();
        let PtyEntry {
            child,
            master,
            generation,
            gate,
            ..
        } = self;
        reap(&gate.name, generation, child, master);
    }
}

/// Reaper threads whose client has not been reaped yet: a client the kernel
/// holds in an uninterruptible wait keeps its reaper in `wait`, so this
/// count is what shows that in app.log (`client not reaped`).
static REAPING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// How long a reaper waits before it says its client has not gone.
const REAP_NOTICE: Duration = Duration::from_secs(2);
/// How long a killed client may take to go before its master is closed
/// under it (the kill's own grace is 200 ms).
const MASTER_GRACE: Duration = Duration::from_millis(300);

/// Kill and reap one attachment's (or one abandoned attach's) own tmux
/// client on a reaper thread, then close its master. A client that does
/// not go within `REAP_NOTICE` is logged with the number of reapers still
/// waiting; the reaper keeps waiting (a kill cannot be taken back), and the
/// line `client reaped after` closes the episode.
fn reap(
    name: &str,
    generation: u64,
    mut child: Box<dyn Child + Send + Sync>,
    master: Box<dyn MasterPty + Send>,
) {
    use std::sync::atomic::Ordering;
    let tag = crate::applog::session_tag(name);
    REAPING.fetch_add(1, Ordering::AcqRel);
    let spawned = std::thread::Builder::new()
        .name("deck-pty-reap".into())
        .spawn(move || {
            let started = std::time::Instant::now();
            let _ = child.kill();
            let mut noticed = false;
            let mut master = Some(master);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    // a session leader exits only once its tty's output
                    // drained, and nothing reads this master any more:
                    // closing it releases that wait
                    Ok(None) if started.elapsed() >= MASTER_GRACE && master.is_some() => {
                        drop(master.take());
                    }
                    Ok(None) if started.elapsed() >= REAP_NOTICE && !noticed => {
                        noticed = true;
                        applog(&format!(
                            "[pty] client not reaped {tag} gen={generation} after {}ms (reapers={})",
                            started.elapsed().as_millis(),
                            REAPING.load(Ordering::Acquire)
                        ));
                        let _ = child.wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            drop(master);
            REAPING.fetch_sub(1, Ordering::AcqRel);
            if noticed {
                applog(&format!(
                    "[pty] client reaped {tag} gen={generation} after {}ms",
                    started.elapsed().as_millis()
                ));
            }
        });
    if spawned.is_err() {
        REAPING.fetch_sub(1, Ordering::AcqRel);
        applog("[pty] reaper thread unavailable; client left to its own exit");
    }
}

/// Bytes the webview may have waiting for one attachment's tty before
/// `pty_write` refuses more (`input-busy`): a tty that takes nothing for this
/// long is not going to, and the refusal is visible instead of the window
/// freezing (a macOS pty master holds only 1022 unread input bytes).
const INPUT_QUEUE_BYTES: usize = 1024 * 1024;
pub(crate) const INPUT_BUSY: &str = "input-busy";

/// One attachment's ordered input: `pty_write` (on the IPC thread, in call
/// order) only queues; this attachment's own pump thread writes the queue
/// to the master, which is non-blocking, waiting for the tty with `poll`.
/// While the tty takes nothing the pump waits and the queue is bounded; when
/// the attachment ends (`close`) bytes the tty still refuses are dropped and
/// counted, never moved to a successor and never resent.
struct PtyInput {
    tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    closed: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    finished: Arc<std::sync::atomic::AtomicBool>,
}

impl PtyInput {
    fn start(
        name: &str,
        generation: u64,
        mut writer: Box<dyn Write + Send>,
        fd: Option<std::os::fd::RawFd>,
    ) -> Result<PtyInput, DeckError> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(4096);
        let queued = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (q, c, f) = (queued.clone(), closed.clone(), finished.clone());
        let tag = crate::applog::session_tag(name);
        std::thread::Builder::new()
            .name("deck-pty-input".into())
            .spawn(move || {
                let mut stalled: Option<std::time::Instant> = None;
                'chunks: while let Ok(chunk) = rx.recv() {
                    let mut at = 0;
                    while at < chunk.len() {
                        match writer.write(&chunk[at..]) {
                            Ok(0) => break 'chunks,
                            Ok(n) => {
                                at += n;
                                if let Some(since) = stalled.take() {
                                    applog(&format!(
                                        "[pty] input resumed {tag} gen={generation} after {}ms",
                                        since.elapsed().as_millis()
                                    ));
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                if c.load(Ordering::Acquire) {
                                    let left = chunk.len() - at
                                        + q.load(Ordering::Acquire).saturating_sub(chunk.len());
                                    applog(&format!(
                                        "[pty] input dropped {tag} gen={generation}: {left}B never reached the tty (attachment ended)"
                                    ));
                                    break 'chunks;
                                }
                                if stalled.is_none() {
                                    stalled = Some(std::time::Instant::now());
                                    applog(&format!(
                                        "[pty] input stall {tag} gen={generation}: the tty is not taking input ({}B queued)",
                                        q.load(Ordering::Acquire)
                                    ));
                                }
                                wait_writable(fd);
                            }
                            Err(_) => break 'chunks,
                        }
                    }
                    q.fetch_sub(chunk.len(), Ordering::AcqRel);
                }
                // the writer's own drop writes `\n` + VEOF: non-blocking
                // here, and on this thread whatever it does
                drop(writer);
                f.store(true, Ordering::Release);
            })
            .map_err(|_| DeckError::new(ErrorKind::Other, "pty input unavailable"))?;
        Ok(PtyInput {
            tx,
            queued,
            closed,
            #[cfg(test)]
            finished,
        })
    }

    /// Queue `bytes` behind everything queued before them, or refuse them
    /// whole: nothing of a refused call is written.
    fn send(&self, bytes: Vec<u8>) -> Result<(), DeckError> {
        use std::sync::atomic::Ordering;
        let len = bytes.len();
        if self.queued.fetch_add(len, Ordering::AcqRel) + len > INPUT_QUEUE_BYTES {
            self.queued.fetch_sub(len, Ordering::AcqRel);
            return Err(DeckError::new(ErrorKind::Other, INPUT_BUSY));
        }
        match self.tx.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.queued.fetch_sub(len, Ordering::AcqRel);
                Err(match e {
                    std::sync::mpsc::TrySendError::Full(_) => {
                        DeckError::new(ErrorKind::Other, INPUT_BUSY)
                    }
                    std::sync::mpsc::TrySendError::Disconnected(_) => {
                        DeckError::new(ErrorKind::Other, "not attached")
                    }
                })
            }
        }
    }

    fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

/// The pump's own handle on the master: a `dup` whose drop only closes it.
/// portable-pty's `take_writer` is never used: its writer's drop writes
/// `\n` and VEOF into the tty, which an attached tmux client hands to the pane as
/// Enter and Ctrl-D, so ending an attachment would end the user's shell
/// (seen 2026-11-26 in the candidate before this: a card switch retired an
/// idle zsh as a verified normal exit). Ending an attachment types nothing.
fn master_input(fd: std::os::fd::RawFd) -> Result<std::fs::File, DeckError> {
    master_dup(fd)
}

/// A descriptor of the master owned by one thread: closed exactly once, by
/// its own drop, and never a number another owner may close and reuse.
fn master_dup(fd: std::os::fd::RawFd) -> Result<std::fs::File, DeckError> {
    use std::os::fd::FromRawFd;
    // SAFETY: dup of this attachment's open master.
    let own = unsafe { libc::dup(fd) };
    if own < 0 {
        return Err(DeckError::new(ErrorKind::Other, "pty unavailable"));
    }
    // SAFETY: `own` is a fresh descriptor no one else owns.
    Ok(unsafe { std::fs::File::from_raw_fd(own) })
}

/// Wait (at most 100 ms, so a closed attachment is noticed) until the master
/// can take input. Without a descriptor (tests' sinks), just pause.
fn wait_writable(fd: Option<std::os::fd::RawFd>) {
    match fd {
        Some(fd) => {
            let mut p = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: one valid pollfd; the descriptor outlives this call
            // (the master is closed only by the reaper, after `close`).
            unsafe { libc::poll(&mut p, 1, 100) };
        }
        None => std::thread::sleep(Duration::from_millis(10)),
    }
}

/// Wait until the master has output to read (or 1 s passed).
fn wait_readable(fd: std::os::fd::RawFd) {
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid pollfd for the reader's own descriptor.
    unsafe { libc::poll(&mut p, 1, 1000) };
}

/// The master is shared by the reader, the input pump and resize; it is
/// made non-blocking once so that no thread, the IPC thread least of all,
/// can be parked in a write the tty will not take.
fn set_nonblocking(fd: std::os::fd::RawFd) -> Result<(), DeckError> {
    // SAFETY: fcntl on this attachment's own open master descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(DeckError::new(ErrorKind::Other, "pty unavailable"));
    }
    Ok(())
}

/// The attachments, and the one attach still in flight per session, under
/// ONE lock: whether a finished attach is still wanted, and every change
/// that makes it unwanted (a newer attach, a detach, `detach_all`), are
/// decided under the same lock. Derefs to the attachments.
#[derive(Default)]
pub(crate) struct Table {
    entries: HashMap<String, PtyEntry>,
    pending: HashMap<String, Arc<Attempt>>,
}

impl std::ops::Deref for Table {
    type Target = HashMap<String, PtyEntry>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for Table {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

/// Where one attach request is in its life. Every transition happens under
/// the table lock; a request leaves `InFlight` exactly once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    /// its worker is still opening the pty / starting the client
    InFlight,
    /// its attachment is the one in the table under its generation
    Delivered,
    /// its caller stopped waiting (`terminal-timeout`): retired when it ends
    Abandoned,
    /// a newer attach, a detach or `detach_all` replaced it: retired
    Superseded,
}

/// One attach request: its generation is assigned when it begins, so a
/// request that never delivers burns a number and nothing else.
pub(crate) struct Attempt {
    generation: u64,
    phase: Mutex<Phase>,
}

impl Attempt {
    pub(crate) fn phase(&self) -> Phase {
        *self.phase.lock_or_recover()
    }
}

/// Past this many attach workers started and not yet returned (answered or
/// not: one held by a stalled process creation keeps counting), a new
/// attach is refused `terminal-busy` (`PtyState::workers`).
pub(crate) const MAX_ATTACH_WORKERS: usize = 4;
/// The caller's budget (limits.json `terminal_waits.attach_budget_ms`).
/// app.log 2026-11-27: attach → first read p50 6 ms, p99 32 ms, max 212 ms
/// over 321 attaches. 10 s (5 s in the first draft, raised 2026-11-27 at
/// the user's request): on a Mac with an endpoint-security agent every exec
/// is taxed and a shell can be slow to start, and a timeout there would be
/// a false failure; the pane still says "Connecting terminal…" after 1 s.
pub(crate) const ATTACH_BUDGET: Duration = Duration::from_secs(10);
/// A delivered attachment with no first read after this long is logged
/// once (`no first read`; limits.json `terminal_waits.first_frame_wait_ms`,
/// the pane's own "no picture yet" notice).
pub(crate) const FIRST_READ_WAIT: Duration = Duration::from_secs(3);
pub(crate) const ATTACH_SUPERSEDED: &str = "attach-superseded";

#[derive(Default)]
pub(crate) struct PtyState {
    map: Mutex<Table>,
    counter: Mutex<u64>,
    /// attach workers started and not yet returned
    workers: Arc<std::sync::atomic::AtomicUsize>,
}

impl PtyState {
    /// The sessions a pane shows right now: each visible pane holds one
    /// attach here, and nothing else does (the Board's query client is not
    /// in this map). An attach still in flight is NOT counted: its pane
    /// shows no terminal yet, so a bell then is not one a person saw, and
    /// marking it seen would drop a notice nobody read (`bell.rs`). The
    /// frontend's `hasPane`, which guards every automatic close, counts the
    /// pane from its creation, whatever its attach is doing.
    pub(crate) fn attached(&self) -> std::collections::HashSet<String> {
        self.map.lock_or_recover().keys().cloned().collect()
    }

    /// Detach every GUI tmux client before an intentional server restart.
    /// The tmux sessions still belong to the server until the lifecycle
    /// transaction kills it; this only prevents stale PTY-exit events and
    /// releases all ACK waiters. Every attach still in flight is superseded:
    /// whatever it starts later is retired, never inserted.
    pub(crate) fn detach_all(&self) {
        let entries: Vec<PtyEntry> = {
            let mut map = self.map.lock_or_recover();
            for (name, attempt) in map.pending.drain() {
                supersede(&name, &attempt);
            }
            map.entries.drain().map(|(_, e)| e).collect()
        };
        for entry in entries {
            entry.retire();
        }
    }

    /// Begin one attach of `name`: refuse past the worker bound, supersede
    /// the session's attach still in flight, retire its current attachment
    /// (exactly what a re-attach always did), and record this one as the
    /// one wanted.
    pub(crate) fn begin_attach(&self, name: &str) -> Result<Arc<Attempt>, DeckError> {
        use std::sync::atomic::Ordering;
        let running = self.workers.load(Ordering::Acquire);
        if running >= MAX_ATTACH_WORKERS {
            applog(&format!(
                "[pty] attach {} refused: {running} attach workers still running",
                crate::applog::session_tag(name)
            ));
            return Err(DeckError::new(
                ErrorKind::Tmux,
                crate::session_runtime::TERMINAL_BUSY,
            ));
        }
        let generation = {
            let mut c = self.counter.lock_or_recover();
            *c += 1;
            *c
        };
        let attempt = Arc::new(Attempt {
            generation,
            phase: Mutex::new(Phase::InFlight),
        });
        let previous = {
            let mut map = self.map.lock_or_recover();
            if let Some(old) = map.pending.insert(name.to_string(), attempt.clone()) {
                supersede(name, &old);
            }
            map.entries.remove(name)
        };
        // replacing an attachment closes its gate, which releases an emitter
        // waiting on ACKs that will never come
        if let Some(old) = previous {
            old.retire();
        }
        Ok(attempt)
    }

    /// The worker's attachment is ready: insert it if this attempt is still
    /// the one wanted, else hand it back to be retired. Never emits.
    fn deliver(&self, name: &str, attempt: &Arc<Attempt>, entry: PtyEntry) -> Result<(), PtyEntry> {
        let mut map = self.map.lock_or_recover();
        let wanted = map
            .pending
            .get(name)
            .is_some_and(|current| Arc::ptr_eq(current, attempt));
        let mut phase = attempt.phase.lock_or_recover();
        if !wanted || *phase != Phase::InFlight {
            return Err(entry);
        }
        map.pending.remove(name);
        *phase = Phase::Delivered;
        map.entries.insert(name.to_string(), entry);
        Ok(())
    }

    /// The caller's budget ran out: abandon the attempt unless it was
    /// delivered (or replaced) first. Returns the phase it ends in.
    pub(crate) fn abandon(&self, name: &str, attempt: &Arc<Attempt>) -> Phase {
        let mut map = self.map.lock_or_recover();
        let mut phase = attempt.phase.lock_or_recover();
        if *phase == Phase::InFlight {
            *phase = Phase::Abandoned;
            if map
                .pending
                .get(name)
                .is_some_and(|current| Arc::ptr_eq(current, attempt))
            {
                map.pending.remove(name);
            }
        }
        *phase
    }
}

/// Smoke evidence only (`smoke_faults::smoke_attach_state`): whether
/// `name` has a current attachment and an attach in flight, how many attach
/// workers have not returned, and how many reapers have not reaped.
pub(crate) fn smoke_counts(state: &PtyState, name: &str) -> (bool, bool, usize, usize) {
    use std::sync::atomic::Ordering;
    let map = state.map.lock_or_recover();
    (
        map.entries.contains_key(name),
        map.pending.contains_key(name),
        state.workers.load(Ordering::Acquire),
        REAPING.load(Ordering::Acquire),
    )
}

/// Mark a replaced attempt (under the table lock) and say so once.
fn supersede(name: &str, attempt: &Arc<Attempt>) {
    let mut phase = attempt.phase.lock_or_recover();
    if *phase == Phase::InFlight {
        *phase = Phase::Superseded;
        applog(&format!(
            "[pty] attach {} gen={} superseded",
            crate::applog::session_tag(name),
            attempt.generation
        ));
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyData {
    name: String,
    gen: u64,
    seq: u64,
    data: String, // base64
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyExit {
    name: String,
    gen: u64,
}

/// Attach = subscribe to the session's byte stream. The tmux session keeps
/// running whether or not anyone is attached; detach just closes the stream.
/// Off the IPC thread (module header, "Attach lifecycle"): the caller is
/// answered within `ATTACH_BUDGET` whatever process creation does.
#[tauri::command]
pub(crate) async fn attach_session(
    app: AppHandle,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<u64, DeckError> {
    crate::tmux::validate_session_name(&name)?;
    let state = app.state::<PtyState>();
    attach_within(&state, &name, ATTACH_BUDGET, {
        let app = app.clone();
        let name = name.clone();
        move |attempt| attach_now(&app, &name, cols, rows, &attempt)
    })
    .await
}

/// The attach lifecycle around `work` (the blocking part, run on a pool
/// thread): begin, wait at most `budget`, then decide under the table lock.
/// Split from the command so tests drive it with a work they control.
pub(crate) async fn attach_within(
    state: &PtyState,
    name: &str,
    budget: Duration,
    work: impl FnOnce(Arc<Attempt>) -> Result<u64, DeckError> + Send + 'static,
) -> Result<u64, DeckError> {
    use std::sync::atomic::Ordering;
    let attempt = state.begin_attach(name)?;
    let started = std::time::Instant::now();
    let workers = state.workers.clone();
    workers.fetch_add(1, Ordering::AcqRel);
    let mut task = tauri::async_runtime::spawn_blocking({
        let attempt = attempt.clone();
        let workers = workers.clone();
        move || {
            // counted down however the work ends, a panic included
            let _counted = crate::sync::CountDown(&workers);
            work(attempt)
        }
    });
    let tag = crate::applog::session_tag(name);
    match tokio::time::timeout(budget, &mut task).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(DeckError::new(ErrorKind::Tmux, "terminal-worker-failed")),
        Err(_) => match state.abandon(name, &attempt) {
            // delivered in the race with the budget: the caller is told the
            // truth, and the generation it is told is the inserted one
            Phase::Delivered => Ok(attempt.generation),
            Phase::Superseded => Err(DeckError::new(ErrorKind::ContextChanged, ATTACH_SUPERSEDED)),
            Phase::Abandoned | Phase::InFlight => {
                applog(&format!(
                    "[pty] attach {tag} gen={} still spawning after {}ms (attach workers={})",
                    attempt.generation,
                    started.elapsed().as_millis(),
                    workers.load(Ordering::Acquire)
                ));
                Err(DeckError::new(
                    ErrorKind::Tmux,
                    crate::session_runtime::TERMINAL_TIMEOUT,
                ))
            }
        },
    }
}

/// The blocking part of one attach, on its worker: open a pty, start this
/// attachment's own tmux client (`pane_client.rs`), and offer the result to
/// the table. A result nobody wants any more (abandoned, superseded) is
/// retired here — its client killed and reaped, its master closed — and is
/// never inserted and never emits. The activity guard is held for the
/// whole worker: an attach never races a server replacement, and while a
/// worker is held by a stalled process creation, a restart is refused as
/// busy, exactly as it is during a delivery (module header).
fn attach_now(
    app: &AppHandle,
    name: &str,
    cols: u16,
    rows: u16,
    attempt: &Arc<Attempt>,
) -> Result<u64, DeckError> {
    let _activity = crate::session_runtime::activity_guard()?;
    crate::smoke_faults::stall_point(crate::smoke_faults::ATTACH_SPAWN);
    let state = app.state::<PtyState>();
    let generation = attempt.generation;
    let tag = crate::applog::session_tag(name);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| DeckError::classified(e.to_string()))?;
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "pty unavailable"))?;
    let tty = crate::pane_client::slave_path(master_fd)?;
    let conf = tmux_conf();
    let program = tmux_program()?;
    let target = session_target(name);
    let mut args = vec![
        crate::tmux_clients::UTF8_CLIENT,
        "-f",
        &conf,
        "-L",
        socket(),
    ];
    args.extend(crate::tmux_clients::pane_client_args(&target));
    let child = crate::pane_client::spawn(
        std::path::Path::new(program),
        &args,
        &[("TERM", "xterm-256color"), ("LANG", "en_US.UTF-8")],
        &tty,
    )
    .inspect_err(|error| {
        // the raw error (may embed the tmux path) goes to the caller only
        applog(&format!(
            "[pty] attach spawn failed for {tag} ({})",
            error.code()
        ));
    })?;
    // the client holds the slave now; ours is not needed
    drop(pair.slave);
    let child: Box<dyn Child + Send + Sync> = Box::new(child);
    let attached_at = std::time::Instant::now();

    let started = (|| {
        set_nonblocking(master_fd)?;
        // the reader and the pump each poll their OWN descriptor: the
        // master's own number is closed by the reaper and may be reused
        let reader = master_dup(master_fd)?;
        let writer = master_input(master_fd)?;
        let write_fd = writer.as_raw_fd();
        Ok::<_, DeckError>((
            reader,
            PtyInput::start(name, generation, Box::new(writer), Some(write_fd))?,
        ))
    })();
    let (reader, input) = match started {
        Ok(parts) => parts,
        Err(error) => {
            reap(name, generation, child, pair.master);
            return Err(error);
        }
    };
    let gate = AckGate::new(name.to_string());
    let entry = PtyEntry {
        input,
        master: pair.master,
        child,
        generation,
        gate: gate.clone(),
    };
    offer(&state, name, attempt, entry)?;
    // the tmux client is running (the spawn returned after its exec);
    // that is all this line says
    applog(&format!(
        "[pty] attached {tag} gen={generation} ({cols}x{rows}) client started"
    ));
    stream(app, name, generation, reader, gate, attached_at);
    Ok(generation)
}

/// Offer a finished attach to the table: inserted if still wanted, else
/// retired here (its client killed and reaped, its master closed) and
/// answered as what replaced it. Never emits.
fn offer(
    state: &PtyState,
    name: &str,
    attempt: &Arc<Attempt>,
    entry: PtyEntry,
) -> Result<(), DeckError> {
    let Err(unwanted) = state.deliver(name, attempt, entry) else {
        return Ok(());
    };
    let phase = attempt.phase();
    applog(&format!(
        "[pty] late attach {} gen={} retired ({})",
        crate::applog::session_tag(name),
        attempt.generation,
        match phase {
            Phase::Abandoned => "abandoned",
            _ => "superseded",
        }
    ));
    unwanted.retire();
    Err(match phase {
        Phase::Abandoned => {
            DeckError::new(ErrorKind::Tmux, crate::session_runtime::TERMINAL_TIMEOUT)
        }
        _ => DeckError::new(ErrorKind::ContextChanged, ATTACH_SUPERSEDED),
    })
}

/// Pipeline of one DELIVERED attachment: reader → bounded sync channel →
/// coalescing, ACK-gated emitter (see module docs). The channel caps
/// reader→emitter in-flight data at PTY_CHANNEL_CHUNKS × 8KB; the emitter
/// drains whatever accumulated into ONE pty-data event (up to
/// EMIT_COALESCE_MAX) so a fast producer yields few large events, and never
/// runs more than MAX_INFLIGHT_BATCHES ahead of the webview's ACKs. Started
/// only after `deliver`: nothing of an attach nobody wanted ever emits.
fn stream(
    app: &AppHandle,
    name: &str,
    generation: u64,
    mut reader: std::fs::File,
    gate: Arc<AckGate>,
    attached_at: std::time::Instant,
) {
    const PTY_CHANNEL_CHUNKS: usize = 64; // × 8KB = 512KB bound
    const EMIT_COALESCE_MAX: usize = 256 * 1024;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(PTY_CHANNEL_CHUNKS);
    let fd = reader.as_raw_fd();
    let read_tag = crate::applog::session_tag(name);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        let mut first = true;
        let mut quiet_noted = false;
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if first {
                        first = false;
                        applog(&format!(
                            "[pty] first read {n}B from {read_tag} gen={generation} after {}ms",
                            attached_at.elapsed().as_millis()
                        ));
                    }
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break; // emitter gone
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if first && !quiet_noted && attached_at.elapsed() >= FIRST_READ_WAIT {
                        quiet_noted = true;
                        applog(&format!(
                            "[pty] no first read from {read_tag} gen={generation} within {}ms",
                            FIRST_READ_WAIT.as_millis()
                        ));
                    }
                    wait_readable(fd)
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        // dropping tx ends the emitter's recv loop
    });

    let thread_app = app.clone();
    let thread_name = name.to_string();
    std::thread::spawn(move || {
        let mut emits: u64 = 0;
        pump_gated(
            &rx,
            EMIT_COALESCE_MAX,
            &gate,
            MAX_INFLIGHT_BATCHES,
            |seq, batch| {
                emits += 1;
                let emit_started = std::time::Instant::now();
                let r = thread_app.emit(
                    "pty-data",
                    PtyData {
                        name: thread_name.clone(),
                        gen: generation,
                        seq,
                        data: B64.encode(&batch),
                    },
                );
                if emits == 1 {
                    // time from attach to the first byte the webview sees:
                    // for a fresh pane this is the login shell's startup. The
                    // frontend's `pty-rx` is the liveness trace after that.
                    applog(&format!(
                        "[pty] first emit {}B to {} gen={generation} ok={:?} after {}ms (emit {}ms)",
                        batch.len(),
                        crate::applog::session_tag(&thread_name),
                        r.is_ok(),
                        attached_at.elapsed().as_millis(),
                        emit_started.elapsed().as_millis()
                    ));
                }
                // a failed emit ends the pump (see pump_gated) — the webview
                // can never ACK an event it never received
                r.map_err(|e| DeckError::classified(e.to_string()))
            },
        );
        // clean up only if this attachment is still the current one
        let state = thread_app.state::<PtyState>();
        let mut map = state.map.lock_or_recover();
        if map.get(&thread_name).map(|e| e.generation) == Some(generation) {
            map.remove(&thread_name);
            drop(map);
            applog(&format!(
                "[pty] stream ended for {}",
                crate::applog::session_tag(&thread_name)
            ));
            let _ = thread_app.emit(
                "pty-exit",
                PtyExit {
                    name: thread_name,
                    gen: generation,
                },
            );
        }
    });
}

/// Drain the channel into as-large-as-available batches: blocking recv for
/// the first chunk, then wait across a short idle gap while a terminal repaint
/// is still arriving. tmux full-frame redraws commonly cross the reader's 8KB
/// boundary; emitting the clear-prefix separately lets xterm visibly paint a
/// half-frame and looks like flicker during selection and scrolling. The 2ms
/// burst window keeps one repaint atomic without perceptible input latency.
/// Each batch gets the next sequence number and is
/// emitted only once the gate admits it (≤`window` un-ACKed); the sequence
/// is registered as emitted (high-water) BEFORE the emit so the webview's
/// ACK is always valid. Returns when the sender is dropped (stream end),
/// the gate closes (detach/replace), or an emit FAILS — a webview that
/// never received an event can never ACK it, so continuing would deadlock;
/// the failure closes the gate (which also lets the reader thread die via
/// the dropped channel) and logs a classified, content-free diagnostic.
/// Order-preserving, nothing dropped, nothing emitted past the window.
pub(crate) fn pump_gated<F: FnMut(u64, Vec<u8>) -> Result<(), DeckError>>(
    rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    max: usize,
    gate: &AckGate,
    window: u64,
    mut emit: F,
) {
    const BURST_IDLE: Duration = Duration::from_millis(2);
    let mut seq: u64 = 0;
    while let Ok(first) = rx.recv() {
        let mut batch = first;
        while batch.len() < max {
            match rx.recv_timeout(BURST_IDLE) {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        // explicit overflow handling — 2^64 batches is unreachable in any
        // real stream, but wrap-around must end the stream cleanly rather
        // than rely on a debug-build panic (or silently reuse seq 0)
        seq = match seq.checked_add(1) {
            Some(s) => s,
            None => {
                gate.close();
                return;
            }
        };
        if !gate.admit(seq, window) {
            return; // closed: this attachment is over, drop the tail
        }
        gate.mark_emitted(seq);
        if let Err(e) = emit(seq, batch) {
            applog(&format!("[pty] emit failed ({}) — ending stream", e.code()));
            gate.close();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bound_entry, pty_write, pump_gated, AckGate, PtyEntry, PtyState, B64, MAX_INFLIGHT_BATCHES,
        STALE_ATTACHMENT,
    };
    use crate::error::{DeckError, ErrorKind};
    use crate::sync::LockRecover;
    use base64::Engine;
    use portable_pty::MasterPty;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A gate that ACKs everything instantly = pure coalescing behavior.
    fn open_gate() -> Arc<AckGate> {
        AckGate::new("t".into())
    }
    fn pump_free<F: FnMut(u64, Vec<u8>)>(
        rx: &std::sync::mpsc::Receiver<Vec<u8>>,
        max: usize,
        mut emit: F,
    ) {
        let gate = open_gate();
        pump_gated(rx, max, &gate, u64::MAX, |s, b| {
            emit(s, b);
            Ok(())
        });
    }

    #[test]
    fn coalesces_queued_chunks_into_one_emit() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for i in 0u8..10 {
            tx.send(vec![i; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<usize> = Vec::new();
        pump_free(&rx, 1 << 20, |_, b| emits.push(b.len()));
        assert_eq!(emits, vec![1000], "10 queued chunks → one emit");
    }

    #[test]
    fn respects_coalesce_ceiling_and_sequences_batches() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for _ in 0..10 {
            tx.send(vec![0u8; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<(u64, usize)> = Vec::new();
        pump_free(&rx, 250, |s, b| emits.push((s, b.len())));
        // ceiling is checked before each drain, so batches stop growing at
        // the first size ≥ max — bounded, order-preserving, nothing lost
        assert_eq!(emits.iter().map(|&(_, n)| n).sum::<usize>(), 1000);
        assert!(emits.len() > 1, "ceiling must split the stream: {emits:?}");
        assert!(emits.iter().all(|&(_, n)| n <= 300), "{emits:?}");
        let seqs: Vec<u64> = emits.iter().map(|&(s, _)| s).collect();
        assert_eq!(seqs, (1..=emits.len() as u64).collect::<Vec<_>>());
    }

    #[test]
    fn ends_when_sender_drops() {
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        drop(tx);
        let mut called = false;
        pump_free(&rx, 1024, |_, _| called = true);
        assert!(!called);
    }

    #[test]
    fn slow_consumer_keeps_inflight_bounded_and_ack_resumes() {
        let gate = open_gate();
        let acked = Arc::new(AtomicU64::new(0));
        let max_inflight = Arc::new(AtomicU64::new(0));
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        let g2 = gate.clone();
        let (acked2, maxi2) = (acked.clone(), max_inflight.clone());
        let consumer_gate = gate.clone();
        let h = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            pump_gated(&rx, 512, &g2, MAX_INFLIGHT_BATCHES, |seq, b| {
                let inflight = seq - acked2.load(Ordering::SeqCst);
                maxi2.fetch_max(inflight, Ordering::SeqCst);
                out.extend_from_slice(&b);
                // consumer ACKs ~5ms behind, from "another thread" timing-wise
                let a = acked2.clone();
                let cg = consumer_gate.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(5));
                    a.fetch_max(seq, Ordering::SeqCst); // delayed threads may race
                    cg.ack(seq);
                });
                Ok(())
            });
            out
        });
        let payload: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
        for chunk in payload.chunks(300) {
            tx.send(chunk.to_vec()).unwrap();
        }
        drop(tx);
        let out = h.join().unwrap();
        assert_eq!(out, payload, "no loss, no reorder");
        assert!(
            max_inflight.load(Ordering::SeqCst) <= MAX_INFLIGHT_BATCHES,
            "window respected: {}",
            max_inflight.load(Ordering::SeqCst)
        );
        assert!(
            acked.load(Ordering::SeqCst) > 0,
            "acks actually resumed the pump"
        );
    }

    #[test]
    fn no_ack_stalls_after_window_memory_stays_bounded() {
        let gate = open_gate();
        let emitted = Arc::new(AtomicU64::new(0));
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        let (g2, e2) = (gate.clone(), emitted.clone());
        let h = std::thread::spawn(move || {
            pump_gated(&rx, 64, &g2, MAX_INFLIGHT_BATCHES, |_, _| {
                e2.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        });
        // feed far more than the window; consumer never ACKs
        let feeder = std::thread::spawn(move || {
            for _ in 0..100 {
                if tx.send(vec![0u8; 64]).is_err() {
                    break;
                }
            }
            // keep tx alive long enough that "pump ended" can't explain the stop
            std::thread::sleep(Duration::from_millis(300));
        });
        std::thread::sleep(Duration::from_millis(200));
        let n = emitted.load(Ordering::SeqCst);
        assert!(
            n <= MAX_INFLIGHT_BATCHES,
            "emitter must stop at the window without ACKs, emitted {n}"
        );
        // detach path: closing the gate releases the stalled emitter promptly
        let t0 = Instant::now();
        gate.close();
        feeder.join().unwrap();
        h.join().unwrap();
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "close() must unblock the emitter"
        );
    }

    #[test]
    fn close_releases_a_waiting_admit() {
        let gate = open_gate();
        let g2 = gate.clone();
        let h = std::thread::spawn(move || g2.admit(10, 2)); // way past the window
        std::thread::sleep(Duration::from_millis(50));
        gate.close();
        assert!(
            !h.join().unwrap(),
            "closed admit reports the stream is over"
        );
    }

    #[test]
    fn stale_generation_gate_ignores_acks_after_close() {
        // replacement closed the old gate; a late ACK routed to it (or a
        // direct call) must not resurrect anything
        let gate = open_gate();
        gate.close();
        gate.ack(99);
        assert!(!gate.admit(1, MAX_INFLIGHT_BATCHES), "closed stays closed");
    }

    #[test]
    fn ack_regression_and_duplicates_are_ignored() {
        let gate = open_gate();
        gate.mark_emitted(5);
        gate.ack(5);
        assert_eq!(gate.state().0, 5);
        gate.ack(3); // out-of-order ACK must not move the window backwards
        assert_eq!(gate.state().0, 5);
        gate.ack(5); // duplicate: no change
        assert_eq!(gate.state().0, 5);
        assert!(gate.admit(5 + MAX_INFLIGHT_BATCHES, MAX_INFLIGHT_BATCHES));
    }

    /// The round-3 exploit this closes: a buggy/hostile webview ACKing a
    /// gigantic seq used to open the window permanently. Now an ACK is only
    /// valid up to the emitted high-water mark.
    #[test]
    fn future_ack_never_widens_the_window() {
        let gate = open_gate();
        gate.mark_emitted(2); // two events actually sent
        gate.ack(u64::MAX); // hostile ACK far past anything emitted
        gate.ack(3); // even one-past-emitted is refused
        assert_eq!(gate.state().0, 0, "future ACKs ignored entirely");
        // the emitter therefore still stalls exactly at the window
        let g2 = gate.clone();
        let h =
            std::thread::spawn(move || g2.admit(MAX_INFLIGHT_BATCHES + 1, MAX_INFLIGHT_BATCHES));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!h.is_finished(), "window must NOT have been widened");
        gate.close();
        assert!(!h.join().unwrap());
    }

    /// An emit failure must end the pump: the webview never received the
    /// event, so it can never ACK it — waiting would deadlock forever.
    #[test]
    fn emit_failure_ends_pump_and_closes_gate() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(8);
        for _ in 0..8 {
            tx.send(vec![0u8; 16]).unwrap();
        }
        // keep the sender alive: "input exhausted" cannot explain the return
        let mut calls = 0u32;
        pump_gated(&rx, 16, &gate, MAX_INFLIGHT_BATCHES, |_, _| {
            calls += 1;
            Err(DeckError::classified("event bus broken"))
        });
        assert_eq!(calls, 1, "pump stops at the first failed emit");
        assert!(gate.state().2, "gate closed so nothing can wait forever");
        drop(tx);
    }

    /// mark_emitted runs before the emit, so an ACK arriving DURING the
    /// emit callback (the realistic fast-webview race) is always valid.
    #[test]
    fn ack_during_emit_is_valid() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        tx.send(vec![1u8; 8]).unwrap();
        drop(tx);
        let g = gate.clone();
        pump_gated(&rx, 64, &gate, MAX_INFLIGHT_BATCHES, |seq, _| {
            g.ack(seq); // webview ACKs while emit is still on the stack
            Ok(())
        });
        assert_eq!(gate.state(), (1, 1, false));
    }

    /// Stress: 8MB through the full pump with an ACKing consumer thread.
    /// Asserts integrity + boundedness and logs throughput for the record.
    #[test]
    fn stress_throughput_with_acking_consumer() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        const TOTAL: usize = 8 * 1024 * 1024;
        let feeder = std::thread::spawn(move || {
            let chunk: Vec<u8> = (0..8192u32).map(|i| (i % 249) as u8).collect();
            let mut sent = 0;
            while sent < TOTAL {
                tx.send(chunk.clone()).unwrap();
                sent += chunk.len();
            }
            sent
        });
        let g2 = gate.clone();
        let t0 = Instant::now();
        let mut bytes = 0usize;
        let mut batches = 0u64;
        let mut max_batch = 0usize;
        pump_gated(&rx, 256 * 1024, &g2, MAX_INFLIGHT_BATCHES, |seq, b| {
            bytes += b.len();
            batches += 1;
            max_batch = max_batch.max(b.len());
            gate.ack(seq); // consumer keeps up (valid: emitted was marked)
            Ok(())
        });
        let sent = feeder.join().unwrap();
        let el = t0.elapsed();
        assert_eq!(bytes, sent, "every byte accounted for");
        assert!(max_batch <= 256 * 1024 + 8192, "coalesce ceiling held");
        // 8MB should stream in well under a second on any dev machine; the
        // bound is generous so CI noise can't flake it, but a deadlock or
        // per-batch sleep would blow straight past it
        assert!(el < Duration::from_secs(10), "throughput collapsed: {el:?}");
        println!(
            "stress: {}MB in {:?} ({} batches, max {}KB)",
            bytes / (1024 * 1024),
            el,
            batches,
            max_batch / 1024
        );
    }

    /// A pane client that records whether it was killed; nothing is spawned.
    #[derive(Debug)]
    struct RecordingChild(Arc<std::sync::atomic::AtomicBool>);

    impl portable_pty::ChildKiller for RecordingChild {
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(RecordingChild(self.0.clone()))
        }
    }

    impl portable_pty::Child for RecordingChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
    }

    /// The webview's input bytes land in a shared sink instead of a tty.
    struct SharedSink(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedSink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock_or_recover().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// One attachment over an unspawned pty pair: `attached` is the flag the
    /// entry's client sets when killed.
    type Attachment = (
        Arc<std::sync::Mutex<Vec<u8>>>,
        Arc<std::sync::atomic::AtomicBool>,
        Arc<AckGate>,
        Box<dyn portable_pty::SlavePty + Send>,
    );

    fn attach(state: &PtyState, name: &str, generation: u64) -> Attachment {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("pty pair");
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gate = AckGate::new(name.into());
        state.map.lock_or_recover().insert(
            name.into(),
            PtyEntry {
                input: super::PtyInput::start(
                    name,
                    generation,
                    Box::new(SharedSink(sink.clone())),
                    None,
                )
                .expect("input pump"),
                master: pair.master,
                child: Box::new(RecordingChild(killed.clone())),
                generation,
                gate: gate.clone(),
            },
        );
        (sink, killed, gate, pair.slave)
    }

    /// The slave side of a real kernel pty, read by this test: it records
    /// the bytes it receives and executes nothing.
    fn slave_reader(master: &(dyn MasterPty + Send)) -> std::fs::File {
        let fd = master.as_raw_fd().expect("master fd");
        // SAFETY: `fd` is this test's open pty master; ptsname returns a
        // NUL-terminated name in static storage, copied before any other call.
        let path = unsafe {
            let name = libc::ptsname(fd);
            assert!(!name.is_null(), "slave tty name");
            std::ffi::CStr::from_ptr(name)
                .to_string_lossy()
                .into_owned()
        };
        std::fs::File::open(path).expect("open slave tty")
    }

    fn read_line(reader: &mut std::io::BufReader<std::fs::File>) -> String {
        use std::io::BufRead;
        let mut line = String::new();
        reader.read_line(&mut line).expect("slave read");
        line
    }

    /// One real attachment-shaped entry: the production writer of a real pty
    /// master, under `generation`, with nothing spawned on the slave.
    fn attach_real(
        state: &PtyState,
        name: &str,
        generation: u64,
    ) -> (
        std::io::BufReader<std::fs::File>,
        Box<dyn portable_pty::SlavePty + Send>,
    ) {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("pty pair");
        let reader = std::io::BufReader::new(slave_reader(&*pair.master));
        let fd = pair.master.as_raw_fd().expect("master fd");
        super::set_nonblocking(fd).expect("non-blocking master");
        let writer = super::master_input(fd).expect("pty writer");
        let fd = std::os::fd::AsRawFd::as_raw_fd(&writer);
        let writer: Box<dyn std::io::Write + Send> = Box::new(writer);
        let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        state.map.lock_or_recover().insert(
            name.into(),
            PtyEntry {
                input: super::PtyInput::start(name, generation, writer, Some(fd))
                    .expect("input pump"),
                master: pair.master,
                child: Box::new(RecordingChild(killed)),
                generation,
                gate: AckGate::new(name.into()),
            },
        );
        (reader, pair.slave)
    }

    /// The `pty_write` command over real kernel ptys: fixed test bytes reach
    /// the slave of the attachment they were bound to, in call order; bytes
    /// bound to a replaced generation are refused and reach neither the
    /// replacement nor another session. The receiver is this test reading
    /// the slave tty (canonical mode, so one line per read); no tmux, shell
    /// or other program is involved.
    #[test]
    fn pty_write_reaches_only_the_bound_attachment_in_call_order() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(PtyState::default());
        let state = || app.state::<PtyState>();
        let write = |name: &str, gen: Option<u64>, text: &str| {
            pty_write(state(), name.into(), gen, B64.encode(text))
        };
        let (mut a, _slave_a) = attach_real(&state(), "deck-pty-real-a", 7);
        let (mut b, _slave_b) = attach_real(&state(), "deck-pty-real-b", 8);

        for (gen, text) in [
            (Some(7), "a-1 \u{4e2d}\u{6587}\n"),
            (Some(7), "a-2\n"),
            (Some(7), "a-3\n"),
        ] {
            write("deck-pty-real-a", gen, text).expect("current attachment");
        }
        write("deck-pty-real-b", Some(8), "b-1\n").expect("current attachment");
        assert_eq!(read_line(&mut a), "a-1 \u{4e2d}\u{6587}\n");
        assert_eq!(read_line(&mut a), "a-2\n");
        assert_eq!(read_line(&mut a), "a-3\n");
        assert_eq!(read_line(&mut b), "b-1\n");

        // The attachment is replaced after the webview bound its input to
        // generation 7 and sent the call: the old identity is refused.
        state().detach("deck-pty-real-a");
        let (mut replacement, _slave_c) = attach_real(&state(), "deck-pty-real-a", 9);
        let refused = write("deck-pty-real-a", Some(7), "stale\n").unwrap_err();
        assert_eq!(refused.message(), STALE_ATTACHMENT);
        assert_eq!(refused.kind(), ErrorKind::ContextChanged);
        assert_eq!(
            write("deck-pty-real-a", None, "unbound\n")
                .unwrap_err()
                .message(),
            STALE_ATTACHMENT
        );
        write("deck-pty-real-a", Some(9), "fresh\n").expect("replacement");
        assert_eq!(
            read_line(&mut replacement),
            "fresh\n",
            "the replacement received only what was bound to it"
        );
        write("deck-pty-real-b", Some(8), "b-2\n").expect("untouched session");
        assert_eq!(read_line(&mut b), "b-2\n");

        state().detach("deck-pty-real-a");
        assert_eq!(
            write("deck-pty-real-a", Some(9), "gone\n")
                .unwrap_err()
                .message(),
            "not attached"
        );
    }

    /// Missing, null and stale generations are refused over the real IPC
    /// argument decoder; only bytes bound to the current attachment arrive.
    #[test]
    fn pty_write_accepts_the_webview_argument_shapes_over_ipc() {
        use tauri::ipc::{CallbackFn, InvokeBody};
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
        use tauri::webview::InvokeRequest;
        use tauri::Manager;

        let app = mock_builder()
            .manage(PtyState::default())
            .invoke_handler(tauri::generate_handler![super::pty_write])
            .build(mock_context(noop_assets()))
            .expect("mock app");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock webview");
        let (mut slave, _keep) = attach_real(&app.state::<PtyState>(), "deck-pty-ipc", 11);
        let call = |body: serde_json::Value| {
            get_ipc_response(
                &webview,
                InvokeRequest {
                    cmd: "pty_write".into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: "tauri://localhost".parse().expect("url"),
                    body: InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_string(),
                },
            )
        };
        let data = |text: &str| B64.encode(text);
        for body in [
            serde_json::json!({"name": "deck-pty-ipc", "gen": null, "dataB64": data("unbound\n")}),
            serde_json::json!({"name": "deck-pty-ipc", "dataB64": data("missing\n")}),
            serde_json::json!({"name": "deck-pty-ipc", "gen": 10, "dataB64": data("stale\n")}),
        ] {
            assert_eq!(
                call(body).expect_err("no current generation"),
                serde_json::json!(STALE_ATTACHMENT)
            );
        }
        call(serde_json::json!({"name": "deck-pty-ipc", "gen": 11, "dataB64": data("bound\n")}))
            .expect("current generation");
        assert_eq!(
            read_line(&mut slave),
            "bound\n",
            "refused input delivered no bytes"
        );
    }

    /// The generation is compared on the entry the write is about to use.
    #[test]
    fn bound_entry_refuses_a_replaced_generation() {
        let state = PtyState::default();
        let (_sink, _killed, _gate, _slave) = attach(&state, "deck-pty-unit-bound", 3);
        let mut map = state.map.lock_or_recover();
        assert!(bound_entry(&mut map, "deck-pty-unit-bound", Some(3)).is_ok());
        assert_eq!(
            bound_entry(&mut map, "deck-pty-unit-bound", None)
                .err()
                .expect("missing generation")
                .message(),
            STALE_ATTACHMENT
        );
        for stale in [2, 4] {
            let refused = bound_entry(&mut map, "deck-pty-unit-bound", Some(stale))
                .err()
                .expect("a generation that is not the entry's");
            assert_eq!(refused.message(), STALE_ATTACHMENT);
        }
        assert_eq!(
            bound_entry(&mut map, "deck-pty-unit-none", Some(3))
                .err()
                .expect("no entry")
                .message(),
            "not attached"
        );
    }

    /// Resize, ACK and detach each reach exactly the named attachment
    /// under its own generation; detaching kills only that pane client and
    /// releases its gate, and the restart path drains every attachment.
    #[test]
    fn attachment_commands_reach_only_the_named_entry_and_detach_releases_it() {
        let state = PtyState::default();
        let (sink, killed, gate, _slave) = attach(&state, "deck-pty-unit-a", 3);
        state
            .map
            .lock_or_recover()
            .get_mut("deck-pty-unit-a")
            .expect("attached")
            .input
            .send(b"ls\r".to_vec())
            .unwrap();
        assert!(
            eventually(|| *sink.lock_or_recover() == b"ls\r"),
            "the entry's own pump wrote its input"
        );
        state.resize("deck-pty-unit-a", 100, 30).unwrap();
        let size = state.map.lock_or_recover()["deck-pty-unit-a"]
            .master
            .get_size()
            .expect("kernel winsize");
        assert_eq!((size.cols, size.rows), (100, 30));
        assert_eq!(
            state
                .resize("deck-pty-unit-none", 1, 1)
                .unwrap_err()
                .message(),
            "not attached"
        );

        gate.mark_emitted(2);
        state.ack("deck-pty-unit-a", 2, 2);
        assert_eq!(gate.state().0, 0, "a stale generation's ACK is dropped");
        state.ack("deck-pty-unit-a", 3, 2);
        assert_eq!(gate.state().0, 2);

        state.detach("deck-pty-unit-none");
        assert!(!killed.load(Ordering::SeqCst));
        state.detach("deck-pty-unit-a");
        assert!(
            eventually(|| killed.load(Ordering::SeqCst)),
            "detach kills the pane client"
        );
        assert!(gate.state().2, "detach releases a waiting emitter");
        assert!(state.map.lock_or_recover().is_empty());
        state.ack("deck-pty-unit-a", 3, 2);

        let (_, killed_b, gate_b, _slave_b) = attach(&state, "deck-pty-unit-b", 4);
        let (_, killed_c, gate_c, _slave_c) = attach(&state, "deck-pty-unit-c", 5);
        state.detach_all();
        assert!(eventually(
            || killed_b.load(Ordering::SeqCst) && killed_c.load(Ordering::SeqCst)
        ));
        assert!(gate_b.state().2 && gate_c.state().2);
        assert!(state.map.lock_or_recover().is_empty());
    }
    /// One attachment-shaped entry for `offer`: its input lands in `sink`
    /// and `killed` says whether its client was killed.
    /// An attachment-shaped entry, its input sink, its client's killed
    /// flag, and the slave it keeps alive.
    type FakeEntry = (
        PtyEntry,
        Arc<std::sync::Mutex<Vec<u8>>>,
        Arc<std::sync::atomic::AtomicBool>,
        Box<dyn portable_pty::SlavePty + Send>,
    );

    fn fake_entry(name: &str, generation: u64) -> FakeEntry {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("pty pair");
        let sink = Arc::new(std::sync::Mutex::new(Vec::new()));
        let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let entry = PtyEntry {
            input: super::PtyInput::start(
                name,
                generation,
                Box::new(SharedSink(sink.clone())),
                None,
            )
            .expect("input pump"),
            master: pair.master,
            child: Box::new(RecordingChild(killed.clone())),
            generation,
            gate: AckGate::new(name.into()),
        };
        (entry, sink, killed, pair.slave)
    }

    fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    /// An attach whose worker outlives the budget: the caller is answered
    /// `terminal-timeout` in time; what the worker finishes later is retired
    /// — never inserted, its own client killed — and the session's current
    /// attachment and generation are unchanged.
    #[test]
    fn a_late_attach_is_retired_and_never_becomes_current() {
        let state = Arc::new(PtyState::default());
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (done_tx, done) = std::sync::mpsc::channel();
        let worker_state = state.clone();
        let begin = Instant::now();
        let answer = block_on(super::attach_within(
            &state,
            "deck-pty-late",
            Duration::from_millis(100),
            move |attempt| {
                let _ = held.recv(); // the stalled spawn
                let (entry, _sink, killed, _slave) =
                    fake_entry("deck-pty-late", attempt.generation);
                let offered = super::offer(&worker_state, "deck-pty-late", &attempt, entry);
                done_tx
                    .send((offered.map_err(|e| e.message().to_string()), killed))
                    .unwrap();
                Err(DeckError::new(ErrorKind::Other, "unused"))
            },
        ));
        assert_eq!(
            answer.unwrap_err().message(),
            crate::session_runtime::TERMINAL_TIMEOUT
        );
        assert!(begin.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        let (offered, killed) = done
            .recv_timeout(Duration::from_secs(2))
            .expect("worker ended");
        assert_eq!(
            offered.unwrap_err(),
            crate::session_runtime::TERMINAL_TIMEOUT
        );
        assert!(
            eventually(|| killed.load(Ordering::SeqCst)),
            "its client was killed"
        );
        assert!(state.attached().is_empty(), "nothing was inserted");
        assert!(state.map.lock_or_recover().pending.is_empty());
        assert!(eventually(|| state.workers.load(Ordering::SeqCst) == 0));
    }

    /// A newer attach of the same session, a detach, and `detach_all` each
    /// supersede the attach still in flight; whatever it finishes later is
    /// retired, and a newer attach that delivers is the one current.
    #[test]
    fn newer_attach_detach_and_detach_all_supersede_an_attach_in_flight() {
        let state = PtyState::default();
        // a newer attach of the same session
        let old = state.begin_attach("deck-pty-sup").unwrap();
        let new = state.begin_attach("deck-pty-sup").unwrap();
        assert_eq!(old.phase(), super::Phase::Superseded);
        let (entry, _s, old_killed, _a) = fake_entry("deck-pty-sup", old.generation);
        assert_eq!(
            super::offer(&state, "deck-pty-sup", &old, entry)
                .unwrap_err()
                .message(),
            super::ATTACH_SUPERSEDED
        );
        assert!(eventually(|| old_killed.load(Ordering::SeqCst)));
        let (entry, _s, _k, _b) = fake_entry("deck-pty-sup", new.generation);
        super::offer(&state, "deck-pty-sup", &new, entry).expect("the newest is wanted");
        assert_eq!(new.phase(), super::Phase::Delivered);
        assert_eq!(
            state.map.lock_or_recover()["deck-pty-sup"].generation,
            new.generation
        );
        assert!(new.generation > old.generation);

        // a detach supersedes the one in flight
        let pending = state.begin_attach("deck-pty-det").unwrap();
        state.detach("deck-pty-det");
        assert_eq!(pending.phase(), super::Phase::Superseded);
        let (entry, _s, killed, _c) = fake_entry("deck-pty-det", pending.generation);
        assert!(super::offer(&state, "deck-pty-det", &pending, entry).is_err());
        assert!(eventually(|| killed.load(Ordering::SeqCst)));
        assert!(!state.attached().contains("deck-pty-det"));

        // detach_all supersedes every one in flight and drains the table
        let a = state.begin_attach("deck-pty-all-a").unwrap();
        let b = state.begin_attach("deck-pty-all-b").unwrap();
        state.detach_all();
        assert_eq!(a.phase(), super::Phase::Superseded);
        assert_eq!(b.phase(), super::Phase::Superseded);
        assert!(
            state.attached().is_empty(),
            "the delivered one was drained too"
        );
        let (entry, _s, _k, _d) = fake_entry("deck-pty-all-a", a.generation);
        assert!(super::offer(&state, "deck-pty-all-a", &a, entry).is_err());
        assert!(state.attached().is_empty());
    }

    /// The race between delivery and the budget: a worker that delivered
    /// before the caller decided makes the caller succeed, with exactly the
    /// generation that was inserted.
    #[test]
    fn a_delivery_that_beats_the_timeout_decision_is_a_success() {
        let state = Arc::new(PtyState::default());
        let worker_state = state.clone();
        let answer = block_on(super::attach_within(
            &state,
            "deck-pty-race",
            Duration::from_millis(100),
            move |attempt| {
                let (entry, _sink, _killed, slave) =
                    fake_entry("deck-pty-race", attempt.generation);
                super::offer(&worker_state, "deck-pty-race", &attempt, entry).unwrap();
                // delivered; the worker's own answer comes after the budget
                std::thread::sleep(Duration::from_millis(300));
                drop(slave);
                Ok(attempt.generation)
            },
        ));
        let generation = answer.expect("delivered before the decision");
        assert_eq!(
            state.map.lock_or_recover()["deck-pty-race"].generation,
            generation,
            "the caller was told the inserted attachment"
        );
        state.detach("deck-pty-race");
    }

    /// Attach workers are bounded: while `MAX_ATTACH_WORKERS` are held by a
    /// stalled spawn, the next attach is refused `terminal-busy` at once.
    #[test]
    fn stuck_attach_workers_are_bounded() {
        let state = Arc::new(PtyState::default());
        let (release, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(std::sync::Mutex::new(held));
        for i in 0..super::MAX_ATTACH_WORKERS {
            let held = held.clone();
            let answer = block_on(super::attach_within(
                &state,
                &format!("deck-pty-cap-{i}"),
                Duration::from_millis(30),
                move |_| {
                    let _ = held.lock_or_recover().recv();
                    Err(DeckError::new(ErrorKind::Other, "unused"))
                },
            ));
            assert_eq!(
                answer.unwrap_err().message(),
                crate::session_runtime::TERMINAL_TIMEOUT
            );
        }
        let begin = Instant::now();
        let refused = block_on(super::attach_within(
            &state,
            "deck-pty-cap-next",
            Duration::from_secs(5),
            |_| Ok(0),
        ));
        assert_eq!(
            refused.unwrap_err().message(),
            crate::session_runtime::TERMINAL_BUSY
        );
        assert!(begin.elapsed() < Duration::from_millis(100));
        for _ in 0..super::MAX_ATTACH_WORKERS {
            release.send(()).unwrap();
        }
        assert!(eventually(|| state.workers.load(Ordering::SeqCst) == 0));
        assert!(block_on(super::attach_within(
            &state,
            "deck-pty-cap-next",
            Duration::from_secs(1),
            |_| { Err(DeckError::new(ErrorKind::Other, "ran")) }
        ))
        .is_err_and(|e| e.message() == "ran"));
    }

    /// Input bound to an attach that timed out names a generation that was
    /// never inserted: it is refused, and the successor that did deliver
    /// receives only what was bound to it.
    #[test]
    fn input_bound_to_a_timed_out_attach_never_reaches_its_successor() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(PtyState::default());
        let state = || app.state::<PtyState>();
        let timed_out = state().begin_attach("deck-pty-bound").unwrap();
        assert_eq!(
            state().abandon("deck-pty-bound", &timed_out),
            super::Phase::Abandoned
        );
        let next = state().begin_attach("deck-pty-bound").unwrap();
        let (entry, sink, _killed, _slave) = fake_entry("deck-pty-bound", next.generation);
        super::offer(&state(), "deck-pty-bound", &next, entry).unwrap();
        let refused = pty_write(
            state(),
            "deck-pty-bound".into(),
            Some(timed_out.generation),
            B64.encode("typed while connecting\n"),
        )
        .unwrap_err();
        assert_eq!(refused.message(), STALE_ATTACHMENT);
        pty_write(
            state(),
            "deck-pty-bound".into(),
            Some(next.generation),
            B64.encode("ok\n"),
        )
        .expect("bound to the successor");
        assert!(eventually(|| *sink.lock_or_recover() == b"ok\n"));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            *sink.lock_or_recover(),
            b"ok\n",
            "nothing of the timed-out one arrived"
        );
        state().detach("deck-pty-bound");
    }

    /// A pane client whose reaping waits for the test: the reaper keeps the
    /// master open meanwhile (closing the last master flushes the tty's
    /// input queue, which would hide what was typed into it).
    #[derive(Debug)]
    struct HeldChild(Arc<std::sync::Mutex<std::sync::mpsc::Receiver<()>>>);

    impl portable_pty::ChildKiller for HeldChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(HeldChild(self.0.clone()))
        }
    }

    impl portable_pty::Child for HeldChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }
        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            let _ = self
                .0
                .lock_or_recover()
                .recv_timeout(Duration::from_secs(5));
            Ok(portable_pty::ExitStatus::with_exit_code(0))
        }
        fn process_id(&self) -> Option<u32> {
            None
        }
    }

    /// Ending an attachment types nothing into its tty: no `\n`, no VEOF.
    /// portable-pty's writer drop sends both; a live tmux client hands them
    /// to the pane as Enter + Ctrl-D and an idle shell exits (the candidate
    /// before `master_input` retired a zsh card on a card switch).
    #[test]
    fn ending_an_attachment_writes_nothing_to_the_tty() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(PtyState::default());
        let state = || app.state::<PtyState>();
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("pty pair");
        let slave = slave_reader(&*pair.master);
        make_raw(&slave);
        let fd = pair.master.as_raw_fd().expect("master fd");
        super::set_nonblocking(fd).expect("non-blocking");
        let writer = super::master_input(fd).unwrap();
        let write_fd = std::os::fd::AsRawFd::as_raw_fd(&writer);
        let input = super::PtyInput::start("deck-pty-quiet", 41, Box::new(writer), Some(write_fd))
            .expect("pump");
        let finished = input.finished.clone();
        let (release, held) = std::sync::mpsc::channel();
        state().map.lock_or_recover().insert(
            "deck-pty-quiet".into(),
            PtyEntry {
                input,
                master: pair.master,
                child: Box::new(HeldChild(Arc::new(std::sync::Mutex::new(held)))),
                generation: 41,
                gate: AckGate::new("deck-pty-quiet".into()),
            },
        );
        state().detach("deck-pty-quiet");
        assert!(
            eventually(|| finished.load(Ordering::SeqCst)),
            "the pump ended"
        );
        let sfd = std::os::fd::AsRawFd::as_raw_fd(&slave);
        let mut queued: libc::c_int = 0;
        // SAFETY: FIONREAD on this test's own open slave descriptor.
        assert_eq!(unsafe { libc::ioctl(sfd, libc::FIONREAD, &mut queued) }, 0);
        let _ = release.send(());
        assert_eq!(queued, 0, "bytes were typed into the tty on detach");
    }

    /// Poll `done` for up to 2 s: work handed to a pump or reaper thread.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(2);
        while Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        done()
    }

    /// Raw mode, as tmux sets an attached client's tty: input is queued
    /// (and blocks the master when full) instead of being line-edited, and
    /// a canonical tty's silent discard past MAX_INPUT cannot hide a stall.
    fn make_raw(slave: &std::fs::File) {
        // SAFETY: termios calls on this test's own open slave descriptor.
        unsafe {
            let fd = std::os::fd::AsRawFd::as_raw_fd(slave);
            let mut tio: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut tio), 0);
            libc::cfmakeraw(&mut tio);
            assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &tio), 0);
        }
    }

    /// Fill a pty's input queue the way a tty nobody reads fills: write
    /// through a non-blocking master until the kernel refuses more.
    fn fill_input(master: &(dyn MasterPty + Send)) -> usize {
        let fd = master.as_raw_fd().expect("master fd");
        super::set_nonblocking(fd).expect("non-blocking");
        let mut n = 0;
        loop {
            // SAFETY: writing one byte from a live buffer to the test's master.
            let w = unsafe { libc::write(fd, b"a".as_ptr().cast(), 1) };
            if w <= 0 {
                break n;
            }
            n += 1;
        }
    }

    /// The hazard behind the 2026-11-26 investigation, pinned on the library
    /// itself: portable-pty's writer, dropped while the tty's input queue is
    /// full, blocks in its drop (`\n` + VEOF written with `write_all`). The
    /// old detach/replace dropped it on the IPC (main) thread, holding the
    /// PTY map lock.
    #[test]
    fn portable_pty_writer_drop_blocks_while_the_tty_takes_no_input() {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("pty pair");
        let writer = pair.master.take_writer().expect("writer");
        // raw slave: a read returns what is queued, no line needed
        let mut slave = slave_reader(&*pair.master);
        make_raw(&slave);
        assert!(fill_input(&*pair.master) > 0);
        let fd = pair.master.as_raw_fd().expect("master fd");
        // back to the blocking master the old code used
        // SAFETY: fcntl on this test's own master descriptor.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
        }
        let (done_tx, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(writer);
            let _ = done_tx.send(());
        });
        assert!(
            done.recv_timeout(Duration::from_millis(500)).is_err(),
            "the drop is parked in write(2) while nothing reads the tty"
        );
        // reading the slave releases it: the wait was the full queue
        let mut buf = [0u8; 4096];
        let _ = std::io::Read::read(&mut slave, &mut buf);
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
    }

    /// `pty_write` never waits for the tty: with nothing reading it, every
    /// call returns at once, the bounded queue refuses the excess whole
    /// (`input-busy`), and detaching ends the pump promptly instead of
    /// parking anyone in write(2) — the IPC thread least of all.
    #[test]
    fn pty_write_returns_at_once_when_the_tty_takes_nothing() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(PtyState::default());
        let state = || app.state::<PtyState>();
        let (unread, _slave) = attach_real(&state(), "deck-pty-full", 21);
        make_raw(unread.get_ref());
        let finished = state().map.lock_or_recover()["deck-pty-full"]
            .input
            .finished
            .clone();
        let chunk = "x".repeat(16 * 1024);
        let mut busy = 0;
        let mut slowest = Duration::ZERO;
        for _ in 0..80 {
            let t0 = Instant::now();
            match pty_write(
                state(),
                "deck-pty-full".into(),
                Some(21),
                B64.encode(&chunk),
            ) {
                Ok(()) => {}
                Err(e) => {
                    assert_eq!(e.message(), super::INPUT_BUSY);
                    busy += 1;
                }
            }
            slowest = slowest.max(t0.elapsed());
        }
        assert!(
            slowest < Duration::from_millis(100),
            "a write waited {slowest:?}"
        );
        assert!(
            busy > 0,
            "1.25 MiB against a tty that takes nothing is refused past the bound"
        );
        let t0 = Instant::now();
        state().detach("deck-pty-full");
        assert!(t0.elapsed() < Duration::from_millis(100), "detach waited");
        assert!(
            eventually(|| finished.load(Ordering::SeqCst)),
            "the pump ended"
        );
    }

    /// Input larger than the tty takes at once is delivered whole and in
    /// call order once the reader catches up; nothing is reordered by the
    /// pump's waiting.
    #[test]
    fn queued_input_arrives_in_order_after_the_tty_drains() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        app.manage(PtyState::default());
        let state = || app.state::<PtyState>();
        let (mut slave, _keep) = attach_real(&state(), "deck-pty-order", 31);
        for i in 0..400 {
            pty_write(
                state(),
                "deck-pty-order".into(),
                Some(31),
                B64.encode(format!("line-{i:04}\n")),
            )
            .expect("queued");
        }
        for i in 0..400 {
            assert_eq!(read_line(&mut slave), format!("line-{i:04}\n"));
        }
        state().detach("deck-pty-order");
    }
}

#[tauri::command]
pub(crate) fn pty_write(
    state: State<'_, PtyState>,
    name: String,
    gen: Option<u64>,
    data_b64: String,
) -> Result<(), DeckError> {
    {
        // The fence lists tmux panes only when the MCP ledger is unreadable;
        // on this (IPC) thread that listing is bounded, and a listing that
        // does not answer refuses the input (the fence fails closed).
        let _deadline = crate::session_runtime::Deadline::until(
            std::time::Instant::now() + Duration::from_secs(1),
        );
        crate::mcp::guard_terminal_input(&name)?;
    }
    let _activity = crate::session_runtime::activity_guard()?;
    let bytes = B64
        .decode(data_b64)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    let mut map = state.map.lock_or_recover();
    let entry = bound_entry(&mut map, &name, gen)?;
    // queued behind this attachment's earlier input, under the same lock
    // that proved the generation; never written here (see `PtyInput`)
    entry.input.send(bytes)
}

/// The attachment a write is for, chosen under the map lock the write itself
/// holds. `gen` is the generation the webview bound the input to when it
/// accepted it; a different current generation means that attachment was
/// replaced, and the bytes are refused instead of reaching its successor.
/// `None` is never a target: early frontend input waits for its own attach
/// reply, and every caller must supply that exact generation.
fn bound_entry<'a>(
    map: &'a mut HashMap<String, PtyEntry>,
    name: &str,
    gen: Option<u64>,
) -> Result<&'a mut PtyEntry, DeckError> {
    let entry = map
        .get_mut(name)
        .ok_or(DeckError::new(ErrorKind::Other, "not attached"))?;
    if gen == Some(entry.generation) {
        Ok(entry)
    } else {
        Err(DeckError::new(ErrorKind::ContextChanged, STALE_ATTACHMENT))
    }
}

/// Wire message of a write refused for a replaced attachment (layout.js
/// logs it as `pty-write-cancel backend`).
pub(crate) const STALE_ATTACHMENT: &str = "stale-attachment";

#[tauri::command]
pub(crate) async fn pty_resize(
    app: AppHandle,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), DeckError> {
    // tmux reflows the pane synchronously when the PTY changes size. Keep
    // that reflow out of the status -> capture row -> cursor movement window
    // used by terminal selection commands. That lock can be held by a
    // command waiting on tmux, so the wait happens off the IPC thread and
    // ends with the budget (`terminal-busy`), like the commands it orders.
    crate::session_runtime::off_ipc_thread("pty_resize", Duration::from_secs(2), move || {
        let _selection_operation =
            crate::terminal::terminal_selection_operation_lock().acquire()?;
        app.state::<PtyState>().resize(&name, cols, rows)
    })
    .await
}

/// Webview confirmation that everything up to `seq` of attachment `gen`
/// was written into xterm. Holds the map lock only to clone the gate Arc —
/// the actual wake-up runs on the gate's own mutex, so this can never block
/// pty_write / pty_resize / detach.
#[tauri::command]
pub(crate) fn pty_ack(state: State<'_, PtyState>, name: String, gen: u64, seq: u64) {
    state.ack(&name, gen, seq);
}

#[tauri::command]
pub(crate) fn detach_session(state: State<'_, PtyState>, name: String) {
    state.detach(&name);
}

/// The attachment table behind the resize, ACK and detach commands above,
/// keyed by session name (`pty_write` keeps its own write, the reviewed
/// terminal-input site). Each method touches exactly one entry (or none)
/// and is what the unit test drives with an unspawned pty pair.
impl PtyState {
    fn resize(&self, name: &str, cols: u16, rows: u16) -> Result<(), DeckError> {
        let map = self.map.lock_or_recover();
        let entry = map
            .get(name)
            .ok_or(DeckError::new(ErrorKind::Other, "not attached"))?;
        entry
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| DeckError::classified(e.to_string()))
    }

    fn ack(&self, name: &str, gen: u64, seq: u64) {
        let gate = {
            let map = self.map.lock_or_recover();
            match map.get(name) {
                Some(e) if e.generation == gen => Some(e.gate.clone()),
                _ => None, // stale generation / already detached: its gate was closed
            }
        };
        if let Some(g) = gate {
            g.ack(seq);
        }
    }

    /// Fast and waits for nothing: the attachment leaves the table, the
    /// session's attach still in flight is superseded (whatever it starts
    /// later is retired), and the reaper does the rest.
    fn detach(&self, name: &str) {
        let entry = {
            let mut map = self.map.lock_or_recover();
            if let Some(attempt) = map.pending.remove(name) {
                supersede(name, &attempt);
            }
            map.entries.remove(name)
        };
        if let Some(entry) = entry {
            entry.retire();
        }
    }
}
