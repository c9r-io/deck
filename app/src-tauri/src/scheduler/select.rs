//! Candidate selection: deterministic priority (backoff retry → due `at`
//! → cadence `every` → chain), one candidate per session per tick, and
//! expiry.
//!
//! # Agent hold
//! The agent hook's closed state word (`agent_status`) can only HOLD a row,
//! never release one, and it holds automatic selection only (`agent_holds`):
//! - no row of any mode is selected while the session's agent reports
//!   `needs-input` — the pasted text and its Enter would answer the agent's
//!   question or permission prompt (typically accepting the highlighted
//!   "Yes") instead of reaching the prompt box;
//! - an `external` follow-up row (chain) additionally requires a POSITIVE
//!   `turn-done`: quiet alone cannot tell a finished turn from a permission
//!   prompt, and text that arrived from outside deck must never be what
//!   answers one. Without a hook report (hooks not enabled, the agent
//!   exited, a dead session) such a row keeps waiting; the user can still
//!   send it by hand.
//!
//! Owner rows without a hook report keep the quiet-only rule. Manual
//! send-now (`select_requested`) is the user acting while looking at the
//! pane and is not held. A stale `needs-input` (a question dismissed with
//! Esc fires no Stop hook) holds until the next hook word, or until the
//! poll reconciliation sees the agent leave the foreground.

use std::collections::HashMap;

use super::*;
use crate::agent_status;
use crate::tmux::PaneRow;

/// One tick's view of a session: its pane's last output instant and the
/// agent hook's closed state word, if an agent module reported one.
#[derive(Clone, Copy, Default)]
pub(crate) struct Observed {
    pub(crate) activity: u64,
    pub(crate) agent: Option<&'static str>,
}

/// Session name → observation, one snapshot per tick (first pane wins).
pub(crate) type Observations = HashMap<String, Observed>;

pub(crate) fn observe(rows: Vec<PaneRow>) -> Observations {
    let mut seen = Observations::new();
    for row in rows {
        let activity = row.window_activity;
        seen.entry(row.session_name)
            .or_insert_with_key(|session| Observed {
                activity,
                agent: crate::agent_status::current(session),
            });
    }
    seen
}

/// The agent hold (module header): true while the hook state forbids an
/// automatic paste of `i` into its session.
pub(crate) fn agent_holds(i: &QueueItem, seen: Option<&Observed>) -> bool {
    let agent = seen.and_then(|o| o.agent);
    agent == Some(agent_status::NEEDS_INPUT)
        || (i.external && i.mode == "chain" && agent != Some(agent_status::TURN_DONE))
}

/// Deterministic candidate order within a session: retries whose backoff
/// elapsed, then the earliest-due at, then a cadence-due rule, then chains.
fn priority(i: &QueueItem) -> u8 {
    if i.state == ItemState::Failed {
        return 0;
    }
    match i.mode.as_str() {
        "at" => 1,
        "every" => 2,
        _ => 3,
    }
}

/// The first live step of an item's group (lowest seq still queued). Only
/// the head step may fire; a dead head therefore stalls the whole group.
fn group_head<'a>(q: &'a QueueState, i: &QueueItem) -> Option<&'a QueueItem> {
    let g = i.group.as_deref()?;
    q.items
        .iter()
        .filter(|x| x.group.as_deref() == Some(g) && x.state != ItemState::ReviewApproved)
        .min_by_key(|x| (x.seq.unwrap_or(1), x.added))
}

fn eligible(
    q: &QueueState,
    i: &QueueItem,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> bool {
    if is_review(i)
        || !review_allows(q, i)
        || i.paused
        || i.state.blocks_firing()
        || item_dead(i)
        || !retry_ok(i, now)
    {
        return false;
    }
    // the card (or its project) was deleted: nothing of this session ever
    // fires again, and in particular fire_item never restarts its tmux
    // session. Belt and braces — clear_session_items already dropped the
    // items; this also covers a file hand-edited between runs.
    if is_cancelled(q, &i.session) {
        return false;
    }
    // one prompt per session at a time: every mode honors the send gap
    if q.last_fired
        .get(&i.session)
        .is_some_and(|t| now < t + SESSION_MIN_GAP_SECS)
    {
        return false;
    }
    // group discipline: only the head step of a group is ever a candidate
    if i.mode != "every" {
        if let Some(h) = group_head(q, i) {
            if h.id != i.id {
                return false;
            }
        }
    }
    if agent_holds(i, activity.get(&i.session)) {
        return false;
    }
    match i.mode.as_str() {
        "at" => i.at.map(|t| now >= t).unwrap_or(false),
        "chain" => activity
            .get(&i.session)
            .map(|o| now >= o.activity + i.quiet_secs.unwrap_or(CHAIN_QUIET_SECS))
            .unwrap_or(true), // dead session = quiet; fire_item restarts it
        "every" => {
            // a rule may not start a new iteration while any item of its
            // previous one is still live (queued, retrying or blocked)
            every_due(i, now, now_min)
                && !q
                    .items
                    .iter()
                    .any(|x| x.rule.as_deref() == Some(i.id.as_str()))
        }
        _ => false,
    }
}

/// The ONE item this session may fire now — recomputed from fresh state
/// immediately before every send, never from a stale tick-wide snapshot.
pub(crate) fn select_for_session(
    q: &QueueState,
    session: &str,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> Option<QueueItem> {
    q.items
        .iter()
        .filter(|i| i.session == session && eligible(q, i, now, now_min, activity))
        .min_by_key(|i| {
            let class_time = match (i.state, i.mode.as_str()) {
                (ItemState::Failed, _) => i.last_attempt_at.unwrap_or(0),
                (_, "at") => i.at.unwrap_or(0),
                _ => i.added,
            };
            (priority(i), class_time, i.added)
        })
        .cloned()
}

/// Explicit manual-now selection skips only the schedule now_min/quiet/window
/// and the agent hold (the user is acting while looking at the pane).
/// It retains every ordering and exclusivity invariant: pause/ambiguous/dead,
/// tombstone, session gap, group head and one-active-rule-iteration.
pub(super) fn select_requested(
    q: &QueueState,
    session: &str,
    id: &str,
    now: u64,
) -> Option<QueueItem> {
    let item = q
        .items
        .iter()
        .find(|i| i.id == id && i.session == session)?;
    if is_review(item)
        || !review_allows(q, item)
        || item.paused
        || item.state.blocks_firing()
        || item_dead(item)
        || is_cancelled(q, session)
        || q.last_fired
            .get(session)
            .is_some_and(|t| now < t + SESSION_MIN_GAP_SECS)
    {
        return None;
    }
    if item.mode != "every" && group_head(q, item).is_some_and(|head| head.id != item.id) {
        return None;
    }
    if item.mode == "every"
        && q.items
            .iter()
            .any(|other| other.rule.as_deref() == Some(item.id.as_str()))
    {
        return None;
    }
    Some(item.clone())
}

pub(super) fn select_for_request(
    q: &QueueState,
    session: &str,
    now: u64,
    now_min: u32,
    activity: &Observations,
    requested: Option<&str>,
) -> Option<QueueItem> {
    match requested {
        Some(id) => select_requested(q, session, id, now),
        None => select_for_session(q, session, now, now_min, activity),
    }
}

/// Pure per-tick candidate selection (unit-tested; the thread only adds IO):
/// at most ONE candidate per session, sessions independent of each other.
pub(crate) fn select_due(
    q: &QueueState,
    now: u64,
    now_min: u32,
    activity: &Observations,
) -> Vec<QueueItem> {
    let mut sessions: Vec<&str> = q.items.iter().map(|i| i.session.as_str()).collect();
    sessions.sort_unstable();
    sessions.dedup();
    sessions
        .into_iter()
        .filter_map(|s| select_for_session(q, s, now, now_min, activity))
        .collect()
}

/// A rule whose stop instant passed (while deck slept, typically).
pub(crate) fn expired(i: &QueueItem, now: u64) -> bool {
    i.mode == "every" && i.until_at.map(|t| now >= t).unwrap_or(false)
}

/// Expired rules die quietly (their stop instant passed while sleeping).
pub(crate) fn purge_expired(q: &mut QueueState, now: u64) -> bool {
    let n0 = q.items.len();
    q.items.retain(|i| !expired(i, now));
    q.items.len() != n0
}
