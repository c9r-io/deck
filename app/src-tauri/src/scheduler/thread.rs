//! The scheduler thread: boot-time queue recovery, the 20s tick with a
//! condition-variable wake, and per-session worker threads. An immediately
//! woken override worker may wait for startup compatibility without blocking
//! any other session or pretending that time proves Agent readiness.

use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use super::*;
use crate::applog::applog;
use crate::datadir::now_epoch;
use crate::error::DeckError;
use crate::storage;
use crate::sync::LockRecover;

/// Boot migration is unusual: the interrupted send is an irreversible fact,
/// so recovered `ambiguous` memory is authoritative even when the first disk
/// write fails. `dirty` then gives the scheduler a real retry driver.
pub(super) fn boot_queues_with(
    mut loaded: QueueState,
    persist: &dyn Fn(&QueueState) -> Result<(), DeckError>,
) -> Queues {
    let has_interrupted = {
        loaded.items.iter().any(|i| i.state == ItemState::Firing)
            || loaded
                .pending
                .iter()
                .any(|p| !loaded.items.iter().any(|i| i.id == p.snapshot.id))
    };
    if has_interrupted {
        let notes = recover_interrupted(&mut loaded);
        for note in notes {
            storage::warn(storage::StorageNotice::QueueInterrupted, note);
        }
        let queues = Queues::new(loaded);
        let q = queues.q.lock_or_recover();
        if let Err(e) = persist(&q) {
            queues.dirty.store(true, AtomicOrdering::Relaxed);
            storage::warn(
                storage::StorageNotice::QueueInterrupted,
                format!(
                    "interrupted deliveries are available to acknowledge or retry now; their recovered state could not be saved yet ({}), so deck will keep retrying",
                    e.code()
                ),
            );
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

/// When each session last got its post-start wake (`start_wake_due`).
static START_WAKES: Mutex<Option<HashMap<String, std::time::Instant>>> = Mutex::new(None);

/// Whether a worker that just STARTED `session`'s agent
/// (`StartedAwaitingInteraction`, zero bytes) may wake the scheduler for an
/// immediate follow-up pass: at most once per session per `TICK_SECS`, so a
/// session that keeps disappearing and being restarted can never spin the
/// scheduler faster than twice its normal cadence. The follow-up pass is an
/// ordinary tick — it reads no new fact and creates no evidence: a normal
/// row is held at `first-send` again (no worker, so no further wake), and
/// only a head row carrying a first-send readiness override
/// (`first_send.rs`) can go on through the existing-session checks and the
/// pre-fire fence without waiting out the rest of the tick.
pub(crate) fn start_wake_due(
    last: &mut HashMap<String, std::time::Instant>,
    session: &str,
    now: std::time::Instant,
) -> bool {
    let window = Duration::from_secs(TICK_SECS);
    last.retain(|_, at| now.saturating_duration_since(*at) < window);
    if last.contains_key(session) {
        return false;
    }
    last.insert(session.to_string(), now);
    true
}

fn wake_after_start(session: &str) {
    let due = start_wake_due(
        START_WAKES
            .lock_or_recover()
            .get_or_insert_with(HashMap::new),
        session,
        std::time::Instant::now(),
    );
    if due {
        wake_scheduler();
    }
}

pub(crate) fn wake_scheduler() {
    let (flag, cv) = tick_wake();
    *flag.lock_or_recover() = true;
    cv.notify_all();
}

fn sleep_until_tick() {
    let (flag, cv) = tick_wake();
    let mut f = flag.lock_or_recover();
    let deadline = std::time::Instant::now() + Duration::from_secs(TICK_SECS);
    while !*f {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        f = crate::sync::wait_timeout_or_recover(cv, f, left);
    }
    *f = false;
}

/// Hand the tick's delivery waits to the notification module as
/// session → an opaque key for that one wait (row and stage).
fn publish_delivery_waits(waits: &HashMap<String, DeliveryWait>) {
    crate::notify::delivery_waits(
        waits
            .iter()
            .map(|(session, wait)| (session.clone(), format!("{}:{}", wait.item, wait.stage)))
            .collect(),
    );
}

pub(crate) fn spawn_scheduler(app: AppHandle) {
    // the delivery waits the last tick published (`review::delivery_waits`)
    let mut waits: HashMap<String, DeliveryWait> = HashMap::new();
    // when each row was first seen held for an unverifiable approval
    // (`review::track_unverified`): attention timing, nothing else
    let mut unverified = UnverifiedSince::new();
    std::thread::spawn(move || loop {
        sleep_until_tick();
        // this tick is the one native cadence deck has: while the webview's
        // poll is not running it also drives the attention observation the
        // poll would have made (`commands::poll_idle_tick`), queue or no
        // queue. Nothing comes back: no scheduler decision can see it.
        crate::commands::poll_idle_tick(&app.state::<crate::pty::PtyState>().attached());
        let state = app.state::<Queues>();
        // A post-send transition can be the last once item. Flush before the
        // empty-queue fast path so dirty state never loses its retry driver.
        flush_dirty(&state.q, &state.dirty, &save_queue);
        if state.q.lock_or_recover().items.is_empty() {
            // nothing queued, nothing waits
            unverified.clear();
            publish_lasting_unverified(&HashSet::new());
            if !waits.is_empty() {
                waits.clear();
                publish_delivery_waits(&waits);
                let _ = app.emit("queue-changed", ());
            }
            continue;
        }
        // pane activity (chain quiet) and agent hook words (agent hold), one
        // snapshot per tick; a failed listing sends nothing this tick
        // a failed listing selects nothing; a POSITIVELY empty or absent
        // Deck server is an empty listing (`scheduler_pane_listing`)
        let mut listing = crate::tmux_lifecycle::scheduler_pane_listing()
            .ok()
            .map(observe);
        // expired rules die quietly, transactionally like every other change
        let now = now_epoch();
        if state
            .q
            .lock_or_recover()
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
                    e.code()
                )),
            }
        }
        // a withdrawn or changed automation approval, or a withdrawn
        // first-send readiness override, stops every unsent row that relied
        // on it BEFORE this tick selects anything; unreadable settings
        // neither grant nor revoke (`authority.rs`, `first_send.rs`)
        // a phone task's override and approval are backed by the Board side
        // instead (the current Board and the devices still paired), and are
        // swept against it the same way
        let (needs_settings, needs_board) = {
            let q = state.q.lock_or_recover();
            let relies = |source| any_authority(&q, source) || first_send::any_override(&q, source);
            (
                relies(first_send::Backing::Settings),
                relies(first_send::Backing::Board),
            )
        };
        if needs_settings || needs_board {
            let config = needs_settings
                .then(crate::inbound::read_config_strict)
                .flatten();
            let board = needs_board.then(first_send::phone_tasks).flatten();
            // no proof either way: rows, approvals and overrides stay;
            // automatic sends that rely on one hold this tick
            if let Some(seen) = listing.as_mut() {
                if needs_settings && config.is_none() {
                    mark_authority_unverified(seen);
                }
                if needs_board && board.is_none() {
                    mark_board_unverified(seen);
                }
            }
            if config.is_some() || board.is_some() {
                let sources = first_send::Sources {
                    settings: config.as_ref(),
                    board: board.as_ref(),
                };
                match with_queue_opt(&state.q, &save_queue, |q| {
                    let approvals = revoke_stale(q, sources);
                    let overrides = first_send::revoke_stale(q, sources);
                    Ok((approvals + overrides > 0).then_some((approvals, overrides)))
                }) {
                    Ok(Some((approvals, overrides))) => {
                        if approvals > 0 {
                            applog(&format!(
                                "[queue] automation approval withdrawn — {approvals} row(s) now wait for send-now"
                            ));
                        }
                        if overrides > 0 {
                            applog(&format!(
                                "[queue] first-send policy withdrawn — {overrides} first step(s) now wait for an agent interaction"
                            ));
                        }
                        let _ = app.emit("queue-changed", ());
                    }
                    Ok(None) => {}
                    Err(e) => {
                        // memory still holds the stale approval: send nothing
                        applog(&format!(
                            "[queue] persist (approval sweep) FAILED ({}) — nothing sent this tick",
                            e.code()
                        ));
                        continue;
                    }
                }
            }
        }
        // tick-start candidate pass: at most one session slot each. The
        // candidates only tell us WHICH sessions to serve — each worker
        // re-selects from FRESH state under the lock before sending.
        let sessions: Vec<String> = {
            let q = state.q.lock_or_recover();
            tick_selection(&q, now_epoch(), local_minutes(), listing.as_ref())
                .into_iter()
                .map(|i| i.session)
                .collect()
        };
        // Attention only: which sessions have a delivery waiting for a
        // person. Read from the same state and observations the selection
        // just used; it selects, sends and releases nothing.
        let current = {
            let q = state.q.lock_or_recover();
            let (now, minutes) = (now_epoch(), local_minutes());
            let lasting = track_unverified(&q, now, minutes, listing.as_ref(), &mut unverified);
            publish_lasting_unverified(&lasting);
            delivery_waits(&q, now, minutes, listing.as_ref(), &waits, &lasting)
        };
        if current != waits {
            waits = current;
            publish_delivery_waits(&waits);
            // a hold can begin or end with no queue mutation (an agent
            // proved an interaction, Codex left the foreground): the
            // webview re-reads its plans so the list says the same
            let _ = app.emit("queue-changed", ());
        }
        let Some(activity) = listing else {
            continue;
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
                let Ok(_activity) = crate::session_runtime::activity_guard() else {
                    release_session(&state.busy, &session);
                    return;
                };
                let res = send_one_safe_stabilized(
                    &state.q,
                    &state.dirty,
                    SendRequest {
                        session: &session,
                        now_min: local_minutes(),
                        activity: &act,
                        requested: None,
                    },
                    &SendHooks {
                        fire: &fire_item,
                        persist: &save_queue,
                        kill: &kill_session_quietly,
                        board: &first_send::phone_tasks,
                        authority: &crate::inbound::read_config_strict,
                    },
                    &ContextHooks {
                        prepare: &prepare_context,
                        final_probe: &final_context_probe,
                    },
                    &StabilizationOps {
                        sleep: &std::thread::sleep,
                        observe: &|| {
                            crate::tmux_lifecycle::scheduler_pane_listing()
                                .ok()
                                .map(observe)
                        },
                    },
                );
                release_session(&state.busy, &session);
                match res {
                    SendResult::Sent { session } => {
                        let _ = app2.emit("queue-fired", QueueFired { session });
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::StartedAwaitingInteraction { session } => {
                        let _ = app2.emit("queue-changed", ());
                        // the busy claim is already released: the woken
                        // pass may serve this now-existing session
                        wake_after_start(&session);
                    }
                    SendResult::Failed { .. }
                    | SendResult::Partial { .. }
                    | SendResult::Blocked { .. } => {
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::Nothing | SendResult::NotPersisted => {}
                }
            });
        }
    });
}
