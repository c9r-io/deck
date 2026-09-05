//! Poison-tolerant locking. deck is a long-running process with scheduler,
//! inbound, PTY reader and status-listener threads. A panic while one of
//! them holds a `Mutex` poisons it, and every later `lock().unwrap()` on
//! that mutex would then panic too — the scheduler stops, every queue
//! command fails, the terminal stalls — a cascade far worse than the
//! original fault. Every value behind a deck lock is a flag, a set, a
//! registry, or state whose durable form is written persist-then-commit
//! (`with_queue`, the Board transaction), so its invariants hold between
//! statements and continuing with the last written value is the correct
//! recovery. The poison is logged once, content-free, so a cascade that
//! *would* have happened is still visible in app.log.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

pub(crate) trait LockRecover<T> {
    /// `lock()` that recovers a poisoned mutex instead of propagating the
    /// panic to an unrelated thread.
    fn lock_or_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_or_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(recover)
    }
}

/// `Condvar::wait` with the same recovery: the guard comes back either way.
pub(crate) fn wait_or_recover<'a, T>(
    cvar: &Condvar,
    guard: MutexGuard<'a, T>,
) -> MutexGuard<'a, T> {
    cvar.wait(guard).unwrap_or_else(recover)
}

/// `Condvar::wait_timeout` with the same recovery.
pub(crate) fn wait_timeout_or_recover<'a, T>(
    cvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: std::time::Duration,
) -> MutexGuard<'a, T> {
    cvar.wait_timeout(guard, timeout)
        .map(|(guard, _)| guard)
        .unwrap_or_else(|poison| poison.into_inner().0)
}

fn recover<G>(poison: PoisonError<G>) -> G {
    // Never touches the log lock: storage::applog takes LOG_LOCK with its
    // own recovery, so a poisoned LOG_LOCK cannot recurse here.
    crate::storage::applog("[sync] poisoned lock recovered");
    poison.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_panic_under_a_lock_does_not_poison_the_next_holder() {
        let shared = Arc::new(Mutex::new(vec![1]));
        let worker = Arc::clone(&shared);
        let _ = std::thread::spawn(move || {
            let mut guard = worker.lock_or_recover();
            guard.push(2);
            panic!("worker fault while holding the lock");
        })
        .join();
        assert!(shared.is_poisoned());
        let guard = shared.lock_or_recover();
        assert_eq!(*guard, vec![1, 2], "the last written value survives");
        drop(guard);
        assert!(
            std::panic::catch_unwind(|| shared.lock().unwrap()).is_err(),
            "plain lock() would still cascade"
        );
    }

    /// Discipline lint: production code takes locks through this module, so
    /// no thread can be taken down by another thread's panic.
    #[test]
    fn production_code_never_unwraps_a_lock() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
            } else {
                files.push(path);
            }
        }
        for path in files
            .into_iter()
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        {
            if path.ends_with("tests.rs") || path.ends_with("sync.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let production = text.split("#[cfg(test)]\nmod tests").next().unwrap();
            for pattern in [".lock().unwrap()", ".lock().expect(", ".wait(g).unwrap()"] {
                assert!(
                    !production.contains(pattern),
                    "{}: use lock_or_recover / wait_or_recover instead of {pattern}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn condvar_waits_recover_too() {
        let pair = Arc::new((Mutex::new(false), Condvar::new()));
        let poisoner = Arc::clone(&pair);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.0.lock_or_recover();
            panic!("poison");
        })
        .join();
        let (lock, cvar) = &*pair;
        let guard = lock.lock_or_recover();
        let guard = wait_timeout_or_recover(cvar, guard, std::time::Duration::from_millis(5));
        assert!(!*guard);
    }
}
