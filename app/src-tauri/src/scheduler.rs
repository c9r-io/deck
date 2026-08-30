//! Scheduled prompts: at / chain (quiet-based) / every (recurring rules
//! with daily windows) + per-project templates. A persisted firing intent is
//! deliberately treated as ambiguous after a crash: deck neither claims it
//! succeeded nor sends it again until the user resolves it. Tick logic is
//! pure and unit-tested; the thread only adds IO.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::start_session;
use crate::context::{self, ContextCheck, ContextCode, ContextStatus, PaneIdentity, ProbeResult};
use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::{tmux, tmux_owned};

// ---------- scheduled prompts ----------------------------------------------------
// Queue prompts to be typed into a session later — the rate-limit workflow:
// "when my Claude quota window resets in 5h, send these tasks in order".
// tmux send-keys needs no attached client, so this works fully detached.
// The scheduler lives in a Rust thread (webview timers get frozen by App Nap).

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct QueueItem {
    id: String,
    session: String,
    /// Stable Board identity chosen when the task was created.
    #[serde(default)]
    card_id: String,
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
    /// lifecycle: "pending" (default) | "firing" | "failed" | "ambiguous".
    /// "firing" is persisted BEFORE injection. If the process disappears
    /// before the post-send state lands, boot migrates it to "ambiguous" and
    /// requires an explicit acknowledge or risk-accepting retry.
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
    /// Sanitized executable basename only; never written to logs.
    #[serde(default)]
    expected_process: Option<String>,
    /// tmux generation binding. Exact pane targeting prevents a recreated
    /// session with the same user-facing name from inheriting permission.
    #[serde(default)]
    binding: Option<PaneIdentity>,
    /// Closed result only: no foreground string or terminal contents.
    #[serde(default)]
    last_context: Option<ContextCheck>,
    /// Incremented by edits that invalidate an in-progress readiness wait.
    #[serde(default)]
    revision: u64,
}

pub(crate) fn default_state() -> String {
    "pending".into()
}

/// Audit record of one prompt delivery. `assumed` is retained for schema
/// compatibility and now means the user explicitly acknowledged an
/// ambiguous delivery; recovery itself never fabricates success.
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
/// window; until the write lands, a crash sees the older firing intent and
/// exposes it as ambiguous, so the scheduler keeps retrying every tick.
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
    storage::deck_dir().join("queue.json")
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
    migrate_context(&mut q);
    q
}

/// Compatibility migration ignores all legacy policy/agent/hook authority.
/// Serde discards those unknown fields and the next save cleans them from
/// disk. Preserve scheduling/delivery state, derive only a sanitized process
/// basename when possible, and clear stale hook-derived context results.
pub(crate) fn migrate_context(q: &mut QueueState) {
    for item in q
        .items
        .iter_mut()
        .chain(q.pending.iter_mut().map(|pending| &mut pending.snapshot))
    {
        if item.expected_process.is_none() {
            item.expected_process = context::expected_from_command(&item.cmd);
        }
        if item.last_context.as_ref().is_some_and(|check| {
            check.status == ContextStatus::Unknown || check.code == ContextCode::Unknown
        }) {
            item.last_context = None;
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
    if crate::smoke_faults::take("queue-save") {
        return Err("injected queue save failure".into());
    }
    let raw = serde_json::to_string(q).map_err(|e| e.to_string())?;
    storage::save_typed::<QueueState>(&queue_path(), &raw)
}

#[tauri::command]
pub(crate) fn queue_list(state: State<'_, Queues>) -> QueueState {
    state.q.lock().unwrap().clone()
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
) -> Result<ContextProbeView, String> {
    let item = state
        .q
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or("scheduled prompt not found")?;
    let result = final_context_probe(&item);
    persist_context_result(&state.q, &save_queue, &item, &result)?
        .ok_or("scheduled prompt changed while probing")?;
    Ok(ContextProbeView {
        status: result.status,
        code: result.code,
        expected_process: item.expected_process,
        current_process: result.current_process,
    })
}

/// Explicitly bind an identity-changed item to the pane currently owned by
/// its card. This is a persisted target change, never an implicit override.
#[tauri::command]
pub(crate) fn queue_rebind(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
    let source = state
        .q
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or("scheduled prompt not found")?;
    let raw = context::raw_probe(&source.session)?;
    with_queue(&state.q, &save_queue, |q| {
        firing_conflict(q, &id)?;
        let item = q
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or("scheduled prompt not found")?;
        if item.revision != source.revision {
            return Err("scheduled prompt changed while rebinding".into());
        }
        item.binding = Some(raw.identity.clone());
        if item.expected_process.is_none() && !context::shell_process(raw.foreground.as_deref()) {
            item.expected_process = raw.foreground.clone();
        }
        item.revision = item.revision.wrapping_add(1);
        item.last_context = Some(ContextCheck {
            status: ContextStatus::Ready,
            code: ContextCode::TargetChecked,
            checked_at: now_epoch(),
        });
        Ok(())
    })?;
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) fn smoke_seed_ambiguous(state: State<'_, Queues>) -> Result<(), String> {
    if !crate::smoke_faults::enabled() {
        return Err("smoke queue hooks are unavailable".into());
    }
    let mut q = state.q.lock().unwrap();
    let item = q.items.first_mut().ok_or("smoke queue is empty")?;
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
pub(crate) fn smoke_queue_state(state: State<'_, Queues>) -> Result<SmokeQueueState, String> {
    if !crate::smoke_faults::enabled() {
        return Err("smoke queue hooks are unavailable".into());
    }
    let q = state.q.lock().unwrap().clone();
    let disk =
        storage::load_typed::<QueueState>(&queue_path())?.ok_or("smoke queue file is missing")?;
    let disk: QueueState = serde_json::from_str(&disk.payload).map_err(|e| e.to_string())?;
    Ok(SmokeQueueState {
        dirty: state.dirty.load(AtomicOrdering::Relaxed),
        disk_matches: serde_json::to_value(q).ok() == serde_json::to_value(disk).ok(),
    })
}

#[tauri::command]
pub(crate) fn smoke_flush_queue(state: State<'_, Queues>) -> Result<bool, String> {
    if !crate::smoke_faults::enabled() {
        return Err("smoke queue hooks are unavailable".into());
    }
    Ok(flush_dirty(&state.q, &state.dirty, &save_queue))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QueueAddArgs {
    session: String,
    #[serde(default)]
    card_id: String,
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
    if a.card_id.is_empty()
        || a.card_id.len() > 128
        || !a
            .card_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err("scheduled prompt needs a valid card identity".into());
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
pub(crate) fn add_item(q: &mut QueueState, args: QueueAddArgs, text: String) -> Result<(), String> {
    let expected_process = context::expected_from_command(&args.cmd);
    add_item_bound(q, args, text, None, expected_process)
}

fn add_item_bound(
    q: &mut QueueState,
    args: QueueAddArgs,
    text: String,
    binding: Option<PaneIdentity>,
    expected_process: Option<String>,
) -> Result<(), String> {
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
) -> Result<(), String> {
    validate_add(&args)?;
    let text = args.text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err("empty prompt".into());
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
pub(crate) fn firing_conflict(q: &QueueState, id: &str) -> Result<(), String> {
    if let Some(i) = q.items.iter().find(|i| i.id == id) {
        if i.state == "firing" {
            return Err("this prompt is being sent right now — try again in a few seconds".into());
        }
        if i.state == "ambiguous" {
            return Err(
                "this prompt has an ambiguous delivery — acknowledge or retry it first".into(),
            );
        }
    }
    Ok(())
}

/// Pure core of queue_update (unit-tested with the firing contract).
pub(crate) fn update_text(q: &mut QueueState, id: &str, text: String) -> Result<(), String> {
    firing_conflict(q, id)?;
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.text = text;
        item.revision = item.revision.wrapping_add(1);
        item.last_context = None;
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
    if q.items.iter().any(|i| i.id == id && i.state == "firing") {
        return Err("this prompt is being sent right now — try again in a few seconds".into());
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
            return Err("prompt has an unknown delivery state".into());
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
pub(crate) fn acknowledge_ambiguous(q: &mut QueueState, id: &str) -> Result<(), String> {
    let Some(item) = q.items.iter().find(|i| i.id == id).cloned() else {
        return if q.deliveries.iter().any(|d| d.item == id) {
            Ok(())
        } else {
            Err("scheduled prompt not found".into())
        };
    };
    if item.state != "ambiguous" {
        return if item.delivery.is_none() && q.deliveries.iter().any(|d| d.item == id) {
            Ok(())
        } else {
            Err("this prompt is not awaiting an ambiguous-delivery decision".into())
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

/// Explicitly treat an ambiguous delivery as sent. This is the only recovery
/// path that performs delivery accounting; boot never does so on its own.
#[tauri::command]
pub(crate) fn queue_acknowledge(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
) -> Result<(), String> {
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
) -> Result<(), String> {
    if crate::smoke_faults::take("queue-cancel") {
        return Err("injected queue cancellation failure".into());
    }
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
pub(crate) fn fire_item(item: &QueueItem) -> Result<(), String> {
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
    let line = format!("{}\r", item.text);
    let buffer = format!("deck-send-{delivery}");
    // Pane/session ids can be reused after the entire tmux server exits. Put
    // the bytes in a uniquely named tmux buffer, then compare the FULL
    // generation and paste them in the same server command queue. `-F`
    // evaluates synchronously (no shell); the inner commands contain only
    // deck-generated ids, never user text. paste-buffer is byte-literal, so
    // prompt + CR remain one indivisible terminal input.
    let actual = "#{pid}:#{session_id}:#{window_id}:#{pane_id}:#{pane_pid}";
    let expected = format!(
        "{}:{}:{}:{}:{}",
        pane.server_pid, pane.session_id, pane.window_id, pane.pane_id, pane.pane_pid
    );
    let identity_condition = format!("#{{==:{actual},{expected}}}");
    let condition = match item.expected_process.as_deref() {
        Some(process) => {
            format!("#{{&&:{identity_condition},#{{==:#{{pane_current_command}},{process}}}}}")
        }
        None => identity_condition,
    };
    let yes = format!("paste-buffer -b {buffer} -d -t {}", pane.pane_id);
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
        condition,
        yes,
        no,
    ]);
    if out.as_ref().is_ok_and(|stdout| {
        !stdout
            .lines()
            .any(|line| line.trim() == "deck-context-refused")
    }) {
        return Ok(());
    }
    // A vanished target can abort the command queue before its refusal
    // branch deletes the private buffer. Never leave prompt bytes behind in
    // tmux after a refused/indeterminate injection.
    let _ = tmux(&["delete-buffer", "-b", &buffer]);
    Err("context identity or foreground changed before literal send".into())
}

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
        return context::probe(
            &item.session,
            item.binding.as_ref(),
            item.expected_process.as_deref(),
        );
    }
    match start_session(
        item.session.clone(),
        item.dir.clone(),
        item.cmd.clone(),
        false,
    ) {
        Ok(result) if result.created => {}
        // Another actor won the race between our outer existence check and
        // start_session's idempotent inner check. It is not a deck-created
        // generation and therefore must never inherit permission to send.
        Ok(_) => {
            return ProbeResult::blocked(
                ContextStatus::SessionReplaced,
                ContextCode::IdentityChanged,
            );
        }
        Err(_) => {
            return ProbeResult::blocked(ContextStatus::Unavailable, ContextCode::StartupFailed);
        }
    }

    let interval = std::time::Duration::from_millis(READY_PROBE_INTERVAL_MS);
    let polls = (READY_PROBE_TIMEOUT_MS / READY_PROBE_INTERVAL_MS) as usize + 1;
    poll_readiness(
        polls,
        cancelled,
        &mut |identity| {
            // A deck-initiated restart is allowed to acquire a new binding.
            // Once observed, another generation change is rejected.
            context::probe(&item.session, identity, item.expected_process.as_deref())
        },
        &mut || std::thread::sleep(interval),
    )
}

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
fn select_requested(q: &QueueState, session: &str, id: &str, now: u64) -> Option<QueueItem> {
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

fn select_for_request(
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
        it.last_error = Some(format!("send failed ({})", storage::err_code(err)));
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
    pub(crate) fire: &'a (dyn Fn(&QueueItem) -> Result<(), String> + Sync),
    pub(crate) persist: &'a (dyn Fn(&QueueState) -> Result<(), String> + Sync),
    /// kill a session whose card was deleted DURING this send
    pub(crate) kill: &'a (dyn Fn(&str) + Sync),
}

pub(crate) struct ContextHooks<'a> {
    pub(crate) prepare: &'a (dyn Fn(&QueueItem, &dyn Fn() -> bool) -> ProbeResult + Sync),
    pub(crate) final_probe: &'a (dyn Fn(&QueueItem) -> ProbeResult + Sync),
}

#[derive(Clone, Copy)]
struct SendRequest<'a> {
    session: &'a str,
    now_min: u32,
    activity: &'a HashMap<String, u64>,
    requested: Option<&'a str>,
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
            // The item and log both keep only a category — tmux/start errors
            // can embed paths or raw session names.
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
fn persist_context_result(
    qm: &Mutex<QueueState>,
    persist: &dyn Fn(&QueueState) -> Result<(), String>,
    selected: &QueueItem,
    result: &ProbeResult,
) -> Result<Option<QueueItem>, String> {
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
    .or_else(|e| if e == TX_NOOP { Ok(None) } else { Err(e) })
}

/// A delete can land after a dead-session readiness worker's first
/// cancellation check but before that worker starts tmux. The deleting path
/// cannot kill a session that does not exist yet, so the worker must inspect
/// the tombstone after its probe and reap anything it may have just started.
fn reap_probe_start_after_delete(qm: &Mutex<QueueState>, session: &str, h: &SendHooks) -> bool {
    let cancelled = is_cancelled(&qm.lock().unwrap(), session);
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

fn send_one_safe_requested(
    qm: &Mutex<QueueState>,
    dirty: &AtomicBool,
    request: SendRequest<'_>,
    h: &SendHooks,
    context_hooks: &ContextHooks,
) -> SendResult {
    let selected = {
        let q = qm.lock().unwrap();
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
        let q = qm.lock().unwrap();
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
                    storage::err_code(&e)
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
                storage::err_code(&e)
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
                    storage::err_code(&e)
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

/// Immediate delivery is one-shot. It can bypass only a freshly proven
/// foreground mismatch; exact identity remains mandatory in both probes and
/// the atomic paste guard. No persisted protection is weakened.
#[tauri::command]
pub(crate) fn queue_send_now(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    accept_process_mismatch: bool,
) -> Result<(), String> {
    let item = state
        .q
        .lock()
        .unwrap()
        .items
        .iter()
        .find(|i| i.id == id)
        .cloned()
        .ok_or("scheduled prompt not found")?;
    let observed = final_context_probe(&item);
    if observed.status == ContextStatus::SessionReplaced {
        return Err("context identity changed; rebind or reschedule before sending".into());
    }
    if accept_process_mismatch && observed.status != ContextStatus::ForegroundDifferent {
        return Err("one-shot process bypass requires a current foreground mismatch".into());
    }
    if !claim_session(&state.busy, &item.session) {
        return Err("this session already has a scheduled send in progress".into());
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
        SendResult::Blocked { .. } => Err("target context is unavailable".into()),
        SendResult::Nothing => Err("prompt is no longer eligible to send".into()),
        SendResult::NotPersisted => Err("delivery intent could not be saved".into()),
        SendResult::Failed { .. } => Err("tmux refused the literal send".into()),
    }
}

/// Boot migration is unusual: the interrupted send is an irreversible fact,
/// so recovered `ambiguous` memory is authoritative even when the first disk
/// write fails. `dirty` then gives the scheduler a real retry driver.
fn boot_queues_with(
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

pub(crate) fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn qi(id: &str, mode: &str) -> QueueItem {
        QueueItem {
            id: id.into(),
            session: "s".into(),
            card_id: "card-s".into(),
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
            expected_process: Some("codex".into()),
            binding: None,
            last_context: None,
            revision: 0,
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

    // ---------- finalize / ambiguous crash recovery ----------

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
    fn crash_after_intent_becomes_ambiguous_and_never_auto_resends() {
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
        let once = q.items.iter().find(|i| i.id == "o").unwrap();
        assert_eq!(once.state, "ambiguous");
        let rl = q.items.iter().find(|i| i.id == "t").unwrap();
        assert_eq!(
            (rl.fired, rl.last, rl.state.as_str()),
            (0, None, "ambiguous")
        );
        assert!(q.deliveries.is_empty(), "recovery never claims success");
        assert_eq!(q.items.iter().filter(|i| i.mode == "chain").count(), 0);
        assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
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
        assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
    }

    #[test]
    fn acknowledge_after_crash_matches_live_result_and_is_idempotent() {
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
        acknowledge_ambiguous(&mut rec, "t").unwrap();
        acknowledge_ambiguous(&mut rec, "t").unwrap();
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
        assert!(q
            .items
            .iter()
            .any(|i| i.id == "t" && i.state == "ambiguous"));
        acknowledge_ambiguous(&mut q, "t").unwrap();
        acknowledge_ambiguous(&mut q, "t").unwrap();
        // second (and last) fire counted exactly once after acknowledgement
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
        assert_eq!(q.items[0].state, "ambiguous");
        acknowledge_ambiguous(&mut q, "o").unwrap();
        assert!(q.items.is_empty());
        acknowledge_ambiguous(&mut q, "o").unwrap();
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
            card_id: "card-s".into(),
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
        // recurring rule mid-send: a live result still finishes its audit,
        // but the deleted card and its future schedule never return
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
        assert!(notes.is_empty(), "a cancelled delivery needs no decision");
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
    fn recovery_restores_orphaned_ledger_entries_as_ambiguous() {
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
        assert!(notes[0].contains("choose acknowledge or retry"));
        assert!(q.deliveries.is_empty());
        assert!(q.last_fired.is_empty());
        assert_eq!(q.items[0].state, "ambiguous");
        // repeated recovery changes nothing
        let snapshot = serde_json::to_string(&q).unwrap();
        recover_interrupted(&mut q);
        assert_eq!(serde_json::to_string(&q).unwrap(), snapshot);
    }

    #[test]
    fn boot_persist_failure_keeps_ambiguous_memory_dirty_until_flush() {
        let mut firing = due_at("a", "s");
        firing.state = "firing".into();
        firing.delivery = Some("d1".into());
        let loaded = qs(vec![firing]);
        let fail = AtomicBool::new(true);
        let disk = Mutex::new(String::new());
        let persist = |q: &QueueState| {
            if fail.load(AtomicOrdering::Relaxed) {
                Err("disk unavailable".into())
            } else {
                *disk.lock().unwrap() = serde_json::to_string(q).unwrap();
                Ok(())
            }
        };
        let queues = boot_queues_with(loaded, &persist);
        {
            let q = queues.q.lock().unwrap();
            assert_eq!(q.items[0].state, "ambiguous");
            assert!(select_due(&q, NOW + 100_000, 720, &HashMap::new()).is_empty());
        }
        assert!(queues.dirty.load(AtomicOrdering::Relaxed));
        assert!(!flush_dirty(&queues.q, &queues.dirty, &persist));
        assert!(queues.dirty.load(AtomicOrdering::Relaxed));
        fail.store(false, AtomicOrdering::Relaxed);
        assert!(flush_dirty(&queues.q, &queues.dirty, &persist));
        assert!(!queues.dirty.load(AtomicOrdering::Relaxed));
        let saved: QueueState = serde_json::from_str(&disk.lock().unwrap()).unwrap();
        assert_eq!(saved.items[0].state, "ambiguous");
    }

    #[test]
    fn orphan_ledger_boot_failure_is_immediately_decidable_and_ack_is_transactional() {
        let mut snapshot = due_at("a", "s");
        snapshot.state = "firing".into();
        snapshot.delivery = Some("d1".into());
        let mut loaded = qs(vec![]);
        loaded.pending.push(PendingDelivery {
            id: "d1".into(),
            snapshot,
        });
        let queues = boot_queues_with(loaded, &|_| Err("read only".into()));
        let before = serde_json::to_string(&*queues.q.lock().unwrap()).unwrap();
        assert!(before.contains("ambiguous"));
        assert!(
            with_queue(&queues.q, &|_| Err("still read only".into()), |q| {
                acknowledge_ambiguous(q, "a")
            })
            .is_err()
        );
        assert_eq!(
            serde_json::to_string(&*queues.q.lock().unwrap()).unwrap(),
            before
        );
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

    fn pane(n: u32) -> PaneIdentity {
        PaneIdentity {
            server_pid: 90 + n,
            session_id: format!("${n}"),
            window_id: format!("@{n}"),
            pane_id: format!("%{n}"),
            pane_pid: 100 + n,
        }
    }

    fn probe_result(status: ContextStatus, code: ContextCode, identity: u32) -> ProbeResult {
        ProbeResult {
            status,
            code,
            identity: Some(pane(identity)),
            current_process: Some("codex".into()),
        }
    }

    fn send_safe_test(
        qm: &Mutex<QueueState>,
        fire: &(dyn Fn(&QueueItem) -> Result<(), String> + Sync),
        prepare: &(dyn Fn(&QueueItem, &dyn Fn() -> bool) -> ProbeResult + Sync),
        final_probe: &(dyn Fn(&QueueItem) -> ProbeResult + Sync),
    ) -> SendResult {
        let kill = |_: &str| {};
        send_one_safe(
            qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &SendHooks {
                fire,
                persist: &ok_persist,
                kill: &kill,
            },
            &ContextHooks {
                prepare,
                final_probe,
            },
        )
    }

    #[test]
    fn safe_ready_context_sends_exactly_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let sends = AtomicU32::new(0);
        let ready = |_: &QueueItem, _: &dyn Fn() -> bool| {
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
        };
        let final_ready = |i: &QueueItem| {
            assert_eq!(i.binding.as_ref(), Some(&pane(1)));
            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
        };
        let result = send_safe_test(
            &qm,
            &|i: &QueueItem| {
                assert_eq!(i.binding.as_ref(), Some(&pane(1)));
                sends.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            &ready,
            &final_ready,
        );
        assert!(matches!(result, SendResult::Sent { .. }));
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(qm.lock().unwrap().deliveries.len(), 1);
    }

    #[test]
    fn unsafe_contexts_block_without_attempt_or_ledger() {
        for (status, code) in [
            (
                ContextStatus::ForegroundDifferent,
                ContextCode::ForegroundDifferent,
            ),
            (ContextStatus::Unavailable, ContextCode::ProbeFailed),
        ] {
            let qm = Mutex::new(qs(vec![due_at("a", "s")]));
            let prepare = move |_: &QueueItem, _: &dyn Fn() -> bool| probe_result(status, code, 1);
            let result = send_safe_test(
                &qm,
                &|_: &QueueItem| panic!("blocked context must never send"),
                &prepare,
                &|_: &QueueItem| panic!("blocked context has no final probe"),
            );
            assert!(matches!(result, SendResult::Blocked { status: s, .. } if s == status));
            let q = qm.lock().unwrap();
            assert_eq!(q.items[0].attempts, 0);
            assert_eq!(q.items[0].state, "pending");
            assert!(q.pending.is_empty() && q.deliveries.is_empty());
            assert_eq!(q.items[0].last_context.as_ref().unwrap().status, status);
        }
    }

    #[test]
    fn replacement_between_probe_and_send_is_rejected_without_attempt() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let result = send_safe_test(
            &qm,
            &|_: &QueueItem| panic!("replacement must never receive input"),
            &|_: &QueueItem, _: &dyn Fn() -> bool| {
                probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
            },
            &|_: &QueueItem| {
                probe_result(
                    ContextStatus::SessionReplaced,
                    ContextCode::IdentityChanged,
                    2,
                )
            },
        );
        assert!(matches!(
            result,
            SendResult::Blocked {
                status: ContextStatus::SessionReplaced,
                ..
            }
        ));
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].attempts, 0);
        assert!(q.pending.is_empty());
    }

    #[test]
    fn foreground_change_between_probe_and_send_is_rejected_without_attempt() {
        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let result = send_safe_test(
            &qm,
            &|_: &QueueItem| panic!("changed foreground must never receive input"),
            &|_: &QueueItem, _: &dyn Fn() -> bool| {
                probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
            },
            &|_: &QueueItem| {
                probe_result(
                    ContextStatus::ForegroundDifferent,
                    ContextCode::ForegroundDifferent,
                    1,
                )
            },
        );
        assert!(matches!(
            result,
            SendResult::Blocked {
                status: ContextStatus::ForegroundDifferent,
                ..
            }
        ));
        let q = qm.lock().unwrap();
        assert_eq!(q.items[0].attempts, 0);
        assert!(q.pending.is_empty());
    }

    #[test]
    fn pause_edit_and_delete_during_probe_cancel_the_worker() {
        for action in ["pause", "edit", "delete"] {
            let qm = Mutex::new(qs(vec![due_at("a", "s")]));
            let prepare = |_: &QueueItem, _: &dyn Fn() -> bool| {
                let mut q = qm.lock().unwrap();
                match action {
                    "pause" => q.items[0].paused = true,
                    "edit" => {
                        q.items[0].text = "changed".into();
                        q.items[0].revision += 1;
                    }
                    "delete" => clear_session_items(&mut q, "s"),
                    _ => unreachable!(),
                }
                probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
            };
            let result = send_safe_test(
                &qm,
                &|_: &QueueItem| panic!("stale worker must not send"),
                &prepare,
                &|_: &QueueItem| panic!("stale worker has no final probe"),
            );
            assert_eq!(result, SendResult::Nothing, "{action}");
            assert!(qm
                .lock()
                .unwrap()
                .items
                .first()
                .is_none_or(|i| i.attempts == 0));
        }
    }

    #[test]
    fn delete_during_probe_reaps_a_session_the_worker_may_have_started() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let qm = Mutex::new(qs(vec![due_at("a", "s")]));
        let kills = AtomicU32::new(0);
        let kill = |session: &str| {
            assert_eq!(session, "s");
            kills.fetch_add(1, Ordering::SeqCst);
        };
        let result = send_one_safe(
            &qm,
            &AtomicBool::new(false),
            "s",
            720,
            &HashMap::new(),
            &SendHooks {
                fire: &|_: &QueueItem| panic!("deleted prompt must not send"),
                persist: &ok_persist,
                kill: &kill,
            },
            &ContextHooks {
                prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                    clear_session_items(&mut qm.lock().unwrap(), "s");
                    probe_result(
                        ContextStatus::Unavailable,
                        ContextCode::CancelledOrRevised,
                        1,
                    )
                },
                final_probe: &|_: &QueueItem| panic!("deleted prompt has no final probe"),
            },
        );
        assert_eq!(result, SendResult::Nothing);
        assert_eq!(kills.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn startup_poll_succeeds_times_out_and_detects_replacement_deterministically() {
        let mut calls = 0;
        let success = poll_readiness(
            3,
            &|| false,
            &mut |_| {
                calls += 1;
                if calls == 3 {
                    probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
                } else {
                    probe_result(
                        ContextStatus::ForegroundDifferent,
                        ContextCode::ForegroundDifferent,
                        1,
                    )
                }
            },
            &mut || {},
        );
        assert!(success.is_ready());
        assert_eq!(calls, 3);

        let timeout = poll_readiness(
            2,
            &|| false,
            &mut |_| {
                probe_result(
                    ContextStatus::ForegroundDifferent,
                    ContextCode::ForegroundDifferent,
                    1,
                )
            },
            &mut || {},
        );
        assert_eq!(timeout.status, ContextStatus::ForegroundDifferent);
        assert_eq!(timeout.code, ContextCode::StartupTimeout);

        let mut calls = 0;
        let replaced = poll_readiness(
            4,
            &|| false,
            &mut |bound| {
                calls += 1;
                if bound.is_some() {
                    probe_result(
                        ContextStatus::SessionReplaced,
                        ContextCode::IdentityChanged,
                        2,
                    )
                } else {
                    probe_result(
                        ContextStatus::ForegroundDifferent,
                        ContextCode::ForegroundDifferent,
                        1,
                    )
                }
            },
            &mut || {},
        );
        assert_eq!(replaced.status, ContextStatus::SessionReplaced);
        assert_eq!(calls, 2);
    }

    #[test]
    fn one_sessions_context_wait_does_not_block_another_session() {
        use std::sync::Barrier;
        let qm = Mutex::new(qs(vec![due_at("a", "slow"), due_at("b", "fast")]));
        let entered = Barrier::new(2);
        let release = Barrier::new(2);
        std::thread::scope(|scope| {
            let qm_ref = &qm;
            let (entered_ref, release_ref) = (&entered, &release);
            scope.spawn(move || {
                let kill = |_: &str| {};
                let result = send_one_safe(
                    qm_ref,
                    &AtomicBool::new(false),
                    "slow",
                    720,
                    &HashMap::new(),
                    &SendHooks {
                        fire: &|_: &QueueItem| Ok(()),
                        persist: &ok_persist,
                        kill: &kill,
                    },
                    &ContextHooks {
                        prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                            entered_ref.wait();
                            release_ref.wait();
                            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
                        },
                        final_probe: &|_: &QueueItem| {
                            probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 1)
                        },
                    },
                );
                assert!(matches!(result, SendResult::Sent { .. }));
            });
            entered.wait();
            let kill = |_: &str| {};
            let fast = send_one_safe(
                &qm,
                &AtomicBool::new(false),
                "fast",
                720,
                &HashMap::new(),
                &SendHooks {
                    fire: &|_: &QueueItem| Ok(()),
                    persist: &ok_persist,
                    kill: &kill,
                },
                &ContextHooks {
                    prepare: &|_: &QueueItem, _: &dyn Fn() -> bool| {
                        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
                    },
                    final_probe: &|_: &QueueItem| {
                        probe_result(ContextStatus::Ready, ContextCode::ProcessMatched, 2)
                    },
                },
            );
            assert!(matches!(fast, SendResult::Sent { .. }));
            assert!(!qm.lock().unwrap().items.iter().any(|i| i.session == "fast"));
            release.wait();
        });
        assert_eq!(qm.lock().unwrap().deliveries.len(), 2);
    }

    #[test]
    fn legacy_policy_variants_are_ignored_and_cleaned() {
        for policy in ["agent-ready", "foreground-match", "force-generic"] {
            let mut value = serde_json::to_value(qs(vec![due_at("a", "s")])).unwrap();
            let item = value["items"][0].as_object_mut().unwrap();
            item.remove("expected_process");
            item.insert("safety_policy".into(), serde_json::json!(policy));
            item.insert("expected_agent".into(), serde_json::json!("codex"));
            item.insert(
                "last_context".into(),
                serde_json::json!({"status":"working","code":"hook-working","checked_at":1}),
            );
            item.insert("cmd".into(), serde_json::json!("codex --full-auto"));
            let mut loaded: QueueState = serde_json::from_value(value).unwrap();
            migrate_context(&mut loaded);
            let item = &loaded.items[0];
            assert_eq!(item.expected_process.as_deref(), Some("codex"), "{policy}");
            assert!(item.last_context.is_none(), "{policy}");
            let saved = serde_json::to_string(&loaded).unwrap();
            assert!(!saved.contains("safety_policy"), "{policy}");
            assert!(!saved.contains("expected_agent"), "{policy}");
            assert!(!saved.contains("hook-working"), "{policy}");
        }
    }

    #[test]
    fn manual_now_bypasses_only_time_not_ordering_or_gap() {
        let mut future = due_at("future", "s");
        future.at = Some(u64::MAX);
        let mut q = qs(vec![future]);
        assert!(select_for_session(&q, "s", NOW, 720, &HashMap::new()).is_none());
        assert_eq!(
            select_requested(&q, "s", "future", NOW).unwrap().id,
            "future"
        );
        let mut tail = qi("tail", "chain");
        tail.group = Some("g".into());
        tail.seq = Some(2);
        let mut head = qi("head", "chain");
        head.group = Some("g".into());
        head.seq = Some(1);
        q.items.extend([head, tail]);
        assert!(select_requested(&q, "s", "tail", NOW).is_none());
        q.last_fired.insert("s".into(), NOW - 1);
        assert!(select_requested(&q, "s", "future", NOW).is_none());
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
            card_id: format!("card-{session}"),
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
        // (automatic re-sending could duplicate it), and the write is retried
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
    fn crash_before_dirty_flush_recovers_old_firing_disk_as_ambiguous() {
        // Exact crash window from the regression: intent save succeeds, the
        // tmux injection explicitly refuses, the post-failure save fails,
        // then the process disappears WITHOUT flush_dirty.
        let initial = qs(vec![due_at("a", "s")]);
        let qm = Mutex::new(initial.clone());
        let disk = FakeDisk::new(&initial);
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
                Err("tmux send-keys refused".into())
            },
            &persist,
        );
        assert!(matches!(res, SendResult::Failed { .. }));
        assert!(dirty.load(AtomicOrdering::Relaxed));

        let mut restarted: QueueState = serde_json::from_str(&disk.on_disk()).unwrap();
        assert_eq!(restarted.items[0].state, "firing", "old disk is the intent");
        recover_interrupted(&mut restarted);
        assert_eq!(restarted.items[0].state, "ambiguous");
        assert!(restarted.deliveries.is_empty());
        assert!(select_due(&restarted, NOW + 100_000, 720, &HashMap::new()).is_empty());

        retry_item(&mut restarted, "a").unwrap();
        retry_item(&mut restarted, "a").unwrap();
        assert_eq!(restarted.items[0].state, "pending");
        assert!(restarted.pending.is_empty());
    }

    #[test]
    fn ambiguous_acknowledgement_accounts_once_for_once_rule_and_template_chain() {
        let mut once = due_at("once", "once-session");
        once.state = "firing".into();
        once.delivery = Some("do".into());
        once.last_attempt_at = Some(NOW);

        let mut recurring = rule(300);
        recurring.id = "rule".into();
        recurring.session = "rule-session".into();
        recurring.until_n = Some(2);
        recurring.fired = 1;
        recurring.steps = vec!["step two".into(), "step three".into()];
        recurring.state = "firing".into();
        recurring.delivery = Some("dr".into());
        recurring.last_attempt_at = Some(NOW);

        let mut head = due_at("head", "chain-session");
        head.group = Some("g".into());
        head.seq = Some(1);
        head.state = "firing".into();
        head.delivery = Some("dh".into());
        head.last_attempt_at = Some(NOW);
        let mut tail = qi("tail", "chain");
        tail.session = "chain-session".into();
        tail.group = Some("g".into());
        tail.seq = Some(2);

        let mut q = qs(vec![once, recurring, head, tail]);
        recover_interrupted(&mut q);
        for id in ["once", "rule", "head"] {
            acknowledge_ambiguous(&mut q, id).unwrap();
            acknowledge_ambiguous(&mut q, id).unwrap();
        }
        assert!(!q
            .items
            .iter()
            .any(|i| i.id == "once" || i.id == "rule" || i.id == "head"));
        assert!(q.items.iter().any(|i| i.id == "tail"));
        assert_eq!(
            q.items
                .iter()
                .filter(|i| i.rule.as_deref() == Some("rule"))
                .count(),
            2
        );
        assert_eq!(q.deliveries.len(), 3);
        assert!(q.deliveries.iter().all(|d| d.assumed));
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
