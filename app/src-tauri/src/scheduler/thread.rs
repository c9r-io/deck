//! The scheduler thread: boot-time queue recovery, the 20s tick with a
//! condition-variable wake, and per-session worker threads.

use std::collections::HashMap;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use super::*;
use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::tmux;

/// Boot migration is unusual: the interrupted send is an irreversible fact,
/// so recovered `ambiguous` memory is authoritative even when the first disk
/// write fails. `dirty` then gives the scheduler a real retry driver.
pub(super) fn boot_queues_with(
    mut loaded: QueueState,
    persist: &dyn Fn(&QueueState) -> Result<(), String>,
) -> Queues {
    let has_interrupted = {
        loaded.items.iter().any(|i| i.state == "firing")
            || loaded
                .pending
                .iter()
                .any(|p| !loaded.items.iter().any(|i| i.id == p.snapshot.id))
    };
    if has_interrupted {
        let notes = recover_interrupted(&mut loaded);
        notes.into_iter().for_each(storage::warn);
        let queues = Queues::new(loaded);
        let q = queues.q.lock().unwrap();
        if let Err(e) = persist(&q) {
            queues.dirty.store(true, AtomicOrdering::Relaxed);
            storage::warn(format!(
                "interrupted deliveries are available to acknowledge or retry now; their recovered state could not be saved yet ({}), so deck will keep retrying",
                storage::err_code(&e)
            ));
        }
        drop(q);
        return queues;
    }
    Queues::new(loaded)
}

pub(crate) fn boot_queues() -> Queues {
    boot_queues_with(load_queue(), &save_queue)
}

/// The tick sleeps on a condition so an event that made work due right now
/// (an inbound card with its prompts just queued) can start the scan at once
/// instead of waiting out the remainder of the period.
static TICK_WAKE: std::sync::OnceLock<(Mutex<bool>, std::sync::Condvar)> =
    std::sync::OnceLock::new();
pub(crate) const TICK_SECS: u64 = 20;

fn tick_wake() -> &'static (Mutex<bool>, std::sync::Condvar) {
    TICK_WAKE.get_or_init(|| (Mutex::new(false), std::sync::Condvar::new()))
}

pub(crate) fn wake_scheduler() {
    let (flag, cv) = tick_wake();
    if let Ok(mut f) = flag.lock() {
        *f = true;
    }
    cv.notify_all();
}

fn sleep_until_tick() {
    let (flag, cv) = tick_wake();
    let Ok(mut f) = flag.lock() else {
        std::thread::sleep(Duration::from_secs(TICK_SECS));
        return;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(TICK_SECS);
    while !*f {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let Ok((g, _)) = cv.wait_timeout(f, left) else {
            return;
        };
        f = g;
    }
    *f = false;
}

pub(crate) fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        sleep_until_tick();
        let state = app.state::<Queues>();
        // A post-send transition can be the last once item. Flush before the
        // empty-queue fast path so dirty state never loses its retry driver.
        flush_dirty(&state.q, &state.dirty, &save_queue);
        if state.q.lock().unwrap().items.is_empty() {
            continue;
        }
        // pane activity for chain-mode quiet checks (one snapshot per tick)
        let mut activity: HashMap<String, u64> = HashMap::new();
        if let Ok(out) = tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{window_activity}",
        ]) {
            for line in out.lines() {
                let mut it = line.split('\t');
                if let (Some(s), Some(a)) = (it.next(), it.next()) {
                    if let Ok(a) = a.parse() {
                        activity.entry(s.to_string()).or_insert(a);
                    }
                }
            }
        }
        // expired rules die quietly, transactionally like every other change
        let now = now_epoch();
        if state
            .q
            .lock()
            .unwrap()
            .items
            .iter()
            .any(|i| expired(i, now))
        {
            match with_queue(&state.q, &save_queue, |q| {
                purge_expired(q, now_epoch());
                Ok(())
            }) {
                Ok(()) => {
                    let _ = app.emit("queue-changed", ());
                }
                Err(e) => applog(&format!(
                    "[queue] persist (expiry purge) FAILED ({}) — rules kept",
                    storage::err_code(&e)
                )),
            }
        }
        // tick-start candidate pass: at most one session slot each. The
        // candidates only tell us WHICH sessions to serve — each worker
        // re-selects from FRESH state under the lock before sending.
        let sessions: Vec<String> = {
            let q = state.q.lock().unwrap();
            select_due(&q, now_epoch(), local_minutes(), &activity)
                .into_iter()
                .map(|i| i.session)
                .collect()
        };
        // One short-lived worker thread per session: a slow send (e.g. the
        // 2.5s boot wait of a dead session) delays only its own session.
        // The busy-set claim guarantees a session never has two concurrent
        // workers — including a worker still running from a previous tick.
        for session in sessions {
            if !claim_session(&state.busy, &session) {
                continue; // previous worker still on this session
            }
            let app2 = app.clone();
            let act = activity.clone();
            std::thread::spawn(move || {
                let state = app2.state::<Queues>();
                let res = send_one_safe(
                    &state.q,
                    &state.dirty,
                    &session,
                    local_minutes(),
                    &act,
                    &SendHooks {
                        fire: &fire_item,
                        persist: &save_queue,
                        kill: &kill_session_quietly,
                    },
                    &ContextHooks {
                        prepare: &prepare_context,
                        final_probe: &final_context_probe,
                    },
                );
                release_session(&state.busy, &session);
                match res {
                    SendResult::Sent { session } => {
                        let _ = app2.emit("queue-fired", QueueFired { session });
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::Failed { .. } | SendResult::Blocked { .. } => {
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::Nothing | SendResult::NotPersisted => {}
                }
            });
        }
    });
}
