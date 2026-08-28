//! Scheduled prompts: at / chain (quiet-based) / every (recurring rules
//! with daily windows) + per-project templates. Delivery is at-most-once:
//! the firing intent is persisted before injection and crash recovery never
//! re-sends. Tick logic is pure and unit-tested; the thread only adds IO.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::start_session;
use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::{pane_target, tmux};

// ---------- scheduled prompts ----------------------------------------------------
// Queue prompts to be typed into a session later — the rate-limit workflow:
// "when my Claude quota window resets in 5h, send these tasks in order".
// tmux send-keys needs no attached client, so this works fully detached.
// The scheduler lives in a Rust thread (webview timers get frozen by App Nap).

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct QueueItem {
    id: String,
    session: String,
    dir: String,
    cmd: String,
    text: String,
    /// "at" = fire at `at` (epoch secs); "chain" = fire once the session has
    /// been quiet for CHAIN_QUIET_SECS after the previous send; "every" = a
    /// standing rule that re-fires every `every` secs (optionally only inside
    /// a daily time window) until stopped
    mode: String,
    at: Option<u64>,
    added: u64,
    #[serde(default)]
    every: Option<u64>,
    /// daily window in minutes since local midnight; from > to wraps midnight
    #[serde(default)]
    win_from: Option<u32>,
    #[serde(default)]
    win_to: Option<u32>,
    /// stop conditions for "every": after N fires / after an instant
    #[serde(default)]
    until_n: Option<u32>,
    #[serde(default)]
    until_at: Option<u64>,
    #[serde(default)]
    fired: u32,
    #[serde(default)]
    paused: bool,
    /// last fire instant (recurring only)
    #[serde(default)]
    last: Option<u64>,
    /// lifecycle: "pending" (default) | "firing" | "failed".
    /// "firing" is persisted BEFORE injection: after a crash such an item is
    /// resolved as "assume sent" (at-most-once delivery — see recover()).
    #[serde(default = "default_state")]
    state: String,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_attempt_at: Option<u64>,
    /// template steps 2..N — re-enqueued as chain items on every fire
    #[serde(default)]
    steps: Vec<String>,
    #[serde(default)]
    tpl: Option<String>,
    #[serde(default)]
    tpl_idx: Option<u32>,
    #[serde(default)]
    tpl_total: Option<u32>,
    /// sequential-group membership: chain steps only fire after every earlier
    /// `seq` in their group is gone, and a dead (failed-final) earlier step
    /// blocks the rest of the group until the user retries/skips/removes it.
    /// Rules ("every") carry no group; their spawned iterations do.
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    seq: Option<u32>,
    /// for iteration items spawned by a recurring rule: the rule's id.
    /// A rule is not due again while any item of its iteration is still live.
    #[serde(default)]
    rule: Option<String>,
    /// delivery id of the in-flight send, persisted with the firing intent so
    /// crash recovery can finalize the exact same delivery idempotently.
    #[serde(default)]
    delivery: Option<String>,
}

pub(crate) fn default_state() -> String {
    "pending".into()
}

/// Audit record of one prompt delivery. `assumed` marks boot-time crash
/// recovery, where the send may or may not have reached the session (the
/// at-most-once uncertainty window) — deck never risks sending twice.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct DeliveryRecord {
    id: String,
    item: String,
    session: String,
    mode: String,
    at: u64,
    #[serde(default)]
    assumed: bool,
}

/// How many delivery audit records queue.json retains (oldest dropped first).
pub(crate) const MAX_DELIVERIES: usize = 200;

/// Ledger entry for an in-flight delivery, persisted together with the
/// firing intent. Carries a full snapshot of the item so the delivery can be
/// finalized (audit record + session gap) even if the original QueueItem
/// vanishes mid-flight — finalize must never depend on the item surviving.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct PendingDelivery {
    /// the delivery id (matches QueueItem.delivery while in flight)
    id: String,
    snapshot: QueueItem,
}

/// A session whose card (or whole project) was deleted. Deleting a card
/// means "cancel every future scheduling for this session, permanently" —
/// the tombstone is what makes that survive a crash, an in-flight delivery
/// and a recurring rule's own bookkeeping.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Tombstone {
    session: String,
    at: u64,
}

/// How many deleted sessions are remembered (oldest dropped first). Only
/// reached by boards with hundreds of deletions; a dropped tombstone can do
/// no harm because its items are long gone.
pub(crate) const MAX_TOMBSTONES: usize = 500;

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct QueueState {
    items: Vec<QueueItem>,
    /// session → when we last injected a prompt
    last_fired: HashMap<String, u64>,
    /// delivery audit trail; also the idempotency guard for finalize
    #[serde(default)]
    deliveries: Vec<DeliveryRecord>,
    /// in-flight delivery ledger (empty except during a send / after a crash)
    #[serde(default)]
    pending: Vec<PendingDelivery>,
    /// sessions whose card/project was deleted — permanently unschedulable
    #[serde(default)]
    cancelled: Vec<Tombstone>,
}

pub(crate) struct Queues {
    pub(crate) q: Mutex<QueueState>,
    /// sessions with a send worker currently running — the tick claims a
    /// session here before spawning its worker, so the same session never has
    /// two concurrent sends and a long send never collides with the next tick
    pub(crate) busy: Mutex<HashSet<String>>,
    /// set when a POST-send persist failed: memory is ahead of disk and the
    /// scheduler must keep retrying the write (see `flush_dirty`)
    pub(crate) dirty: AtomicBool,
}

impl Queues {
    pub(crate) fn new(q: QueueState) -> Self {
        Queues {
            q: Mutex::new(q),
            busy: Mutex::new(HashSet::new()),
            dirty: AtomicBool::new(false),
        }
    }
}

/// Sentinel error meaning "this transaction found nothing to do": `with_queue`
/// returns it without persisting or committing, and callers translate it into
/// their own no-op result. It is never shown to the user.
pub(crate) const TX_NOOP: &str = "\u{0}tx-noop";

/// Persist-then-commit: EVERY user-driven queue mutation runs inside this.
///
/// The shared state is cloned, the mutation runs on the CANDIDATE only, the
/// candidate is persisted, and the shared state is replaced only after the
/// write succeeded. A rejected mutation or a failed save therefore leaves the
/// in-memory queue byte-identical to what is on disk — the scheduler can
/// never act on a change the user was told had failed.
///
/// The queue lock is held across the (short, local) save; it is NEVER held
/// across a tmux send or a session-boot wait (see `send_one`).
pub(crate) fn with_queue<T>(
    qm: &Mutex<QueueState>,
    persist: &dyn Fn(&QueueState) -> Result<(), String>,
    f: impl FnOnce(&mut QueueState) -> Result<T, String>,
) -> Result<T, String> {
    let mut guard = qm.lock().unwrap();
    let mut candidate = guard.clone();
    let out = f(&mut candidate)?; // rejected: shared state never touched
    persist(&candidate)?; // disk first…
    *guard = candidate; // …memory only after it landed
    Ok(out)
}

/// Retry a persist that failed AFTER an irreversible side effect (a prompt
/// was really sent, or definitively refused). Memory is authoritative in that
/// window; until the write lands, a crash falls back to the at-most-once
/// recovery rule ("assume sent"), so the scheduler keeps retrying every tick.
pub(crate) fn flush_dirty(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    persist: &dyn Fn(&QueueState) -> Result<(), String>,
) -> bool {
    if !dirty.load(AtomicOrdering::Relaxed) {
        return false;
    }
    let q = qm.lock().unwrap();
    match persist(&q) {
        Ok(()) => {
            dirty.store(false, AtomicOrdering::Relaxed);
            applog("[queue] deferred persist recovered — disk matches memory again");
            true
        }
        Err(e) => {
            applog(&format!(
                "[queue] deferred persist still FAILING ({})",
                storage::err_code(&e)
            ));
            false
        }
    }
}

/// A persist that failed after the side effect already happened: memory keeps
/// the truth, the user is told, and the write is retried on every later tick.
fn note_persist_lag(dirty: &AtomicBool, stage: &str, e: &str) {
    dirty.store(true, AtomicOrdering::Relaxed);
    applog(&format!(
        "[queue] persist ({stage}) FAILED ({}) — memory is ahead of disk, retrying",
        storage::err_code(e)
    ));
    storage::warn(format!(
        "scheduled prompts could not be saved after a send ({stage}); deck keeps retrying — if this persists, free disk space or check permissions on ~/.deck"
    ));
}

/// Claim a session for a send worker; false = a worker is already on it.
pub(crate) fn claim_session(busy: &Mutex<HashSet<String>>, session: &str) -> bool {
    busy.lock().unwrap().insert(session.to_string())
}

pub(crate) fn release_session(busy: &Mutex<HashSet<String>>, session: &str) {
    busy.lock().unwrap().remove(session);
}

/// true when `now_min` (minutes since local midnight) falls inside the daily
/// window; from > to means the window wraps midnight (e.g. 20:00–08:00)
pub(crate) fn in_window(now_min: u32, from: Option<u32>, to: Option<u32>) -> bool {
    match (from, to) {
        (Some(f), Some(t)) if f != t => {
            if f < t {
                now_min >= f && now_min < t
            } else {
                now_min >= f || now_min < t
            }
        }
        _ => true,
    }
}

pub(crate) fn local_minutes() -> u32 {
    // std has no timezone support; /bin/date is always there on macOS
    Command::new("date")
        .arg("+%H %M")
        .output()
        .ok()
        .and_then(|o| {
            let t = String::from_utf8_lossy(&o.stdout);
            let mut it = t.split_whitespace();
            Some(it.next()?.parse::<u32>().ok()? * 60 + it.next()?.parse::<u32>().ok()?)
        })
        .unwrap_or(720)
}

/// Whether an "every" rule has reached its cadence (window/stop-aware).
/// The per-session send gap and the one-active-iteration rule are enforced
/// by candidate selection, not here.
pub(crate) fn every_due(i: &QueueItem, now: u64, now_min: u32) -> bool {
    i.mode == "every"
        && !i.paused
        && i.until_at.map(|t| now < t).unwrap_or(true)
        && in_window(now_min, i.win_from, i.win_to)
        && i.last
            .map(|l| now >= l + i.every.unwrap_or(u64::MAX))
            .unwrap_or(true)
}

pub(crate) const CHAIN_QUIET_SECS: u64 = 180;
/// Minimum spacing between ANY two injections into the same session — one
/// prompt per session at a time, whatever mix of at/every/chain is queued.
pub(crate) const SESSION_MIN_GAP_SECS: u64 = 60;

pub(crate) fn queue_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("queue.json")
}

pub(crate) fn load_queue() -> QueueState {
    let mut q = match storage::load_typed::<QueueState>(&queue_path()) {
        Ok(Some(o)) => {
            if let Some(w) = o.warning {
                storage::warn(w); // boot-time: surfaced via storage_warnings
            }
            serde_json::from_str(&o.payload).unwrap_or_default()
        }
        Ok(None) => QueueState::default(),
        Err(e) => {
            storage::warn(format!("scheduled prompts could not be loaded: {e}"));
            QueueState::default()
        }
    };
    migrate_groups(&mut q);
    q
}

/// Compatibility migration: pre-group queue files expressed chains purely by
/// array adjacency (a chain item followed the previous non-rule item of its
/// session). Derive explicit group/seq with exactly those semantics.
pub(crate) fn migrate_groups(q: &mut QueueState) {
    let mut last_group: HashMap<String, (String, u32)> = HashMap::new(); // session → (group, max seq)
    for i in q.items.iter_mut() {
        if i.mode == "every" {
            continue; // rules carry no group; their iterations get one at spawn
        }
        match &i.group {
            Some(g) => {
                let seq = i.seq.unwrap_or(1);
                match last_group.get_mut(&i.session) {
                    Some(e) if e.0 == *g => e.1 = e.1.max(seq),
                    _ => {
                        last_group.insert(i.session.clone(), (g.clone(), seq));
                    }
                }
            }
            None => {
                if i.mode == "chain" {
                    if let Some((g, seq)) = last_group.get_mut(&i.session) {
                        *seq += 1;
                        i.group = Some(g.clone());
                        i.seq = Some(*seq);
                        continue;
                    }
                }
                i.group = Some(i.id.clone());
                i.seq = Some(1);
                last_group.insert(i.session.clone(), (i.id.clone(), 1));
            }
        }
    }
}

/// Persist the queue. Callers must not proceed with side effects (like
/// injecting a prompt) when this fails.
pub(crate) fn save_queue(q: &QueueState) -> Result<(), String> {
    let raw = serde_json::to_string(q).map_err(|e| e.to_string())?;
    storage::save(&queue_path(), &raw)
}

#[tauri::command]
pub(crate) fn queue_list(state: State<'_, Queues>) -> QueueState {
    state.q.lock().unwrap().clone()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueAddArgs {
    session: String,
    dir: String,
    cmd: String,
    text: String,
    mode: String,
    at: Option<u64>,
    every: Option<u64>,
    win_from: Option<u32>,
    win_to: Option<u32>,
    until_n: Option<u32>,
    until_at: Option<u64>,
    steps: Option<Vec<String>>,
    tpl: Option<String>,
    tpl_idx: Option<u32>,
    tpl_total: Option<u32>,
}

/// Reject invalid schedule combinations up front.
pub(crate) fn validate_add(a: &QueueAddArgs) -> Result<(), String> {
    crate::tmux::validate_session_name(&a.session)?;
    match a.mode.as_str() {
        "at" => {
            if a.at.is_none() {
                return Err("a timed prompt needs its time".into());
            }
        }
        "chain" => {}
        "every" => {
            let e = a.every.ok_or("a recurring rule needs an interval")?;
            if e < 60 {
                return Err("recurring interval must be at least 1 minute".into());
            }
        }
        m => return Err(format!("unknown schedule mode: {m}")),
    }
    if a.mode != "every" && (a.every.is_some() || a.steps.as_ref().is_some_and(|s| !s.is_empty())) {
        return Err("interval/steps only make sense on a recurring rule".into());
    }
    for w in [a.win_from, a.win_to] {
        if w.is_some_and(|m| m >= 1440) {
            return Err("time-window minutes must be below 24h".into());
        }
    }
    if a.win_from.is_some() != a.win_to.is_some() {
        return Err("a time window needs both ends".into());
    }
    if a.until_n.is_some_and(|n| n == 0) {
        return Err("stop-after count must be at least 1".into());
    }
    if a.session.trim().is_empty() {
        return Err("missing session".into());
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
pub(crate) fn add_item(q: &mut QueueState, args: QueueAddArgs, text: String) -> Result<(), String> {
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
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_add(
    state: State<'_, Queues>,
    app: AppHandle,
    args: QueueAddArgs,
) -> Result<(), String> {
    validate_add(&args)?;
    let text = args.text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err("empty prompt".into());
    }
    // never let the scheduler act on an item the disk doesn't know about
    with_queue(&state.q, &save_queue, |q| add_item(q, args, text))?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// The firing contract for user operations: while an item is mid-send
/// ("firing" persisted, injection possibly in flight), mutating it would race
/// the delivery — remove/update/pause/retry/skip are refused with a clear
/// conflict error and the item is kept until finalize completes. The window
/// is at most one send (seconds); the UI surfaces the error as a toast.
pub(crate) fn firing_conflict(q: &QueueState, id: &str) -> Result<(), String> {
    if q.items.iter().any(|i| i.id == id && i.state == "firing") {
        return Err("this prompt is being sent right now — try again in a few seconds".into());
    }
    Ok(())
}

/// Pure core of queue_update (unit-tested with the firing contract).
pub(crate) fn update_text(q: &mut QueueState, id: &str, text: String) -> Result<(), String> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.text = text;
    }
    Ok(())
}

/// Pure core of queue_remove / queue_skip.
pub(crate) fn remove_item(q: &mut QueueState, id: &str) -> Result<bool, String> {
    firing_conflict(q, id)?;
    let n0 = q.items.len();
    q.items.retain(|i| i.id != id);
    Ok(q.items.len() != n0)
}

/// Pure core of queue_pause.
pub(crate) fn pause_item(q: &mut QueueState, id: &str, paused: bool) -> Result<(), String> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.paused = paused;
    }
    Ok(())
}

/// Pure core of queue_retry.
pub(crate) fn retry_item(q: &mut QueueState, id: &str) -> Result<(), String> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.state = default_state();
        item.attempts = 0;
        item.last_error = None;
        item.last_attempt_at = None;
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_update(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    text: String,
) -> Result<(), String> {
    let text = text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err("empty prompt".into());
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
) -> Result<(), String> {
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
) -> Result<(), String> {
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
) -> Result<(), String> {
    // a failed save must not re-arm the item in memory only
    with_queue(&state.q, &save_queue, |q| retry_item(q, &id))?;
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
) -> Result<(), String> {
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
///    — its delivery still finalizes from the pending-ledger snapshot, so
///    the at-most-once audit is complete, but there is no item left to
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
) -> Result<(), String> {
    with_queue(&state.q, &save_queue, |q| {
        clear_sessions(q, &sessions);
        Ok(())
    })?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Single-session form of `queue_clear_sessions`.
#[tauri::command]
pub(crate) fn queue_clear_session(
    state: State<'_, Queues>,
    app: AppHandle,
    session: String,
) -> Result<(), String> {
    queue_clear_sessions(state, app, vec![session])
}

/// Note: deliberately carries no prompt text — the UI only toasts the
/// session name, and event payloads must not haul content around (privacy).
#[derive(Clone, Serialize)]
pub(crate) struct QueueFired {
    session: String,
}

/// Inject one prompt into its session, starting the session if needed.
///
/// The injection is ONE atomic tmux command: the literal text with a
/// trailing CR (byte-identical to pressing Enter) in a single `send-keys -l`.
/// There is no window where the text landed but Enter did not — the server
/// either executes the whole command (exit 0) or refuses it (non-zero, e.g.
/// the session died between the liveness check and the send), so a failure
/// here means NOT SENT and retrying cannot duplicate the prompt. Queue text
/// has \r/\n stripped at add/update time, so the CR we append is the only
/// one. (Residual ambiguity: the tmux client being externally SIGKILLed
/// after the server executed would read as a failure — deck never does this
/// and it is the same class of window as a power loss mid-send.)
pub(crate) fn fire_item(item: &QueueItem) -> Result<(), String> {
    let alive: HashSet<String> = tmux(&["list-sessions", "-F", "#{session_name}"])
        .map(|o| o.lines().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if !alive.contains(&item.session) {
        start_session(item.session.clone(), item.dir.clone(), item.cmd.clone())?;
        // boot wait blocks only THIS session's worker, never other sessions
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    let line = format!("{}\r", item.text);
    tmux(&["send-keys", "-t", &pane_target(&item.session), "-l", &line])?;
    Ok(())
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
    if i.paused || i.state == "firing" || item_dead(i) || !retry_ok(i, now) {
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
/// shared by the live success path and boot-time "assume sent" recovery.
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
    // record above closes the at-most-once window (the prompt may really
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
/// recover, and crash recovery must not "assume sent" for it. The ledger
/// entry goes even when the item itself vanished mid-send (its card was
/// deleted), so a refused send is never resurrected as an assumed delivery.
pub(crate) fn note_failed(q: &mut QueueState, id: &str, delivery: &str, err: &str) {
    if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
        it.delivery = None;
        it.state = "failed".into();
        it.last_error = Some(err.chars().take(200).collect());
    }
    q.pending.retain(|p| p.id != delivery);
}

/// At-most-once crash recovery. The firing intent (with its delivery id) is
/// persisted BEFORE injection, so after a crash such an item may or may not
/// have reached the session — this window cannot be closed from outside the
/// terminal. Deck assumes the send happened and finalizes that exact
/// delivery through the same idempotent path as a live success, so fired
/// counts, until-N retirement and template steps stay correct and repeated
/// recovery changes nothing. It never re-sends.
pub(crate) fn recover_interrupted(q: &mut QueueState) -> Vec<String> {
    let firing: Vec<(String, Option<String>, u64, String, String)> = q
        .items
        .iter()
        .filter(|i| i.state == "firing")
        .map(|i| {
            (
                i.id.clone(),
                i.delivery.clone(),
                i.last_attempt_at.unwrap_or_else(now_epoch),
                i.mode.clone(),
                i.session.clone(),
            )
        })
        .collect();
    let mut notes = Vec::new();
    for (id, delivery, when, mode, session) in firing {
        // pre-delivery-id queue files: synthesize a stable id so repeated
        // recovery of the same interruption stays idempotent
        let d = delivery.unwrap_or_else(|| format!("legacy-{id}"));
        finalize_delivery(q, &id, &d, when, true);
        notes.push(format!(
            "a {} prompt for {session} was interrupted mid-send last run; treated as sent (deck delivers at-most-once)",
            if mode == "every" { "recurring" } else { "scheduled" }
        ));
    }
    // Orphaned ledger entries: the firing item itself is gone (crash combined
    // with an item removal) but the persisted delivery still must be
    // accounted — audit + session gap via the snapshot, assumed sent.
    let orphans: Vec<(String, String, u64, String)> = q
        .pending
        .iter()
        .filter(|p| !q.items.iter().any(|i| i.id == p.snapshot.id))
        .map(|p| {
            (
                p.snapshot.id.clone(),
                p.id.clone(),
                p.snapshot.last_attempt_at.unwrap_or_else(now_epoch),
                p.snapshot.session.clone(),
            )
        })
        .collect();
    for (item_id, delivery, when, session) in orphans {
        finalize_delivery(q, &item_id, &delivery, when, true);
        notes.push(format!(
            "a scheduled prompt for {session} was interrupted mid-send last run; treated as sent (deck delivers at-most-once)"
        ));
    }
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
    /// pre-fire persist failed — the intent never reached disk, nothing sent
    NotPersisted,
}

/// The side-effecting parts of one send, injected so the whole firing state
/// machine can be unit-tested without tmux or a disk.
pub(crate) struct SendHooks<'a> {
    pub(crate) fire: &'a (dyn Fn(&QueueItem) -> Result<(), String> + Sync),
    pub(crate) persist: &'a (dyn Fn(&QueueState) -> Result<(), String> + Sync),
    /// kill a session whose card was deleted DURING this send
    pub(crate) kill: &'a (dyn Fn(&str) + Sync),
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
///   firing ──crash──► recover_interrupted at next boot (assume sent)
///
/// Persistence has two distinct regimes, and the difference is deliberate:
///
/// * BEFORE the injection, the intent is a normal transaction — a failed
///   write rolls everything back and nothing is sent (`NotPersisted`).
/// * AFTER the injection (success or definitive refusal) the side effect is
///   already irreversible, so memory takes the new state unconditionally and
///   a failed write only sets the `dirty` flag: the scheduler retries the
///   save every tick (`flush_dirty`) and the user is warned. Until it lands,
///   a crash resolves through the usual at-most-once rule (disk still shows
///   "firing" → assume sent), which is why the retry — not silence — is the
///   contract for a definitively-NOT-sent prompt.
///
/// The queue lock is held only for state transitions and their persists —
/// never across `fire` (tmux + a possible session-boot wait).
pub(crate) fn send_one(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    session: &str,
    now_min: u32,
    activity: &HashMap<String, u64>,
    h: &SendHooks,
) -> SendResult {
    let persist = h.persist;
    // Persist the firing intent (delivery id + ledger snapshot) BEFORE
    // injecting — this ordering makes delivery at-most-once across crashes,
    // and the snapshot makes finalize independent of the item surviving.
    let pre = with_queue(qm, persist, |q| {
        // fresh re-selection under the lock: a pause, edit or removal since
        // the tick began is honored here
        let Some(sel) = select_for_session(q, session, now_epoch(), now_min, activity) else {
            return Err(TX_NOOP.into());
        };
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
        Err(e) if e == TX_NOOP => return SendResult::Nothing,
        Err(e) => {
            applog(&format!(
                "[queue] persist (pre-fire) FAILED ({}) — not sending this tick",
                storage::err_code(&e)
            ));
            return SendResult::NotPersisted;
        }
    };
    match (h.fire)(&item) {
        Ok(()) => {
            // never log prompt contents — length only (privacy)
            applog(&format!(
                "[queue] sent to {} ({}B, mode {})",
                storage::session_tag(&item.session),
                item.text.len(),
                item.mode
            ));
            let mut q = qm.lock().unwrap();
            finalize_delivery(&mut q, &item.id, &delivery, now_epoch(), false);
            let cancelled = is_cancelled(&q, &item.session);
            if let Err(e) = persist(&q) {
                note_persist_lag(dirty, "post-fire", &e);
            }
            drop(q);
            reap_if_cancelled(cancelled, &item.session, h);
            SendResult::Sent {
                session: item.session.clone(),
            }
        }
        Err(e) => {
            let mut q = qm.lock().unwrap();
            note_failed(&mut q, &item.id, &delivery, &e);
            let gave_up = q.items.iter().any(|i| i.id == item.id && item_dead(i));
            let cancelled = is_cancelled(&q, &item.session);
            if let Err(pe) = persist(&q) {
                note_persist_lag(dirty, "post-failure", &pe);
            }
            drop(q);
            reap_if_cancelled(cancelled, &item.session, h);
            // the raw error stays on the item (last_error → queue UI); the
            // log gets only its category — tmux/start errors can embed paths
            applog(&format!(
                "[queue] send FAILED for {} (attempt {}, {}){}",
                storage::session_tag(&item.session),
                item.attempts,
                storage::err_code(&e),
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

/// Boot the queue: load, resolve any interrupted delivery (at-most-once),
/// persist the resolution. A failed write here is the same "memory ahead of
/// disk" case as a post-send failure — flagged dirty and retried per tick,
/// never silently dropped.
pub(crate) fn boot_queues() -> Queues {
    let mut qs = load_queue();
    let notes = recover_interrupted(&mut qs);
    let queues = Queues::new(qs);
    if !notes.is_empty() {
        for n in notes {
            storage::warn(n);
        }
        let q = queues.q.lock().unwrap();
        if let Err(e) = save_queue(&q) {
            note_persist_lag(&queues.dirty, "crash recovery", &e);
        }
    }
    queues
}

pub(crate) fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let state = app.state::<Queues>();
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
        // a post-send write that failed keeps memory ahead of disk — retry it
        // before anything else touches the queue this tick
        flush_dirty(&state.q, &state.dirty, &save_queue);
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
                let res = send_one(
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
                );
                release_session(&state.busy, &session);
                match res {
                    SendResult::Sent { session } => {
                        let _ = app2.emit("queue-fired", QueueFired { session });
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::Failed { .. } => {
                        let _ = app2.emit("queue-changed", ());
                    }
                    SendResult::Nothing | SendResult::NotPersisted => {}
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qi(id: &str, mode: &str) -> QueueItem {
        QueueItem {
            id: id.into(),
            session: "s".into(),
            dir: String::new(),
            cmd: String::new(),
            text: "x".into(),
            mode: mode.into(),
            at: None,
            added: 0,
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
            tpl: None,
            tpl_idx: None,
            tpl_total: None,
            group: None,
            seq: None,
            rule: None,
            delivery: None,
        }
    }

    fn rule(every: u64) -> QueueItem {
        let mut i = qi("t", "every");
        i.every = Some(every);
        i
    }

    /// Build a QueueState the way load_queue does: adjacency-derived groups
    /// (a chain joins the previous non-rule item of its session).
    fn qs(items: Vec<QueueItem>) -> QueueState {
        let mut q = QueueState {
            items,
            last_fired: HashMap::new(),
            deliveries: Vec::new(),
            pending: Vec::new(),
            cancelled: Vec::new(),
        };
        migrate_groups(&mut q);
        q
    }

    fn ids(v: &[QueueItem]) -> Vec<&str> {
        v.iter().map(|i| i.id.as_str()).collect()
    }

    const NOW: u64 = 1_000_000;

    #[test]
    fn at_fires_only_when_due() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut b = qi("b", "at");
        b.session = "other".into();
        b.at = Some(NOW + 100);
        let q = qs(vec![a, b]);
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
    }

    #[test]
    fn future_at_does_not_block_due_at() {
        // the future item was queued FIRST — it must not own a head slot
        let mut fut = qi("fut", "at");
        fut.at = Some(NOW + 3600);
        let mut due = qi("due", "at");
        due.at = Some(NOW - 1);
        due.added = 10;
        let q = qs(vec![fut, due]);
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["due"]);
    }

    #[test]
    fn two_due_ats_same_session_one_per_tick() {
        let mut a1 = qi("a1", "at");
        a1.at = Some(NOW - 50);
        let mut a2 = qi("a2", "at");
        a2.at = Some(NOW - 5);
        let q = qs(vec![a2.clone(), a1.clone()]);
        // exactly one candidate, deterministically the earliest-due
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a1"]);
    }

    #[test]
    fn two_every_rules_same_session_one_per_tick() {
        let mut r1 = rule(300);
        r1.id = "r1".into();
        let mut r2 = rule(300);
        r2.id = "r2".into();
        let q = qs(vec![r1, r2]);
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["r1"]);
    }

    #[test]
    fn every_plus_at_both_due_picks_the_at() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let q = qs(vec![rule(300), a]);
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
    }

    #[test]
    fn pause_after_selection_is_honored_before_send() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut q = qs(vec![a]);
        // tick-start selection sees it...
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
        // ...user pauses before the send: the loop's FRESH per-send
        // selection (select_for_session) must come up empty
        q.items[0].paused = true;
        assert!(select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none());
    }

    #[test]
    fn update_after_selection_sends_the_new_text() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut q = qs(vec![a]);
        assert_eq!(select_due(&q, NOW, 720, &HashMap::new())[0].text, "x");
        q.items[0].text = "edited".into();
        let fresh = select_for_session(&q, "s", NOW, 720, &HashMap::new()).unwrap();
        assert_eq!(fresh.text, "edited");
    }

    #[test]
    fn failed_head_blocks_group_until_user_skips_or_retries() {
        let mut c1 = qi("c1", "chain");
        c1.state = "failed".into();
        c1.attempts = MAX_ATTEMPTS; // dead: attempts exhausted
        let c2 = qi("c2", "chain"); // adjacency → same group as c1
        let mut q = qs(vec![c1, c2]);
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        // the dead head blocks its group — nothing fires on its own
        assert!(select_due(&q, NOW, 720, &quiet).is_empty());
        // user skip (= remove the failed step) unblocks the successor
        let mut skipped = q.clone();
        skipped.items.retain(|i| i.id != "c1");
        assert_eq!(ids(&select_due(&skipped, NOW, 720, &quiet)), ["c2"]);
        // user retry re-arms the failed step itself instead
        q.items[0].state = default_state();
        q.items[0].attempts = 0;
        assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
    }

    #[test]
    fn failed_item_backs_off_then_retries_with_priority() {
        let mut c1 = qi("c1", "chain");
        c1.state = "failed".into();
        c1.attempts = 1;
        c1.last_attempt_at = Some(NOW - 10);
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        a.added = 10; // separate group (not chained after c1? adjacency: at starts its own group)
        let mut q = qs(vec![c1, a]);
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        // 10s after 1st failure: backoff (20s) holds the retry; the due at runs
        assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["a"]);
        // backoff elapsed → the retry outranks even a due at
        q.items[0].last_attempt_at = Some(NOW - 30);
        assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
        assert_eq!(backoff_secs(1), 20);
        assert_eq!(backoff_secs(20), 1800, "backoff is capped");
    }

    #[test]
    fn session_min_gap_applies_to_every_mode() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut q = qs(vec![a]);
        q.last_fired.insert("s".into(), NOW - 10);
        assert!(select_due(&q, NOW, 720, &HashMap::new()).is_empty());
        q.last_fired
            .insert("s".into(), NOW - SESSION_MIN_GAP_SECS - 1);
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["a"]);
    }

    #[test]
    fn chain_respects_order_quiet_and_gap() {
        let c1 = qi("c1", "chain");
        let c2 = qi("c2", "chain");
        let mut q = qs(vec![c1, c2]);
        // quiet session, no prior fire → only the HEAD chain step fires
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        assert_eq!(ids(&select_due(&q, NOW, 720, &quiet)), ["c1"]);
        // recent activity → nothing
        let busy: HashMap<String, u64> = [("s".into(), NOW - 10)].into();
        assert!(select_due(&q, NOW, 720, &busy).is_empty());
        // fired 10s ago → min-gap blocks even a quiet session
        q.last_fired.insert("s".into(), NOW - 10);
        assert!(select_due(&q, NOW, 720, &quiet).is_empty());
    }

    #[test]
    fn sessions_stay_parallel_one_candidate_each() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        let mut b = qi("b", "at");
        b.session = "other".into();
        b.at = Some(NOW - 1);
        let q = qs(vec![a, b]);
        let due = select_due(&q, NOW, 720, &HashMap::new());
        assert_eq!(due.len(), 2);
        let mut sessions: Vec<_> = due.iter().map(|i| i.session.as_str()).collect();
        sessions.sort_unstable();
        assert_eq!(sessions, ["other", "s"]);
    }

    #[test]
    fn recurring_iterations_do_not_interleave() {
        let mut r = rule(300);
        r.steps = vec!["s2".into()];
        let mut q = qs(vec![r]);
        // first fire spawns the iteration's follow-up step
        finalize_delivery(&mut q, "t", "d1", NOW, false);
        assert_eq!(q.items.len(), 2, "rule + spawned step");
        // cadence elapsed again, session quiet — but the previous iteration
        // still has a live step, so the rule must NOT fire
        let quiet: HashMap<String, u64> = [("s".into(), NOW + 600 - 400)].into();
        let later = NOW + 600;
        let due = select_due(&q, later, 720, &quiet);
        // the only candidate can be the iteration's chain step, never the rule
        assert!(due.iter().all(|i| i.mode != "every"));
        // step done → next iteration may start
        let step_id = q
            .items
            .iter()
            .find(|i| i.mode == "chain")
            .unwrap()
            .id
            .clone();
        finalize_delivery(&mut q, &step_id, "d2", later, false);
        let quiet2: HashMap<String, u64> = [("s".into(), later + 300 - 400)].into();
        let due = select_due(&q, later + 300, 720, &quiet2);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].mode, "every");
    }

    #[test]
    fn paused_rule_skipped_resume_restores() {
        let mut r = rule(300);
        r.paused = true;
        let mut q = qs(vec![r]);
        assert!(select_due(&q, NOW, 720, &HashMap::new()).is_empty());
        q.items[0].paused = false;
        assert_eq!(select_due(&q, NOW, 720, &HashMap::new()).len(), 1);
    }

    #[test]
    fn migration_derives_groups_from_legacy_adjacency() {
        let mut a = qi("a", "at");
        a.at = Some(NOW + 10);
        let c = qi("c", "chain");
        let mut b_other = qi("co", "chain");
        b_other.session = "other".into();
        let r = rule(300);
        let q = qs(vec![a, c, r, b_other]); // qs() runs migrate_groups
        let g = |id: &str| {
            q.items
                .iter()
                .find(|i| i.id == id)
                .map(|i| (i.group.clone(), i.seq))
                .unwrap()
        };
        assert_eq!(g("a"), (Some("a".into()), Some(1)));
        assert_eq!(
            g("c"),
            (Some("a".into()), Some(2)),
            "chain joins the at's group"
        );
        assert_eq!(
            g("co"),
            (Some("co".into()), Some(1)),
            "other session: own group"
        );
        assert_eq!(q.items.iter().find(|i| i.id == "t").unwrap().group, None);
    }

    // ---------- finalize / crash recovery (at-most-once accounting) ----------

    #[test]
    fn finalize_is_idempotent_per_delivery() {
        let mut r = rule(300);
        r.steps = vec!["s2".into(), "s3".into()];
        r.tpl = Some("tp".into());
        r.tpl_idx = Some(1);
        r.tpl_total = Some(3);
        let mut q = qs(vec![r]);
        finalize_delivery(&mut q, "t", "d1", NOW, false);
        finalize_delivery(&mut q, "t", "d1", NOW, false); // re-run: no-op
        let rule_item = q.items.iter().find(|i| i.id == "t").unwrap();
        assert_eq!(rule_item.fired, 1, "no double count");
        let steps: Vec<_> = q.items.iter().filter(|i| i.mode == "chain").collect();
        assert_eq!(steps.len(), 2, "iteration steps spawned exactly once");
        assert_eq!(steps[0].group.as_deref(), Some("d1"));
        assert_eq!(steps[0].seq, Some(2));
        assert_eq!(steps[0].rule.as_deref(), Some("t"));
        assert_eq!(steps[0].tpl_idx, Some(2));
        assert_eq!(q.deliveries.len(), 1);
        assert_eq!(q.last_fired.get("s"), Some(&NOW));
    }

    #[test]
    fn crash_after_intent_recovers_as_assumed_sent_never_resends() {
        // crash windows 1–3 (before injection / between text and Enter /
        // after Enter, before finalize) all leave this exact persisted state:
        let mut once = qi("o", "at");
        once.state = "firing".into();
        once.delivery = Some("dA".into());
        once.last_attempt_at = Some(NOW - 5);
        let mut r = rule(300);
        r.state = "firing".into();
        r.delivery = Some("dB".into());
        r.last_attempt_at = Some(NOW - 5);
        r.steps = vec!["s2".into()];
        let mut q = qs(vec![once, r]);
        let notes = recover_interrupted(&mut q);
        assert_eq!(notes.len(), 2);
        // once-item consumed (assumed sent), never re-queued
        assert!(!q.items.iter().any(|i| i.id == "o"));
        // rule fully accounted: counted, stamped, iteration spawned
        let rl = q.items.iter().find(|i| i.id == "t").unwrap();
        assert_eq!(
            (rl.fired, rl.last, rl.state.as_str()),
            (1, Some(NOW - 5), "pending")
        );
        assert_eq!(q.items.iter().filter(|i| i.mode == "chain").count(), 1);
        assert!(q.deliveries.iter().any(|d| d.id == "dB" && d.assumed));
        // nothing is selectable for immediate re-send of the same prompts
        assert!(!q.items.iter().any(|i| i.state == "firing"));
    }

    #[test]
    fn repeated_recovery_is_idempotent() {
        let mut r = rule(300);
        r.state = "firing".into();
        r.delivery = Some("dB".into());
        r.last_attempt_at = Some(NOW - 5);
        r.steps = vec!["s2".into()];
        let mut q = qs(vec![r]);
        recover_interrupted(&mut q);
        let snapshot = serde_json::to_string(&q).unwrap();
        recover_interrupted(&mut q);
        finalize_delivery(&mut q, "t", "dB", NOW - 5, true); // even a direct replay
        assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
    }

    #[test]
    fn recovery_when_finalize_persisted_nothing_matches_live_result() {
        // finalize completed in memory but persisting failed and deck died:
        // disk still holds the pre-finalize state. Recovery must produce the
        // same accounting a live success would have.
        let mut r = rule(300);
        r.steps = vec!["s2".into()];
        r.state = "firing".into();
        r.delivery = Some("dX".into());
        r.last_attempt_at = Some(NOW);
        let pre = qs(vec![r]);
        let mut live = pre.clone();
        finalize_delivery(&mut live, "t", "dX", NOW, false);
        let mut rec = pre.clone();
        recover_interrupted(&mut rec);
        let strip = |q: &QueueState| {
            let mut v: Vec<_> = q
                .items
                .iter()
                .map(|i| (i.text.clone(), i.mode.clone(), i.group.clone(), i.seq))
                .collect();
            v.sort();
            (v, q.items.iter().find(|i| i.id == "t").map(|i| i.fired))
        };
        assert_eq!(strip(&live), strip(&rec));
    }

    #[test]
    fn stop_after_n_stays_accurate_across_recovery() {
        let mut r = rule(300);
        r.until_n = Some(2);
        r.fired = 1;
        r.state = "firing".into();
        r.delivery = Some("dZ".into());
        r.last_attempt_at = Some(NOW);
        let mut q = qs(vec![r]);
        recover_interrupted(&mut q);
        // second (and last) fire counted → rule retired, not over-counted
        assert!(!q.items.iter().any(|i| i.id == "t"));
        assert_eq!(q.deliveries.len(), 1);
    }

    #[test]
    fn legacy_firing_item_without_delivery_id_still_recovers_once() {
        let mut once = qi("o", "at");
        once.state = "firing".into(); // pre-delivery-id queue file
        once.last_attempt_at = Some(NOW - 5);
        let mut q = qs(vec![once]);
        recover_interrupted(&mut q);
        assert!(q.items.is_empty());
        recover_interrupted(&mut q);
        assert_eq!(q.deliveries.len(), 1, "synthetic id keeps it idempotent");
    }

    #[test]
    fn delivery_audit_is_capped() {
        let mut q = qs(vec![]);
        for k in 0..(MAX_DELIVERIES + 10) {
            let mut a = qi(&format!("a{k}"), "at");
            a.at = Some(NOW);
            q.items.push(a);
            finalize_delivery(&mut q, &format!("a{k}"), &format!("d{k}"), NOW, false);
        }
        assert_eq!(q.deliveries.len(), MAX_DELIVERIES);
        assert_eq!(
            q.deliveries.last().unwrap().id,
            format!("d{}", MAX_DELIVERIES + 9)
        );
    }

    // ---------- misc invariants (kept from round one) ----------

    #[test]
    fn queue_ids_never_collide() {
        let existing = vec![qi("q1-0", "at")];
        let a = next_queue_id(&existing);
        let b = next_queue_id(&existing);
        assert_ne!(a, b);
        assert!(!existing.iter().any(|i| i.id == a || i.id == b));
    }

    #[test]
    fn expired_rules_purge() {
        let mut r = rule(300);
        r.until_at = Some(NOW - 1);
        let mut q = qs(vec![r, qi("keep", "chain")]);
        assert!(purge_expired(&mut q, NOW));
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].id, "keep");
    }

    #[test]
    fn add_validation_rejects_bad_combinations() {
        let base = || QueueAddArgs {
            session: "s".into(),
            dir: String::new(),
            cmd: String::new(),
            text: "x".into(),
            mode: "at".into(),
            at: Some(NOW),
            every: None,
            win_from: None,
            win_to: None,
            until_n: None,
            until_at: None,
            steps: None,
            tpl: None,
            tpl_idx: None,
            tpl_total: None,
        };
        assert!(validate_add(&base()).is_ok());
        let mut a = base();
        a.at = None;
        assert!(validate_add(&a).is_err(), "at without a time");
        let mut a = base();
        a.mode = "every".into();
        a.every = Some(30);
        assert!(validate_add(&a).is_err(), "sub-minute interval");
        let mut a = base();
        a.mode = "every".into();
        a.every = Some(300);
        a.win_from = Some(480);
        assert!(validate_add(&a).is_err(), "one-sided window");
        a.win_to = Some(2000);
        assert!(validate_add(&a).is_err(), "window past 24h");
        let mut a = base();
        a.mode = "chain".into();
        a.at = None;
        a.steps = Some(vec!["y".into()]);
        assert!(validate_add(&a).is_err(), "steps on a non-rule");
        let mut a = base();
        a.mode = "yearly".into();
        assert!(validate_add(&a).is_err(), "unknown mode");
    }

    #[test]
    fn every_rule_due_logic() {
        let now = 1_000_000;
        // never fired → due immediately
        assert!(every_due(&rule(1800), now, 720));
        // fired 10 min ago on a 30-min cadence → not due; after 30 min → due
        let mut r = rule(1800);
        r.last = Some(now - 600);
        assert!(!every_due(&r, now, 720));
        r.last = Some(now - 1800);
        assert!(every_due(&r, now, 720));
        // paused wins over everything
        r.paused = true;
        assert!(!every_due(&r, now, 720));
        r.paused = false;
        // outside the 08:00–18:00 window (22:00) → sleeping
        r.win_from = Some(480);
        r.win_to = Some(1080);
        assert!(!every_due(&r, now, 22 * 60));
        assert!(every_due(&r, now, 9 * 60));
        // stop instant passed → never due again
        r.until_at = Some(now - 1);
        assert!(!every_due(&r, now, 9 * 60));
    }

    // ---------- round 3: firing contract, delivery ledger, send_one ----------

    #[test]
    fn user_mutations_conflict_while_firing() {
        let mut a = qi("a", "at");
        a.at = Some(NOW - 1);
        a.state = "firing".into();
        let mut q = qs(vec![a]);
        assert!(update_text(&mut q, "a", "edited".into()).is_err());
        assert!(remove_item(&mut q, "a").is_err());
        assert!(pause_item(&mut q, "a", true).is_err());
        assert!(retry_item(&mut q, "a").is_err());
        // the item is untouched by all four refused operations
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].state, "firing");
        assert_eq!(q.items[0].text, "x");
        assert!(!q.items[0].paused);
        // once the send finalized (state left "firing") the same ops work
        q.items[0].state = default_state();
        assert!(remove_item(&mut q, "a").unwrap());
        assert!(q.items.is_empty());
    }

    // ---------- round 4: deleting a card cancels its schedule for good ----------

    #[test]
    fn deleting_a_card_empties_its_queue_and_spares_other_sessions() {
        let mut a = qi("a", "at");
        a.at = Some(NOW);
        let b = qi("b", "chain");
        let mut r = rule(300);
        r.id = "r".into();
        let mut c = qi("c", "at");
        c.session = "other".into();
        c.at = Some(NOW);
        let mut q = qs(vec![a, b, r, c]);
        q.last_fired.insert("s".into(), NOW - 10);
        clear_session_items(&mut q, "s");
        let left: Vec<&str> = q.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(left, ["c"], "every item of the deleted card is gone");
        assert!(!q.last_fired.contains_key("s"), "send-gap entry cleared");
        assert!(is_cancelled(&q, "s"));
        assert!(!is_cancelled(&q, "other"));
        // idempotent: clearing again is a no-op, never an error
        let snapshot = serde_json::to_string(&q).unwrap();
        clear_session_items(&mut q, "s");
        assert_eq!(q.cancelled.len(), 1, "one tombstone per session");
        assert_eq!(
            serde_json::to_string(&q)
                .unwrap()
                .replace(&format!("\"at\":{}", q.cancelled[0].at), "\"at\":T"),
            snapshot.replace(&format!("\"at\":{}", q.cancelled[0].at), "\"at\":T")
        );
    }

    #[test]
    fn a_deleted_recurring_rule_never_becomes_a_candidate_again() {
        let mut r = rule(300);
        r.steps = vec!["s2".into()];
        let mut q = qs(vec![r]);
        assert_eq!(select_due(&q, NOW, 720, &HashMap::new()).len(), 1);
        clear_session_items(&mut q, "s");
        assert!(q.items.is_empty());
        // even if a rule for that session somehow reappears (hand-edited
        // file, stale queue from another deck), it is not schedulable
        q.items.push(rule(300));
        let later = NOW + 10_000;
        assert!(select_due(&q, later, 720, &HashMap::new()).is_empty());
        assert!(select_for_session(&q, "s", later, 720, &HashMap::new()).is_none());
    }

    #[test]
    fn a_delete_during_a_send_audits_the_delivery_but_revives_nothing() {
        // recurring rule mid-send: the delivery is inside the at-most-once
        // window, so it must finish its audit — but the card is gone
        let mut r = rule(300);
        r.steps = vec!["s2".into(), "s3".into()];
        r.state = "firing".into();
        r.delivery = Some("dX".into());
        let mut q = qs(vec![r]);
        q.pending.push(PendingDelivery {
            id: "dX".into(),
            snapshot: q.items[0].clone(),
        });
        clear_session_items(&mut q, "s"); // the user deletes the card now
        finalize_delivery(&mut q, "t", "dX", NOW, false); // the send lands
        assert_eq!(q.deliveries.len(), 1, "delivery audited");
        assert!(q.items.is_empty(), "no rule restored, no steps spawned");
        assert!(q.pending.is_empty(), "ledger consumed");
        assert!(
            !q.last_fired.contains_key("s"),
            "no cadence bookkeeping for a session that no longer exists"
        );
        // and nothing can fire for that session afterwards, ever
        assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
    }

    #[test]
    fn a_crash_right_after_a_delete_does_not_revive_anything() {
        let mut r = rule(300);
        r.steps = vec!["s2".into()];
        r.state = "firing".into();
        r.delivery = Some("dX".into());
        r.last_attempt_at = Some(NOW);
        let mut q = qs(vec![r]);
        q.pending.push(PendingDelivery {
            id: "dX".into(),
            snapshot: q.items[0].clone(),
        });
        clear_session_items(&mut q, "s");
        // …deck dies here; this is exactly what is on disk
        let on_disk = serde_json::to_string(&q).unwrap();
        let mut booted: QueueState = serde_json::from_str(&on_disk).unwrap();
        let notes = recover_interrupted(&mut booted);
        assert_eq!(notes.len(), 1, "the interrupted delivery is accounted for");
        assert!(booted.items.is_empty(), "nothing revived");
        assert!(booted.pending.is_empty());
        assert!(
            is_cancelled(&booted, "s"),
            "the tombstone survived the crash"
        );
        assert!(select_due(&booted, NOW + 100_000, 720, &HashMap::new()).is_empty());
        // repeated recovery still changes nothing
        let after = serde_json::to_string(&booted).unwrap();
        recover_interrupted(&mut booted);
        assert_eq!(serde_json::to_string(&booted).unwrap(), after);
    }

    #[test]
    fn deleting_a_project_clears_every_one_of_its_sessions_at_once() {
        let mk = |id: &str, session: &str| {
            let mut i = qi(id, "at");
            i.session = session.into();
            i.at = Some(NOW);
            i
        };
        let mut q = qs(vec![
            mk("a", "p-one"),
            mk("b", "p-two"),
            mk("c", "p-two"),
            mk("keep", "other-project"),
        ]);
        clear_sessions(&mut q, &["p-one".to_string(), "p-two".to_string()]);
        let left: Vec<&str> = q.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(left, ["keep"], "other projects untouched");
        assert!(is_cancelled(&q, "p-one") && is_cancelled(&q, "p-two"));
        assert!(!is_cancelled(&q, "other-project"));
        assert_eq!(ids(&select_due(&q, NOW, 720, &HashMap::new())), ["keep"]);
    }

    #[test]
    fn a_deleted_session_never_reaches_the_send_hook() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        with_queue(&qm, &ok_persist, |q| {
            clear_session_items(q, "s");
            Ok(())
        })
        .unwrap();
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| panic!("a deleted card must never start or feed a session"),
            &ok_persist,
        );
        assert_eq!(res, SendResult::Nothing);
    }

    #[test]
    fn a_card_deleted_mid_send_leaves_no_session_behind() {
        // the worker holds no lock while injecting, so the delete can land
        // between the intent and the send — fire_item may just have started
        // the session, which must not outlive the card
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let killed: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let fire = |_: &QueueItem| {
            // the user deletes the card at exactly this instant
            with_queue(&qm, &ok_persist, |q| {
                clear_session_items(q, "s");
                Ok(())
            })
            .unwrap();
            Ok(())
        };
        let kill = |s: &str| killed.lock().unwrap().push(s.to_string());
        let res = send_one(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &SendHooks {
                fire: &fire,
                persist: &ok_persist,
                kill: &kill,
            },
        );
        assert!(matches!(res, SendResult::Sent { .. }));
        assert_eq!(killed.lock().unwrap().as_slice(), ["s"], "session reaped");
        let q = qm.lock().unwrap();
        assert_eq!(q.deliveries.len(), 1, "the delivery is still audited");
        assert!(q.items.is_empty() && q.pending.is_empty());
    }

    #[test]
    fn scheduling_for_a_session_again_clears_its_tombstone() {
        // a NEW card that happens to reuse a name must schedule normally
        let mut q = qs(vec![]);
        clear_session_items(&mut q, "s");
        add_item(&mut q, add_args("s", "hello"), "hello".into()).unwrap();
        assert!(!is_cancelled(&q, "s"));
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        assert_eq!(select_due(&q, NOW, 720, &quiet).len(), 1);
    }

    #[test]
    fn tombstones_are_capped() {
        let mut q = qs(vec![]);
        for k in 0..(MAX_TOMBSTONES + 5) {
            clear_session_items(&mut q, &format!("s{k}"));
        }
        assert_eq!(q.cancelled.len(), MAX_TOMBSTONES);
        assert!(is_cancelled(&q, &format!("s{}", MAX_TOMBSTONES + 4)));
        assert!(!is_cancelled(&q, "s0"), "oldest dropped first");
    }

    #[test]
    fn finalize_without_item_still_audits_via_ledger() {
        let mut a = qi("a", "at");
        a.state = "firing".into();
        a.delivery = Some("d1".into());
        let mut q = qs(vec![a.clone()]);
        q.pending.push(PendingDelivery {
            id: "d1".into(),
            snapshot: q.items[0].clone(),
        });
        q.items.clear(); // the item vanished mid-flight
        finalize_delivery(&mut q, "a", "d1", NOW, false);
        assert_eq!(q.deliveries.len(), 1, "delivery audited from the snapshot");
        assert_eq!(q.deliveries[0].session, "s");
        assert_eq!(q.last_fired.get("s"), Some(&NOW), "session gap updated");
        assert!(q.pending.is_empty(), "ledger entry consumed");
        assert!(q.items.is_empty(), "no item resurrected, no steps spawned");
        // idempotent replay with the item still missing
        finalize_delivery(&mut q, "a", "d1", NOW, false);
        assert_eq!(q.deliveries.len(), 1);
    }

    #[test]
    fn vanished_rule_delivery_spawns_no_steps() {
        let mut r = rule(300);
        r.steps = vec!["s2".into()];
        r.state = "firing".into();
        r.delivery = Some("d1".into());
        let mut q = qs(vec![r]);
        q.pending.push(PendingDelivery {
            id: "d1".into(),
            snapshot: q.items[0].clone(),
        });
        q.items.clear();
        finalize_delivery(&mut q, "t", "d1", NOW, false);
        assert!(q.items.is_empty(), "removed rule must not respawn steps");
        assert_eq!(q.deliveries.len(), 1);
    }

    #[test]
    fn note_failed_drops_the_ledger_entry() {
        let mut a = qi("a", "at");
        a.state = "firing".into();
        a.delivery = Some("d1".into());
        a.attempts = 1;
        let mut q = qs(vec![a]);
        q.pending.push(PendingDelivery {
            id: "d1".into(),
            snapshot: q.items[0].clone(),
        });
        note_failed(&mut q, "a", "d1", "tmux send-keys failed");
        assert!(q.pending.is_empty(), "not-sent leaves nothing to recover");
        assert_eq!(q.items[0].state, "failed");
        assert!(q.items[0].delivery.is_none());
        assert!(q.deliveries.is_empty(), "a refused send is never audited");
    }

    #[test]
    fn recovery_finalizes_orphaned_ledger_entries() {
        let mut a = qi("a", "at");
        a.state = "firing".into();
        a.delivery = Some("dX".into());
        a.last_attempt_at = Some(NOW - 5);
        let mut q = qs(vec![]);
        q.pending.push(PendingDelivery {
            id: "dX".into(),
            snapshot: a,
        });
        let notes = recover_interrupted(&mut q);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("treated as sent"));
        assert!(q.deliveries.iter().any(|d| d.id == "dX" && d.assumed));
        assert_eq!(q.last_fired.get("s"), Some(&(NOW - 5)));
        assert!(q.pending.is_empty());
        // repeated recovery changes nothing
        let snapshot = serde_json::to_string(&q).unwrap();
        recover_interrupted(&mut q);
        assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
    }

    // ---------- send_one: the full firing state machine with fakes ----------

    fn ok_persist(_: &QueueState) -> Result<(), String> {
        Ok(())
    }

    /// send_one with a no-op kill hook — keeps the state-machine tests about
    /// the state machine. The kill hook has its own test below.
    fn send_test(
        qm: &Mutex<QueueState>,
        dirty: &AtomicBool,
        session: &str,
        now_min: u32,
        activity: &HashMap<String, u64>,
        fire: &(dyn Fn(&QueueItem) -> Result<(), String> + Sync),
        persist: &(dyn Fn(&QueueState) -> Result<(), String> + Sync),
    ) -> SendResult {
        let kill = |_: &str| {};
        send_one(
            qm,
            dirty,
            session,
            now_min,
            activity,
            &SendHooks {
                fire,
                persist,
                kill: &kill,
            },
        )
    }

    fn due_at(id: &str, session: &str) -> QueueItem {
        let mut a = qi(id, "at");
        a.session = session.into();
        a.at = Some(1); // long past — always due against the real clock
        a
    }

    #[test]
    fn send_one_success_runs_the_full_cycle() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let fired = AtomicU32::new(0);
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|i: &QueueItem| {
                fired.fetch_add(1, Ordering::SeqCst);
                assert_eq!(i.text, "x", "worker sends the snapshot text");
                Ok(())
            },
            &ok_persist,
        );
        assert_eq!(
            res,
            SendResult::Sent {
                session: "s".into()
            }
        );
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        let q = qm.lock().unwrap();
        assert!(q.items.is_empty(), "once-item consumed");
        assert_eq!(q.deliveries.len(), 1);
        assert!(!q.deliveries[0].assumed);
        assert!(q.pending.is_empty());
        assert!(q.last_fired.contains_key("s"));
    }

    #[test]
    fn send_one_failure_is_retryable_and_never_audited() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| Err("injection refused".into()),
            &ok_persist,
        );
        assert_eq!(
            res,
            SendResult::Failed {
                session: "s".into(),
                gave_up: false
            }
        );
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].state, "failed");
        assert_eq!(q.items[0].attempts, 1);
        assert!(q.items[0].delivery.is_none());
        assert!(q.pending.is_empty());
        assert!(q.deliveries.is_empty(), "refused send is not a delivery");
        assert!(q.last_fired.is_empty(), "no gap update for a refused send");
    }

    #[test]
    fn retry_after_failure_sends_the_full_text_exactly_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let _ = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| Err("refused".into()),
            &ok_persist,
        );
        qm.lock().unwrap().items[0].last_attempt_at = Some(0); // backoff elapsed
        let sent = AtomicU32::new(0);
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|i: &QueueItem| {
                sent.fetch_add(1, Ordering::SeqCst);
                assert_eq!(i.text, "x", "retry re-sends the WHOLE text once");
                Ok(())
            },
            &ok_persist,
        );
        assert!(matches!(res, SendResult::Sent { .. }));
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        assert_eq!(qm.lock().unwrap().deliveries.len(), 1, "one audit total");
    }

    #[test]
    fn send_one_persist_failure_rolls_back_the_intent() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| panic!("must not inject when the intent never hit disk"),
            &|_: &QueueState| Err("disk full".into()),
        );
        assert_eq!(res, SendResult::NotPersisted);
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].state, "pending");
        assert_eq!(q.items[0].attempts, 0);
        assert!(q.items[0].delivery.is_none());
        assert!(q.pending.is_empty());
    }

    #[test]
    fn send_one_honors_a_pause_that_landed_after_the_tick() {
        let mut a = due_at("a", "s");
        a.paused = true; // user paused between candidate pass and worker
        let qm = Mutex::new(qs(vec![a]));
        let res = send_test(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| panic!("paused item must not fire"),
            &ok_persist,
        );
        assert_eq!(res, SendResult::Nothing);
    }

    #[test]
    fn a_firing_item_is_never_selected_again() {
        let mut a = due_at("a", "s");
        a.state = "firing".into();
        let q = qs(vec![a]);
        assert!(
            select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none(),
            "a second worker on the same session finds nothing"
        );
    }

    #[test]
    fn session_claim_is_exclusive_and_releasable() {
        let busy = Mutex::new(HashSet::new());
        assert!(claim_session(&busy, "s"));
        assert!(!claim_session(&busy, "s"), "second worker refused");
        assert!(claim_session(&busy, "other"), "other sessions independent");
        release_session(&busy, "s");
        assert!(claim_session(&busy, "s"), "released slot reusable");
    }

    /// A slow send on one session must not delay another session — proven
    /// with barriers (deterministic sync points), not sleeps.
    #[test]
    fn sessions_progress_independently_during_a_slow_send() {
        use std::sync::Barrier;
        let qm = Mutex::new(qs(vec![due_at("a", "slow"), due_at("b", "fast")]));
        let entered = Barrier::new(2); // slow worker is inside fire()
        let release = Barrier::new(2); // let the slow send finish
        std::thread::scope(|s| {
            let qref = &qm;
            let (er, rl) = (&entered, &release);
            s.spawn(move || {
                let res = send_test(
                    qref,
                    &AtomicBool::new(false),
                    "slow",
                    720,
                    &HashMap::new(),
                    &|_: &QueueItem| {
                        er.wait(); // signal: mid-send, queue lock NOT held
                        rl.wait(); // block until the main thread saw "fast" done
                        Ok(())
                    },
                    &ok_persist,
                );
                assert!(matches!(res, SendResult::Sent { .. }));
            });
            entered.wait();
            // while "slow" is stalled inside its injection, "fast" completes
            let res = send_test(
                &qm,
                &AtomicBool::new(false),
                "fast",
                720,
                &HashMap::new(),
                &|_: &QueueItem| Ok(()),
                &ok_persist,
            );
            assert!(matches!(res, SendResult::Sent { .. }));
            {
                let q = qm.lock().unwrap();
                assert!(
                    !q.items.iter().any(|i| i.session == "fast"),
                    "fast session progressed while slow was mid-send"
                );
                assert!(
                    q.items.iter().any(|i| i.session == "slow"),
                    "slow send still in flight"
                );
            }
            release.wait();
        });
        let q = qm.lock().unwrap();
        assert_eq!(q.deliveries.len(), 2, "both sessions delivered");
        assert!(q.pending.is_empty());
    }

    // ---------- round 4: persist-then-commit transactions ----------

    /// A persist that can be switched to failing, recording every state it
    /// was asked to write (the "disk").
    struct FakeDisk {
        fail: AtomicBool,
        writes: Mutex<Vec<String>>,
    }
    impl FakeDisk {
        fn new(initial: &QueueState) -> Self {
            FakeDisk {
                fail: AtomicBool::new(false),
                writes: Mutex::new(vec![serde_json::to_string(initial).unwrap()]),
            }
        }
        fn persist(&self, q: &QueueState) -> Result<(), String> {
            if self.fail.load(AtomicOrdering::Relaxed) {
                return Err("No space left on device (os error 28)".into());
            }
            self.writes
                .lock()
                .unwrap()
                .push(serde_json::to_string(q).unwrap());
            Ok(())
        }
        /// what a fresh deck would load right now
        fn on_disk(&self) -> String {
            self.writes.lock().unwrap().last().unwrap().clone()
        }
    }

    fn add_args(session: &str, text: &str) -> QueueAddArgs {
        QueueAddArgs {
            session: session.into(),
            dir: String::new(),
            cmd: String::new(),
            text: text.into(),
            mode: "chain".into(),
            at: None,
            every: None,
            win_from: None,
            win_to: None,
            until_n: None,
            until_at: None,
            steps: None,
            tpl: None,
            tpl_idx: None,
            tpl_total: None,
        }
    }

    /// Every user-driven mutation, run twice: once against a healthy disk
    /// (change visible in memory AND on disk) and once against a failing one
    /// (error returned, memory byte-identical, disk untouched).
    #[test]
    fn every_queue_mutation_is_all_or_nothing() {
        let base = || {
            let mut a = qi("a", "at");
            a.at = Some(NOW - 1);
            let mut b = qi("b", "chain");
            b.text = "second".into();
            let mut f = qi("f", "chain");
            f.state = "failed".into();
            f.attempts = MAX_ATTEMPTS;
            let mut other = qi("o", "at");
            other.session = "other".into();
            qs(vec![a, b, f, other])
        };
        type Mutation = (&'static str, fn(&mut QueueState) -> Result<(), String>);
        let mutations: Vec<Mutation> = vec![
            ("add", |q| {
                add_item(q, add_args("s", "fresh"), "fresh".into())
            }),
            ("update", |q| update_text(q, "a", "edited".into())),
            ("remove", |q| remove_item(q, "a").map(|_| ())),
            ("pause", |q| pause_item(q, "a", true)),
            ("retry", |q| retry_item(q, "f")),
            ("skip", |q| remove_item(q, "f").map(|_| ())),
            ("clear-session", |q| {
                clear_session_items(q, "s");
                Ok(())
            }),
            ("expiry-purge", |q| {
                q.items[0].mode = "every".into();
                q.items[0].until_at = Some(NOW - 1);
                purge_expired(q, NOW);
                Ok(())
            }),
        ];
        for (name, mutate) in mutations {
            // healthy disk: the change lands in memory and on disk together
            let qm = Mutex::new(base());
            let disk = FakeDisk::new(&base());
            let before = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
            with_queue(&qm, &|q| disk.persist(q), mutate).unwrap_or_else(|e| panic!("{name}: {e}"));
            let after = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
            assert_ne!(before, after, "{name}: mutation had no effect");
            assert_eq!(after, disk.on_disk(), "{name}: memory and disk agree");

            // failing disk: same mutation, nothing changes anywhere
            let qm = Mutex::new(base());
            let disk = FakeDisk::new(&base());
            disk.fail.store(true, AtomicOrdering::Relaxed);
            let disk_before = disk.on_disk();
            let err = with_queue(&qm, &|q| disk.persist(q), mutate)
                .expect_err(&format!("{name}: failed save must be an error"));
            assert_eq!(storage::err_code(&err), "disk-full", "{name}: {err}");
            assert_eq!(
                serde_json::to_string(&*qm.lock().unwrap()).unwrap(),
                before,
                "{name}: shared memory must be byte-identical after a failed save"
            );
            assert_eq!(disk.on_disk(), disk_before, "{name}: disk untouched");
        }
    }

    #[test]
    fn a_rejected_mutation_never_reaches_the_disk() {
        // the firing contract rejects before any write is attempted
        let mut a = qi("a", "at");
        a.state = "firing".into();
        let qm = Mutex::new(qs(vec![a]));
        let disk = FakeDisk::new(&qm.lock().unwrap().clone());
        let writes0 = disk.writes.lock().unwrap().len();
        assert!(with_queue(&qm, &|q| disk.persist(q), |q| update_text(
            q,
            "a",
            "edited".into()
        ))
        .is_err());
        assert_eq!(disk.writes.lock().unwrap().len(), writes0, "no save tried");
        assert_eq!(qm.lock().unwrap().items[0].text, "x");
    }

    #[test]
    fn a_failed_retry_save_keeps_the_item_out_of_the_candidate_set() {
        let mut f = qi("f", "chain");
        f.state = "failed".into();
        f.attempts = MAX_ATTEMPTS;
        let qm = Mutex::new(qs(vec![f]));
        let disk = FakeDisk::new(&qm.lock().unwrap().clone());
        disk.fail.store(true, AtomicOrdering::Relaxed);
        assert!(with_queue(&qm, &|q| disk.persist(q), |q| retry_item(q, "f")).is_err());
        let q = qm.lock().unwrap();
        assert!(item_dead(&q.items[0]), "still dead in memory");
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        assert!(
            select_due(&q, NOW, 720, &quiet).is_empty(),
            "a retry the user was told failed must not re-enter the schedule"
        );
    }

    #[test]
    fn a_failed_pre_fire_save_sends_nothing_and_changes_nothing() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let before = serde_json::to_string(&*qm.lock().unwrap()).unwrap();
        let dirty = AtomicBool::new(false);
        let res = send_test(
            &qm,
            &dirty,
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| panic!("must not inject when the intent never hit disk"),
            &|_: &QueueState| Err("No space left on device".into()),
        );
        assert_eq!(res, SendResult::NotPersisted);
        assert_eq!(
            serde_json::to_string(&*qm.lock().unwrap()).unwrap(),
            before,
            "intent rolled back completely"
        );
        assert!(!dirty.load(AtomicOrdering::Relaxed), "nothing owed to disk");
    }

    #[test]
    fn a_failed_post_send_save_keeps_memory_authoritative_and_retries() {
        // the prompt really went out: memory MUST take the finalized state
        // (re-sending would break at-most-once), and the write is retried
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let disk = FakeDisk::new(&qm.lock().unwrap().clone());
        let dirty = AtomicBool::new(false);
        let persist = |q: &QueueState| disk.persist(q);
        let res = send_test(
            &qm,
            &dirty,
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| {
                disk.fail.store(true, AtomicOrdering::Relaxed); // disk dies mid-send
                Ok(())
            },
            &persist,
        );
        assert!(matches!(res, SendResult::Sent { .. }));
        assert!(dirty.load(AtomicOrdering::Relaxed), "write still owed");
        {
            let q = qm.lock().unwrap();
            assert!(q.items.is_empty(), "delivery finalized in memory");
            assert_eq!(q.deliveries.len(), 1);
            assert!(q.pending.is_empty());
        }
        // still failing: nothing changes, the flag stays up
        assert!(!flush_dirty(&qm, &dirty, &persist));
        assert!(dirty.load(AtomicOrdering::Relaxed));
        // disk recovers: the retry lands and the flag clears
        disk.fail.store(false, AtomicOrdering::Relaxed);
        assert!(flush_dirty(&qm, &dirty, &persist));
        assert!(!dirty.load(AtomicOrdering::Relaxed));
        assert_eq!(
            disk.on_disk(),
            serde_json::to_string(&*qm.lock().unwrap()).unwrap()
        );
        assert!(!flush_dirty(&qm, &dirty, &persist), "nothing owed anymore");
    }

    #[test]
    fn a_definitively_refused_send_that_cannot_be_saved_is_retried_not_forgotten() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let disk = FakeDisk::new(&qm.lock().unwrap().clone());
        let dirty = AtomicBool::new(false);
        let persist = |q: &QueueState| disk.persist(q);
        let res = send_test(
            &qm,
            &dirty,
            "s",
            720,
            &HashMap::new(),
            &|_: &QueueItem| {
                disk.fail.store(true, AtomicOrdering::Relaxed);
                Err("tmux send-keys failed: can't find session: x".into())
            },
            &persist,
        );
        assert!(matches!(res, SendResult::Failed { .. }));
        {
            let q = qm.lock().unwrap();
            assert_eq!(q.items[0].state, "failed", "not sent — retryable");
            assert!(q.pending.is_empty(), "no delivery to recover");
            assert!(q.deliveries.is_empty(), "a refused send is never audited");
        }
        assert!(dirty.load(AtomicOrdering::Relaxed));
        disk.fail.store(false, AtomicOrdering::Relaxed);
        assert!(flush_dirty(&qm, &dirty, &persist));
        // the persisted state carries the "not sent" truth, so a restart
        // resumes the retry instead of counting the prompt as delivered
        let recovered: QueueState = serde_json::from_str(&disk.on_disk()).unwrap();
        assert_eq!(recovered.items[0].state, "failed");
        assert!(recovered.pending.is_empty());
        assert!(recovered.deliveries.is_empty());
    }

    #[test]
    fn window_plain_and_midnight_wrap() {
        // 08:00–18:00
        assert!(in_window(8 * 60, Some(480), Some(1080)));
        assert!(in_window(17 * 60 + 59, Some(480), Some(1080)));
        assert!(!in_window(18 * 60, Some(480), Some(1080)));
        assert!(!in_window(3 * 60, Some(480), Some(1080)));
        // 20:00–08:00 wraps midnight
        assert!(in_window(23 * 60, Some(1200), Some(480)));
        assert!(in_window(2 * 60, Some(1200), Some(480)));
        assert!(!in_window(12 * 60, Some(1200), Some(480)));
        // no / degenerate window = always
        assert!(in_window(0, None, None));
        assert!(in_window(700, Some(600), Some(600)));
    }
}
