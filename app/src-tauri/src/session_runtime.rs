//! Shared session-operation coordination, below tmux and restart policy.
//! Activity excludes intentional server replacement; workers keep their guards
//! until all transport/cleanup/finalization ends. Deadline scopes belong to
//! the current OS thread only. Spawned workers must install their own scope
//! (bounded_worker does, and run_bounded through it); scopes cannot be moved
//! across threads or async tasks. A scope decides nothing by itself: a call
//! that cannot be interrupted is bounded for its CALLER only by a worker the
//! caller stops waiting for, and for its RESULT only by asking the scope
//! after the call returned.
//! `off_ipc_thread` is how a terminal IPC command leaves the window's main
//! thread: it installs the deadline on the pool thread that does the work.
//! `off_ipc_answer` bounds only the caller's answer, for work whose own
//! calls must not be cut (a send's firing window). Which commands use
//! which, and which stay on the IPC thread, is `docs/ipc-thread.md`
//! (enforced by `tests/ipc_thread.rs`).
//! This module knows no panes, agents, snapshots, scheduler or restart policy.
//! Existing IPC error codes remain stable for callers.
use crate::applog::applog;
use crate::error::{DeckError, ErrorKind, RestartFailure};
use std::cell::Cell;
use std::io::Read;
use std::marker::PhantomData;
use std::os::fd::AsRawFd;
use std::process::{Command, Output, Stdio};
use std::rc::Rc;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

static ACTIVITY: RwLock<()> = RwLock::new(());
thread_local! { static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) }; }

pub(crate) struct Deadline(Option<Instant>, PhantomData<Rc<()>>);
impl Deadline {
    pub(crate) fn until(end: Instant) -> Self {
        Self(
            DEADLINE.with(|d| {
                let previous = d.get();
                d.set(Some(previous.map_or(end, |old| old.min(end))));
                previous
            }),
            PhantomData,
        )
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        DEADLINE.with(|d| d.set(self.0));
    }
}
pub(crate) fn check_deadline() -> Result<(), DeckError> {
    if DEADLINE.with(|d| d.get().is_some_and(|end| Instant::now() >= end)) {
        Err(DeckError::restart(RestartFailure::Deadline))
    } else {
        Ok(())
    }
}
pub(crate) fn deadline_active() -> bool {
    DEADLINE.with(|deadline| deadline.get().is_some())
}
/// What is left of this thread's deadline scope; `None` without one.
pub(crate) fn deadline_remaining() -> Option<Duration> {
    DEADLINE
        .with(Cell::get)
        .map(|end| end.saturating_duration_since(Instant::now()))
}
/// A deadline no test reaches: for tests of code that asks for a deadline
/// owner and whose subject is not the deadline.
#[cfg(test)]
pub(crate) fn test_phase() -> Deadline {
    Deadline::until(Instant::now() + Duration::from_secs(300))
}
fn error(code: &str) -> DeckError {
    DeckError::new(ErrorKind::Tmux, code)
}

/// Why a bounded worker gave its caller no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unanswered {
    /// `end` passed first. The worker is still running and keeps everything
    /// it holds; whatever it returns later reaches nobody.
    Late,
    /// The worker could not be started, or ended without an answer.
    Failed,
}

/// Run `work` on its own thread, under a deadline scope that ends at `end`,
/// and wait for its answer no longer than until `end`. A call inside `work`
/// that cannot be interrupted is not interrupted: the caller is answered
/// `Late`, the worker goes on owning what it took, and its eventual answer
/// is dropped with the channel. One call starts at most one thread.
pub(crate) fn bounded_worker<T: Send + 'static>(
    name: &'static str,
    end: Instant,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Unanswered> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _deadline = Deadline::until(end);
            let _ = tx.send(work());
        })
        .map_err(|_| Unanswered::Failed)?;
    match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        Ok(answer) => Ok(answer),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(Unanswered::Late),
        Err(_) => Err(Unanswered::Failed),
    }
}

/// Bound the IPC response even if local filesystem IO is stalled. The worker
/// keeps its guards until that IO returns; its absolute deadline then prevents
/// it from proceeding to kill-server. We never detach a timed-out destructive
/// operation from its identity/creation locks.
pub(crate) fn run_bounded<T: Send + 'static>(
    end: Instant,
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    match bounded_worker("deck-shell-restart", end, move || {
        check_deadline().and_then(|_| work())
    }) {
        Ok(result) => result,
        Err(Unanswered::Late) => {
            applog("[tmux-restart] watchdog-timeout worker-retains-guards");
            Err(DeckError::restart(RestartFailure::Deadline))
        }
        Err(Unanswered::Failed) => Err(error("tmux-restart-worker-failed")),
    }
}

/// A webview's IPC is answered on the window's main thread: an ordinary
/// (non-async) Tauri command runs there, inside WebKit's URL-scheme callback.
/// A terminal command that waits there for tmux freezes the whole window, and
/// every request queued behind it, for as long as tmux does not answer
/// (2026-11-26: a stopped tmux server held `scroll_session` in
/// `Command::output` with 100% of main-thread samples). `off_ipc_thread` runs
/// such a command on a blocking-pool thread under its own deadline scope,
/// installed on THAT thread, so every tmux call inside it is bounded by
/// `bounded_output` (its own child killed and reaped at the end) and every
/// `OperationLock` wait gives up at the same instant. The caller is answered
/// `terminal-timeout` no later than `budget` whatever the work does; a late
/// worker keeps what it holds until its own deadline ends its last call, and
/// its answer is dropped and logged. Nothing is retried.
pub(crate) async fn off_ipc_thread<T: Send + 'static>(
    op: &'static str,
    budget: Duration,
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    off_ipc(op, budget, true, work).await
}

/// `off_ipc_thread` for work that must NOT be cut by a deadline (a send
/// whose firing window the scheduler's own workers also run unbounded,
/// `queue_send_now`): the caller's answer is bounded by `budget`, the work
/// runs to its own end exactly as on a scheduler worker, and its late
/// answer is dropped and logged.
pub(crate) async fn off_ipc_answer<T: Send + 'static>(
    op: &'static str,
    budget: Duration,
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    off_ipc(op, budget, false, work).await
}

async fn off_ipc<T: Send + 'static>(
    op: &'static str,
    budget: Duration,
    scoped: bool,
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    use std::sync::atomic::Ordering;
    let started = Instant::now();
    let end = started + budget;
    TERMINAL_WORKERS.fetch_add(1, Ordering::AcqRel);
    let task = tauri::async_runtime::spawn_blocking(move || {
        let result = if scoped {
            let _deadline = Deadline::until(end);
            work()
        } else {
            work()
        };
        let running = TERMINAL_WORKERS.fetch_sub(1, Ordering::AcqRel) - 1;
        let took = started.elapsed();
        if took >= budget {
            rate_limited(op, "late", || {
                format!(
                    "[terminal] {op} finished late after {}ms, answer dropped (workers={running})",
                    took.as_millis()
                )
            });
        } else if took >= SLOW_TERMINAL_OP {
            rate_limited(op, "slow", || {
                format!("[terminal] {op} slow {}ms", took.as_millis())
            });
        }
        result
    });
    match tokio::time::timeout_at(end.into(), task).await {
        Ok(Ok(result)) => result.map_err(terminal_error),
        Ok(Err(_)) => Err(DeckError::new(ErrorKind::Tmux, "terminal-worker-failed")),
        Err(_) => {
            rate_limited(op, "timeout", || {
                format!(
                    "[terminal] {op} no answer within {}ms (workers={})",
                    budget.as_millis(),
                    TERMINAL_WORKERS.load(Ordering::Acquire)
                )
            });
            Err(timeout_error())
        }
    }
}

/// Terminal workers still running (answered or not): a count that only
/// grows while each one is bounded would show accumulation in app.log.
static TERMINAL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const SLOW_TERMINAL_OP: Duration = Duration::from_millis(500);
pub(crate) const TERMINAL_TIMEOUT: &str = "terminal-timeout";
pub(crate) const TERMINAL_BUSY: &str = "terminal-busy";

fn timeout_error() -> DeckError {
    DeckError::new(ErrorKind::Tmux, TERMINAL_TIMEOUT)
}

/// A tmux call cut by the terminal deadline surfaces as the restart
/// deadline inside `captured_output`; a terminal caller is told what it is.
fn terminal_error(error: DeckError) -> DeckError {
    if error.restart_failure() == Some(RestartFailure::Deadline) {
        timeout_error()
    } else {
        error
    }
}

/// At most one line per (op, outcome) every 5 s, carrying how many were
/// folded into it, so a stalled tmux cannot flood app.log at frame rate.
fn rate_limited(op: &'static str, outcome: &'static str, line: impl FnOnce() -> String) {
    use crate::sync::LockRecover;
    use std::collections::HashMap;
    /// (op, outcome) → (last line, lines folded since)
    type Seen = HashMap<(&'static str, &'static str), (Instant, u32)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let mut seen = SEEN.lock_or_recover();
    let entry = seen
        .get_or_insert_with(HashMap::new)
        .entry((op, outcome))
        .or_insert((Instant::now() - Duration::from_secs(60), 0));
    if entry.0.elapsed() < Duration::from_secs(5) {
        entry.1 += 1;
        return;
    }
    let folded = std::mem::take(&mut entry.1);
    entry.0 = Instant::now();
    drop(seen);
    if folded > 0 {
        applog(&format!("{} (+{folded} since last)", line()));
    } else {
        applog(&line());
    }
}

/// A mutual-exclusion lock whose wait obeys the calling thread's deadline
/// scope: it gives up with `terminal-busy` when the scope ends, instead of
/// queueing a caller behind a holder that is itself waiting on tmux. A
/// caller without a scope waits as a plain mutex does (tests only; every
/// production holder runs inside `off_ipc_thread`). Exclusion only: it
/// promises no order between waiters.
pub(crate) struct OperationLock {
    busy: std::sync::Mutex<bool>,
    free: std::sync::Condvar,
}

pub(crate) struct OperationGuard<'a>(&'a OperationLock);

impl OperationLock {
    pub(crate) const fn new() -> Self {
        Self {
            busy: std::sync::Mutex::new(false),
            free: std::sync::Condvar::new(),
        }
    }

    pub(crate) fn acquire(&self) -> Result<OperationGuard<'_>, DeckError> {
        use crate::sync::LockRecover;
        let mut busy = self.busy.lock_or_recover();
        while *busy {
            busy = match deadline_remaining() {
                None => crate::sync::wait_or_recover(&self.free, busy),
                Some(left) if left.is_zero() => {
                    return Err(DeckError::new(ErrorKind::Tmux, TERMINAL_BUSY))
                }
                Some(left) => crate::sync::wait_timeout_or_recover(&self.free, busy, left),
            };
        }
        *busy = true;
        Ok(OperationGuard(self))
    }
}

impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        use crate::sync::LockRecover;
        *self.0.busy.lock_or_recover() = false;
        self.0.free.notify_all();
    }
}

/// Shared by a complete scheduled delivery (including finalization), immediate
/// prompt transport, and the exclusive restart. Never wait on the UI thread.
pub(crate) fn activity_guard() -> Result<RwLockReadGuard<'static, ()>, DeckError> {
    ACTIVITY
        .try_read()
        .map_err(|_| DeckError::restart(RestartFailure::DeliveryBusy))
}
pub(crate) fn exclusive() -> Result<RwLockWriteGuard<'static, ()>, DeckError> {
    ACTIVITY
        .try_write()
        .map_err(|_| DeckError::restart(RestartFailure::DeliveryBusy))
}

/// Tests exercising the process-wide activity gate must own this scope before
/// starting any workers and retain it until those workers release their guards.
/// This isolates independent scenarios; threads WITHIN a test still contend on
/// the real ACTIVITY lock, including the production nonblocking busy behavior.
#[cfg(test)]
pub(crate) fn test_activity_scope() -> std::sync::MutexGuard<'static, ()> {
    use crate::sync::LockRecover;
    static TEST_SCOPE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TEST_SCOPE.lock_or_recover()
}

/// Ordinary calls retain their existing behavior. The restart worker installs
/// one absolute deadline, inherited by all its tmux probes, captures and kills.
/// Drain nonblocking pipes ourselves: a reader thread could outlive a timeout.
pub(crate) fn command_output(command: &mut Command) -> std::io::Result<Output> {
    let Some(end) = DEADLINE.with(Cell::get) else {
        return command.output();
    };
    bounded_output(command, end)
}
pub(crate) fn bounded_output(command: &mut Command, end: Instant) -> std::io::Result<Output> {
    use std::io::{Error, ErrorKind as IoKind};
    if Instant::now() >= end {
        return Err(Error::new(IoKind::TimedOut, "restart deadline"));
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = (|| {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            // SAFETY: these descriptors belong to the live pipe handles above.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(Error::last_os_error());
            }
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut exited = None;
        loop {
            if Instant::now() >= end {
                return Err(Error::new(IoKind::TimedOut, "restart deadline"));
            }
            let mut caught_up = true;
            for (pipe, bytes) in [
                (&mut stdout as &mut dyn Read, &mut out),
                (&mut stderr as &mut dyn Read, &mut err),
            ] {
                let mut buf = [0u8; 8192];
                // Bound each drain so a noisy child cannot starve the deadline.
                let mut drained = false;
                for _ in 0..64 {
                    match pipe.read(&mut buf) {
                        Ok(0) => {
                            drained = true;
                            break;
                        }
                        Ok(n) => {
                            bytes.extend_from_slice(&buf[..n]);
                            if bytes.len() > 8 * 1024 * 1024 {
                                return Err(Error::new(
                                    IoKind::InvalidData,
                                    "restart output limit",
                                ));
                            }
                        }
                        Err(e) if e.kind() == IoKind::WouldBlock => {
                            drained = true;
                            break;
                        }
                        Err(e) if e.kind() == IoKind::Interrupted => continue,
                        Err(e) => return Err(e),
                    }
                }
                caught_up &= drained;
            }
            if let Some(status) = exited.filter(|_| caught_up) {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            // One more nonblocking drain after observing exit, so bytes
            // produced between this drain and try_wait are not dropped.
            exited = child.try_wait()?;
            std::thread::sleep(
                Duration::from_millis(2).min(end.saturating_duration_since(Instant::now())),
            );
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_deadlines_cannot_extend_the_budget_and_restore_the_outer_scope() {
        assert!(check_deadline().is_ok());
        let expired = Instant::now();
        {
            let _outer = Deadline::until(expired);
            {
                let _inner = Deadline::until(Instant::now() + Duration::from_secs(1));
                assert!(check_deadline().is_err());
                assert!(command_output(&mut Command::new("/does-not-exist")).is_err());
            }
            assert!(check_deadline().is_err());
        }
        assert!(check_deadline().is_ok());
    }
    #[test]
    fn watchdog_returns_while_a_stalled_worker_keeps_its_guard_and_cannot_continue() {
        let _scope = test_activity_scope();
        let (release, blocked) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        let begin = Instant::now();
        let result = run_bounded(begin + Duration::from_millis(80), move || {
            let _guard = exclusive()?;
            blocked.recv().unwrap(); // stand-in for a stalled filesystem operation
            done.send(check_deadline().is_err()).unwrap();
            check_deadline()
        });
        assert_eq!(result.unwrap_err().message(), "tmux-restart-timeout");
        assert!(begin.elapsed() < Duration::from_secs(1));
        assert!(
            activity_guard().is_err(),
            "timed-out work still owns the activity gate"
        );
        release.send(()).unwrap();
        assert!(finished.recv_timeout(Duration::from_secs(1)).unwrap());
        let until = Instant::now() + Duration::from_secs(1);
        while activity_guard().is_err() {
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
    }

    #[test]
    fn stalled_child_is_reaped_and_large_output_is_drained() {
        let start = Instant::now();
        let err = bounded_output(
            Command::new("/bin/sleep").arg("5"),
            start + Duration::from_millis(60),
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(1));
        let out = bounded_output(
            Command::new("/usr/bin/head").args(["-c", "262144", "/dev/zero"]),
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout.len(), 262144);
    }

    #[test]
    fn expired_deadline_never_launches_a_command() {
        let err = bounded_output(&mut Command::new("/does-not-exist"), Instant::now()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    }

    /// A waiter under a deadline gives up with `terminal-busy` when its
    /// scope ends instead of queueing behind a holder stuck on tmux; the
    /// lock is usable again as soon as the holder lets go.
    #[test]
    fn operation_lock_wait_ends_with_the_callers_deadline() {
        static LOCK: OperationLock = OperationLock::new();
        let held = LOCK.acquire().expect("free");
        let begin = Instant::now();
        let waited = std::thread::spawn(move || {
            let _scope = Deadline::until(Instant::now() + Duration::from_millis(80));
            LOCK.acquire()
                .map(|_| ())
                .map_err(|e| e.message().to_string())
        })
        .join()
        .unwrap();
        assert_eq!(waited.unwrap_err(), TERMINAL_BUSY);
        assert!(begin.elapsed() < Duration::from_secs(1));
        drop(held);
        let _scope = Deadline::until(Instant::now() + Duration::from_millis(80));
        assert!(LOCK.acquire().is_ok(), "released, the lock is free again");
    }

    /// The caller is answered within the budget whatever the work does; the
    /// work's own tmux-style child is still killed at the same deadline, so
    /// a late worker ends shortly after it and holds nothing.
    #[test]
    fn off_ipc_thread_answers_within_budget_and_the_late_worker_still_ends() {
        let (finished_tx, finished) = std::sync::mpsc::channel();
        let begin = Instant::now();
        let answer = tauri::async_runtime::block_on(off_ipc_thread(
            "test-stall",
            Duration::from_millis(150),
            move || {
                // a child that would never answer: bounded by this thread's scope
                let out = command_output(Command::new("/bin/sleep").arg("30"));
                let _ = finished_tx.send(Instant::now());
                out.map(|_| ())
                    .map_err(|_| DeckError::restart(RestartFailure::Deadline))
            },
        ));
        assert_eq!(answer.unwrap_err().message(), TERMINAL_TIMEOUT);
        assert!(
            begin.elapsed() < Duration::from_secs(1),
            "{:?}",
            begin.elapsed()
        );
        let ended = finished
            .recv_timeout(Duration::from_secs(2))
            .expect("worker ended");
        assert!(ended.duration_since(begin) < Duration::from_secs(1));
        // an ordinary answer passes through unchanged
        let ok = tauri::async_runtime::block_on(off_ipc_thread(
            "test-ok",
            Duration::from_secs(1),
            || Ok::<_, DeckError>(7),
        ));
        assert_eq!(ok.unwrap(), 7);
    }
}
