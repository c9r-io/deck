//! Shared session-operation coordination, below tmux and restart policy.
//! Activity excludes intentional server replacement; workers keep their guards
//! until all transport/cleanup/finalization ends. Deadline scopes belong to
//! the current OS thread only. Spawned workers must install their own scope
//! (run_bounded does); scopes cannot be moved across threads or async tasks.
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
fn error(code: &str) -> DeckError {
    DeckError::new(ErrorKind::Tmux, code)
}

/// Bound the IPC response even if local filesystem IO is stalled. The worker
/// keeps its guards until that IO returns; its absolute deadline then prevents
/// it from proceeding to kill-server. We never detach a timed-out destructive
/// operation from its identity/creation locks.
pub(crate) fn run_bounded<T: Send + 'static>(
    end: Instant,
    work: impl FnOnce() -> Result<T, DeckError> + Send + 'static,
) -> Result<T, DeckError> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("deck-shell-restart".into())
        .spawn(move || {
            let _deadline = Deadline::until(end);
            let result = check_deadline().and_then(|_| work());
            let _ = tx.send(result);
        })
        .map_err(|_| error("tmux-restart-worker-failed"))?;
    match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            applog("[tmux-restart] watchdog-timeout worker-retains-guards");
            Err(DeckError::restart(RestartFailure::Deadline))
        }
        Err(_) => Err(error("tmux-restart-worker-failed")),
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
}
