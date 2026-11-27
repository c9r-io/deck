//! Worker cards: the cards a delegated client started through its Agent
//! automations or `deck_worker_start`, which the user also let it drive. The
//! tools it uses (start, list, wait, reply, send, close), the ledger records that say which cards are
//! its own and which ones the user took over, and the facts the scheduler,
//! the reply store and the webview read.
//!
//! The contract lives in `mcp/mod.rs` ("Worker cards"). In short:
//! - A worker card is recorded when an Agent trigger of a client with
//!   `allow_workers` commits its card, or when the webview commits a
//!   `deck_worker_start` Board intent (`record_worker`); nothing else makes
//!   one, and a client sees only its own.
//! - Worker types (`WorkerType`): the user's list, per client, of the kinds
//!   of worker card it may start — a name, the user's own command (`claude`,
//!   `codex --no-daemon`, while Pi's work status is on `pi`, or, while
//!   Deck's agy hooks entry is on, `agy`, with simple arguments,
//!   `validate_worker_types`; a Pi worker's lines are handed to Pi by Deck's
//!   observer, never typed: `agent_pi::handover`; an agy worker's are typed
//!   under agy's first-interaction gate: `agent_agy`; a Claude command may
//!   begin with one literal `CLAUDE_CONFIG_DIR=<absolute folder>`, a second
//!   account, approved 2026-11-25: `admission::config_root`,
//!   `config_root_problem`, carried again by re-entry and crash recovery)
//!   and a description the orchestrator reads, optionally with the models
//!   a start may choose (`models`: single simple arguments, at most
//!   `MAX_TYPE_MODELS`, only on a command that names no model itself).
//!   `deck_worker_start` names a type (never a command) and optionally one
//!   of its listed models (byte for byte, else `MODEL_NOT_LISTED`; the card's
//!   command is the type's plus `--model <the listed entry>`,
//!   `start_command`), its required absolute directory, which alone decides
//!   the project (`start_project`: the one of the client's scope whose
//!   directory holds it; none `PATH_NOT_ACCESSIBLE`, several
//!   `PATH_AMBIGUOUS` until `project_id` names one) and, only under the
//!   organize-cards choice (`Choice::CardEdits`, the one delegation that
//!   places a card), one of that project's columns (`named_column`: a wrong
//!   one is `COLUMN_NOT_FOUND`, never a fallback). Unnamed, the card opens
//!   beside its orchestrator card (`origin::beside`), else in the project's
//!   new-card column (`mcp.js`, pure.js `newSessionColumn`). It journals a
//!   `worker-start` intent with the first
//!   line; the webview creates and starts the card (`mcp.js`), and the
//!   commit queues that line as the run's head row (`settle_start`), which
//!   then leaves the ledger. Its first-send override rests on the client's
//!   `worker_first_send` choice alone (`worker_first_send_key`,
//!   `scheduler/first_send.rs`); with that choice every later line carries
//!   it too, which matters only once the card's agent lost its interaction
//!   evidence (a Deck restart or an Agent re-entry: `agent` then reads
//!   `unavailable`, episode 0, until the agent's next turn).
//! - Reading is content-free (`deck_worker_list`, `deck_worker_wait`: the
//!   card, whether its session is alive, the Agent observation's closed word
//!   and episode, the human hold, its lines' queue states) except
//!   `deck_worker_reply`, which needs the separate `allow_replies` and
//!   returns the Claude Code final message the Stop hook forwarded.
//! - `deck_worker_send` queues one line on the card through the scheduler's
//!   external path (`scheduler::worker_queue_add`), its authority the
//!   client's delegation (`ContentClass::Delegated`). It refuses, never
//!   holds, when the card's episode is not the one the client named
//!   (`EPISODE_MOVED`): Signal may refuse a send, never release one. Every
//!   scheduler hold still decides when it goes out.
//! - `deck_worker_interject` is the same line without the chain's quiet
//!   time (`queue_line` with `Line::Interject`): only from the orchestrator
//!   card that started the card (`opened_by`, never `own_card`'s wider
//!   reach), only into an agent that takes a message while it works
//!   (`interject_agent`), plain text only (`plain_text`); its operation
//!   carries `mode` and the orchestrator card, and the row it queues the
//!   binding the scheduler checks again (`interject_bindings`). The views
//!   say each line's `mode` and `waiting` stage and each card's
//!   `mayInterject`.
//! - The user takes a card over by typing into its pane and hands it back
//!   only by hand, both through `worker_set_held` from the webview.
//!   While held, the card's delegated lines hold at `human`.
//! - `deck_worker_close` closes one worker card for the orchestrator card
//!   that started it, and only for it (`opened_by`: the kernel-proven
//!   calling card is the one recorded at the start; from no card, nothing).
//!   It is refused while the user holds the card, while its Agent asks the
//!   user (`needs-input`) and once its episode moved on, and goes as a
//!   `worker-close` Board intent through the webview's ordinary close as a
//!   remote one (`mcp.js` `closeWorker`); the admission reads the card again
//!   (`close_still_admissible`). Its reports stay readable.
//! - Within one client, a worker started from an orchestrator card
//!   (`origin.rs`) is that card's: only calls from it (and from no card) see
//!   and drive it, its sequences are that card's lane, and once the card
//!   left the Board the user holds it (`origin::taken_over`, read wherever
//!   `held` is) until handing it back makes it unowned.
//! - A worker card's agent may hand its result to Deck for its orchestrator
//!   to read (`deck_worker_report`, `reports.rs`): data at rest, never a
//!   send, a wake or authority.
//! - Withdrawing `allow_workers`, revoking the client or disabling MCP takes
//!   the client out of `automation_delegations` under the settings fence the
//!   scheduler's firing intent also takes: no delegated line starts after
//!   that returns, and the tick strips their authority.

use super::*;

pub(super) const WORKER_SEND_KIND: &str = "worker-send";
/// A worker card its orchestrator card asked to close: a Board intent the
/// webview's close commits (`mcp.js` `closeWorker`), admitted like a managed
/// session's close (`commands::close_admit`).
pub(super) const WORKER_CLOSE_KIND: &str = "worker-close";
/// The user's worker capacity (`Config::worker_capacity`, Settings ›
/// MCP, approved 2026-11-25): worker cards one orchestrator card (or the
/// client's callers from no card, together) may have on the Board at once;
/// a start beyond it is refused (`CAPACITY_EXCEEDED`). Every other worker
/// bound follows from it (`client_capacity`, `sends_per_minute`) or is
/// sized for its ceiling (`MAX_WORKER_CAPACITY`), so lowering it never
/// makes stored data invalid.
pub(crate) const DEFAULT_WORKER_CAPACITY: u32 = 16;
/// The highest capacity the user may choose: one client then has at most
/// `MAX_WORKER_CAPACITY * CLIENT_FACTOR` (128) worker cards, whose reports
/// (`reports::MAX_TOTAL_BYTES`) still fit the ledger (`MAX_STATE_BYTES`).
pub(crate) const MAX_WORKER_CAPACITY: u32 = 32;
/// One client's worker cards, all its orchestrator cards together: this
/// many orchestrators' worth.
const CLIENT_FACTOR: usize = 4;
/// The most worker cards any client may have, at the highest capacity.
pub(crate) const MAX_CLIENT_WORKERS_CEILING: usize = MAX_WORKER_CAPACITY as usize * CLIENT_FACTOR;

/// Worker cards one orchestrator card may have now.
pub(super) fn orchestrator_capacity(doc: &DiskDoc) -> usize {
    doc.config.worker_capacity as usize
}

/// Worker cards one client may have now, and so the cards one
/// `deck_worker_wait` may follow.
pub(super) fn client_capacity(doc: &DiskDoc) -> usize {
    orchestrator_capacity(doc) * CLIENT_FACTOR
}

/// Lines and closes one client may send a minute: one for each card it
/// may have, never fewer than 60.
fn sends_per_minute(doc: &DiskDoc) -> usize {
    client_capacity(doc).max(60)
}
/// Worker records kept in all (closed cards are pruned first).
const MAX_WORKER_RECORDS: usize = 256;
/// Unsent delegated lines one card may hold.
const MAX_UNSENT_PER_CARD: usize = 4;
/// The longest line a client may send.
pub(super) const MAX_SEND_BYTES: usize = 16 * 1024;
/// Recent send operations reported per card.
const SENDS_REPORTED: usize = 8;

/// One worker card: which client started it, through which automation, and
/// whether the user took it over.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Worker {
    pub(super) card_id: String,
    pub(super) client_id: String,
    pub(super) operation_id: String,
    pub(super) automation_id: String,
    pub(super) created_at: u64,
    #[serde(default)]
    pub(super) held: bool,
    /// The orchestrator card the client's call came from when it started
    /// this card (`origin.rs`); `None` for an unowned worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) orchestrator: Option<String>,
    /// The newest report sequence this card was given (`reports.rs`); it
    /// never goes back, whatever reports were dropped since.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(super) reported: u64,
    /// The model the start chose from its type's list (`worker_start`);
    /// `None` when it chose none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The key a client's worker delegation takes in `automation_delegations`
/// (the scheduler's one delegation source): the client id alone means its
/// automation delegation, this one means it may drive its worker cards.
pub(crate) fn worker_delegation_key(client: &str) -> String {
    format!("worker:{client}")
}

/// Whether the ledger holds anything a v7 reader would misread.
pub(super) fn uses_workers(doc: &DiskDoc) -> bool {
    !doc.workers.is_empty()
        || doc.config.clients.iter().any(|c| {
            c.allow_workers
                || c.allow_replies
                || c.send_sequence > 0
                || c.worker_first_send
                || !c.worker_types.is_empty()
        })
        || doc.operations.iter().any(|o| {
            matches!(
                o.kind.as_str(),
                WORKER_SEND_KIND | WORKER_START_KIND | WORKER_CLOSE_KIND
            )
        })
}

/// Whether the ledger holds a worker type's model list or a worker card
/// started with a chosen model: a v17 reader would drop them.
pub(super) fn uses_worker_models(doc: &DiskDoc) -> bool {
    doc.config
        .clients
        .iter()
        .any(|c| c.worker_types.iter().any(|t| !t.models.is_empty()))
        || doc.workers.iter().any(|w| w.model.is_some())
        || doc.operations.iter().any(|o| {
            o.kind == WORKER_START_KIND
                && o.result
                    .as_ref()
                    .is_some_and(|r| r.get("model").is_some_and(|m| !m.is_null()))
        })
}

/* ---------- change notification (for wait) ---------- */

fn change() -> &'static (Mutex<u64>, Condvar) {
    static CHANGE: OnceLock<(Mutex<u64>, Condvar)> = OnceLock::new();
    CHANGE.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

/// Something a waiting client may want to see happened: an Agent Signal
/// event was accepted, or a card was taken over or handed back.
pub(crate) fn note_worker_change() {
    let (count, changed) = change();
    *count.lock_or_recover() += 1;
    changed.notify_all();
}

fn change_count() -> u64 {
    *change().0.lock_or_recover()
}

/// Wait until the change count moved past `seen` or `deadline` passed.
fn wait_change(seen: u64, deadline: Instant) {
    let (count, changed) = change();
    let mut guard = count.lock_or_recover();
    while *guard == seen {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        guard = crate::sync::wait_timeout_or_recover(changed, guard, deadline - now);
    }
}

/* ---------- recording ---------- */

/// A committed Agent trigger made `card` for `operation`'s client: when the
/// client may drive its worker cards, the card becomes one of them. Called
/// inside the outcome write (`automation::outcome_in`).
pub(super) fn record_worker(doc: &mut DiskDoc, operation: &Operation, card: &str) {
    let allowed = doc
        .config
        .clients
        .iter()
        .any(|c| c.id == operation.client_id && c.revoked_at.is_none() && c.allow_workers);
    if !allowed || doc.workers.iter().any(|w| w.card_id == card) {
        return;
    }
    let automation = operation
        .result
        .as_ref()
        .and_then(|r| r.get("automationId"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if doc.workers.len() >= MAX_WORKER_RECORDS {
        if let Some(cards) = crate::documents::card_targets(false) {
            doc.workers.retain(|w| cards.contains_key(&w.card_id));
        }
        while doc.workers.len() >= MAX_WORKER_RECORDS {
            doc.workers.remove(0);
        }
        super::reports::prune(doc);
    }
    doc.workers.push(Worker {
        card_id: card.into(),
        client_id: operation.client_id.clone(),
        operation_id: operation.operation_id.clone(),
        automation_id: automation,
        created_at: now_ms(),
        held: false,
        orchestrator: super::origin::of_operation(operation),
        reported: 0,
        model: operation
            .result
            .as_ref()
            .and_then(|r| r.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    });
}

/// Whether `client` may start one more worker card from `orchestrator`:
/// at most `orchestrator_capacity` for that orchestrator card (or for its
/// callers from no card) and `client_capacity` in all, counting the
/// worker cards on the Board and the starts and handovers accepted that
/// have no card yet — each for the orchestrator card it came from.
pub(super) fn room_for_worker(
    doc: &DiskDoc,
    client: &str,
    orchestrator: Option<&str>,
) -> Result<(), Value> {
    let waiting: Vec<Option<String>> = doc
        .operations
        .iter()
        .filter(|o| {
            o.client_id == client
                && matches!(o.kind.as_str(), WORKER_START_KIND | TRIGGER_KIND)
                && matches!(o.state.as_str(), "accepted" | "executing")
        })
        .map(super::origin::of_operation)
        .collect();
    let pending_own = waiting
        .iter()
        .filter(|card| card.as_deref() == orchestrator)
        .count();
    let pending = waiting.len();
    let cards = crate::documents::card_targets(false);
    let live: Vec<&Worker> = doc
        .workers
        .iter()
        .filter(|w| w.client_id == client)
        .filter(|w| cards.as_ref().is_none_or(|c| c.contains_key(&w.card_id)))
        .collect();
    let own = live
        .iter()
        .filter(|w| w.orchestrator.as_deref() == orchestrator)
        .count();
    let full = |message: &str| {
        error_value(
            "CAPACITY_EXCEEDED",
            message,
            "Ask the local Deck user to close finished worker cards, then start again with a new request.",
        )
    };
    let (per_orchestrator, per_client) = (orchestrator_capacity(doc), client_capacity(doc));
    if own + pending_own >= per_orchestrator {
        return Err(full(&format!(
            "this orchestrator already has {per_orchestrator} worker cards on the Board"
        )));
    }
    if live.len() + pending >= per_client {
        return Err(full(&format!(
            "this client already has {per_client} worker cards on the Board"
        )));
    }
    Ok(())
}

/* ---------- facts other modules read ---------- */

fn with_doc<T>(default: T, read: impl FnOnce(&DiskDoc) -> T) -> T {
    let Some(runtime) = RUNTIME.get() else {
        return default;
    };
    if runtime.emergency.lock_or_recover().disabled {
        return default;
    }
    runtime
        .read(|doc| {
            if doc.config.enabled {
                Some(read(doc))
            } else {
                None
            }
        })
        .ok()
        .flatten()
        .unwrap_or(default)
}

/// The sessions of the worker cards the user took over (`Hold::Human`).
pub(crate) fn held_worker_sessions() -> HashSet<String> {
    let cards = crate::documents::card_targets(false);
    let held: Vec<String> = with_doc(Vec::new(), |doc| {
        doc.workers
            .iter()
            .filter(|w| super::origin::taken_over(w, cards.as_ref()))
            .map(|w| w.card_id.clone())
            .collect()
    });
    if held.is_empty() {
        return HashSet::new();
    }
    let Some(cards) = cards else {
        return HashSet::new();
    };
    held.iter()
        .filter_map(|card| cards.get(card).map(|t| t.session.clone()))
        .collect()
}

/// Whether the user took worker card `card` over (read at the pre-fire
/// re-selection, `scheduler/delivery.rs`). An unreadable ledger holds.
pub(crate) fn worker_card_held(card: &str) -> bool {
    let Some(runtime) = RUNTIME.get() else {
        return false;
    };
    let cards = crate::documents::card_targets(false);
    runtime
        .read(|doc| {
            doc.workers
                .iter()
                .any(|w| w.card_id == card && super::origin::taken_over(w, cards.as_ref()))
        })
        .unwrap_or(true)
}

/// The key one orchestrator-worker binding takes in `interject_bindings`
/// (ids are `valid_id`: the separators cannot occur in them).
pub(crate) fn interject_key(client: &str, card: &str, orchestrator: &str) -> String {
    format!("interject:{client}:{card}:{orchestrator}")
}

/// Every orchestrator-worker binding that stands now, for the scheduler's
/// sweep and pre-fire fence of interjected lines (`scheduler/authority.rs`
/// `binding_stands`): the card's client may still drive its worker cards
/// (MCP on, the client neither revoked nor fenced, its worker choice on),
/// the card's record still names that orchestrator card (a card handed back
/// to nobody names none), and both cards are on the current Board. `None`
/// when the ledger or the current Board cannot be read (no Board yet, or one
/// answered from its backup): no proof either way, so those lines hold.
pub(crate) fn interject_bindings() -> Option<HashSet<String>> {
    let Some(runtime) = RUNTIME.get() else {
        return Some(HashSet::new());
    };
    bindings_in(runtime, crate::documents::card_targets(true).as_ref())
}

/// `interject_bindings` for one runtime and one reading of the current
/// Board (`None`: no current Board).
pub(super) fn bindings_in(
    runtime: &Runtime,
    cards: Option<&HashMap<String, crate::documents::CardTarget>>,
) -> Option<HashSet<String>> {
    let emergency = runtime.emergency.lock_or_recover();
    if emergency.disabled {
        return Some(HashSet::new());
    }
    let fenced = emergency.clients.clone();
    drop(emergency);
    let cards = cards?;
    runtime
        .read(|doc| {
            if !doc.config.enabled {
                return HashSet::new();
            }
            let driving: HashSet<&str> = doc
                .config
                .clients
                .iter()
                .filter(|c| c.revoked_at.is_none() && !fenced.contains(&c.id) && c.allow_workers)
                .map(|c| c.id.as_str())
                .collect();
            doc.workers
                .iter()
                .filter(|w| driving.contains(w.client_id.as_str()))
                .filter_map(|w| {
                    let orchestrator = w.orchestrator.as_deref()?;
                    (cards.contains_key(orchestrator) && cards.contains_key(&w.card_id))
                        .then(|| interject_key(&w.client_id, &w.card_id, orchestrator))
                })
                .collect()
        })
        .ok()
}

/// The clients that may read worker replies now.
fn reply_readers(doc: &DiskDoc) -> HashSet<String> {
    doc.config
        .clients
        .iter()
        .filter(|c| c.revoked_at.is_none() && c.allow_replies)
        .map(|c| c.id.clone())
        .collect()
}

/// Whether some client may read worker replies now (the Stop hook then
/// forwards the final message, `agent_status::sync_worker_replies`).
pub(crate) fn replies_wanted_any() -> bool {
    with_doc(false, |doc| !reply_readers(doc).is_empty())
}

/// Whether `session` is a worker card whose client may read its replies:
/// only then is its final message kept (`turn_summary::offer`).
pub(crate) fn replies_wanted_for(session: &str) -> bool {
    let cards: Vec<String> = with_doc(Vec::new(), |doc| {
        let readers = reply_readers(doc);
        doc.workers
            .iter()
            .filter(|w| readers.contains(&w.client_id))
            .map(|w| w.card_id.clone())
            .collect()
    });
    if cards.is_empty() {
        return false;
    }
    crate::documents::card_targets(false).is_some_and(|targets| {
        cards
            .iter()
            .any(|card| targets.get(card).is_some_and(|t| t.session == session))
    })
}

/// For an Agent item about to become a card (`inbound_pending`): the name
/// of its client when that client may drive its worker cards, so the card
/// is made quiet and labelled. Presentation only.
pub(crate) fn worker_client_for_item(item_key: &str) -> Option<String> {
    let client = crate::inbound::pending_client(item_key)?;
    with_doc(None, |doc| {
        doc.config
            .clients
            .iter()
            .find(|c| c.id == client && c.revoked_at.is_none() && c.allow_workers)
            .map(|c| c.name.clone())
    })
}

/* ---------- observation ---------- */

/// One card's Agent observation as a client reads it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Seen {
    alive: bool,
    state: Option<&'static str>,
    episode: u64,
    /// The Codex Signal trust of the session's current foreground
    /// generation (`agent_status::CodexSignalTrust`), when one applies:
    /// what `why` says when no state is observed.
    codex: Option<crate::agent_status::CodexSignalTrust>,
}

/// One pane listing: for each session, whether it is alive and what its
/// Signal target reported for the current generation. `None` when tmux
/// could not be read.
pub(super) fn observe_sessions() -> Option<HashMap<String, Seen>> {
    #[cfg(test)]
    if let Some(forced) = OBSERVE_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced;
    }
    let rows = crate::tmux::list_panes().ok()?;
    Some(seen_in(&rows))
}

/// The same listing read twice, for the client's views: each session as the
/// client sees it, and the scheduler's own observation of it, which says at
/// which stage each of the client's unsent lines waits
/// (`scheduler::worker_rows`). One tmux call, as before. `None` when tmux
/// could not be read.
fn observe_for_views() -> Option<(HashMap<String, Seen>, crate::scheduler::Observations)> {
    #[cfg(test)]
    if let Some(forced) = OBSERVE_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced.map(|seen| (seen, crate::scheduler::Observations::new()));
    }
    let rows = crate::tmux::list_panes().ok()?;
    let seen = seen_in(&rows);
    Some((seen, crate::scheduler::observe(rows)))
}

fn seen_in(rows: &[crate::tmux::PaneRow]) -> HashMap<String, Seen> {
    let agents = crate::agent_status::projections(rows);
    let current = crate::agent_status::reporting(rows, crate::agent_status::live_generation);
    let evidence = crate::agent_status::generation_evidence(
        rows,
        crate::agent_status::live_generation,
        |_| None,
    );
    let mut seen: HashMap<String, Seen> = rows
        .iter()
        .map(|row| {
            (
                row.session_name.clone(),
                Seen {
                    alive: true,
                    state: None,
                    episode: 0,
                    codex: evidence.get(&row.session_name).and_then(|e| e.codex),
                },
            )
        })
        .collect();
    for (session, observation) in agents {
        if current.get(&session).copied().flatten() != Some(true) {
            continue;
        }
        if let Some(entry) = seen.get_mut(&session) {
            entry.state = Some(observation.state);
            entry.episode = observation.episode;
        }
    }
    seen
}

#[cfg(test)]
thread_local! {
    /// A test's pane listing in place of tmux: session → (state, episode).
    pub(super) static OBSERVE_FOR_TESTS: std::cell::RefCell<Option<Option<HashMap<String, Seen>>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(super) fn seen_for_tests(state: Option<&'static str>, episode: u64) -> Seen {
    Seen {
        alive: true,
        state,
        episode,
        codex: None,
    }
}

#[cfg(test)]
pub(super) fn codex_seen_for_tests(codex: crate::agent_status::CodexSignalTrust) -> Seen {
    Seen {
        alive: true,
        state: None,
        episode: 0,
        codex: Some(codex),
    }
}

/// Why a live worker has no observed state, for the orchestrator: a Codex
/// whose status Deck cannot attribute (it holds until that Codex exits, and
/// so do the lines queued into it), a Codex that has not proven its status
/// yet (its lines wait for its first own hook), or no turn seen yet.
fn unavailable_why(codex: Option<crate::agent_status::CodexSignalTrust>) -> &'static str {
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable, Unknown};
    match codex {
        Some(Unavailable) => "codex-unattributed",
        Some(Unknown) => "codex-unproven",
        Some(Trusted) | None => "no-turn-seen",
    }
}

/* ---------- views ---------- */

fn queues(runtime: &Runtime) -> Option<tauri::State<'_, crate::scheduler::Queues>> {
    use tauri::Manager;
    runtime
        .app
        .as_ref()
        .and_then(|app| app.try_state::<crate::scheduler::Queues>())
}

fn sends_of(doc: &DiskDoc, client: &str, card: &str) -> Vec<String> {
    let mut ids: Vec<String> = doc
        .operations
        .iter()
        .filter(|o| {
            o.client_id == client
                && matches!(o.kind.as_str(), WORKER_SEND_KIND | WORKER_START_KIND)
                && o.result
                    .as_ref()
                    .and_then(|r| r.get("cardId"))
                    .and_then(Value::as_str)
                    == Some(card)
        })
        .map(|o| o.operation_id.clone())
        .collect();
    let keep = ids.len().saturating_sub(SENDS_REPORTED);
    ids.drain(..keep);
    ids
}

struct WorkerRow {
    worker: Worker,
    taken_over: bool,
    target: Option<crate::documents::CardTarget>,
    sends: Vec<String>,
    /// Which of `sends` are interjected lines (`result.mode`).
    interjected: HashSet<String>,
    /// The calling card started this card and its agent takes interjected
    /// lines: `deck_worker_interject` would not be refused for who or what
    /// it is (the episode, rate and holds still decide).
    may_interject: bool,
    /// `reports::summary`: content-free.
    reports: Value,
}

fn rows_of(doc: &DiskDoc, client: &str, orchestrator: Option<&str>) -> Vec<WorkerRow> {
    let targets = crate::documents::card_targets(false).unwrap_or_default();
    doc.workers
        .iter()
        .filter(|w| w.client_id == client && super::origin::sees(w, orchestrator))
        .filter_map(|w| {
            let target = targets.get(&w.card_id).cloned();
            let cmd = target.as_ref()?.cmd.clone();
            let sends = sends_of(doc, client, &w.card_id);
            let interjected = doc
                .operations
                .iter()
                .filter(|o| sends.contains(&o.operation_id) && is_interjected(o))
                .map(|o| o.operation_id.clone())
                .collect();
            Some(WorkerRow {
                worker: w.clone(),
                taken_over: super::origin::taken_over(w, Some(&targets)),
                target,
                sends,
                interjected,
                may_interject: orchestrator.is_some()
                    && w.orchestrator.as_deref() == orchestrator
                    && interject_agent(&cmd),
                reports: super::reports::summary(doc, w),
            })
        })
        .collect()
}

fn view(
    runtime: &Runtime,
    client: &str,
    row: &WorkerRow,
    seen: Option<&HashMap<String, Seen>>,
    plans: Option<&crate::scheduler::Observations>,
) -> Value {
    let target = row.target.as_ref();
    let observed = target.and_then(|t| seen.and_then(|s| s.get(&t.session)).copied());
    let (unsent, states) = queues(runtime)
        .map(|q| {
            crate::scheduler::worker_rows(&q.q, &row.worker.card_id, client, &row.sends, plans)
        })
        .unwrap_or_default();
    let agent = match (seen, observed) {
        (None, _) => json!(null),
        (
            Some(_),
            Some(Seen {
                state: Some(state),
                episode,
                ..
            }),
        ) => json!({"state": state, "episode": episode}),
        (Some(_), Some(seen)) => json!({
            "state": "unavailable",
            "episode": 0,
            "why": unavailable_why(seen.codex),
        }),
        (Some(_), None) => json!({"state": "unavailable", "episode": 0}),
    };
    json!({
        "cardId": row.worker.card_id,
        "title": target.map(|t| t.title.as_str()).unwrap_or_default(),
        "automationId": row.worker.automation_id,
        "model": row.worker.model,
        "alive": observed.is_some_and(|o| o.alive),
        "agent": agent,
        "takenOver": row.taken_over,
        "mayInterject": row.may_interject,
        "unsent": unsent,
        "reports": row.reports,
        // each line's queue state, how it goes (`queued` after the card is
        // quiet, `interject` without waiting for that) and, while it is
        // unsent, the stage it waits at (the panel's own plan stage)
        "sends": states
            .iter()
            .map(|(id, state, waiting)| {
                let mut line = json!({
                    "operationId": id,
                    "state": state,
                    "mode": if row.interjected.contains(id) { INTERJECT_MODE } else { "queued" },
                });
                if let Some(stage) = waiting {
                    line["waiting"] = json!(stage);
                }
                line
            })
            .collect::<Vec<_>>(),
    })
}

type Snapshot = (Vec<WorkerRow>, u64, bool, Vec<Value>);

fn snapshot(runtime: &Runtime, client_id: &str) -> Result<Snapshot, Value> {
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    runtime
        .read(|doc| {
            permitted(doc, client_id, Choice::Workers).map(|client| {
                (
                    rows_of(doc, client_id, orchestrator),
                    super::origin::send_sequence(client, orchestrator),
                    client.allow_replies,
                    types_view(client),
                )
            })
        })
        .map_err(map_error)?
        .map_err(map_error)
}

pub(super) fn worker_list(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let (rows, next, replies, types) = snapshot(runtime, client_id)?;
    let observed = observe_for_views();
    let seen = observed.as_ref().map(|(seen, _)| seen);
    let plans = observed.as_ref().map(|(_, plans)| plans);
    Ok(json!({
        "ok": true,
        "workers": rows.iter().map(|row| view(runtime, client_id, row, seen, plans)).collect::<Vec<_>>(),
        "observed": seen.is_some(),
        "mayReadReplies": replies,
        "workerTypes": types,
        "nextSendSequence": next,
    }))
}

pub(super) fn worker_wait(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerWaitArgs = parse(arguments)?;
    let wait = args.wait_ms.unwrap_or(DEFAULT_WAIT_MS);
    // one wait may follow every worker card the client may have
    let most = runtime.read(client_capacity).map_err(map_error)?;
    if wait > MAX_WAIT_MS || args.cards.is_empty() || args.cards.len() > most {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            &format!(
                "cards must name 1 to {most} worker cards and wait_ms be at most {MAX_WAIT_MS}"
            ),
            "Name the cards and episodes deck_worker_list returned; call again after a timeout.",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(wait);
    loop {
        let mark = change_count();
        let (rows, next, replies, types) = snapshot(runtime, client_id)?;
        let observed = observe_for_views();
        let seen = observed.as_ref().map(|(seen, _)| seen);
        let plans = observed.as_ref().map(|(_, plans)| plans);
        let views: Vec<Value> = rows
            .iter()
            .filter(|row| args.cards.iter().any(|c| c.card_id == row.worker.card_id))
            .map(|row| view(runtime, client_id, row, seen, plans))
            .collect();
        let changed = args.cards.iter().any(|asked| {
            match views.iter().find(|v| v["cardId"] == asked.card_id.as_str()) {
                None => true,
                Some(v) => {
                    v["takenOver"] == true
                        || v["alive"] == false
                        || v["agent"]["episode"].as_u64() != Some(asked.episode)
                        || asked
                            .reports
                            .is_some_and(|seen| v["reports"]["latest"].as_u64() != Some(seen))
                }
            }
        });
        if changed || Instant::now() >= deadline {
            return Ok(json!({
                    "ok": true,
                    "changed": changed,
                    "workers": views,
                    "observed": seen.is_some(),
                    "mayReadReplies": replies,
            "workerTypes": types,
                    "nextSendSequence": next,
                }));
        }
        wait_change(mark, deadline);
        // at most five listings a second, however busy the other agents are
        std::thread::sleep(
            Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

pub(super) fn own_card(
    doc: &DiskDoc,
    client_id: &str,
    orchestrator: Option<&str>,
    card: &str,
) -> Result<Worker, Value> {
    doc.workers
        .iter()
        .find(|w| {
            w.card_id == card
                && w.client_id == client_id
                && super::origin::sees(w, orchestrator)
        })
        .cloned()
        .ok_or_else(|| {
            error_value(
                "WORKER_NOT_FOUND",
                "that card is not one of this client's worker cards (or another orchestrator card's)",
                "Use a cardId from deck_worker_list; only cards this client started through its automations are listed, and only this orchestrator card's own.",
            )
        })
}

pub(super) fn worker_reply(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerReplyArgs = parse(arguments)?;
    let orchestrator = super::origin::orchestrator();
    let worker = runtime
        .read(|doc| -> Result<Result<Worker, Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            if !client.allow_replies {
                return Err(DeckError::new(
                    ErrorKind::Perm,
                    "client may not read worker replies",
                ));
            }
            Ok(own_card(
                doc,
                client_id,
                orchestrator.as_deref(),
                &args.card_id,
            ))
        })
        .map_err(map_error)?
        .map_err(map_error)??;
    let gone = || {
        error_value(
            "REPLY_GONE",
            "Deck holds no final message for that turn ending",
            "Only Claude Code, Codex and Pi turn endings Deck saw since it started are kept (the newest 64), Codex's and Pi's only while the user lets Deck forward them; ask the worker to write durable results to a file instead.",
        )
    };
    let target = crate::documents::card_targets(false)
        .and_then(|cards| cards.get(&worker.card_id).cloned())
        .ok_or_else(gone)?;
    let text = crate::turn_summary::reply_for(&target.session, args.episode).ok_or_else(gone)?;
    Ok(json!({
        "ok": true,
        "cardId": worker.card_id,
        "episode": args.episode,
        "truncated": text.contains(crate::turn_summary::REPLY_CUT),
        "text": text,
    }))
}

fn send_window() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    static WINDOW: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();
    WINDOW.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether `client` may send one more line or close this minute, and how
/// many it may (`sends_per_minute`): `Err(limit)` when not.
fn within_rate(runtime: &Runtime, client: &str) -> Result<(), usize> {
    let limit = runtime.read(sends_per_minute).unwrap_or(60);
    let mut window = send_window().lock_or_recover();
    let sent = window.entry(client.to_string()).or_default();
    let now = Instant::now();
    sent.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
    if sent.len() >= limit {
        return Err(limit);
    }
    sent.push(now);
    Ok(())
}

fn capacity(message: &str) -> Value {
    error_value(
        "CAPACITY_EXCEEDED",
        message,
        "Wait for the card's unsent lines to go out (deck_worker_list `unsent`) or slow down, then send again with a new request.",
    )
}

/// How a worker line goes: queued (`deck_worker_send`, the chain's quiet
/// time and the list's order) or interjected (`deck_worker_interject`: no
/// quiet time, only from the orchestrator card that started the card,
/// `scheduler/select.rs` "Interjected lines"). The tool's name is part of
/// the request's identity (`request_hash`), so one request id never names
/// both.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    Queued,
    Interject,
}

impl Line {
    fn tool(self) -> &'static str {
        match self {
            Line::Queued => "deck_worker_send",
            Line::Interject => "deck_worker_interject",
        }
    }
}

/// The value an interjected line's operation carries in `result.mode`; a
/// queued line's carries none, as before.
pub(super) const INTERJECT_MODE: &str = "interject";

/// Whether an operation is an interjected line (`result.mode`).
fn is_interjected(operation: &Operation) -> bool {
    operation
        .result
        .as_ref()
        .and_then(|r| r.get("mode"))
        .and_then(Value::as_str)
        == Some(INTERJECT_MODE)
}

/// The agents whose worker cards take an interjected line: those that take
/// a message while they work through their own message path and say when
/// they ask the user something — Claude Code (a submitted line is queued
/// into the running turn), Codex (it steers the running turn) and Pi (the
/// observer hands it over as a steering message). agy reports no question
/// at all (`agent_agy`: a confirmation reads `working`), so a line typed
/// while it works could answer one: refused, never queued in its place.
pub(super) fn interject_agent(cmd: &str) -> bool {
    matches!(
        crate::admission::worker_agent_command(cmd),
        Some("claude" | "codex" | crate::agent_pi::PROCESS)
    )
}

/// An interjected line is plain text: newlines and tabs, no other control
/// character. It is pasted while the agent works, when a question of the
/// agent's may come up at any moment, and an escape sequence in a paste is
/// a keystroke to an application that reads it outside its input.
fn plain_text(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

fn not_interjectable(from_no_card: bool) -> Value {
    error_value(
        "WORKER_NOT_FOUND",
        if from_no_card {
            "this call comes from no Deck card: only the orchestrator card that started a worker card can interject into it"
        } else {
            "that card is not a worker card this orchestrator card started"
        },
        "Interject only into the cards you started from this Deck card (deck_worker_list mayInterject); queue a line with deck_worker_send for any other card you drive.",
    )
}

fn interject_unsupported() -> Value {
    error_value(
        "INTERJECT_UNSUPPORTED",
        "this card's agent cannot take an interjected line safely: it does not tell Deck when it asks the user something",
        "Queue the line with deck_worker_send instead: it goes once the card is quiet, under the same checks.",
    )
}

pub(super) fn worker_send(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    queue_line(runtime, client_id, arguments, Line::Queued)
}

/// `deck_worker_interject`: one line for a worker card, which Deck sends
/// without waiting for the card to fall quiet. Only for the orchestrator
/// card that started the card, proven from the kernel (`opened_by`; a
/// caller from no card, an unowned card and another orchestrator's card are
/// `WORKER_NOT_FOUND`), and only into an agent that takes a message while it
/// works (`interject_agent`). The same choice, sequence, rate, unsent bound,
/// episode check, delegated authority and holds as `deck_worker_send`.
pub(super) fn worker_interject(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    queue_line(runtime, client_id, arguments, Line::Interject)
}

fn queue_line(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
    line: Line,
) -> Result<Value, Value> {
    let args: WorkerSendArgs = parse(arguments)?;
    if !valid_id(&args.request_id)
        || args.text.trim().is_empty()
        || args.text.len() > MAX_SEND_BYTES
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id or text is out of shape or bounds",
            "Send non-empty text of at most 16 KiB with a fresh request id.",
        ));
    }
    if line == Line::Interject && !plain_text(&args.text) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "an interjected line must be plain text: no control character but newlines and tabs",
            "Send the message as plain text with a fresh request id.",
        ));
    }
    let hash = request_hash(line.tool(), &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    let replay = replay_of(runtime, client_id, Choice::Workers, &args.request_id, &hash)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    // an interjected line: only from the orchestrator card that started the
    // card, never from no card (the kernel's word, never the client's)
    let caller = match (line, orchestrator) {
        (Line::Interject, None) => return Err(not_interjectable(true)),
        (Line::Interject, Some(card)) => Some(card),
        (Line::Queued, _) => None,
    };
    let (worker, stale) = runtime
        .read(|doc| -> Result<Result<(Worker, bool), Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            if !super::origin::lane_room(client, orchestrator) {
                return Ok(Err(super::origin::no_lane()));
            }
            let stale = super::origin::send_sequence(client, orchestrator) != args.send_sequence;
            let worker = match caller {
                Some(caller) => opened_by(doc, client_id, caller, &args.card_id)
                    .cloned()
                    .ok_or_else(|| not_interjectable(false)),
                None => own_card(doc, client_id, orchestrator, &args.card_id),
            };
            Ok(worker.map(|w| (w, stale)))
        })
        .map_err(map_error)?
        .map_err(map_error)??;
    if stale {
        return Err(map_error(DeckError::new(
            ErrorKind::ContextChanged,
            STALE_REQUEST,
        )));
    }
    // the target as the committed Board says now; a recovered Board admits
    // nothing
    let target = crate::documents::card_targets(true)
        .and_then(|cards| cards.get(&worker.card_id).cloned())
        .ok_or_else(|| {
            error_value(
                "WORKER_NOT_FOUND",
                "the card is no longer on the Board, or Deck cannot read the Board as current",
                "Read deck_worker_list again; a closed card cannot be driven.",
            )
        })?;
    if line == Line::Interject && !interject_agent(&target.cmd) {
        return Err(interject_unsupported());
    }
    // the episode the client decided on must still be the card's: a turn
    // that moved on since makes this line stale (refused, never held)
    let seen = observe_sessions().ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "Deck could not read the card's terminal",
        ))
    })?;
    let current = seen.get(&target.session).map_or(0, |s| s.episode);
    if current != args.after_episode {
        return Err(json!({"ok": false, "error": {
            "code": "EPISODE_MOVED",
            "message": "the card's Agent observation is no longer the episode this line was written after; nothing was queued",
            "nextAction": "Read the card again (deck_worker_wait or deck_worker_list) and decide on its current state before sending.",
            "currentEpisode": current
        }}));
    }
    let queue = queues(runtime).ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "Deck's queue is not available",
        ))
    })?;
    let sends = runtime
        .read(|doc| sends_of(doc, client_id, &worker.card_id))
        .map_err(map_error)?;
    let (unsent, _) =
        crate::scheduler::worker_rows(&queue.q, &worker.card_id, client_id, &sends, None);
    if unsent >= MAX_UNSENT_PER_CARD {
        return Err(capacity(
            "the card already holds 4 unsent lines from this client",
        ));
    }
    if let Err(limit) = within_rate(runtime, client_id) {
        return Err(capacity(&format!(
            "this client sent {limit} lines in the last minute"
        )));
    }
    let minted = random_id("op_", 16).map_err(map_error)?;
    let operation = runtime
        .write(|doc| -> Result<Result<Operation, Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Ok(existing.clone()));
            }
            if args.send_sequence != super::origin::send_sequence(client, orchestrator) {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            // the card handed back to nobody meanwhile: refused whole
            if let Some(caller) = caller {
                if opened_by(doc, client_id, caller, &worker.card_id).is_none() {
                    return Ok(Err(not_interjectable(false)));
                }
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let result = match caller {
                Some(caller) => json!({
                    "cardId": worker.card_id,
                    "afterEpisode": args.after_episode,
                    "mode": INTERJECT_MODE,
                    // `origin::RESULT_KEY`: the binding the line rests on
                    "orchestratorCardId": caller,
                }),
                None => json!({"cardId": worker.card_id, "afterEpisode": args.after_episode}),
            };
            let operation = Operation {
                result: Some(result),
                ..Operation::new(
                    minted.clone(),
                    client_id,
                    args.request_id.clone(),
                    hash.clone(),
                    WORKER_SEND_KIND,
                    "accepted",
                )
            };
            if let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) {
                super::origin::bump_send(client, orchestrator);
            }
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)??;
    if operation.state != "accepted" || operation.operation_id != minted {
        return Ok(operation_view(&operation));
    }
    let app = runtime.app.clone().ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "Deck's queue is not available",
        ))
    })?;
    let queued = crate::scheduler::worker_queue_add(
        queue,
        app,
        crate::scheduler::WorkerSend {
            client: client_id,
            automation: &worker.automation_id,
            operation: &operation.operation_id,
            card: &worker.card_id,
            session: &target.session,
            dir: &target.dir,
            cmd: &target.cmd,
            text: &args.text,
            interject: caller,
        },
        false,
        first_send_allowed(runtime, client_id),
    );
    let settled = runtime
        .write(|doc| {
            let Some(record) = doc
                .operations
                .iter_mut()
                .find(|o| o.operation_id == operation.operation_id)
            else {
                return Err(DeckError::new(ErrorKind::Missing, "operation vanished"));
            };
            match &queued {
                Ok(()) => record.state = "committed".into(),
                Err(error) => {
                    record.state = "rejected".into();
                    record.code = Some(
                        if error.kind() == ErrorKind::Invalid {
                            "NOT_AN_AGENT_CARD"
                        } else {
                            "QUEUE_REFUSED"
                        }
                        .into(),
                    );
                }
            }
            record.updated_at = now_ms();
            Ok(record.clone())
        })
        .map_err(map_error)?;
    Ok(operation_view(&settled))
}

/* ---------- closing ---------- */

/// Why a close is refused, as the client reads it (closed codes).
fn not_opened_here(from_no_card: bool) -> Value {
    error_value(
        "WORKER_NOT_FOUND",
        if from_no_card {
            "this call comes from no Deck card: only the orchestrator card that started a worker card can close it"
        } else {
            "that card is not a worker card this orchestrator card started"
        },
        "Close only the cards you started from this Deck card (deck_worker_list); the user closes every other card.",
    )
}

fn taken_over_refusal() -> Value {
    error_value(
        "WORKER_TAKEN_OVER",
        "the user took this card over: it is theirs until they hand it back",
        "Leave it to the user; once they hand it back you may close it.",
    )
}

fn needs_you_refusal() -> Value {
    error_value(
        "WORKER_NEEDS_YOU",
        "the card's Agent is asking the local user something: that question is theirs to answer",
        "Leave the card to the user; you may close it once its turn has ended.",
    )
}

fn episode_moved_refusal(current: u64) -> Value {
    json!({"ok": false, "error": {
        "code": "EPISODE_MOVED",
        "message": "the card's Agent observation is no longer the episode this close was decided on; nothing was closed",
        "nextAction": "Read the card again (deck_worker_wait or deck_worker_list) and decide on its current state.",
        "currentEpisode": current
    }})
}

/// The worker card `card` of `client` that orchestrator card `caller`
/// started (`Worker.orchestrator`, read from the kernel at its start):
/// never another card's, an unowned one, or any other card.
fn opened_by<'a>(doc: &'a DiskDoc, client: &str, caller: &str, card: &str) -> Option<&'a Worker> {
    doc.workers.iter().find(|w| {
        w.card_id == card && w.client_id == client && w.orchestrator.as_deref() == Some(caller)
    })
}

/// Whether a planned close may still go ahead, read again where it is
/// admitted (`commands::close_admit`, inside the webview's Board
/// transaction): still this client's choice, still a card the planning
/// orchestrator card started and nobody took over, its Agent not asking the
/// user, its episode the one the close was decided on. `Err` is the closed
/// code the operation is rejected with.
pub(super) fn close_still_admissible(
    doc: &DiskDoc,
    operation: &Operation,
    targets: Option<&HashMap<String, crate::documents::CardTarget>>,
    seen: Option<&HashMap<String, Seen>>,
) -> Result<(), &'static str> {
    let plan = operation.result.as_ref().ok_or("worker-close-plan")?;
    let text = |key: &str| plan.get(key).and_then(Value::as_str).unwrap_or_default();
    if permitted(doc, &operation.client_id, Choice::Workers).is_err() {
        return Err("client-revoked");
    }
    let worker = opened_by(
        doc,
        &operation.client_id,
        text("orchestratorCardId"),
        text("cardId"),
    )
    .ok_or("worker-not-found")?;
    if super::origin::taken_over(worker, targets) {
        return Err("worker-taken-over");
    }
    let seen = seen.ok_or("worker-unobserved")?;
    let now = seen.get(text("session")).copied();
    if now.map_or(0, |s| s.episode)
        != plan
            .get("afterEpisode")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    {
        return Err("episode-moved");
    }
    if now.and_then(|s| s.state) == Some("needs-input") {
        return Err("worker-needs-you");
    }
    Ok(())
}

/// The tmux session an admitted worker close may end (`validate_close_admission`).
pub(super) fn close_session(operation: &Operation) -> Option<&str> {
    operation.result.as_ref()?.get("session")?.as_str()
}

/// `deck_worker_close`: the orchestrator card that started a worker card
/// asks Deck to close it. Refused unless the call comes from that very
/// card (the kernel's word, `origin.rs`; a caller from no card closes
/// nothing), the card is on the current Board, the user has not taken it
/// over, its episode is still the one the client decided on and its Agent
/// is not asking the user anything (an observation may refuse a close,
/// never release one). Bound to the client's send sequence exactly as a
/// start or a line; journaled as a `worker-close` Board intent the
/// webview's one close commits or refuses (`mcp.js` `closeWorker`: never
/// while a pane shows the card, with a Reminder or a kept scratchpad).
pub(super) fn worker_close(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerCloseArgs = parse(arguments)?;
    if !valid_id(&args.request_id) || !valid_id(&args.card_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id or card id is out of shape",
            "Use a cardId from deck_worker_list and a fresh request id.",
        ));
    }
    let hash = request_hash("deck_worker_close", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    let replay = replay_of(runtime, client_id, Choice::Workers, &args.request_id, &hash)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    let Some(caller) = orchestrator else {
        return Err(not_opened_here(true));
    };
    let (worker, stale) = runtime
        .read(|doc| -> Result<Result<(Worker, bool), Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            if !super::origin::lane_room(client, orchestrator) {
                return Ok(Err(super::origin::no_lane()));
            }
            let stale = super::origin::send_sequence(client, orchestrator) != args.send_sequence;
            Ok(opened_by(doc, client_id, caller, &args.card_id)
                .cloned()
                .map(|w| (w, stale))
                .ok_or_else(|| not_opened_here(false)))
        })
        .map_err(map_error)?
        .map_err(map_error)??;
    if stale {
        return Err(map_error(DeckError::new(
            ErrorKind::ContextChanged,
            STALE_REQUEST,
        )));
    }
    // the card as the committed Board says now; a recovered Board closes
    // nothing
    let targets = crate::documents::card_targets(true);
    let target = targets
        .as_ref()
        .and_then(|cards| cards.get(&worker.card_id).cloned())
        .ok_or_else(|| {
            error_value(
                "WORKER_NOT_FOUND",
                "the card is no longer on the Board, or Deck cannot read the Board as current",
                "Read deck_worker_list again; a card already closed needs nothing more.",
            )
        })?;
    if super::origin::taken_over(&worker, targets.as_ref()) {
        return Err(taken_over_refusal());
    }
    let seen = observe_sessions().ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "Deck could not read the card's terminal",
        ))
    })?;
    let now = seen.get(&target.session).copied();
    let current = now.map_or(0, |s| s.episode);
    if current != args.after_episode {
        return Err(episode_moved_refusal(current));
    }
    if now.and_then(|s| s.state) == Some("needs-input") {
        return Err(needs_you_refusal());
    }
    if let Err(limit) = within_rate(runtime, client_id) {
        return Err(error_value(
            "CAPACITY_EXCEEDED",
            &format!("this client sent {limit} lines or closes in the last minute"),
            "Slow down, then close again with a new request.",
        ));
    }
    let minted = random_id("op_", 16).map_err(map_error)?;
    let operation = runtime
        .write(|doc| -> Result<Result<Operation, Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Ok(existing.clone()));
            }
            if args.send_sequence != super::origin::send_sequence(client, orchestrator) {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            // taken over meanwhile: refused whole
            match opened_by(doc, client_id, caller, &worker.card_id) {
                None => return Ok(Err(not_opened_here(false))),
                Some(w) if super::origin::taken_over(w, targets.as_ref()) => {
                    return Ok(Err(taken_over_refusal()))
                }
                Some(_) => {}
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let operation = Operation {
                result: Some(json!({
                    "cardId": worker.card_id,
                    "session": target.session,
                    "projectId": target.project_id,
                    "title": target.title,
                    "afterEpisode": args.after_episode,
                    "orchestratorCardId": caller,
                })),
                ..Operation::new(
                    minted.clone(),
                    client_id,
                    args.request_id.clone(),
                    hash.clone(),
                    WORKER_CLOSE_KIND,
                    "accepted",
                )
            };
            if let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) {
                super::origin::bump_send(client, orchestrator);
            }
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)??;
    emit_changed(runtime);
    Ok(operation_view(&operation))
}

/* ---------- local commands ---------- */

/// Bring the Stop hook in line with whether any client may read replies
/// (or the phone shows them: `agent_status::forwarding_wanted`).
pub(super) fn sync_reply_hook() {
    crate::agent_status::sync_forwarded_replies();
}

/// The webview's view of the worker cards: which client drives each and
/// whether the user took it over.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkerCardView {
    card_id: String,
    client_name: String,
    taken_over: bool,
    /// The orchestrator card that started it (`origin.rs`); the webview
    /// names the card and tells a closed one from the Board it holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestrator_card_id: Option<String>,
    /// When the card's newest kept report came (`reports.rs`), epoch ms;
    /// the text never reaches the webview.
    #[serde(skip_serializing_if = "Option::is_none")]
    reported_at: Option<u64>,
}

pub(super) fn workers_ui(runtime: &Runtime) -> Result<Vec<WorkerCardView>, DeckError> {
    let targets = crate::documents::card_targets(false);
    runtime.read(|doc| {
        doc.workers
            .iter()
            .filter_map(|w| {
                let client = doc
                    .config
                    .clients
                    .iter()
                    .find(|c| c.id == w.client_id && c.revoked_at.is_none())?;
                Some(WorkerCardView {
                    card_id: w.card_id.clone(),
                    client_name: client.name.clone(),
                    taken_over: super::origin::taken_over(w, targets.as_ref()),
                    orchestrator_card_id: w.orchestrator.clone(),
                    reported_at: super::reports::reported_at(doc, w),
                })
            })
            .collect()
    })
}

/// The user typed into worker card `card`'s pane (or handed it back): the
/// card's delegated lines hold, or go on. Idempotent; `false` when the
/// card is no worker card (nothing changed).
pub(super) fn worker_set_held(
    runtime: &Runtime,
    card: &str,
    held: bool,
) -> Result<bool, DeckError> {
    let targets = crate::documents::card_targets(false);
    let changed = runtime.write(|doc| {
        let Some(worker) = doc.workers.iter_mut().find(|w| w.card_id == card) else {
            return Ok(false);
        };
        // a worker whose orchestrator card is gone is the user's: handing it
        // back makes it unowned, for any of the client's orchestrators
        let orphaned = super::origin::orphaned(worker, targets.as_ref());
        if worker.held == held && !(orphaned && !held) {
            return Ok(false);
        }
        worker.held = held;
        if orphaned && !held {
            worker.orchestrator = None;
        }
        let client = worker.client_id.clone();
        audit(
            doc,
            if held {
                "worker-taken-over"
            } else {
                "worker-handed-back"
            },
            AuditLink {
                principal_id: Some(&client),
                ..Default::default()
            },
        )?;
        Ok(true)
    })?;
    if changed {
        note_worker_change();
        if let Some(app) = runtime.app.as_ref() {
            let _ = app.emit("mcp-workers-changed", ());
        }
        if !held {
            crate::scheduler::wake_scheduler();
        }
    }
    Ok(changed)
}

/* ---------- worker types and deck_worker_start ---------- */

pub(super) const WORKER_START_KIND: &str = "worker-start";
/// Worker types one client may have.
pub(super) const MAX_WORKER_TYPES: usize = 8;
const MAX_TYPE_DESCRIPTION_CHARS: usize = 200;
const MAX_WORKER_TITLE_CHARS: usize = 80;
/// The prefix of a worker type's first-send override (`ReadinessOverride.rule`):
/// it rests on the client's worker first-send choice, never on a rule.
pub(crate) const WORKER_TYPE_RULE_PREFIX: &str = "worker-type:";

/// One kind of worker card the user offers a client: the command its cards
/// start, a line the orchestrator reads to choose between types and,
/// optionally, the models a start may choose from (`models`, approved
/// 2026-11-21: Deck appends `--model <the listed entry>`, `start_command`).
/// The client only ever names a type and one of its listed models; it never
/// writes a command or a model of its own. Where a card
/// opens is not the type's: `worker_start` decides it (the column the start
/// names, else beside the orchestrator card, else the project's new-card
/// column).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", from = "WorkerTypeRecord")]
pub(crate) struct WorkerType {
    pub(crate) name: String,
    pub(crate) cmd: String,
    pub(crate) description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) models: Vec<String>,
}

/// A worker type as it is read: builds before 2026-11-19 gave each type a
/// column (`columnId`), accepted here only so their ledgers still load;
/// it is ignored and omitted on the next save (as `allowShell` is). An
/// older build cannot read a type without one and refuses the whole ledger
/// untouched.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerTypeRecord {
    name: String,
    cmd: String,
    #[serde(default, rename = "columnId")]
    _legacy_column: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    models: Vec<String>,
}

impl From<WorkerTypeRecord> for WorkerType {
    fn from(record: WorkerTypeRecord) -> Self {
        Self {
            name: record.name,
            cmd: record.cmd,
            description: record.description,
            models: record.models,
        }
    }
}

/// The closed reasons a worker type list is refused (stable `mcp-*` codes the
/// webview maps to one sentence each).
pub(super) const TYPE_NAME_INVALID: &str = "mcp-worker-type-name";
pub(super) const TYPE_COMMAND_INVALID: &str = "mcp-worker-type-command";
pub(super) const TYPE_CODEX_DAEMON: &str = "mcp-worker-type-codex-daemon";
pub(super) const TYPE_LIST_INVALID: &str = "mcp-worker-type-list";
pub(super) const TYPE_PI_OBSERVER: &str = "mcp-worker-type-pi-observer";
pub(super) const TYPE_AGY_SIGNAL: &str = "mcp-worker-type-agy-signal";
pub(super) const TYPE_MODELS_INVALID: &str = "mcp-worker-type-models";
pub(super) const TYPE_MODEL_IN_COMMAND: &str = "mcp-worker-type-model-in-command";
pub(super) const TYPE_CONFIG_ROOT: &str = "mcp-worker-type-config-root";
pub(super) const TYPE_CONFIG_HOOKS: &str = "mcp-worker-type-config-hooks";

/// A Claude worker type's own config folder (`admission::config_root`):
/// a folder that exists now, and — unless it is `~/.claude` itself, where
/// Deck's hooks are (`agent_status/hooks.rs`) — a `--settings` of the
/// command's own, or that Claude would load none of Deck's hooks: no Agent
/// status, so its lines would never go out and its orchestrator would
/// never see a turn end. What the settings file holds is the user's.
fn config_root_problem(cmd: &str) -> Option<&'static str> {
    let (root, rest) = crate::admission::config_root(cmd)?;
    let root = std::path::Path::new(root);
    if !root.is_dir() {
        return Some(TYPE_CONFIG_ROOT);
    }
    let default = dirs::home_dir().is_some_and(|home| home.join(".claude") == root);
    let settings = rest
        .split(' ')
        .any(|word| word == "--settings" || word.starts_with("--settings="));
    (!default && !settings).then_some(TYPE_CONFIG_HOOKS)
}

/// Models one worker type may list, and the longest entry.
pub(super) const MAX_TYPE_MODELS: usize = 20;
const MAX_MODEL_CHARS: usize = 100;

/// One listed model: a single simple argument (the alphabet of
/// `admission::worker_agent_command`) that is not itself a flag.
fn valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MAX_MODEL_CHARS
        && !model.starts_with('-')
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=+,@".contains(&byte))
}

/// Whether a command word already chooses a model, so a list would make two
/// places say which one: Claude, Codex and Pi `--model`/`--model=`, Codex
/// and Claude `-m` (also with its value attached), Pi `--models` and
/// `--provider`, Codex's config override `-c model=…` (any word holding
/// `model=`).
fn names_a_model(word: &str) -> bool {
    matches!(word, "--model" | "--models" | "--provider")
        || word.starts_with("--provider=")
        || word.contains("model=")
        || (word.starts_with("-m") && !word.starts_with("--"))
}

/// The command a worker card of `worker_type` starts: the type's own, or
/// with `--model <entry>` when the start chose one of its listed models.
/// The entry comes from the user's list, never from the request.
pub(super) fn start_command(worker_type: &WorkerType, listed: Option<&str>) -> String {
    match listed {
        Some(model) => format!("{} --model {model}", worker_type.cmd),
        None => worker_type.cmd.clone(),
    }
}

fn valid_type_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

/// A worker type list as the user saved it: at most `MAX_WORKER_TYPES`,
/// unique names, each command one the external admission accepts (an
/// interactive `claude` or `codex` with simple arguments, a Claude one
/// perhaps under its own config folder: `config_root_problem`), a Codex command
/// only with `--no-daemon` (its shared-daemon Signal cannot be trusted, so its
/// lines would never go out), a short one-line description.
pub(super) fn validate_worker_types(types: &[WorkerType]) -> Result<(), DeckError> {
    let invalid = |code: &str| DeckError::new(ErrorKind::Invalid, code);
    if types.len() > MAX_WORKER_TYPES {
        return Err(invalid(TYPE_LIST_INVALID));
    }
    let mut names = HashSet::new();
    for worker_type in types {
        if !valid_type_name(&worker_type.name) || !names.insert(worker_type.name.as_str()) {
            return Err(invalid(TYPE_NAME_INVALID));
        }
        let agent = crate::admission::worker_agent_command(&worker_type.cmd)
            .ok_or_else(|| invalid(TYPE_COMMAND_INVALID))?;
        if let Some(problem) = config_root_problem(&worker_type.cmd) {
            return Err(invalid(problem));
        }
        // a Pi worker's lines go through Deck's observer in that Pi: without
        // Pi's work status turned on there is none (`agent_pi::handover`)
        if agent == crate::agent_pi::PROCESS && !crate::agent_pi::config::enabled().signal {
            return Err(invalid(TYPE_PI_OBSERVER));
        }
        if agent == "codex" && !worker_type.cmd.split(' ').any(|word| word == "--no-daemon") {
            return Err(invalid(TYPE_CODEX_DAEMON));
        }
        // an agy worker's lines wait for agy's own words (the scheduler's
        // first-interaction gate): without Deck's agy entry there are none,
        // and its orchestrator could not see a turn end either
        if agent == crate::agent_agy::PROCESS && !crate::agent_agy::enabled() {
            return Err(invalid(TYPE_AGY_SIGNAL));
        }
        if worker_type.description.chars().count() > MAX_TYPE_DESCRIPTION_CHARS
            || worker_type.description.chars().any(char::is_control)
        {
            return Err(invalid(TYPE_LIST_INVALID));
        }
        if worker_type.models.is_empty() {
            continue;
        }
        if worker_type.cmd.split(' ').skip(1).any(names_a_model) {
            return Err(invalid(TYPE_MODEL_IN_COMMAND));
        }
        let mut listed = HashSet::new();
        if worker_type.models.len() > MAX_TYPE_MODELS
            || !worker_type
                .models
                .iter()
                .all(|model| valid_model(model) && listed.insert(model.as_str()))
        {
            return Err(invalid(TYPE_MODELS_INVALID));
        }
        // each command a start could make is one the admission accepts
        for model in &worker_type.models {
            if crate::admission::worker_agent_command(&start_command(worker_type, Some(model)))
                != Some(agent)
            {
                return Err(invalid(TYPE_MODELS_INVALID));
            }
        }
    }
    Ok(())
}

/// Whether `client`'s worker first-send choice is on (false when the ledger
/// cannot be read): the override its worker lines carry.
fn first_send_allowed(runtime: &Runtime, client: &str) -> bool {
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .any(|c| c.id == client && c.allow_workers && c.worker_first_send)
        })
        .unwrap_or(false)
}

/// The key a client's worker first-send choice takes in
/// `automation_delegations`: a worker card's first line may go to its freshly
/// started agent without interaction evidence while it is there.
pub(crate) fn worker_first_send_key(client: &str) -> String {
    format!("worker-first:{client}")
}

fn types_view(client: &Client) -> Vec<Value> {
    client
        .worker_types
        .iter()
        .map(|t| {
            let mut view = json!({"type": t.name, "description": t.description});
            if !t.models.is_empty() {
                view["models"] = json!(t.models);
            }
            view
        })
        .collect()
}

/// A start that names its column while the client may not place cards:
/// that is the organize-cards choice (`Choice::CardEdits`, the one
/// delegation that places a card), never the worker choice.
fn column_not_permitted() -> Value {
    error_value(
        "PERMISSION_DENIED",
        "naming the column a worker card opens in needs the local user's permission to organize cards",
        "Leave column_id out (the card opens beside your own card, or where the user's new cards open in that project), or ask the local Deck user to let this client organize cards (deck_capabilities mayEditCards).",
    )
}

/// The project a worker card opens in, decided by its directory alone
/// (approved 2026-11-23): the caller may run anywhere, so where it runs
/// decides nothing. The directory must exist; the projects this client
/// reaches whose directory holds it are the candidates. None:
/// `PATH_NOT_ACCESSIBLE`. One: that project. Several (nested or shared
/// directories): `PATH_AMBIGUOUS` with each candidate, never the deepest
/// one — the project also decides which environment variables the shell
/// gets, so Deck never picks among them. `project_id` only chooses among
/// the candidates: a reached project whose directory does not hold `dir`
/// is `PATH_NOT_ACCESSIBLE` too.
fn start_project(
    client: &Client,
    dir: &str,
    named: Option<&str>,
) -> Result<Result<ProjectScope, Value>, DeckError> {
    let not_accessible = |message: &str| {
        Ok(Err(error_value(
            "PATH_NOT_ACCESSIBLE",
            message,
            "Use an existing directory inside one of the projects deck_capabilities authorizedWorkspaces lists, or ask the local Deck user to give this client that project.",
        )))
    };
    let Some(path) = std::fs::canonicalize(dir).ok().filter(|p| p.is_dir()) else {
        return not_accessible("that directory does not exist");
    };
    let holds = |scope: &ProjectScope| {
        scope.roots.iter().any(|root| {
            std::fs::canonicalize(root)
                .ok()
                .is_some_and(|root| path.starts_with(root))
        })
    };
    if let Some(id) = named {
        let scope = scoped_project(client, id)?;
        return if holds(&scope) {
            Ok(Ok(scope))
        } else {
            not_accessible("that directory is not inside the project named")
        };
    }
    let mut candidates: Vec<ProjectScope> =
        client_scopes(client).into_iter().filter(holds).collect();
    match candidates.len() {
        0 => not_accessible("that directory is not inside any project this client reaches"),
        1 => Ok(Ok(candidates.remove(0))),
        _ => {
            let names: HashMap<String, String> = crate::documents::board_layout()
                .into_iter()
                .map(|p| (p.id, p.name))
                .collect();
            Ok(Err(json!({"ok": false, "error": {
                "code": "PATH_AMBIGUOUS",
                "message": "that directory lies inside more than one project",
                "nextAction": "Ask or decide which project the card belongs to, and start again with its project_id and the same dir.",
                "projects": candidates.iter().map(|c| json!({
                    "projectId": c.project_id,
                    "project": names.get(&c.project_id),
                    "roots": c.roots,
                })).collect::<Vec<_>>(),
            }})))
        }
    }
}

/// A start's named column: the client may organize cards, the Board is the
/// current one (never one recovered from its backup) and the column is one
/// of the project's the card opens in; otherwise refused, a wrong column
/// with that project's columns.
fn named_column(
    runtime: &Runtime,
    client_id: &str,
    project: &str,
    column: &str,
) -> Result<(), Value> {
    let may = runtime
        .read(|doc| permitted(doc, client_id, Choice::CardEdits).is_ok())
        .map_err(map_error)?;
    if !may {
        return Err(column_not_permitted());
    }
    if !crate::documents::board_is_current() {
        return Err(super::cards::source_unavailable());
    }
    let columns = crate::documents::board_layout()
        .into_iter()
        .find(|p| p.id == project)
        .map(|p| p.columns)
        .unwrap_or_default();
    if columns.iter().any(|(id, _)| id == column) {
        return Ok(());
    }
    Err(json!({"ok": false, "error": {
        "code": "COLUMN_NOT_FOUND",
        "message": "that column is not a column of the project the card opens in",
        "nextAction": "Use one of the columns listed here, name the project the column belongs to (project_id), or leave column_id out.",
        "columns": columns
            .iter()
            .map(|(id, name)| json!({"columnId": id, "column": name}))
            .collect::<Vec<_>>(),
    }}))
}

pub(super) fn worker_start(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerStartArgs = parse(arguments)?;
    if !valid_id(&args.request_id)
        || args.column_id.as_deref().is_some_and(|id| !valid_id(id))
        || !std::path::Path::new(&args.dir).is_absolute()
        || args.text.trim().is_empty()
        || args.text.len() > MAX_SEND_BYTES
        || args
            .title
            .as_ref()
            .is_some_and(|t| t.trim().is_empty() || t.chars().count() > MAX_WORKER_TITLE_CHARS)
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id, directory, column id, text or title is out of shape or bounds",
            "Send the absolute directory the card works in (dir), a first line of at most 16 KiB, an optional title of at most 80 characters, an optional column_id from deck_cards_snapshot (projects[].columns), and a fresh request id.",
        ));
    }
    let hash = request_hash("deck_worker_start", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    // the project comes from the directory, never from where the caller
    // runs (resolved before the write: a client of all projects reads the
    // project's directory from disk)
    let scope = runtime
        .read(|doc| -> Result<Result<ProjectScope, Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            start_project(client, &args.dir, args.project_id.as_deref())
        })
        .map_err(map_error)?
        .map_err(map_error)??;
    let replay = replay_of(runtime, client_id, Choice::Workers, &args.request_id, &hash)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    if let Some(column) = args.column_id.as_deref() {
        named_column(runtime, client_id, &scope.project_id, column)?;
    }
    let operation = runtime
        .write(|doc| -> Result<Result<Operation, Value>, DeckError> {
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Ok(existing.clone()));
            }
            let client = permitted(doc, client_id, Choice::Workers)?.clone();
            // withdrawn since the check above: the start is refused whole
            if args.column_id.is_some() && permitted(doc, client_id, Choice::CardEdits).is_err() {
                return Ok(Err(column_not_permitted()));
            }
            if !super::origin::lane_room(&client, orchestrator) {
                return Ok(Err(super::origin::no_lane()));
            }
            if args.send_sequence != super::origin::send_sequence(&client, orchestrator) {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            let Some(worker_type) = (match args.r#type.as_deref() {
                Some(name) => client.worker_types.iter().find(|t| t.name == name),
                None => client.worker_types.first(),
            }) else {
                return Ok(Err(json!({"ok": false, "error": {
                    "code": "WORKER_TYPE_NOT_FOUND",
                    "message": "this client has no worker type of that name",
                    "nextAction": "Use one of the types deck_worker_list lists (workerTypes), or ask the local Deck user to add one.",
                    "workerTypes": types_view(&client)
                }})));
            };
            // a model only from the type's own list, compared byte for byte;
            // the command takes the listed entry, never the request's string
            let listed = match args.model.as_deref() {
                None => None,
                Some(asked) => match worker_type.models.iter().find(|m| m.as_str() == asked) {
                    Some(listed) => Some(listed.as_str()),
                    None => {
                        return Ok(Err(json!({"ok": false, "error": {
                            "code": "MODEL_NOT_LISTED",
                            "message": "this worker type does not list that model",
                            "nextAction": if worker_type.models.is_empty() {
                                "This worker type offers no model choice: leave model out, or use a type deck_worker_list lists with models."
                            } else {
                                "Use one of this type's models exactly as listed, or leave model out for the type's own command."
                            },
                            "type": worker_type.name,
                            "models": worker_type.models,
                        }})));
                    }
                },
            };
            let cmd = start_command(worker_type, listed);
            // the type was admitted when saved; the command it makes now is
            // checked again before anything is journaled
            let admitted = crate::admission::worker_agent_command(&cmd);
            if admitted.is_none()
                || (admitted == Some("codex") && !cmd.split(' ').any(|word| word == "--no-daemon"))
            {
                return Err(DeckError::new(ErrorKind::Invalid, TYPE_COMMAND_INVALID));
            }
            let project = &scope;
            // checked again under the write: the directory still lies inside
            let dir = canonical_scope(&args.dir, &project.roots)?;
            if let Err(full) = room_for_worker(doc, client_id, orchestrator) {
                return Ok(Err(full));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let title = args
                .title
                .clone()
                .unwrap_or_else(|| format!("{} worker", worker_type.name));
            let operation = Operation { result: Some(json!({
                    "cardId": random_id("W", 16)?,
                    "projectId": project.project_id,
                    // the column the start names, else beside the orchestrator
                    // card, else none: the project's new-card column (`mcp.js`)
                    "columnId": args.column_id.clone().or_else(|| {
                        super::origin::beside(&client, orchestrator, &project.project_id)
                    }),
                    "cmd": cmd,
                    "model": listed,
                    "dir": dir,
                    "title": title,
                    "type": worker_type.name,
                    // the first line waits here until the card exists, then
                    // leaves the ledger for the queue (`settle_start`)
                    "text": args.text,
                    "orchestratorCardId": orchestrator,
                })), ..Operation::new(random_id("op_", 16)?, client_id, args.request_id.clone(), hash.clone(), WORKER_START_KIND, "accepted") };
            if let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) {
                super::origin::bump_send(client, orchestrator);
            }
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)??;
    emit_changed(runtime);
    let mut view = operation_view(&operation);
    // the first line is the client's own text: not echoed back
    if let Some(result) = view.get_mut("result").and_then(Value::as_object_mut) {
        result.remove("text");
    }
    Ok(view)
}

/// A worker-start operation the webview committed: the card is a worker card
/// of the client, and the first line goes to the queue as the card's head row
/// (with the first-send override when the client's choice allows it). Called
/// by `commands::complete` after the commit write, outside the ledger locks.
pub(super) fn settle_start(runtime: &Runtime, operation_id: &str, session: &str) {
    let Ok(Some((client, card, kind_name, cmd, dir, text))) = runtime.read(|doc| {
        doc.operations
            .iter()
            .find(|o| o.operation_id == operation_id && o.kind == WORKER_START_KIND)
            .and_then(|o| {
                let r = o.result.as_ref()?;
                let get = |k: &str| r.get(k).and_then(Value::as_str).map(str::to_owned);
                Some((
                    o.client_id.clone(),
                    get("cardId")?,
                    get("type")?,
                    get("cmd")?,
                    get("dir")?,
                    get("text")?,
                ))
            })
    }) else {
        return;
    };
    let first_send = first_send_allowed(runtime, &client);
    let queued = match (queues(runtime), runtime.app.clone()) {
        (Some(queue), Some(app)) => crate::scheduler::worker_queue_add(
            queue,
            app,
            crate::scheduler::WorkerSend {
                client: &client,
                automation: &format!("{WORKER_TYPE_RULE_PREFIX}{kind_name}"),
                operation: operation_id,
                card: &card,
                session,
                dir: &dir,
                cmd: &cmd,
                text: &text,
                interject: None,
            },
            true,
            first_send,
        ),
        _ => Err(DeckError::new(
            ErrorKind::ContextChanged,
            "queue unavailable",
        )),
    };
    let _ = runtime.write(|doc| {
        if let Some(o) = doc
            .operations
            .iter_mut()
            .find(|o| o.operation_id == operation_id)
        {
            if let Some(result) = o.result.as_mut().and_then(Value::as_object_mut) {
                result.remove("text");
                result.insert(
                    "firstLine".into(),
                    json!(if queued.is_ok() { "queued" } else { "refused" }),
                );
            }
            o.updated_at = now_ms();
        }
        Ok(())
    });
    note_worker_change();
}

/// A project list as compared for a change: ids and roots.
fn scope_key(projects: &[ProjectScope]) -> Vec<(String, Vec<String>)> {
    projects
        .iter()
        .map(|p| (p.project_id.clone(), p.roots.clone()))
        .collect()
}

/// A client's choices as one value, compared before and after a save.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Choices {
    create: bool,
    automations: bool,
    workers: bool,
    replies: bool,
    worker_first_send: bool,
    worker_types: Vec<WorkerType>,
    drafts: bool,
    mentions: bool,
    cards: bool,
    card_edits: bool,
    rule_column: bool,
}

impl Choices {
    fn of(client: &Client) -> Self {
        Choices {
            create: client.allow_create,
            automations: client.allow_automations,
            workers: client.allow_workers,
            replies: client.allow_replies,
            worker_first_send: client.worker_first_send,
            worker_types: client.worker_types.clone(),
            drafts: client.allow_drafts,
            mentions: client.allow_mentions,
            cards: client.allow_cards,
            card_edits: client.allow_card_edits,
            rule_column: client.rule_column,
        }
    }

    /// What the dialog asked for, with its dependencies enforced: replies
    /// and the first-send choice need driving.
    fn from_input(input: &super::commands::PermissionsInput) -> Self {
        Choices {
            create: input.allow_create,
            automations: input.allow_automations,
            workers: input.allow_workers,
            replies: input.allow_workers && input.allow_replies,
            worker_first_send: input.allow_workers && input.worker_first_send,
            worker_types: input.worker_types.clone(),
            drafts: input.allow_drafts,
            mentions: input.allow_mentions,
            cards: input.allow_cards,
            card_edits: input.allow_cards && input.allow_card_edits,
            rule_column: input.rule_column,
        }
    }

    fn apply(self, client: &mut Client) {
        client.allow_create = self.create;
        client.allow_automations = self.automations;
        client.allow_workers = self.workers;
        client.allow_replies = self.replies;
        client.worker_first_send = self.worker_first_send;
        client.worker_types = self.worker_types;
        client.allow_drafts = self.drafts;
        client.allow_mentions = self.mentions;
        client.allow_cards = self.cards;
        client.allow_card_edits = self.card_edits;
        client.rule_column = self.rule_column;
    }
}

/// Save every choice of one client at once (`commands::mcp_client_permissions`).
/// Under the settings fence and the delivery lock, so a withdrawal is ordered
/// against the scheduler's firing intent exactly as `client_automations` is:
/// once this returns, no automatic send starts on a withdrawn choice.
/// Dependencies are enforced here, not only in the dialog: replies, worker
/// types' first-send choice need driving; driving keeps its types.
pub(super) fn client_permissions(
    runtime: &Runtime,
    client_id: &str,
    permissions: super::commands::PermissionsInput,
) -> Result<(), DeckError> {
    validate_worker_types(&permissions.worker_types)?;
    // a new scope, checked before any lock: all projects, or a list of one
    // to 64 existing projects with canonical roots
    let scope: Option<(bool, Vec<ProjectScope>)> = if permissions.all_projects {
        Some((true, Vec::new()))
    } else if let Some(list) = permissions.projects.clone() {
        if list.is_empty() || list.len() > MAX_PROJECTS_PER_CLIENT {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "invalid MCP client scope",
            ));
        }
        let scopes = super::commands::validate_new_client_scopes(
            list,
            runtime.app.is_some(),
            crate::documents::board_project_exists,
        )?;
        Some((false, scopes))
    } else {
        None
    };
    let withdrew_automations;
    let mut withdrew_drafts = false;
    let mut fenced = Vec::new();
    {
        let _fence = crate::storage::settings_fence();
        let _delivery = runtime.delivery.lock_or_recover();
        withdrew_automations = runtime.write(|doc| {
            let held = held_projects(doc, client_id);
            let client = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id && client.revoked_at.is_none())
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
            let before = Choices::of(client);
            // what it reached, and what the ledger says it holds anywhere
            let mut reach_before = reach(client);
            reach_before.extend(held);
            let scope_before = (client.all_projects, scope_key(&client.projects));
            if let Some((all, scopes)) = &scope {
                client.all_projects = *all;
                client.projects = scopes.clone();
            }
            let scope_changed = scope_before != (client.all_projects, scope_key(&client.projects));
            // a client of all projects keeps every project: nothing leaves
            // only a scope change withdraws anything
            let removed: HashSet<String> = if client.all_projects || scope.is_none() {
                HashSet::new()
            } else {
                reach_before.difference(&reach(client)).cloned().collect()
            };
            Choices::from_input(&permissions).apply(client);
            let after = Choices::of(client);
            if before == after && !scope_changed {
                return Ok(false);
            }
            fenced = withdraw_projects(doc, client_id, &removed);
            let withdrew_automations = before.automations && !after.automations;
            let withdrew_create = before.create && !after.create;
            let withdrew_workers = before.workers && !after.workers;
            if withdrew_automations {
                doc.reject_pending(
                    |operation| operation.client_id == client_id && operation.kind == TRIGGER_KIND,
                    "client-revoked",
                );
            }
            if withdrew_create {
                doc.reject_pending(
                    |operation| {
                        operation.client_id == client_id && operation.kind == "session-create"
                    },
                    "client-revoked",
                );
            }
            // withdrawing the mention choice takes the client's marks off
            if before.mentions && !after.mentions {
                doc.mention_marks.retain(|mark| mark.client_id != client_id);
            }
            if before.drafts && !after.drafts && super::proposals::withdraw_pending(doc, client_id)
            {
                withdrew_drafts = true;
            }
            if before.card_edits && !after.card_edits {
                doc.reject_pending(
                    |operation| {
                        operation.client_id == client_id
                            && operation.kind == super::cards::CARD_UPDATE_KIND
                    },
                    "client-revoked",
                );
            }
            if withdrew_workers {
                doc.reject_pending(
                    |operation| {
                        operation.client_id == client_id
                            && matches!(
                                operation.kind.as_str(),
                                WORKER_START_KIND | WORKER_CLOSE_KIND
                            )
                    },
                    "client-revoked",
                );
            }
            audit(
                doc,
                "permissions-changed",
                AuditLink {
                    principal_id: Some(client_id),
                    ..Default::default()
                },
            )?;
            Ok(withdrew_automations)
        })?;
    }
    if withdrew_automations {
        retire_inbox_for(client_id);
    }
    // the managed sessions of a project that left the scope go to the user
    for session in fenced {
        let _ = runner_control_result(send_runner_control(runtime, &session, "human"));
    }
    if withdrew_drafts {
        super::proposals::note_withdrawn(runtime);
    }
    super::cards::note_readers_changed(runtime);
    sync_reply_hook();
    note_worker_change();
    Ok(())
}
