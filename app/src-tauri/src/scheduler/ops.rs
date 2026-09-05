//! Queue commands and user-driven mutations. Every mutation goes through
//! `with_queue` (persist-then-commit) and a mid-send item refuses edits
//! until its delivery finalizes.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};

use super::*;
use crate::applog::applog;
use crate::context::{self, ContextCode, ContextStatus, PaneIdentity};
use crate::datadir::now_epoch;
use crate::error::{DeckError, ErrorKind};
use crate::storage;
use crate::sync::LockRecover;

#[tauri::command]
pub(crate) fn queue_list(state: State<'_, Queues>) -> QueueState {
    state.q.lock_or_recover().clone()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextProbeView {
    status: ContextStatus,
    code: ContextCode,
    expected_process: Option<String>,
    current_process: Option<String>,
}

/// User-requested observation for the queue UI. It is metadata-only and does
/// not start a session, mutate the queue, consume an attempt, or capture pane
/// contents.
#[tauri::command]
pub(crate) fn queue_probe_context(
    state: State<'_, Queues>,
    id: String,
) -> Result<ContextProbeView, DeckError> {
    let item = state
        .q
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or(DeckError::new(
            ErrorKind::Missing,
            "scheduled prompt not found",
        ))?;
    let result = current_context_probe(&item);
    persist_context_result(&state.q, &save_queue, &item, &result)?.ok_or(DeckError::new(
        ErrorKind::Other,
        "scheduled prompt changed while probing",
    ))?;
    Ok(ContextProbeView {
        status: result.status,
        code: result.code,
        expected_process: item.expected_process,
        current_process: result.current_process,
    })
}

#[tauri::command]
pub(crate) fn smoke_seed_ambiguous(state: State<'_, Queues>) -> Result<(), DeckError> {
    if !crate::smoke_faults::enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke queue hooks are unavailable",
        ));
    }
    let mut q = state.q.lock_or_recover();
    let item = q
        .items
        .first_mut()
        .ok_or(DeckError::new(ErrorKind::Other, "smoke queue is empty"))?;
    let delivery = "smoke-delivery".to_string();
    item.state = "firing".into();
    item.delivery = Some(delivery.clone());
    let snapshot = item.clone();
    q.pending.clear();
    q.pending.push(PendingDelivery {
        id: delivery,
        snapshot,
    });
    save_queue(&q)
}

#[derive(Serialize)]
pub(crate) struct SmokeQueueState {
    dirty: bool,
    disk_matches: bool,
}

#[tauri::command]
pub(crate) fn smoke_queue_state(state: State<'_, Queues>) -> Result<SmokeQueueState, DeckError> {
    if !crate::smoke_faults::enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke queue hooks are unavailable",
        ));
    }
    let q = state.q.lock_or_recover().clone();
    let disk = storage::load_typed::<QueueState>(&queue_path())?.ok_or(DeckError::new(
        ErrorKind::Other,
        "smoke queue file is missing",
    ))?;
    let disk: QueueState = serde_json::from_str(&disk.payload).map_err(DeckError::from)?;
    Ok(SmokeQueueState {
        dirty: state.dirty.load(AtomicOrdering::Relaxed),
        disk_matches: serde_json::to_value(q).ok() == serde_json::to_value(disk).ok(),
    })
}

#[tauri::command]
pub(crate) fn smoke_flush_queue(state: State<'_, Queues>) -> Result<bool, DeckError> {
    if !crate::smoke_faults::enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke queue hooks are unavailable",
        ));
    }
    Ok(flush_dirty(&state.q, &state.dirty, &save_queue))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueAddArgs {
    pub(crate) session: String,
    #[serde(default)]
    pub(crate) card_id: String,
    pub(crate) dir: String,
    pub(crate) cmd: String,
    pub(crate) text: String,
    pub(crate) mode: String,
    pub(crate) at: Option<u64>,
    pub(crate) every: Option<u64>,
    pub(crate) win_from: Option<u32>,
    pub(crate) win_to: Option<u32>,
    pub(crate) until_n: Option<u32>,
    pub(crate) until_at: Option<u64>,
    pub(crate) steps: Option<Vec<String>>,
    pub(crate) tpl: Option<String>,
    pub(crate) tpl_idx: Option<u32>,
    pub(crate) tpl_total: Option<u32>,
}

/// Reject invalid schedule combinations up front.
pub(crate) fn validate_add(a: &QueueAddArgs) -> Result<(), DeckError> {
    crate::tmux::validate_session_name(&a.session)?;
    match a.mode.as_str() {
        "at" => {
            if a.at.is_none() {
                return Err(DeckError::new(
                    ErrorKind::Other,
                    "a timed prompt needs its time",
                ));
            }
        }
        "chain" => {}
        "every" => {
            let e = a.every.ok_or(DeckError::new(
                ErrorKind::Other,
                "a recurring rule needs an interval",
            ))?;
            if e < 60 {
                return Err(DeckError::new(
                    ErrorKind::Other,
                    "recurring interval must be at least 1 minute",
                ));
            }
        }
        m => {
            return Err(DeckError::new(
                ErrorKind::Other,
                format!("unknown schedule mode: {m}"),
            ))
        }
    }
    if a.mode != "every" && (a.every.is_some() || a.steps.as_ref().is_some_and(|s| !s.is_empty())) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "interval/steps only make sense on a recurring rule",
        ));
    }
    for w in [a.win_from, a.win_to] {
        if w.is_some_and(|m| m >= 1440) {
            return Err(DeckError::new(
                ErrorKind::Other,
                "time-window minutes must be below 24h",
            ));
        }
    }
    if a.win_from.is_some() != a.win_to.is_some() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "a time window needs both ends",
        ));
    }
    if a.until_n.is_some_and(|n| n == 0) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "stop-after count must be at least 1",
        ));
    }
    if a.session.trim().is_empty() {
        return Err(DeckError::new(ErrorKind::Other, "missing session"));
    }
    if a.card_id.is_empty()
        || a.card_id.len() > 128
        || !a
            .card_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(DeckError::new(
            ErrorKind::Other,
            "scheduled prompt needs a valid card identity",
        ));
    }
    Ok(())
}

/// Collision-proof id: ms clock + process-wide counter, verified against the
/// live queue (the old `q<sec>-<len>` scheme collided after a remove+add in
/// the same second).
pub(crate) fn next_queue_id(existing: &[QueueItem]) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    loop {
        let id = format!("q{ms}-{}", SEQ.fetch_add(1, Ordering::Relaxed));
        if !existing.iter().any(|i| i.id == id) {
            return id;
        }
    }
}

/// Pure core of queue_add: append one already-validated item to the
/// candidate state (never to the shared state directly — see `with_queue`).
#[cfg(test)]
pub(crate) fn add_item(
    q: &mut QueueState,
    args: QueueAddArgs,
    text: String,
) -> Result<(), DeckError> {
    let expected_process = context::expected_from_command(&args.cmd);
    add_item_bound(q, args, text, None, expected_process)
}

fn add_item_bound(
    q: &mut QueueState,
    args: QueueAddArgs,
    text: String,
    binding: Option<PaneIdentity>,
    expected_process: Option<String>,
) -> Result<(), DeckError> {
    // Scheduling for a session again means it is alive again (the UI can
    // only add prompts from a live card), so a stale tombstone from an
    // earlier card of the same name must not silently swallow the schedule.
    q.cancelled.retain(|t| t.session != args.session);
    let id = next_queue_id(&q.items);
    let (group, seq) = if args.mode == "every" {
        (None, None) // rules carry no group; their iterations get one at spawn
    } else if args.mode == "chain" {
        // join the newest existing group of this session (matches the queue
        // panel's visual grouping); otherwise start a group of its own
        let joined = q
            .items
            .iter()
            .rev()
            .filter(|i| i.session == args.session && i.mode != "every")
            .find_map(|i| i.group.clone());
        match joined {
            Some(g) => {
                let max = q
                    .items
                    .iter()
                    .filter(|i| i.group.as_deref() == Some(g.as_str()))
                    .filter_map(|i| i.seq)
                    .max()
                    .unwrap_or(1);
                (Some(g), Some(max + 1))
            }
            None => (Some(id.clone()), Some(1)),
        }
    } else {
        (Some(id.clone()), Some(1))
    };
    q.items.push(QueueItem {
        id,
        session: args.session,
        card_id: args.card_id,
        dir: args.dir,
        cmd: args.cmd,
        text,
        mode: args.mode,
        at: args.at,
        added: now_epoch(),
        every: args.every,
        win_from: args.win_from,
        win_to: args.win_to,
        until_n: args.until_n,
        until_at: args.until_at,
        fired: 0,
        paused: false,
        last: None,
        state: default_state(),
        attempts: 0,
        last_error: None,
        last_attempt_at: None,
        steps: args.steps.unwrap_or_default(),
        tpl: args.tpl,
        tpl_idx: args.tpl_idx,
        tpl_total: args.tpl_total,
        group,
        seq,
        rule: None,
        delivery: None,
        expected_process,
        binding,
        last_context: None,
        revision: 0,
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_add(
    state: State<'_, Queues>,
    app: AppHandle,
    args: QueueAddArgs,
) -> Result<(), DeckError> {
    validate_add(&args)?;
    let text = args.text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err(DeckError::new(ErrorKind::Other, "empty prompt"));
    }
    // never let the scheduler act on an item the disk doesn't know about
    let creation = context::creation_context(&args.session, &args.cmd);
    with_queue(&state.q, &save_queue, |q| {
        add_item_bound(q, args, text, creation.binding, creation.expected_process)
    })?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// The firing contract for user operations: while an item is mid-send
/// ("firing" persisted, injection possibly in flight), mutating it would race
/// the delivery — remove/update/pause/retry/skip are refused with a clear
/// conflict error and the item is kept until finalize completes. The window
/// is at most one send (seconds); the UI surfaces the error as a toast.
pub(crate) fn firing_conflict(q: &QueueState, id: &str) -> Result<(), DeckError> {
    if let Some(i) = q.items.iter().find(|i| i.id == id) {
        if i.state == "firing" {
            return Err(DeckError::new(
                ErrorKind::Other,
                "this prompt is being sent right now — try again in a few seconds",
            ));
        }
        if i.state == "ambiguous" {
            return Err(DeckError::new(
                ErrorKind::Other,
                "this prompt has an ambiguous delivery — acknowledge or retry it first",
            ));
        }
    }
    Ok(())
}

/// Pure core of queue_update (unit-tested with the firing contract).
pub(crate) fn update_text(q: &mut QueueState, id: &str, text: String) -> Result<(), DeckError> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.text = text;
        item.revision = item.revision.wrapping_add(1);
        item.last_context = None;
    }
    Ok(())
}

/// Pure core of queue_remove / queue_skip.
pub(crate) fn remove_item(q: &mut QueueState, id: &str) -> Result<bool, DeckError> {
    firing_conflict(q, id)?;
    let n0 = q.items.len();
    q.items.retain(|i| i.id != id);
    Ok(q.items.len() != n0)
}

/// Pure core of queue_pause.
pub(crate) fn pause_item(q: &mut QueueState, id: &str, paused: bool) -> Result<(), DeckError> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.paused = paused;
    }
    Ok(())
}

/// Pure core of queue_retry.
pub(crate) fn retry_item(q: &mut QueueState, id: &str) -> Result<(), DeckError> {
    if q.items.iter().any(|i| i.id == id && i.state == "firing") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "this prompt is being sent right now — try again in a few seconds",
        ));
    }
    let delivery = q
        .items
        .iter()
        .find(|i| i.id == id && i.state == "ambiguous")
        .and_then(|i| i.delivery.clone());
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        if !matches!(
            item.state.as_str(),
            "firing" | "ambiguous" | "failed" | "pending"
        ) {
            return Err(DeckError::new(
                ErrorKind::Other,
                "prompt has an unknown delivery state",
            ));
        }
        item.state = default_state();
        item.attempts = 0;
        item.last_error = None;
        item.last_attempt_at = None;
        item.delivery = None;
    } else if q.deliveries.iter().any(|d| d.item == id) {
        return Ok(()); // repeated resolution of a consumed once item
    }
    if let Some(delivery) = delivery {
        q.pending.retain(|p| p.id != delivery);
    }
    Ok(())
}

/// Resolve an uncertain delivery as sent. Accounting is performed on the
/// candidate state and reaches memory only after persistence succeeds.
/// Replays are no-ops, including once/template steps already consumed.
pub(crate) fn acknowledge_ambiguous(q: &mut QueueState, id: &str) -> Result<(), DeckError> {
    let Some(item) = q.items.iter().find(|i| i.id == id).cloned() else {
        return if q.deliveries.iter().any(|d| d.item == id) {
            Ok(())
        } else {
            Err(DeckError::new(
                ErrorKind::Missing,
                "scheduled prompt not found",
            ))
        };
    };
    if item.state != "ambiguous" {
        return if item.delivery.is_none() && q.deliveries.iter().any(|d| d.item == id) {
            Ok(())
        } else {
            Err(DeckError::new(
                ErrorKind::Other,
                "this prompt is not awaiting an ambiguous-delivery decision",
            ))
        };
    }
    let delivery = item.delivery.unwrap_or_else(|| format!("legacy-{id}"));
    let when = item.last_attempt_at.unwrap_or_else(now_epoch);
    finalize_delivery(q, id, &delivery, when, true);
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_update(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    text: String,
) -> Result<(), DeckError> {
    let text = text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err(DeckError::new(ErrorKind::Other, "empty prompt"));
    }
    // a failed save must not leave the new text in memory: the scheduler
    // would then send a prompt the user was told was not saved
    with_queue(&state.q, &save_queue, |q| update_text(q, &id, text))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_remove(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), DeckError> {
    with_queue(&state.q, &save_queue, |q| remove_item(q, &id))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_pause(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    paused: bool,
) -> Result<(), DeckError> {
    // a failed save keeps the OLD pause state in force, matching the error
    with_queue(&state.q, &save_queue, |q| pause_item(q, &id, paused))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Give a failed item a fresh set of attempts (user-driven).
#[tauri::command]
pub(crate) fn queue_retry(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), DeckError> {
    // a failed save must not re-arm the item in memory only
    with_queue(&state.q, &save_queue, |q| retry_item(q, &id))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Explicitly treat an ambiguous delivery as sent. This is the only recovery
/// path that performs delivery accounting; boot never does so on its own.
#[tauri::command]
pub(crate) fn queue_acknowledge(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), DeckError> {
    with_queue(&state.q, &save_queue, |q| acknowledge_ambiguous(q, &id))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Explicitly skip a failed step so the rest of its group can continue.
/// (A dead step blocks its group's later steps until the user decides.)
#[tauri::command]
pub(crate) fn queue_skip(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), DeckError> {
    if with_queue(&state.q, &save_queue, |q| remove_item(q, &id))? {
        applog("[queue] step skipped by user — group unblocked");
    }
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Has this session been deleted? A tombstoned session is never eligible,
/// never restarted, and never re-armed by a finalizing delivery.
pub(crate) fn is_cancelled(q: &QueueState, session: &str) -> bool {
    q.cancelled.iter().any(|t| t.session == session)
}

/// Pure core of queue_clear_session(s) — the deletion semantics:
///
/// 1. the session is tombstoned (persisted, so it survives a crash);
/// 2. EVERY item of that session is dropped, including one that is mid-send
///    — a live delivery still finalizes from the pending-ledger snapshot, so
///    its audit is complete, but there is no item left to
///    re-arm, no rule to restore, no template step to spawn and no next
///    cadence (see finalize_delivery's cancelled branch);
/// 3. the session's last-fired bookkeeping goes with it.
///
/// Idempotent: clearing twice refreshes the tombstone and changes nothing
/// else, and it never errors.
pub(crate) fn clear_session_items(q: &mut QueueState, session: &str) {
    let now = now_epoch();
    match q.cancelled.iter_mut().find(|t| t.session == session) {
        Some(t) => t.at = now,
        None => q.cancelled.push(Tombstone {
            session: session.to_string(),
            at: now,
        }),
    }
    if q.cancelled.len() > MAX_TOMBSTONES {
        let n = q.cancelled.len() - MAX_TOMBSTONES;
        q.cancelled.drain(..n);
    }
    q.items.retain(|i| i.session != session);
    q.last_fired.remove(session);
}

/// Cancel a whole set of sessions in ONE transaction — deleting a project
/// must not be able to half-clear its cards.
pub(crate) fn clear_sessions(q: &mut QueueState, sessions: &[String]) {
    for s in sessions {
        clear_session_items(q, s);
    }
}

/// Permanently cancel every scheduled prompt of these sessions — called
/// when a card is closed, a project deleted, or a shell exits on its own.
/// The frontend must not remove the card(s) until this resolved: a rejected
/// promise means the cancellation is NOT on disk and the board must keep
/// showing the card rather than hide a session that still has a schedule.
#[tauri::command]
pub(crate) fn queue_clear_sessions(
    state: State<'_, Queues>,
    app: AppHandle,
    sessions: Vec<String>,
) -> Result<(), DeckError> {
    if crate::smoke_faults::take("queue-cancel") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "injected queue cancellation failure",
        ));
    }
    with_queue(&state.q, &save_queue, |q| {
        clear_sessions(q, &sessions);
        Ok(())
    })?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Immediate delivery is one-shot. It can bypass only a freshly proven
/// foreground mismatch; exact identity remains mandatory in both probes and
/// the atomic paste guard. No persisted protection is weakened.
#[tauri::command]
pub(crate) fn queue_send_now(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    accept_process_mismatch: bool,
) -> Result<(), DeckError> {
    let item = state
        .q
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or(DeckError::new(
            ErrorKind::Missing,
            "scheduled prompt not found",
        ))?;
    let observed = current_context_probe(&item);
    if accept_process_mismatch && observed.status != ContextStatus::ForegroundDifferent {
        return Err(DeckError::new(
            ErrorKind::Other,
            "one-shot process bypass requires a current foreground mismatch",
        ));
    }
    if !claim_session(&state.busy, &item.session) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "this session already has a scheduled send in progress",
        ));
    }
    let prepare_once = |source: &QueueItem, cancelled: &dyn Fn() -> bool| {
        let mut one_shot = source.clone();
        if accept_process_mismatch {
            one_shot.expected_process = None;
        }
        prepare_context(&one_shot, cancelled)
    };
    let final_once = |source: &QueueItem| {
        let mut one_shot = source.clone();
        if accept_process_mismatch {
            one_shot.expected_process = None;
        }
        final_context_probe(&one_shot)
    };
    let fire_once = |source: &QueueItem| {
        let mut one_shot = source.clone();
        if accept_process_mismatch {
            one_shot.expected_process = None;
        }
        fire_item(&one_shot)
    };
    let result = send_one_safe_requested(
        &state.q,
        &state.dirty,
        SendRequest {
            session: &item.session,
            now_min: local_minutes(),
            activity: &HashMap::new(),
            requested: Some(&id),
        },
        &SendHooks {
            fire: &fire_once,
            persist: &save_queue,
            kill: &kill_session_quietly,
        },
        &ContextHooks {
            prepare: &prepare_once,
            final_probe: &final_once,
        },
    );
    release_session(&state.busy, &item.session);
    match result {
        SendResult::Sent { session } => {
            let _ = app.emit("queue-fired", QueueFired { session });
            let _ = app.emit("queue-changed", ());
            Ok(())
        }
        SendResult::Blocked { .. } => Err(DeckError::new(
            ErrorKind::Other,
            "target context is unavailable",
        )),
        SendResult::Nothing => Err(DeckError::new(
            ErrorKind::Other,
            "prompt is no longer eligible to send",
        )),
        SendResult::NotPersisted => Err(DeckError::new(
            ErrorKind::Other,
            "delivery intent could not be saved",
        )),
        SendResult::Failed { .. } => Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux refused the literal send",
        )),
    }
}
