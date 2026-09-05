//! Scheduled prompts: at / chain (quiet-based) / every (recurring rules
//! with daily windows) + per-project templates. A persisted firing intent is
//! deliberately treated as ambiguous after a crash: deck neither claims it
//! succeeded nor sends it again until the user resolves it. Tick logic is
//! pure and unit-tested; the thread only adds IO.
//!
//! # Contract
//! Scheduled prompts: Rust-side scheduler thread (NOT webview timers — App Nap
//! freezes those), 20s tick that sleeps on a condition so `wake_scheduler()`
//! (called when an inbound card's prompts are queued) starts a scan at once,
//! queue persisted at `~/.deck/queue.json` and loaded at boot. Every item persists its card id, optional full tmux
//! server/session/window/pane/pid binding, optional sanitized executable
//! basename, revision and a closed last-context result (never terminal text,
//! arguments or paths). Context protection is automatic and has no saved/UI/API
//! policy: an executable derived from the explicit card launch command is
//! required in the foreground, otherwise a live non-shell foreground is
//! captured at creation, otherwise same-pane compatibility delivery is
//! allowed. Hooks, agent class, output activity and
//! quiet time never gate delivery. Legacy policy/AgentClass/hook fields are
//! ignored and cleaned on the next save without changing schedule/delivery
//! state. `context.rs` owns metadata-only probing and sanitization.
//! Injection loads the literal text into a uniquely named tmux buffer, then
//! one synchronous tmux command queue compares the full generation plus
//! optional foreground executable and byte-literal-pastes only on a match
//! (`paste-buffer -p`: bracketed only for an application that asked). Enter
//! is sent 300ms later as a SEPARATE key under the same condition — a CR
//! inside the paste burst is a pasted newline to agent inputs (Claude Code,
//! Codex) and the prompt sat unsent in the box; a refused Enter is logged and
//! the delivery still counts, because the bytes are visibly in the pane. The
//! foreground check matches tmux's `pane_current_command` OR the argv name of
//! the tty's foreground process group (`ps … stat=+`): a launcher symlink to a
//! versioned binary (Claude Code's `claude → versions/2.1.259`) reports the
//! version to tmux and the command to ps, and the atomic paste pins the tmux
//! name it observed for the recognized process. The persisted binding
//! is a GENERATION STAMP that must hold within ONE delivery, never a permanent
//! target: `current_context_probe` re-observes the card's own pane (deck's own
//! name on deck's own socket, and a deleted card tombstones its items), the
//! readiness probe persists whatever generation it finds, and startup polling,
//! the final probe and the atomic paste then require THAT generation — numeric
//! tmux ids alone are insufficient there because a restarted server reuses
//! them, so server pid is part of the stamp. What decides whether the target is
//! the right one is `expected_process`, not the stamp. Never restore a hard
//! block on a changed generation: every production upgrade replaces the tmux
//! server, so it fired on every item after every update, stalled whole chain
//! groups behind their head, rewrote queue.json on each tick and taught the
//! user to click a rebind button that only ever confirmed the pane deck had
//! already picked. Manual immediate delivery may pointer-confirm a one-shot
//! process mismatch bypass; the process comparison is the only thing it can
//! bypass. "chain" mode fires after
//! `window_activity` has been quiet ≥180s (a permission prompt also counts as
//! quiet — documented behavior; quiet NEVER means "the agent finished").
//! Round-2/3 semantics (`scheduler/` is the reference, all unit-tested):
//! - at most ONE candidate per session per tick, ≥60s between any two
//!   injections into the same session; each due session gets its own
//!   short-lived worker thread claimed via a busy-set, so sessions are truly
//!   independent (a startup wait delays only its own session), the
//!   same session never has two concurrent sends, and a worker outliving its
//!   tick can't collide with the next tick;
//! - deterministic priority: backoff-elapsed retry → earliest-due `at` →
//!   cadence-due `every` → chain; a future `at` never blocks a due one;
//! - each worker re-selects from fresh state under the lock (`send_one` is
//!   the delivery state machine and `send_one_safe` is its context-safe front
//!   half, both testable with fake probe/fire/persist);
//!   chain steps carry explicit `group`/`seq` (legacy files migrate from
//!   array adjacency);
//! - the firing contract: while an item is mid-send, queue remove/update/
//!   pause/retry/skip return a conflict error (UI toasts it) and the item
//!   survives until finalize;
//! - EVERY user-driven mutation goes through `with_queue` (persist-then-
//!   commit): clone the state, mutate the CANDIDATE, persist, only then swap
//!   it in — a rejected mutation or a failed save leaves memory byte-identical
//!   to disk, so the scheduler never acts on a change the user was told
//!   failed. The two POST-send transitions are deliberately the opposite: the
//!   injection cannot be rolled back, so memory takes the new state and a
//!   failed write sets `Queues.dirty`, warns the user, and is retried by
//!   `flush_dirty` every tick — that retry is what stops a definitively
//!   NOT-sent prompt from being counted as delivered after a restart;
//! - deleting a card/project is PERMANENT cancellation: `queue_clear_session(s)`
//!   tombstones the session (`cancelled`, capped 500) and drops ALL its items
//!   INCLUDING one mid-send — that delivery still finalizes from the
//!   pending-ledger snapshot (the audit completes), but no rule is restored,
//!   no template step spawned, no cadence and no send-gap entry left behind,
//!   and a tombstoned session is never eligible again (so `fire_item` cannot
//!   restart it). A delete landing while a worker is inside its injection is
//!   reaped afterwards (`SendHooks.kill`); scheduling for a session again
//!   clears its tombstone. The frontend removes a card ONLY after that
//!   cancellation is on disk — close, project delete (one atomic
//!   `queue_clear_sessions`) and the shell-exited auto-retire share the path,
//!   and a failure keeps the card with an explicit toast. The frontend then
//!   kills every tmux session (already-missing is success) and persists the
//!   candidate Board BEFORE committing removal; any kill/save failure keeps
//!   the card/project and pane visible for retry;
//! - a step that exhausts its 8 attempts BLOCKS its group until the user
//!   retries/skips/removes it (queue_retry / queue_skip commands);
//! - a recurring rule has at most one active iteration (its spawned steps,
//!   keyed `rule`/`group`=delivery id) — iterations never interleave;
//! - firing intent + delivery id + a full item snapshot (the `pending` ledger)
//!   is persisted only AFTER readiness, binding persistence, fresh re-selection
//!   and a final probe, and still BEFORE injection. Blocked context never
//!   increments attempts, creates a ledger or becomes ambiguous. A live confirmed success uses idempotent
//!   `finalize_delivery` (fired count, until-N retirement, template-step spawn,
//!   audit record — `deliveries`, capped 200). A persisted `firing` found after
//!   a crash becomes `ambiguous`: it is never auto-retried or silently counted.
//!   User acknowledge finalizes it once; risk-accepting retry clears the ledger
//!   and re-arms it, both persist-then-commit and idempotent. A definitively
//!   refused atomic injection becomes retryable `failed`; if that post-failure
//!   save and the process both fail, the old disk intent is honestly ambiguous.
//!   Boot recovery installs every persisted `firing`/orphan ledger entry as
//!   in-memory `ambiguous` BEFORE attempting the repair write. If that write
//!   fails, the ambiguous actions remain available and unschedulable while
//!   `dirty` retries the exact snapshot. `flush_dirty` runs before the
//!   empty-queue fast path.

mod delivery;
mod ops;
mod select;
#[cfg(test)]
mod tests;
mod thread;

pub(crate) use delivery::*;
pub(crate) use ops::*;
pub(crate) use select::*;
pub(crate) use thread::*;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Mutex;

use crate::context::{self, ContextCheck, ContextCode, ContextStatus, PaneIdentity};
use crate::storage;
use crate::storage::applog;

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
    crate::procinfo::local_minutes()
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
