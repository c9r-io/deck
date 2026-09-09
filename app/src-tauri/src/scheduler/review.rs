//! C v01: human checkpoints in the existing queue group, never agent readiness.
//! A sent row remains `review`; only a persisted, revision/target-bound human
//! decision makes it `review-approved`. Its successor still passes normal
//! scheduling/context checks. Keep that checkpoint until the successor is sent,
//! so edits, target changes and retries can revoke the unused permission.
//! The last checkpoint is removed on explicit inspection. It has no timeout.
//! Other groups remain independent. Content-free decisions are capped at 200.

use super::*;
use crate::datadir::now_epoch;
use crate::sync::LockRecover;
use tauri::{AppHandle, Emitter, State};

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ReviewCheckpoint {
    pub(crate) delivery: String,
    pub(crate) permit: Option<ReviewPermit>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewPermit {
    pub(crate) next_id: String,
    next_revision: u64,
    binding: PaneIdentity,
    expected_process: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ReviewRecord {
    item: String,
    delivery: String,
    revision: u64,
    session: String,
    at: u64,
    next: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ReviewDecision {
    id: String,
    delivery: String,
    revision: u64,
    next: Option<ReviewPermit>,
    binding: PaneIdentity,
}

#[derive(Serialize)]
pub(crate) struct ReviewPreview {
    decision: ReviewDecision,
    next_text: Option<String>,
    current_process: Option<String>,
    expected_process: Option<String>,
    repeated: bool,
}

pub(crate) fn is_review(i: &QueueItem) -> bool {
    matches!(i.state.as_str(), "review" | "review-approved")
}

pub(crate) fn review_error() -> DeckError {
    DeckError::new(
        ErrorKind::Other,
        "checkpoint changed or no longer available — inspect the current plan again",
    )
}

fn successor<'a>(q: &'a QueueState, item: &QueueItem) -> Option<&'a QueueItem> {
    q.items
        .iter()
        .filter(|i| {
            i.session == item.session
                && i.group == item.group
                && i.id != item.id
                && i.seq > item.seq
        })
        .min_by_key(|i| (i.seq, i.added))
}

pub(crate) fn review_allows(q: &QueueState, item: &QueueItem) -> bool {
    q.items
        .iter()
        .filter(|i| {
            i.session == item.session && i.group == item.group && i.state == "review-approved"
        })
        .all(|i| {
            i.review
                .as_ref()
                .and_then(|r| r.permit.as_ref())
                .is_some_and(|p| {
                    p.next_id == item.id
                        && p.next_revision == item.revision
                        && p.expected_process == item.expected_process
                })
        })
}

pub(crate) fn invalidate_review_successor(q: &mut QueueState, id: &str) {
    for item in &mut q.items {
        if item
            .review
            .as_ref()
            .and_then(|r| r.permit.as_ref())
            .is_some_and(|p| p.next_id == id)
        {
            item.state = "review".into();
            item.revision = item.revision.wrapping_add(1);
            item.review.as_mut().unwrap().permit = None;
        }
    }
}

/// Persisting the newly observed target also revokes a permit for an older one.
/// Returns true if selection must stop and ask for inspection again.
pub(crate) fn invalidate_review_target(
    q: &mut QueueState,
    id: &str,
    identity: Option<&PaneIdentity>,
) -> bool {
    let changed = q.items.iter().any(|i| {
        i.review
            .as_ref()
            .and_then(|r| r.permit.as_ref())
            .is_some_and(|p| p.next_id == id && identity != Some(&p.binding))
    });
    if changed {
        invalidate_review_successor(q, id);
    }
    changed
}

pub(super) fn decision_for(
    q: &QueueState,
    id: &str,
    binding: PaneIdentity,
) -> Result<ReviewDecision, DeckError> {
    let item = q
        .items
        .iter()
        .find(|i| i.id == id && is_review(i))
        .ok_or_else(review_error)?;
    if is_cancelled(q, &item.session) {
        return Err(review_error());
    }
    let next = successor(q, item);
    if next.is_some_and(|i| matches!(i.state.as_str(), "firing" | "ambiguous")) {
        return Err(review_error());
    }
    Ok(ReviewDecision {
        id: item.id.clone(),
        delivery: item
            .review
            .as_ref()
            .ok_or_else(review_error)?
            .delivery
            .clone(),
        revision: item.revision,
        next: next.map(|n| ReviewPermit {
            next_id: n.id.clone(),
            next_revision: n.revision,
            binding: binding.clone(),
            expected_process: n.expected_process.clone(),
        }),
        binding,
    })
}

/// Pure transactional core: the preview's exact successor/revision/target must
/// still be current. Replays of the same decision never release a later step.
pub(crate) fn confirm_review(
    q: &mut QueueState,
    decision: &ReviewDecision,
    binding: PaneIdentity,
    now: u64,
) -> Result<(), DeckError> {
    if q.reviews.iter().any(|r| {
        r.item == decision.id && r.delivery == decision.delivery && r.revision == decision.revision
    }) {
        return Ok(());
    }
    if &decision_for(q, &decision.id, binding)? != decision {
        return Err(review_error());
    }
    let item = q
        .items
        .iter_mut()
        .find(|i| i.id == decision.id)
        .ok_or_else(review_error)?;
    if item.paused {
        return Err(review_error());
    }
    let record = ReviewRecord {
        item: item.id.clone(),
        delivery: decision.delivery.clone(),
        revision: decision.revision,
        session: item.session.clone(),
        at: now,
        next: decision.next.as_ref().map(|p| p.next_id.clone()),
    };
    if let Some(permit) = &decision.next {
        item.state = "review-approved".into();
        item.review.as_mut().ok_or_else(review_error)?.permit = Some(permit.clone());
    } else {
        q.items.retain(|i| i.id != decision.id);
    }
    if decision.next.is_none()
        && !q
            .items
            .iter()
            .any(|i| i.session == record.session && (i.review_each || is_review(i)))
    {
        q.review_completed.insert(record.session.clone());
    }
    q.reviews.push(record);
    if q.reviews.len() > MAX_DELIVERIES {
        q.reviews.remove(0);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_review_preview(
    state: State<'_, Queues>,
    id: String,
) -> Result<ReviewPreview, DeckError> {
    let q = state.q.lock_or_recover().clone();
    let item = q
        .items
        .iter()
        .find(|i| i.id == id && is_review(i))
        .ok_or_else(review_error)?;
    // Read only: inspecting a stopped target never starts it.
    let raw = context::raw_probe(&item.session)?;
    let next = successor(&q, item);
    Ok(ReviewPreview {
        decision: decision_for(&q, &id, raw.identity.clone())?,
        next_text: next.map(|i| i.text.clone()),
        current_process: raw.foreground_name(),
        expected_process: next.unwrap_or(item).expected_process.clone(),
        repeated: item.rule.is_some(),
    })
}

#[tauri::command]
pub(crate) fn queue_review_confirm(
    state: State<'_, Queues>,
    app: AppHandle,
    decision: ReviewDecision,
) -> Result<(), DeckError> {
    let session = {
        let q = state.q.lock_or_recover();
        // A replay after the last checkpoint was removed is an honest no-op.
        if q.reviews.iter().any(|r| {
            r.item == decision.id
                && r.delivery == decision.delivery
                && r.revision == decision.revision
        }) {
            return Ok(());
        }
        q.items
            .iter()
            .find(|i| i.id == decision.id)
            .ok_or_else(review_error)?
            .session
            .clone()
    };
    let binding = context::raw_probe(&session)?.identity;
    with_queue(&state.q, &save_queue, |q| {
        confirm_review(q, &decision, binding, now_epoch())
    })?;
    let _ = app.emit("queue-changed", ());
    wake_scheduler();
    Ok(())
}

/// Explicit list cancellation is the only way to remove an unchecked row
/// without claiming inspection. Includes a repeating rule and its live iteration.
pub(crate) fn cancel_list(q: &mut QueueState, id: &str) -> Result<(), DeckError> {
    let Some(item) = q.items.iter().find(|i| i.id == id).cloned() else {
        return Ok(());
    };
    let rule = if item.mode == "every" {
        Some(item.id.clone())
    } else {
        item.rule.clone()
    };
    let selected = |i: &QueueItem| {
        i.session == item.session
            && (if let Some(r) = &rule {
                i.id == *r || i.rule.as_ref() == Some(r)
            } else {
                i.group == item.group
            })
    };
    for i in q.items.iter().filter(|i| selected(i) && !is_review(i)) {
        firing_conflict(q, &i.id)?;
    }
    if q.items
        .iter()
        .any(|i| selected(i) && (i.review_each || is_review(i)))
    {
        q.review_completed.remove(&item.session);
    }
    q.items.retain(|i| !selected(i));
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_cancel_list(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), DeckError> {
    with_queue(&state.q, &save_queue, |q| cancel_list(q, &id))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Explicitly change future unsent rows only. Existing checkpoints always stay;
/// disabling the mode cannot silently release already-required inspection.
pub(crate) fn set_review_mode(
    q: &mut QueueState,
    id: &str,
    enabled: bool,
) -> Result<(), DeckError> {
    let item = q
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or_else(review_error)?;
    let rule = if item.mode == "every" {
        Some(item.id.clone())
    } else {
        item.rule.clone()
    };
    let ids: Vec<_> = q
        .items
        .iter()
        .filter(|i| {
            i.session == item.session
                && (if let Some(r) = &rule {
                    i.id == *r || i.rule.as_ref() == Some(r)
                } else {
                    i.group == item.group
                })
        })
        .map(|i| i.id.clone())
        .collect();
    for id in &ids {
        if q.items.iter().any(|i| i.id == *id && !is_review(i)) {
            firing_conflict(q, id)?;
        }
    }
    for id in &ids {
        invalidate_review_successor(q, id);
    }
    for i in q.items.iter_mut().filter(|i| ids.contains(&i.id)) {
        i.review_each = enabled;
        i.revision = i.revision.wrapping_add(1);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_review_mode(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<(), DeckError> {
    with_queue(&state.q, &save_queue, |q| set_review_mode(q, &id, enabled))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct QueuePlan {
    item: String,
    stage: &'static str,
    checked_at: u64,
    quiet_remaining: Option<u64>,
    gap_until: Option<u64>,
}

pub(crate) fn plan_item(
    q: &QueueState,
    i: &QueueItem,
    now: u64,
    minutes: u32,
    activity: Option<&HashMap<String, u64>>,
) -> QueuePlan {
    let quiet_remaining = activity
        .and_then(|a| a.get(&i.session))
        .map(|a| (a + i.quiet_secs.unwrap_or(CHAIN_QUIET_SECS)).saturating_sub(now));
    let gap_until = q
        .last_fired
        .get(&i.session)
        .map(|t| t + SESSION_MIN_GAP_SECS)
        .filter(|t| *t > now);
    let earlier = q.items.iter().any(|x| {
        x.session == i.session
            && x.group == i.group
            && x.id != i.id
            && x.mode != "every"
            && x.state != "review-approved"
            && (x.seq, x.added) < (i.seq, i.added)
    });
    let stage = if i.state == "review" {
        "review"
    } else if i.state == "review-approved" {
        "review-approved"
    } else if i.state == "ambiguous" {
        "ambiguous"
    } else if i.state == "firing" {
        "firing"
    } else if item_dead(i) {
        "failed"
    } else if i.paused {
        "paused"
    } else if !retry_ok(i, now) {
        "retry"
    } else if i.mode != "every" && (earlier || !review_allows(q, i)) {
        "previous"
    } else if i.mode == "every" && q.items.iter().any(|x| x.rule.as_deref() == Some(&i.id)) {
        "iteration"
    } else if gap_until.is_some() {
        "gap"
    } else if i.mode == "at" && i.at.is_some_and(|at| at > now)
        || i.mode == "every" && !every_due(i, now, minutes)
    {
        "time"
    } else if i.mode == "chain" && quiet_remaining.is_some_and(|s| s > 0) {
        "quiet"
    } else if activity.is_none() {
        "unknown"
    } else {
        "context"
    };
    QueuePlan {
        item: i.id.clone(),
        stage,
        checked_at: now,
        quiet_remaining,
        gap_until,
    }
}

#[derive(Serialize)]
pub(crate) struct QueueView {
    #[serde(flatten)]
    queue: QueueState,
    plans: Vec<QueuePlan>,
}

pub(crate) fn queue_view(q: QueueState) -> QueueView {
    let now = now_epoch();
    let activity = crate::tmux::list_panes().ok().map(|rows| {
        rows.into_iter()
            .map(|r| (r.session_name, r.window_activity))
            .collect()
    });
    let plans = q
        .items
        .iter()
        .map(|i| plan_item(&q, i, now, local_minutes(), activity.as_ref()))
        .collect();
    QueueView { queue: q, plans }
}
