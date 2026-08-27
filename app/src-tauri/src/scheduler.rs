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
}

pub(crate) fn default_state() -> String {
    "pending".into()
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct QueueState {
    items: Vec<QueueItem>,
    /// session → when we last injected a prompt
    last_fired: HashMap<String, u64>,
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

/// Whether an "every" rule is due to fire, pure for testability.
pub(crate) fn every_due(i: &QueueItem, now: u64, now_min: u32, session_last: Option<u64>) -> bool {
    i.mode == "every"
        && !i.paused
        && i.until_at.map(|t| now < t).unwrap_or(true)
        && in_window(now_min, i.win_from, i.win_to)
        && i.last
            .map(|l| now >= l + i.every.unwrap_or(u64::MAX))
            .unwrap_or(true)
        && session_last
            .map(|t| now >= t + CHAIN_MIN_GAP_SECS)
            .unwrap_or(true)
}

pub(crate) const CHAIN_QUIET_SECS: u64 = 180;
pub(crate) const CHAIN_MIN_GAP_SECS: u64 = 60;

pub(crate) fn queue_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("queue.json")
}

pub(crate) fn load_queue() -> QueueState {
    match storage::load(&queue_path()) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            storage::warn(format!(
                "queue.json parsed but has an unexpected shape ({e}); starting with an empty queue —                  the original file is preserved as .bak"
            ));
            QueueState::default()
        }),
        Ok(None) => QueueState::default(),
        Err(e) => {
            storage::warn(format!("scheduled prompts could not be loaded: {e}"));
            QueueState::default()
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
    });
    save_queue(&q)?;
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

#[derive(Clone, Serialize)]
pub(crate) struct QueueFired {
    session: String,
    text: String,
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
/// error) but never blocks the rest of the session's chain.
pub(crate) fn item_dead(i: &QueueItem) -> bool {
    i.state == "failed" && i.attempts >= MAX_ATTEMPTS
}

pub(crate) fn retry_ok(i: &QueueItem, now: u64) -> bool {
    i.last_attempt_at
        .map(|t| now >= t + backoff_secs(i.attempts))
        .unwrap_or(true)
}

/// Pure per-tick candidate selection (unit-tested; the thread only adds IO).
pub(crate) fn select_due(
    q: &QueueState,
    now: u64,
    now_min: u32,
    activity: &HashMap<String, u64>,
) -> Vec<QueueItem> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for i in &q.items {
        if item_dead(i) || i.state == "firing" {
            continue; // dead items don't occupy the head slot either
        }
        if i.mode != "every" && !seen.insert(i.session.clone()) {
            continue; // only the head once-item per session is a candidate
        }
        if !retry_ok(i, now) {
            continue;
        }
        let due = match i.mode.as_str() {
            "at" => i.at.map(|t| now >= t).unwrap_or(false),
            "chain" => {
                let gap_ok = q
                    .last_fired
                    .get(&i.session)
                    .map(|t| now >= t + CHAIN_MIN_GAP_SECS)
                    .unwrap_or(true);
                let quiet_ok = activity
                    .get(&i.session)
                    .map(|a| now >= a + CHAIN_QUIET_SECS)
                    .unwrap_or(true); // dead session = quiet; fire_item restarts it
                gap_ok && quiet_ok
            }
            "every" => every_due(i, now, now_min, q.last_fired.get(&i.session).copied()),
            _ => false,
        };
        if due {
            out.push(i.clone());
        }
    }
    out
}

/// Expired rules die quietly (their stop instant passed while sleeping).
pub(crate) fn purge_expired(q: &mut QueueState, now: u64) -> bool {
    let n0 = q.items.len();
    q.items
        .retain(|i| !(i.mode == "every" && i.until_at.map(|t| now >= t).unwrap_or(false)));
    q.items.len() != n0
}

/// Success bookkeeping: once-items are consumed; a rule counts the fire,
/// re-enqueues its template steps 2..N as chain items, and retires itself
/// when its stop-after count is reached.
pub(crate) fn note_fired(q: &mut QueueState, item: &QueueItem, now: u64) {
    if item.mode == "every" {
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
            });
        }
        let mut done = false;
        if let Some(it) = q.items.iter_mut().find(|i| i.id == item.id) {
            it.fired += 1;
            it.last = Some(now);
            it.state = default_state();
            it.last_error = None;
            done = it.until_n.map(|n| it.fired >= n).unwrap_or(false);
        }
        if done {
            q.items.retain(|i| i.id != item.id);
        }
    } else {
        q.items.retain(|i| i.id != item.id);
    }
    q.last_fired.insert(item.session.clone(), now);
}

pub(crate) fn note_failed(q: &mut QueueState, id: &str, err: &str) {
    if let Some(it) = q.items.iter_mut().find(|i| i.id == id) {
        it.state = "failed".into();
        it.last_error = Some(err.chars().take(200).collect());
    }
}

/// At-most-once crash recovery. "firing" is persisted BEFORE injection, so
/// after a crash such an item may or may not have reached the session. We
/// assume it did — deck never risks sending a prompt twice: once-items are
/// dropped (with a user-visible notice), rules just count the attempt.
pub(crate) fn recover_interrupted(q: &mut QueueState) -> Vec<String> {
    let mut notes = Vec::new();
    let mut drop_ids = Vec::new();
    for it in q.items.iter_mut() {
        if it.state != "firing" {
            continue;
        }
        if it.mode == "every" {
            it.state = default_state();
            it.last = Some(it.last_attempt_at.unwrap_or_else(now_epoch));
            notes.push(format!(
                "a recurring prompt for {} was interrupted mid-send last run; treated as sent (deck delivers at-most-once)",
                it.session
            ));
        } else {
            drop_ids.push(it.id.clone());
            notes.push(format!(
                "a scheduled prompt for {} was interrupted mid-send last run; removed to avoid double-sending (deck delivers at-most-once)",
                it.session
            ));
        }
    }
    q.items.retain(|i| !drop_ids.contains(&i.id));
    notes
}

pub(crate) fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let state = app.state::<Queues>();
        let due: Vec<QueueItem> = {
            let q = state.0.lock().unwrap();
            if q.items.is_empty() {
                continue;
            }
            // pane activity for chain-mode quiet checks
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
            select_due(&q, now_epoch(), local_minutes(), &activity)
        };
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
        for item in due {
            // Persist the firing intent BEFORE injecting — this ordering is
            // what makes delivery at-most-once across crashes.
            {
                let mut q = state.0.lock().unwrap();
                let Some(it) = q.items.iter_mut().find(|i| i.id == item.id) else {
                    continue;
                };
                it.state = "firing".into();
                it.attempts += 1;
                it.last_attempt_at = Some(now_epoch());
                if let Err(e) = save_queue(&q) {
                    applog(&format!(
                        "[queue] persist (pre-fire) FAILED: {e} — not sending this tick"
                    ));
                    if let Some(it) = q.items.iter_mut().find(|i| i.id == item.id) {
                        it.state = default_state();
                        it.attempts -= 1;
                    }
                    continue;
                }
            }
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
                    note_fired(&mut q, &item, now_epoch());
                    if let Err(e) = save_queue(&q) {
                        applog(&format!("[queue] persist (post-fire) FAILED: {e}"));
                    }
                    drop(q);
                    let _ = app.emit(
                        "queue-fired",
                        QueueFired {
                            session: item.session.clone(),
                            text: item.text.clone(),
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
                        item.attempts + 1,
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
        }
    }

    fn rule(every: u64) -> QueueItem {
        let mut i = qi("t", "every");
        i.every = Some(every);
        i
    }

    fn qs(items: Vec<QueueItem>) -> QueueState {
        QueueState {
            items,
            last_fired: HashMap::new(),
        }
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
        let due = select_due(&q, NOW, 720, &HashMap::new());
        assert_eq!(due.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn chain_respects_order_quiet_and_gap() {
        let c1 = qi("c1", "chain");
        let c2 = qi("c2", "chain");
        let mut q = qs(vec![c1, c2]);
        // quiet session, no prior fire → only the HEAD chain item fires
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        let due = select_due(&q, NOW, 720, &quiet);
        assert_eq!(
            due.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["c1"]
        );
        // recent activity → nothing
        let busy: HashMap<String, u64> = [("s".into(), NOW - 10)].into();
        assert!(select_due(&q, NOW, 720, &busy).is_empty());
        // fired 10s ago → min-gap blocks even a quiet session
        q.last_fired.insert("s".into(), NOW - 10);
        assert!(select_due(&q, NOW, 720, &quiet).is_empty());
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
    fn failed_item_backs_off_then_dies_without_blocking_chain() {
        let mut c1 = qi("c1", "chain");
        c1.state = "failed".into();
        c1.attempts = 1;
        c1.last_attempt_at = Some(NOW - 10);
        let c2 = qi("c2", "chain");
        let mut q = qs(vec![c1, c2]);
        let quiet: HashMap<String, u64> = [("s".into(), NOW - 400)].into();
        // 10s after 1st failure: backoff (20s) holds it, and it still owns the head
        assert!(select_due(&q, NOW, 720, &quiet).is_empty());
        // backoff elapsed → retried
        q.items[0].last_attempt_at = Some(NOW - 30);
        assert_eq!(select_due(&q, NOW, 720, &quiet)[0].id, "c1");
        // attempts exhausted → dead: never selected, and c2 takes the head
        q.items[0].attempts = MAX_ATTEMPTS;
        assert_eq!(select_due(&q, NOW, 720, &quiet)[0].id, "c2");
        assert_eq!(backoff_secs(1), 20);
        assert_eq!(backoff_secs(20), 1800, "backoff is capped");
    }

    #[test]
    fn crash_recovery_is_at_most_once() {
        let mut once = qi("o", "at");
        once.state = "firing".into();
        let mut r = rule(300);
        r.state = "firing".into();
        r.last_attempt_at = Some(NOW - 5);
        let mut q = qs(vec![once, r]);
        let notes = recover_interrupted(&mut q);
        assert_eq!(notes.len(), 2);
        // the once-item is gone (assumed sent), the rule survives with the
        // attempt counted as a fire instant
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].mode, "every");
        assert_eq!(q.items[0].state, "pending");
        assert_eq!(q.items[0].last, Some(NOW - 5));
    }

    #[test]
    fn queue_ids_never_collide() {
        let existing = vec![qi("q1-0", "at")];
        let a = next_queue_id(&existing);
        let b = next_queue_id(&existing);
        assert_ne!(a, b);
        assert!(!existing.iter().any(|i| i.id == a || i.id == b));
    }

    #[test]
    fn template_steps_reenqueue_on_rule_fire() {
        let mut r = rule(300);
        r.steps = vec!["s2".into(), "s3".into()];
        r.tpl = Some("tp".into());
        r.tpl_idx = Some(1);
        r.tpl_total = Some(3);
        r.until_n = Some(1);
        let item = r.clone();
        let mut q = qs(vec![r]);
        note_fired(&mut q, &item, NOW);
        // rule retired (until_n=1), steps 2..3 queued as chain items in order
        let modes: Vec<_> = q
            .items
            .iter()
            .map(|i| (i.mode.as_str(), i.text.as_str()))
            .collect();
        assert_eq!(modes, [("chain", "s2"), ("chain", "s3")]);
        assert_eq!(q.items[0].tpl_idx, Some(2));
        assert_eq!(q.items[1].tpl_idx, Some(3));
        assert_eq!(q.last_fired.get("s"), Some(&NOW));
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
        assert!(every_due(&rule(1800), now, 720, None));
        // fired 10 min ago on a 30-min cadence → not due; after 30 min → due
        let mut r = rule(1800);
        r.last = Some(now - 600);
        assert!(!every_due(&r, now, 720, None));
        r.last = Some(now - 1800);
        assert!(every_due(&r, now, 720, None));
        // paused wins over everything
        r.paused = true;
        assert!(!every_due(&r, now, 720, None));
        r.paused = false;
        // outside the 08:00–18:00 window (22:00) → sleeping
        r.win_from = Some(480);
        r.win_to = Some(1080);
        assert!(!every_due(&r, now, 22 * 60, None));
        assert!(every_due(&r, now, 9 * 60, None));
        // stop instant passed → never due again
        r.until_at = Some(now - 1);
        assert!(!every_due(&r, now, 9 * 60, None));
        r.until_at = None;
        // a prompt was injected into the session 10s ago → min-gap holds it back
        assert!(!every_due(&r, now, 9 * 60, Some(now - 10)));
        assert!(every_due(&r, now, 9 * 60, Some(now - 61)));
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
