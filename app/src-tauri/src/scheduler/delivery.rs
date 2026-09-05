//! The delivery state machine: readiness probing, context protection,
//! the persisted firing ledger, atomic injection, finalization and crash
//! recovery. `send_one` is the reference sequence; `send_one_safe` is its
//! context-safe front half.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;
use crate::applog::applog;
use crate::commands::start_session;
use crate::context::{self, ContextCheck, ContextCode, ContextStatus, PaneIdentity, ProbeResult};
use crate::datadir::now_epoch;
use crate::error::DeckError;
use crate::sync::LockRecover;
use crate::tmux::{tmux, tmux_owned};

/// Note: deliberately carries no prompt text — the UI only toasts the
/// session name, and event payloads must not haul content around (privacy).
#[derive(Clone, Serialize)]
pub(crate) struct QueueFired {
    pub(super) session: String,
}

/// Inject one prompt into the exact pane approved by the context probe.
///
/// The injection is one tmux server command queue: store the literal prompt
/// plus CR in a private buffer, compare the full server/session/window/pane
/// generation plus the optional expected foreground process, and paste only
/// on an exact match. There is no window where
/// the text landed but Enter did not. Queue text has \r/\n stripped at
/// add/update time, so the appended CR is the only one. (Residual ambiguity:
/// an externally killed tmux client after the server pasted could still read
/// as a failure; deck never does that, and it is the same class of window as
/// a power loss mid-send.)
pub(crate) fn fire_item(item: &QueueItem) -> Result<(), DeckError> {
    let pane = item.binding.as_ref().ok_or("context binding is missing")?;
    let delivery = item
        .delivery
        .as_deref()
        .ok_or("delivery identity is missing")?;
    if !delivery
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("delivery identity is invalid".into());
    }
    let line = item.text.clone();
    let buffer = format!("deck-send-{delivery}");
    // Pane/session ids can be reused after the entire tmux server exits. Put
    // the bytes in a uniquely named tmux buffer, then compare the FULL
    // generation and paste them in the same server command queue. `-F`
    // evaluates synchronously (no shell); the inner commands contain only
    // deck-generated ids, never user text. paste-buffer is byte-literal and
    // `-p` wraps the bytes in bracketed-paste marks only for an application
    // that asked for them, so an agent TUI sees one paste, not keystrokes.
    // Enter follows as a SEPARATE key after a short pause: a CR inside the
    // same burst is treated by agent inputs (Claude Code, Codex) as a
    // pasted newline and the prompt sits unsent in the input box.
    let actual = "#{pid}:#{session_id}:#{window_id}:#{pane_id}:#{pane_pid}";
    let expected = format!(
        "{}:{}:{}:{}:{}",
        pane.server_pid, pane.session_id, pane.window_id, pane.pane_id, pane.pane_pid
    );
    let identity_condition = format!("#{{==:{actual},{expected}}}");
    // tmux can only compare its own `pane_current_command` atomically. When
    // the expected process was recognized through its argv name (a launcher
    // symlink to a versioned binary — tmux says `2.1.259`, ps says `claude`),
    // pin the paste to the exact tmux name observed for that very process,
    // so the atomic check still means "the verified process is still here".
    let condition_process =
        item.expected_process
            .as_deref()
            .map(|expected| match context::raw_probe(&item.session) {
                Ok(raw)
                    if raw.foreground.as_deref() != Some(expected)
                        && raw.foreground_argv.as_deref() == Some(expected) =>
                {
                    raw.foreground.unwrap_or_else(|| expected.to_string())
                }
                _ => expected.to_string(),
            });
    let condition = match condition_process.as_deref() {
        Some(process) => {
            format!("#{{&&:{identity_condition},#{{==:#{{pane_current_command}},{process}}}}}")
        }
        None => identity_condition,
    };
    let yes = format!("paste-buffer -p -b {buffer} -d -t {}", pane.pane_id);
    let no = format!("delete-buffer -b {buffer}; display-message -p deck-context-refused");
    let out = tmux_owned(&[
        "set-buffer".into(),
        "-b".into(),
        buffer.clone(),
        line,
        ";".into(),
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        pane.pane_id.clone(),
        condition.clone(),
        yes,
        no,
    ]);
    let refused = |stdout: &String| {
        stdout
            .lines()
            .any(|line| line.trim() == "deck-context-refused")
    };
    if !out.as_ref().is_ok_and(|stdout| !refused(stdout)) {
        // A vanished target can abort the command queue before its refusal
        // branch deletes the private buffer. Never leave prompt bytes behind
        // in tmux after a refused/indeterminate injection.
        let _ = tmux(&["delete-buffer", "-b", &buffer]);
        return Err("context identity or foreground changed before literal send".into());
    }
    std::thread::sleep(Duration::from_millis(ENTER_DELAY_MS));
    let enter = format!("send-keys -t {} Enter", pane.pane_id);
    let out = tmux_owned(&[
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        pane.pane_id.clone(),
        condition,
        enter,
        "display-message -p deck-context-refused".into(),
    ]);
    if !out.as_ref().is_ok_and(|stdout| !refused(stdout)) {
        // The text is already in the pane; the user sees it and can submit
        // it. Counting this as sent keeps the audit honest about the bytes
        // that landed, and the log names the one thing that did not.
        applog("[queue] Enter refused after paste — prompt left in the input");
    }
    Ok(())
}

/// Long enough for an agent input to close its paste burst before Enter.
const ENTER_DELAY_MS: u64 = 600;

pub(crate) const READY_PROBE_INTERVAL_MS: u64 = 250;
pub(crate) const READY_PROBE_TIMEOUT_MS: u64 = 15_000;

pub(crate) fn poll_readiness(
    max_polls: usize,
    cancelled: &dyn Fn() -> bool,
    probe: &mut dyn FnMut(Option<&PaneIdentity>) -> ProbeResult,
    wait: &mut dyn FnMut(),
) -> ProbeResult {
    let mut started_identity: Option<PaneIdentity> = None;
    let polls = max_polls.max(1);
    for index in 0..polls {
        if cancelled() {
            return ProbeResult::blocked(
                ContextStatus::Unavailable,
                ContextCode::CancelledOrRevised,
            );
        }
        let result = probe(started_identity.as_ref());
        if result.status == ContextStatus::SessionReplaced {
            return result;
        }
        if let Some(identity) = &result.identity {
            if started_identity.is_none() {
                started_identity = Some(identity.clone());
            }
        }
        if result.is_ready() {
            return result;
        }
        if index + 1 == polls {
            return ProbeResult {
                status: result.status,
                code: ContextCode::StartupTimeout,
                identity: result.identity,
                current_process: result.current_process,
            };
        }
        wait();
    }
    unreachable!()
}

/// Production readiness probe. Existing live sessions are checked once.
/// A dead session is started once, then polled with a bounded timeout. The
/// cancellation callback is consulted between every probe and sleep, so a
/// pause/edit/delete does not wait for the timeout.
pub(crate) fn prepare_context(item: &QueueItem, cancelled: &dyn Fn() -> bool) -> ProbeResult {
    if cancelled() {
        return ProbeResult::blocked(ContextStatus::Unavailable, ContextCode::CancelledOrRevised);
    }
    let existed = tmux(&[
        "has-session",
        "-t",
        &crate::tmux::session_target(&item.session),
    ])
    .is_ok();
    if existed {
        return current_context_probe(item);
    }
    // A session that appeared between the existence check and start_session's
    // idempotent inner check is probed like any other live one.
    match start_session(
        item.session.clone(),
        item.dir.clone(),
        item.cmd.clone(),
        false,
    ) {
        Ok(_) => {}
        Err(_) => {
            return ProbeResult::blocked(ContextStatus::Unavailable, ContextCode::StartupFailed);
        }
    }

    let interval = std::time::Duration::from_millis(READY_PROBE_INTERVAL_MS);
    let polls = (READY_PROBE_TIMEOUT_MS / READY_PROBE_INTERVAL_MS) as usize + 1;
    let ready = poll_readiness(
        polls,
        cancelled,
        &mut |identity| {
            // The first observation acquires the binding; a pane replaced
            // again while deck is still waiting for startup is rejected.
            context::probe(&item.session, identity, item.expected_process.as_deref())
        },
        &mut || std::thread::sleep(interval),
    );
    if ready.status != ContextStatus::Ready {
        return ready;
    }
    // The process is in the foreground the instant it execs, while its TUI
    // is still booting and not yet reading stdin. Bytes pasted now queue in
    // the pty and are read together with the Enter that follows — one burst,
    // which agent inputs treat as a paste with a trailing newline. Give a
    // fresh start a moment to settle, then confirm it is still the same pane.
    std::thread::sleep(std::time::Duration::from_millis(FRESH_START_SETTLE_MS));
    if cancelled() {
        return ProbeResult::blocked(ContextStatus::Unavailable, ContextCode::CancelledOrRevised);
    }
    context::probe(
        &item.session,
        ready.identity.as_ref(),
        item.expected_process.as_deref(),
    )
}

/// Grace between "the agent is in the foreground" and the first paste into a
/// session the scheduler started itself.
pub(crate) const FRESH_START_SETTLE_MS: u64 = 2500;

/// Observe the pane the card owns right now. A tmux generation change (the
/// server was replaced by an upgrade, a crash or a reboot) is adopted rather
/// than blocked: the pane is located by deck's own session name on deck's own
/// socket, a deleted card tombstones its items, and `expected_process` — not
/// the generation stamp — is what decides whether the target is the right one.
pub(crate) fn current_context_probe(item: &QueueItem) -> ProbeResult {
    context::probe(&item.session, None, item.expected_process.as_deref())
}

/// Immediately before opening the irreversible firing window, the identity
/// persisted by the readiness probe must still be the live one.
pub(crate) fn final_context_probe(item: &QueueItem) -> ProbeResult {
    context::probe(
        &item.session,
        item.binding.as_ref(),
        item.expected_process.as_deref(),
    )
}

/// Best-effort kill of a session deck must not leave running (already dead
/// is the normal case, hence the swallowed error).
pub(crate) fn kill_session_quietly(session: &str) {
    let _ = tmux(&["kill-session", "-t", &crate::tmux::session_target(session)]);
}

pub(crate) const MAX_ATTEMPTS: u32 = 8;

/// Retry backoff after a failed injection: 20s · 2^(n-1), capped at 30 min.
pub(crate) fn backoff_secs(attempts: u32) -> u64 {
    if attempts == 0 {
        return 0;
    }
    20u64.saturating_mul(1 << (attempts.min(10) - 1)).min(1800)
}

/// Permanently failed: attempts exhausted. Stays visible in the UI (with its
/// error) and BLOCKS the later steps of its group until the user explicitly
/// retries, skips or removes it — a chain never runs past a failed step.
pub(crate) fn item_dead(i: &QueueItem) -> bool {
    i.state == "failed" && i.attempts >= MAX_ATTEMPTS
}

pub(crate) fn retry_ok(i: &QueueItem, now: u64) -> bool {
    i.last_attempt_at
        .map(|t| now >= t + backoff_secs(i.attempts))
        .unwrap_or(true)
}

/// Collision-proof delivery id (ms clock + process-wide counter).
pub(crate) fn next_delivery_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!("d{ms}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Idempotent delivery bookkeeping — the ONE function that records a send,
/// shared by the live success path and explicit user acknowledgement.
/// In one pass it: writes the audit record, updates the session's last-fired
/// instant, consumes a once-item or counts a rule fire (last + fired),
/// spawns the iteration's template steps exactly once, and retires a rule
/// that reached stop-after-N. Re-running it for the same delivery id is a
/// no-op, so repeated crash recovery can never double-account.
pub(crate) fn finalize_delivery(
    q: &mut QueueState,
    item_id: &str,
    delivery: &str,
    now: u64,
    assumed: bool,
) {
    if q.deliveries.iter().any(|d| d.id == delivery) {
        q.pending.retain(|p| p.id != delivery);
        return; // this delivery is already fully accounted
    }
    // The live item is preferred; if it vanished mid-flight (a race the
    // firing contract normally forbids, or a crash+user-edit combination),
    // the pending-ledger snapshot still lets the delivery be accounted:
    // audit record + session gap always happen, item-side accounting
    // (consume/count/spawn steps) only if the item still exists.
    let live = q.items.iter().any(|i| i.id == item_id);
    let item = match q
        .items
        .iter()
        .find(|i| i.id == item_id)
        .cloned()
        .or_else(|| {
            q.pending
                .iter()
                .find(|p| p.id == delivery)
                .map(|p| p.snapshot.clone())
        }) {
        Some(i) => i,
        None => {
            // pre-ledger legacy recovery with the item already gone —
            // nothing is known about this delivery beyond its id; record
            // nothing rather than fabricate an audit row
            return;
        }
    };
    q.deliveries.push(DeliveryRecord {
        id: delivery.to_string(),
        item: item_id.to_string(),
        session: item.session.clone(),
        mode: item.mode.clone(),
        at: now,
        assumed,
    });
    if q.deliveries.len() > MAX_DELIVERIES {
        let n = q.deliveries.len() - MAX_DELIVERIES;
        q.deliveries.drain(..n);
    }
    // The card was deleted while this delivery was in flight. The audit
    // record above closes the in-flight delivery window (the prompt may really
    // have landed), but NOTHING about the session may come back to life:
    // no rule restored to pending, no template steps spawned, no next
    // cadence, no send-gap bookkeeping for a session that no longer exists.
    if is_cancelled(q, &item.session) {
        q.items.retain(|i| i.session != item.session);
        q.pending.retain(|p| p.id != delivery);
        return;
    }
    q.last_fired.insert(item.session.clone(), now);
    if !live {
        // item-side accounting has no item to act on; the audit record and
        // session gap above are the whole story
        q.pending.retain(|p| p.id != delivery);
        return;
    }
    if item.mode == "every" {
        // spawn this iteration's follow-up steps 2..N exactly once, keyed by
        // the delivery id (also their group id — one group per iteration)
        if !q.items.iter().any(|i| i.group.as_deref() == Some(delivery)) {
            for (k, step) in item.steps.iter().enumerate() {
                let id = next_queue_id(&q.items);
                q.items.push(QueueItem {
                    id,
                    session: item.session.clone(),
                    card_id: item.card_id.clone(),
                    dir: item.dir.clone(),
                    cmd: item.cmd.clone(),
                    text: step.clone(),
                    mode: "chain".into(),
                    at: None,
                    added: now,
                    every: None,
                    win_from: None,
                    win_to: None,
                    until_n: None,
                    until_at: None,
                    fired: 0,
                    paused: false,
                    last: None,
                    state: default_state(),
                    attempts: 0,
                    last_error: None,
                    last_attempt_at: None,
                    steps: Vec::new(),
                    tpl: item.tpl.clone(),
                    tpl_idx: item.tpl_idx.map(|_| k as u32 + 2),
                    tpl_total: item.tpl_total,
                    group: Some(delivery.to_string()),
                    seq: Some(k as u32 + 2),
                    rule: Some(item.id.clone()),
                    delivery: None,
                    expected_process: item.expected_process.clone(),
                    binding: item.binding.clone(),
                    last_context: None,
                    revision: 0,
                });
            }
        }
        let mut done = false;
        if let Some(it) = q.items.iter_mut().find(|i| i.id == item_id) {
            it.fired += 1;
            it.last = Some(now);
            it.state = default_state();
            it.attempts = 0;
            it.last_error = None;
            it.delivery = None;
            done = it.until_n.map(|n| it.fired >= n).unwrap_or(false);
        }
        if done {
            q.items.retain(|i| i.id != item_id);
        }
    } else {
        q.items.retain(|i| i.id != item_id);
    }
    q.pending.retain(|p| p.id != delivery);
}

/// The send definitively did NOT happen (atomic injection refused) — mark
/// the item for retry and drop its ledger entry: there is no delivery to
/// recover, and crash recovery must not treat it as ambiguous. The ledger
/// entry goes even when the item itself vanished mid-send (its card was
/// deleted), so a refused send is never resurrected as an assumed delivery.
pub(crate) fn note_failed(q: &mut QueueState, id: &str, delivery: &str, err: &str) {
    if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
        it.delivery = None;
        it.state = "failed".into();
        it.last_error = Some(format!("send failed ({})", crate::error::err_code(err)));
    }
    q.pending.retain(|p| p.id != delivery);
}

/// Crash recovery exposes unresolved firing intents as ambiguous. The send
/// may or may not have reached tmux, so neither automatic retry nor automatic
/// delivery accounting is honest. The user must acknowledge it as sent or
/// explicitly retry while accepting the duplicate-delivery risk.
pub(crate) fn recover_interrupted(q: &mut QueueState) -> Vec<String> {
    let mut notes = Vec::new();
    for item in q.items.iter_mut().filter(|i| i.state == "firing") {
        item.state = "ambiguous".into();
        notes.push(format!(
            "a {} prompt was interrupted during delivery; choose acknowledge or retry in its session queue",
            if item.mode == "every" { "recurring" } else { "scheduled" }
        ));
    }
    // A pre-v0.4.30 file can contain a ledger entry whose live item is gone.
    // Restore its snapshot as ambiguous unless its session was tombstoned.
    let orphans: Vec<PendingDelivery> = q
        .pending
        .iter()
        .filter(|p| {
            !q.items.iter().any(|i| i.id == p.snapshot.id) && !is_cancelled(q, &p.snapshot.session)
        })
        .cloned()
        .collect();
    for pending in orphans {
        let mut item = pending.snapshot;
        item.state = "ambiguous".into();
        item.delivery = Some(pending.id);
        notes.push(
            "a scheduled prompt was interrupted during delivery; choose acknowledge or retry in its session queue"
                .into(),
        );
        q.items.push(item);
    }
    let cancelled: HashSet<&str> = q.cancelled.iter().map(|t| t.session.as_str()).collect();
    q.pending
        .retain(|p| !cancelled.contains(p.snapshot.session.as_str()));
    notes
}

/// What one send attempt did — the worker emits UI events from this.
#[derive(Debug, PartialEq)]
pub(crate) enum SendResult {
    /// nothing eligible at send time (state changed since the tick began)
    Nothing,
    Sent {
        session: String,
    },
    Failed {
        session: String,
        gave_up: bool,
    },
    /// Due, but automatic identity/process protection blocked the target.
    /// This is not a delivery attempt and never creates firing ambiguity.
    Blocked {
        session: String,
        status: ContextStatus,
        code: ContextCode,
    },
    /// pre-fire persist failed — the intent never reached disk, nothing sent
    NotPersisted,
}

/// The side-effecting parts of one send, injected so the whole firing state
/// machine can be unit-tested without tmux or a disk.
pub(crate) struct SendHooks<'a> {
    pub(crate) fire: &'a (dyn Fn(&QueueItem) -> Result<(), DeckError> + Sync),
    pub(crate) persist: &'a (dyn Fn(&QueueState) -> Result<(), DeckError> + Sync),
    /// kill a session whose card was deleted DURING this send
    pub(crate) kill: &'a (dyn Fn(&str) + Sync),
}

pub(crate) struct ContextHooks<'a> {
    pub(crate) prepare: &'a (dyn Fn(&QueueItem, &dyn Fn() -> bool) -> ProbeResult + Sync),
    pub(crate) final_probe: &'a (dyn Fn(&QueueItem) -> ProbeResult + Sync),
}

#[derive(Clone, Copy)]
pub(super) struct SendRequest<'a> {
    pub(super) session: &'a str,
    pub(super) now_min: u32,
    pub(super) activity: &'a HashMap<String, u64>,
    pub(super) requested: Option<&'a str>,
}

/// Closing the last hole in "a deleted card leaves nothing running": the
/// worker holds no lock while it injects, so a card deleted in exactly that
/// instant is tombstoned only AFTER `fire_item` may already have restarted
/// its session. Once the send is over we re-read the tombstone and kill the
/// session, so the scheduler can never leave a session behind a deleted card.
fn reap_if_cancelled(cancelled: bool, session: &str, h: &SendHooks) {
    if cancelled {
        applog("[queue] card was deleted mid-send — its session is being killed");
        (h.kill)(session);
    }
}

/// One complete send attempt for one session — the ENTIRE firing state
/// machine lives here, unit-testable with a fake `fire`/`persist`:
///
///   pending ──(re-select fresh under lock, persist intent+ledger)──► firing
///   firing ──fire Ok──► finalize_delivery (audit, gap, consume/count/spawn)
///   firing ──fire Err──► note_failed (retryable, ledger dropped)
///   firing ──crash──► ambiguous (user acknowledge or risk-accepting retry)
///
/// Persistence has two distinct regimes, and the difference is deliberate:
///
/// * BEFORE the injection, the intent is a normal transaction — a failed
///   write rolls everything back and nothing is sent (`NotPersisted`).
/// * AFTER the injection (success or definitive refusal) the side effect is
///   already irreversible, so memory takes the new state unconditionally and
///   a failed write only sets the `dirty` flag: the scheduler retries the
///   save every tick (`flush_dirty`) and the user is warned. Until it lands,
///   a crash sees the older "firing" intent and exposes the uncertainty for
///   an explicit user decision.
///
/// The queue lock is held only for state transitions and their persists —
/// never across `fire` (tmux + a possible session-boot wait).
#[cfg(test)]
pub(crate) fn send_one(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    session: &str,
    now_min: u32,
    activity: &HashMap<String, u64>,
    h: &SendHooks,
) -> SendResult {
    send_one_guarded(
        qm,
        dirty,
        SendRequest {
            session,
            now_min,
            activity,
            requested: None,
        },
        h,
        None,
    )
}

fn send_one_guarded(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    request: SendRequest<'_>,
    h: &SendHooks,
    expected: Option<(&str, u64, &PaneIdentity)>,
) -> SendResult {
    let persist = h.persist;
    // Persist the firing intent (delivery id + ledger snapshot) BEFORE
    // injecting — this ordering preserves an honest ambiguity record across
    // crashes, and the snapshot makes resolution independent of item survival.
    let pre = with_queue(qm, persist, |q| {
        // fresh re-selection under the lock: a pause, edit or removal since
        // the tick began is honored here
        let Some(sel) = select_for_request(
            q,
            request.session,
            now_epoch(),
            request.now_min,
            request.activity,
            request.requested,
        ) else {
            return Err(TX_NOOP.into());
        };
        if expected.is_some_and(|(id, revision, binding)| {
            sel.id != id || sel.revision != revision || sel.binding.as_ref() != Some(binding)
        }) {
            return Err(TX_NOOP.into());
        }
        let delivery = next_delivery_id();
        let Some(it) = q.items.iter_mut().find(|i| i.id == sel.id) else {
            return Err(TX_NOOP.into());
        };
        it.state = "firing".into();
        it.attempts += 1;
        it.last_attempt_at = Some(now_epoch());
        it.delivery = Some(delivery.clone());
        let snapshot = it.clone();
        q.pending.push(PendingDelivery {
            id: delivery.clone(),
            snapshot: snapshot.clone(),
        });
        Ok((snapshot, delivery))
    });
    let (item, delivery) = match pre {
        Ok(v) => v,
        Err(e) if e.message() == TX_NOOP => return SendResult::Nothing,
        Err(e) => {
            applog(&format!(
                "[queue] persist (pre-fire) FAILED ({}) — not sending this tick",
                e.code()
            ));
            return SendResult::NotPersisted;
        }
    };
    match (h.fire)(&item) {
        Ok(()) => {
            // never log prompt contents — length only (privacy)
            applog(&format!(
                "[queue] sent to {} ({}B, mode {})",
                crate::applog::session_tag(&item.session),
                item.text.len(),
                item.mode
            ));
            let mut q = qm.lock_or_recover();
            finalize_delivery(&mut q, &item.id, &delivery, now_epoch(), false);
            let cancelled = is_cancelled(&q, &item.session);
            if let Err(e) = persist(&q) {
                note_persist_lag(dirty, "post-fire", e.message());
            }
            drop(q);
            reap_if_cancelled(cancelled, &item.session, h);
            SendResult::Sent {
                session: item.session.clone(),
            }
        }
        Err(e) => {
            let mut q = qm.lock_or_recover();
            note_failed(&mut q, &item.id, &delivery, e.message());
            let gave_up = q.items.iter().any(|i| i.id == item.id && item_dead(i));
            let cancelled = is_cancelled(&q, &item.session);
            if let Err(pe) = persist(&q) {
                note_persist_lag(dirty, "post-failure", pe.message());
            }
            drop(q);
            reap_if_cancelled(cancelled, &item.session, h);
            // The item and log both keep only a category — tmux/start errors
            // can embed paths or raw session names.
            applog(&format!(
                "[queue] send FAILED for {} (attempt {}, {}){}",
                crate::applog::session_tag(&item.session),
                item.attempts,
                e.code(),
                if gave_up {
                    " — giving up"
                } else {
                    " (will back off and retry)"
                }
            ));
            SendResult::Failed {
                session: item.session.clone(),
                gave_up,
            }
        }
    }
}

fn context_check(result: &ProbeResult) -> ContextCheck {
    ContextCheck {
        status: result.status,
        code: result.code,
        checked_at: now_epoch(),
    }
}

/// Persist a closed context observation without entering the delivery state
/// machine. A stale worker (item edited/paused/removed while probing) is a
/// no-op. The first valid observation binds an unbound legacy/newly-started
/// item even while its expected process is still pending; identity mismatch
/// observations never rewrite an existing target.
pub(super) fn persist_context_result(
    qm: &Mutex<QueueState>,
    persist: &dyn Fn(&QueueState) -> Result<(), DeckError>,
    selected: &QueueItem,
    result: &ProbeResult,
) -> Result<Option<QueueItem>, DeckError> {
    with_queue(qm, persist, |q| {
        if is_cancelled(q, &selected.session) {
            return Err(TX_NOOP.into());
        }
        let Some(item) = q.items.iter_mut().find(|i| i.id == selected.id) else {
            return Err(TX_NOOP.into());
        };
        if item.revision != selected.revision
            || item.paused
            || matches!(item.state.as_str(), "firing" | "ambiguous")
        {
            return Err(TX_NOOP.into());
        }
        item.last_context = Some(context_check(result));
        if item.binding.is_none() && result.status != ContextStatus::SessionReplaced {
            item.binding = result.identity.clone();
        }
        if result.is_ready() {
            let Some(identity) = result.identity.clone() else {
                return Err("ready context has no pane identity".into());
            };
            item.binding = Some(identity);
        }
        Ok(Some(item.clone()))
    })
    .or_else(|e| {
        if e.message() == TX_NOOP {
            Ok(None)
        } else {
            Err(e)
        }
    })
}

/// A delete can land after a dead-session readiness worker's first
/// cancellation check but before that worker starts tmux. The deleting path
/// cannot kill a session that does not exist yet, so the worker must inspect
/// the tombstone after its probe and reap anything it may have just started.
fn reap_probe_start_after_delete(qm: &Mutex<QueueState>, session: &str, h: &SendHooks) -> bool {
    let cancelled = is_cancelled(&qm.lock_or_recover(), session);
    reap_if_cancelled(cancelled, session, h);
    cancelled
}

/// Context-safe front half of one send. No firing intent or delivery attempt
/// exists until both readiness and a final exact-identity probe pass.
pub(crate) fn send_one_safe(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    session: &str,
    now_min: u32,
    activity: &HashMap<String, u64>,
    h: &SendHooks,
    context_hooks: &ContextHooks,
) -> SendResult {
    send_one_safe_requested(
        qm,
        dirty,
        SendRequest {
            session,
            now_min,
            activity,
            requested: None,
        },
        h,
        context_hooks,
    )
}

pub(super) fn send_one_safe_requested(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    request: SendRequest<'_>,
    h: &SendHooks,
    context_hooks: &ContextHooks,
) -> SendResult {
    let selected = {
        let q = qm.lock_or_recover();
        select_for_request(
            &q,
            request.session,
            now_epoch(),
            request.now_min,
            request.activity,
            request.requested,
        )
    };
    let Some(selected) = selected else {
        return SendResult::Nothing;
    };
    let cancelled = || {
        let q = qm.lock_or_recover();
        is_cancelled(&q, &selected.session)
            || !q.items.iter().any(|i| {
                i.id == selected.id
                    && i.revision == selected.revision
                    && !i.paused
                    && !matches!(i.state.as_str(), "firing" | "ambiguous")
            })
    };
    let prepared = (context_hooks.prepare)(&selected, &cancelled);
    if reap_probe_start_after_delete(qm, &selected.session, h) {
        return SendResult::Nothing;
    }
    if !prepared.is_ready() {
        return match persist_context_result(qm, h.persist, &selected, &prepared) {
            Ok(Some(_)) => SendResult::Blocked {
                session: selected.session,
                status: prepared.status,
                code: prepared.code,
            },
            Ok(None) => {
                reap_probe_start_after_delete(qm, &selected.session, h);
                SendResult::Nothing
            }
            Err(e) => {
                applog(&format!(
                    "[queue] persist (context-blocked) FAILED ({})",
                    e.code()
                ));
                SendResult::NotPersisted
            }
        };
    }
    let bound = match persist_context_result(qm, h.persist, &selected, &prepared) {
        Ok(Some(item)) => item,
        Ok(None) => {
            reap_probe_start_after_delete(qm, &selected.session, h);
            return SendResult::Nothing;
        }
        Err(e) => {
            applog(&format!(
                "[queue] persist (context-ready) FAILED ({}) — not sending this tick",
                e.code()
            ));
            return SendResult::NotPersisted;
        }
    };

    // Re-read metadata immediately before opening the irreversible firing
    // window. Exact pane targeting below closes the name-reuse race.
    let final_result = (context_hooks.final_probe)(&bound);
    if !final_result.is_ready() {
        return match persist_context_result(qm, h.persist, &bound, &final_result) {
            Ok(Some(_)) => SendResult::Blocked {
                session: bound.session,
                status: final_result.status,
                code: final_result.code,
            },
            Ok(None) => {
                reap_probe_start_after_delete(qm, &bound.session, h);
                SendResult::Nothing
            }
            Err(e) => {
                applog(&format!(
                    "[queue] persist (context-final) FAILED ({})",
                    e.code()
                ));
                SendResult::NotPersisted
            }
        };
    }
    let identity = bound.binding.as_ref().expect("ready binding persisted");
    send_one_guarded(
        qm,
        dirty,
        request,
        h,
        Some((&bound.id, bound.revision, identity)),
    )
}
