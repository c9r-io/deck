//! Candidate selection: deterministic priority (backoff retry → due `at`
//! → cadence `every` → chain), one candidate per session per tick, and
//! expiry.

use std::collections::HashMap;

use super::*;

/// Deterministic candidate order within a session: retries whose backoff
/// elapsed, then the earliest-due at, then a cadence-due rule, then chains.
fn priority(i: &QueueItem) -> u8 {
    if i.state == "failed" {
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
        .filter(|x| x.group.as_deref() == Some(g))
        .min_by_key(|x| (x.seq.unwrap_or(1), x.added))
}

fn eligible(
    q: &QueueState,
    i: &QueueItem,
    now: u64,
    now_min: u32,
    activity: &HashMap<String, u64>,
) -> bool {
    if i.paused
        || matches!(i.state.as_str(), "firing" | "ambiguous")
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
    match i.mode.as_str() {
        "at" => i.at.map(|t| now >= t).unwrap_or(false),
        "chain" => activity
            .get(&i.session)
            .map(|a| now >= a + CHAIN_QUIET_SECS)
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
    activity: &HashMap<String, u64>,
) -> Option<QueueItem> {
    q.items
        .iter()
        .filter(|i| i.session == session && eligible(q, i, now, now_min, activity))
        .min_by_key(|i| {
            let class_time = match (i.state.as_str(), i.mode.as_str()) {
                ("failed", _) => i.last_attempt_at.unwrap_or(0),
                (_, "at") => i.at.unwrap_or(0),
                _ => i.added,
            };
            (priority(i), class_time, i.added)
        })
        .cloned()
}

/// Explicit manual-now selection skips only the schedule clock/quiet/window.
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
    if item.paused
        || matches!(item.state.as_str(), "firing" | "ambiguous")
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
    activity: &HashMap<String, u64>,
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
    activity: &HashMap<String, u64>,
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
