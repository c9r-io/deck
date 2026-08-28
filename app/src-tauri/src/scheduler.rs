//! Scheduled prompts: at / chain (quiet-based) / every (recurring rules
//! with daily windows) + per-project templates. Delivery is at-most-once:
//! the firing intent is persisted before injection and crash recovery never
//! re-sends. Tick logic is pure and unit-tested; the thread only adds IO.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
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

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct QueueState {
    items: Vec<QueueItem>,
    /// session → when we last injected a prompt
    last_fired: HashMap<String, u64>,
    /// delivery audit trail; also the idempotency guard for finalize
    #[serde(default)]
    deliveries: Vec<DeliveryRecord>,
}

pub(crate) struct Queues(pub(crate) Mutex<QueueState>);

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
    match storage::load(&queue_path()) {
        Ok(Some(raw)) => {
            let mut q: QueueState = serde_json::from_str(&raw).unwrap_or_else(|e| {
                storage::warn(format!(
                    "queue.json parsed but has an unexpected shape ({e}); starting with an empty queue —                  the original file is preserved as .bak"
                ));
                QueueState::default()
            });
            migrate_groups(&mut q);
            q
        }
        Ok(None) => QueueState::default(),
        Err(e) => {
            storage::warn(format!("scheduled prompts could not be loaded: {e}"));
            QueueState::default()
        }
    }
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
    state.0.lock().unwrap().clone()
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
    let mut q = state.0.lock().unwrap();
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
    if let Err(e) = save_queue(&q) {
        // never let the scheduler act on an item the disk doesn't know about
        q.items.pop();
        return Err(e);
    }
    let _ = app.emit("queue-changed", ());
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
    let mut q = state.0.lock().unwrap();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.text = text;
    }
    save_queue(&q)?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn queue_remove(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let mut q = state.0.lock().unwrap();
    q.items.retain(|i| i.id != id);
    save_queue(&q)?;
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
    let mut q = state.0.lock().unwrap();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.paused = paused;
    }
    save_queue(&q)?;
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
    let mut q = state.0.lock().unwrap();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.state = default_state();
        item.attempts = 0;
        item.last_error = None;
        item.last_attempt_at = None;
    }
    save_queue(&q)?;
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
    let mut q = state.0.lock().unwrap();
    let n0 = q.items.len();
    q.items.retain(|i| i.id != id);
    if q.items.len() != n0 {
        applog("[queue] step skipped by user — group unblocked");
    }
    save_queue(&q)?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Drop all queued prompts for a session — called when its card closes.
#[tauri::command]
pub(crate) fn queue_clear_session(
    state: State<'_, Queues>,
    app: AppHandle,
    session: String,
) -> Result<(), String> {
    let mut q = state.0.lock().unwrap();
    q.items.retain(|i| i.session != session);
    q.last_fired.remove(&session);
    save_queue(&q)?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

/// Note: deliberately carries no prompt text — the UI only toasts the
/// session name, and event payloads must not haul content around (privacy).
#[derive(Clone, Serialize)]
pub(crate) struct QueueFired {
    session: String,
}

/// Inject one prompt into its session, starting the session if needed.
pub(crate) fn fire_item(item: &QueueItem) -> Result<(), String> {
    let alive: HashSet<String> = tmux(&["list-sessions", "-F", "#{session_name}"])
        .map(|o| o.lines().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if !alive.contains(&item.session) {
        start_session(item.session.clone(), item.dir.clone(), item.cmd.clone())?;
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    // -l = literal text (no key-name interpretation), then a real Enter
    tmux(&[
        "send-keys",
        "-t",
        &pane_target(&item.session),
        "-l",
        &item.text,
    ])?;
    tmux(&["send-keys", "-t", &pane_target(&item.session), "Enter"])?;
    Ok(())
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

/// Expired rules die quietly (their stop instant passed while sleeping).
pub(crate) fn purge_expired(q: &mut QueueState, now: u64) -> bool {
    let n0 = q.items.len();
    q.items
        .retain(|i| !(i.mode == "every" && i.until_at.map(|t| now >= t).unwrap_or(false)));
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
        return; // this delivery is already fully accounted
    }
    let Some(item) = q.items.iter().find(|i| i.id == item_id).cloned() else {
        return; // removed mid-flight (user action) — nothing left to account
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
    q.last_fired.insert(item.session.clone(), now);
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
}

pub(crate) fn note_failed(q: &mut QueueState, id: &str, err: &str) {
    if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
        it.state = "failed".into();
        it.last_error = Some(err.chars().take(200).collect());
        it.delivery = None; // the send did NOT happen — no delivery to recover
    }
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
    notes
}

pub(crate) fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let state = app.state::<Queues>();
        if state.0.lock().unwrap().items.is_empty() {
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
        {
            let mut q = state.0.lock().unwrap();
            if purge_expired(&mut q, now_epoch()) {
                if let Err(e) = save_queue(&q) {
                    applog(&format!("[queue] persist (expiry purge) FAILED: {e}"));
                }
                drop(q);
                let _ = app.emit("queue-changed", ());
            }
        }
        // tick-start candidate pass: at most one session slot each. The
        // candidates only tell us WHICH sessions to serve — each actual send
        // below re-selects from FRESH state under the lock, so a pause, text
        // edit or removal that happened since the tick began is honored.
        let sessions: Vec<String> = {
            let q = state.0.lock().unwrap();
            select_due(&q, now_epoch(), local_minutes(), &activity)
                .into_iter()
                .map(|i| i.session)
                .collect()
        };
        for session in sessions {
            let (item, delivery) = {
                let mut q = state.0.lock().unwrap();
                let Some(sel) =
                    select_for_session(&q, &session, now_epoch(), local_minutes(), &activity)
                else {
                    continue;
                };
                // Persist the firing intent (with its delivery id) BEFORE
                // injecting — this ordering is what makes delivery
                // at-most-once across crashes.
                let delivery = next_delivery_id();
                let prev_attempt_at = sel.last_attempt_at;
                let Some(it) = q.items.iter_mut().find(|i| i.id == sel.id) else {
                    continue;
                };
                it.state = "firing".into();
                it.attempts += 1;
                it.last_attempt_at = Some(now_epoch());
                it.delivery = Some(delivery.clone());
                if let Err(e) = save_queue(&q) {
                    applog(&format!(
                        "[queue] persist (pre-fire) FAILED: {e} — not sending this tick"
                    ));
                    if let Some(it) = q.items.iter_mut().find(|i| i.id == sel.id) {
                        it.state = default_state();
                        it.attempts -= 1;
                        it.last_attempt_at = prev_attempt_at;
                        it.delivery = None;
                    }
                    continue;
                }
                let snapshot = q.items.iter().find(|i| i.id == sel.id).unwrap().clone();
                (snapshot, delivery)
            };
            match fire_item(&item) {
                Ok(()) => {
                    // never log prompt contents — length only (privacy)
                    applog(&format!(
                        "[queue] sent to {} ({}B, mode {})",
                        item.session,
                        item.text.len(),
                        item.mode
                    ));
                    let mut q = state.0.lock().unwrap();
                    finalize_delivery(&mut q, &item.id, &delivery, now_epoch(), false);
                    if let Err(e) = save_queue(&q) {
                        applog(&format!("[queue] persist (post-fire) FAILED: {e}"));
                    }
                    drop(q);
                    let _ = app.emit(
                        "queue-fired",
                        QueueFired {
                            session: item.session.clone(),
                        },
                    );
                    let _ = app.emit("queue-changed", ());
                }
                Err(e) => {
                    let mut q = state.0.lock().unwrap();
                    note_failed(&mut q, &item.id, &e);
                    let gave_up = q.items.iter().any(|i| i.id == item.id && item_dead(i));
                    if let Err(pe) = save_queue(&q) {
                        applog(&format!("[queue] persist (post-failure) FAILED: {pe}"));
                    }
                    drop(q);
                    applog(&format!(
                        "[queue] send FAILED for {} (attempt {}): {e}{}",
                        item.session,
                        item.attempts,
                        if gave_up {
                            " — giving up"
                        } else {
                            " (will back off and retry)"
                        }
                    ));
                    let _ = app.emit("queue-changed", ());
                }
            }
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
