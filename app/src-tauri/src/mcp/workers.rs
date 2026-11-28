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
//!   It is refused while the user holds the card and, in every mode, by the
//!   shared checks (`shared_verdict`): another program instance or session
//!   than the one decided on, a running program Deck has no Agent
//!   observation of (`OBSERVATION_UNKNOWN`), its Agent asking the user
//!   (`needs-input`), a line still in the queue or accepted and not yet
//!   queued. Its mode (`CloseMode`) adds what the decision was about: its
//!   episode (`episode`; `after_episode: 0` passes nothing the shared
//!   checks refuse), the instance and newest line (`instance`; the
//!   instance's own turns never refuse it, an ended instance leaves the
//!   card to close), or a gone session and the newest line (`gone`, for a
//!   card whose `liveness` is `gone`; refused once the session exists
//!   again). It goes as a `worker-close` Board intent through the
//!   webview's ordinary close as a remote one (`mcp.js` `closeWorker`);
//!   while it is under way the card takes no line (`WORKER_CLOSING`) or
//!   second close; the admission reads the card again with the same rules
//!   (`close_still_admissible`, the linearization point) and keeps what it
//!   saw (`admission_refusal`). Every refusal names its stage. Its reports
//!   stay readable.
//! - The views carry what an instance-bound close names: `instance` (the
//!   program in front of the card's pane, `instance_token`) and `lastLine`;
//!   and what a listing proved (`liveness`: `alive`, `gone`, `unknown`) and
//!   what is in front (`foreground`: program, shell or unknown, when it
//!   began, whether it began before this Deck process — the program that
//!   outlived a Deck restart, whose observation Deck lost — and the newest
//!   Agent re-entry Deck made into the session this run, its outcome word:
//!   entered, never resumed or ready). No old working/turn-done is ever
//!   restored as a new observation.
//! - A card's lines are the queue's rows (`scheduler::worker_lines`):
//!   every line still queued, being sent or uncertain is listed and counted
//!   whatever the history, the per-card bound counts all three in the
//!   queue's own transaction, and `deck_operation_get` answers one line's
//!   `delivery` by id. `deck_worker_cancel` cancels one queued line exactly
//!   (`worker_cancel`, `scheduler::worker_cancel`).
//! - Within one client, a worker started from an orchestrator card
//!   (`origin.rs`) is that card's: only calls from it (and from no card) see
//!   and drive it, its sequences are that card's lane, and once the card
//!   left the Board the user holds it (`origin::taken_over`, read wherever
//!   `held` is) until handing it back makes it unowned.
//! - A worker card's agent may hand its result to Deck for its orchestrator
//!   to read (`deck_worker_report`, `reports.rs`): data at rest, never a
//!   send or authority; the one thing it ends is a `deck_worker_wait` its
//!   orchestrator is making for it (approved 2026-11-28).
//! - Waiting (`worker_wait`, approved 2026-11-28): `until: change` (the
//!   original) or `until: report` (a new report past the caller's cursor,
//!   or the card leaving its reach: taken over, exited, gone; never just
//!   another episode); every answer names its `reason` and per-card
//!   `events`, and with `include_reports` and `allow_replies` the new
//!   reports inline (bounded). One Deck call blocks at most
//!   `WORKER_WAIT_CHUNK_MS`; at most `WAIT_SLOTS` block at once, all
//!   clients together (a wait past them answers `busy` at once), so waits
//!   never take the connections a report or a line needs. Longer waits are
//!   the adapter's string of such calls inside one tool call
//!   (`mcp-adapter` `long_wait`). The order is the ledger's: a report is
//!   persisted, then `note_worker_change` wakes the waits; a wait reads its
//!   change mark BEFORE its snapshot, so a report between the two ends the
//!   wait at once. Nothing is consumed: the caller's cursors stay good
//!   after a timeout, a cancel or a lost answer.
//! - Lane binding (`admit.rs`, approved 2026-11-28): start, send,
//!   interject and close take `send_sequence` (one at a time, as before)
//!   or `lane_epoch` (several at once, each decided on its own); every
//!   receipt says where the lane stands (`lane`), every refusal of the
//!   binding says what it expected (`STALE_REQUEST`, `scope: worker-lane`).
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
/// Delegated lines one card may hold still in its queue: queued (pending
/// or failed), being sent, or of uncertain delivery (`WorkerLines::active`;
/// an uncertain line keeps its place until the user decides it). Checked
/// again inside the queue transaction that adds a line
/// (`scheduler::ActiveBound`).
const MAX_ACTIVE_PER_CARD: usize = 4;
/// The longest line a client may send.
pub(super) const MAX_SEND_BYTES: usize = 16 * 1024;
/// Settled send operations reported per card: the history only. A line
/// still in the queue is always reported besides them
/// (`scheduler::worker_lines`).
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
    /// The operations of the newest lines accepted for this card
    /// (`queue_line`, at most `LINES_KEPT`): what a report may answer
    /// (`reports::reply_target`) after the journal retired their records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) lines: Vec<String>,
    /// The highest report sequence the orchestrator card that started this
    /// card acknowledged it received (`reports::worker_reports_ack`,
    /// approved 2026-11-28). `None`: not tracked — no report yet, a record
    /// whose reports came before acknowledgments existed (nothing is known
    /// of them), or an unowned card nobody may acknowledge for. A card of an
    /// orchestrator starts tracking (`Some(0)`) with its first report
    /// recorded after this build, in that report's write; its orchestrator's
    /// acknowledgment of an older one starts it too. It never goes back and
    /// never passes `reported`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) acknowledged: Option<u64>,
}

/// Line operations a worker record keeps (`Worker.lines`).
const LINES_KEPT: usize = 16;

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

#[cfg(test)]
pub(super) fn change_count_for_tests() -> u64 {
    change_count()
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
    let orchestrator = super::origin::of_operation(operation);
    doc.workers.push(Worker {
        card_id: card.into(),
        client_id: operation.client_id.clone(),
        operation_id: operation.operation_id.clone(),
        automation_id: automation,
        created_at: now_ms(),
        held: false,
        // tracking starts with the card's first report (`reports.rs`), so a
        // card that never reports leaves the ledger as it was
        acknowledged: None,
        orchestrator,
        reported: 0,
        model: operation
            .result
            .as_ref()
            .and_then(|r| r.get("model"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        lines: Vec::new(),
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
    /// The session's Signal-target pane as the kernel and tmux name it
    /// (`instance_key`: tmux server, session id, pane process); 0 when the
    /// session has no single target pane.
    pane: u64,
    /// That pane's foreground generation (`agent_status::live_generation`:
    /// the foreground process and its birth instant) folded with `pane`:
    /// the program instance in front, the same across every observation
    /// of its turns and another one after an exit, a re-entry or a new
    /// session. 0 when it could not be read (proves nothing).
    instance: u64,
    /// The foreground is the pane's own shell: no program runs in front.
    shell: bool,
    /// When the foreground process began, epoch seconds (0: unknown): an
    /// instance older than this Deck process outlived a Deck restart.
    born: u64,
}

/// A stable 64-bit key of `parts` (the first 8 bytes of their SHA-256):
/// the same facts give the same key in every Deck process.
fn instance_key(parts: &str) -> u64 {
    let digest = sha(parts.as_bytes());
    u64::from_str_radix(&digest[..16], 16).unwrap_or(0)
}

/// The instance token a worker view carries and a close may bind
/// (`close_mode: instance`): the pane key, then the instance key.
fn instance_token(seen: &Seen) -> Option<String> {
    (seen.pane != 0 && seen.instance != 0)
        .then(|| format!("{:016x}{:016x}", seen.pane, seen.instance))
}

/// A token back into its pane and instance keys.
fn parse_instance(token: &str) -> Option<(u64, u64)> {
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let pane = u64::from_str_radix(&token[..16], 16).ok()?;
    let instance = u64::from_str_radix(&token[16..], 16).ok()?;
    (pane != 0 && instance != 0).then_some((pane, instance))
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
/// (`scheduler::worker_lines`). One tmux call, as before. `None` when tmux
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
    let targets = crate::agent_status::signal_targets(rows);
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
                    pane: 0,
                    instance: 0,
                    shell: false,
                    born: 0,
                },
            )
        })
        .collect();
    for (session, row) in targets {
        let Some(entry) = seen.get_mut(&session) else {
            continue;
        };
        let pane = instance_key(&format!(
            "pane\0{}\0{}\0{}\0{}",
            row.server_pid, row.session_id, row.pane_id, row.pane_pid
        ));
        entry.pane = pane;
        if let Some(generation) = crate::agent_status::live_generation(row.pane_pid) {
            let (seconds, micros) = generation.birth();
            entry.instance = instance_key(&format!(
                "instance\0{pane:016x}\0{}\0{seconds}\0{micros}",
                generation.pid()
            ));
            entry.shell = generation.pid() == row.pane_pid;
            entry.born = seconds;
        }
    }
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
    /// A test's unsent-line count in place of the queue (`close_lines`):
    /// `Some(None)` = the queue cannot be read.
    pub(super) static UNSENT_FOR_TESTS: std::cell::Cell<Option<Option<usize>>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// A test's pane listing in place of tmux: session → (state, episode).
    pub(super) static OBSERVE_FOR_TESTS: std::cell::RefCell<Option<Option<HashMap<String, Seen>>>> =
        const { std::cell::RefCell::new(None) };
}

/// The pane and instance keys a test's observation carries unless it names
/// its own (`instance_seen_for_tests`).
#[cfg(test)]
pub(super) const TEST_PANE: u64 = 0x1111;
#[cfg(test)]
pub(super) const TEST_INSTANCE: u64 = 0x2222;

#[cfg(test)]
pub(super) fn seen_for_tests(state: Option<&'static str>, episode: u64) -> Seen {
    instance_seen_for_tests(state, episode, TEST_PANE, TEST_INSTANCE, false)
}

/// A test's observation with its own pane and instance keys.
#[cfg(test)]
pub(super) fn instance_seen_for_tests(
    state: Option<&'static str>,
    episode: u64,
    pane: u64,
    instance: u64,
    shell: bool,
) -> Seen {
    Seen {
        alive: true,
        state,
        episode,
        codex: None,
        pane,
        instance,
        shell,
        born: 0,
    }
}

/// The token a test's observation is read as (`instance_token`).
#[cfg(test)]
pub(super) fn token_for_tests(pane: u64, instance: u64) -> String {
    format!("{pane:016x}{instance:016x}")
}

#[cfg(test)]
pub(super) fn codex_seen_for_tests(codex: crate::agent_status::CodexSignalTrust) -> Seen {
    Seen {
        alive: true,
        state: None,
        episode: 0,
        codex: Some(codex),
        pane: TEST_PANE,
        instance: TEST_INSTANCE,
        shell: false,
        born: 0,
    }
}

/// Why a live worker has no observed state, for the orchestrator: a Codex
/// whose status Deck cannot attribute (it holds until that Codex exits, and
/// so do the lines queued into it), a Codex that has not proven its status
/// yet (its lines wait for its first own hook), or no turn seen yet.
fn unavailable_why(seen: &Seen) -> &'static str {
    use crate::agent_status::CodexSignalTrust::{Trusted, Unavailable, Unknown};
    if seen.shell {
        // the pane's own shell is in front: no agent program runs there
        return "no-agent-running";
    }
    match seen.codex {
        Some(Unavailable) => "codex-unattributed",
        Some(Unknown) => "codex-unproven",
        Some(Trusted) | None => "no-turn-seen",
    }
}

/// When this Deck process started, epoch seconds: a card's instance born
/// before it outlived a Deck restart, and Deck has observed none of its
/// turns since (`foreground.startedAt`).
fn deck_started_at() -> u64 {
    static STARTED: OnceLock<u64> = OnceLock::new();
    *STARTED.get_or_init(|| {
        // the kernel's word for this process, not the first call's clock
        crate::procinfo::process(std::process::id())
            .map(|p| p.start_seconds)
            .filter(|s| *s > 0)
            .unwrap_or(0)
    })
}

/// What is in front of a card's pane, for the views: the closed kind, when
/// it began, whether it began before this Deck process (it outlived a Deck
/// restart: the program is the one that ran before, its Agent observation
/// is what Deck lost), and the newest Agent re-entry Deck made into the
/// session in this run (`agent_reentry` outcome words: `started` and
/// `confirmed` say a program was launched or entered its session, never
/// that it resumed its task or is ready for input). `None` when the
/// listing has no such session.
fn foreground_view(now: Option<Seen>, session: Option<&str>) -> Value {
    let Some(now) = now else {
        return json!(null);
    };
    let kind = if now.pane == 0 || now.instance == 0 {
        "unknown"
    } else if now.shell {
        "shell"
    } else {
        "program"
    };
    let mut view = json!({
        "kind": kind,
        "startedAt": (now.born > 0).then_some(now.born),
        "beforeDeckStarted": (now.born > 0 && deck_started_at() > 0)
            .then(|| now.born < deck_started_at()),
    });
    if let Some((outcome, at)) = session.and_then(crate::agent_reentry::last_outcome) {
        view["reentry"] = json!({"outcome": outcome, "at": at});
    }
    view
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

/// Every operation Deck still records as one of `worker`'s lines: its
/// start, its accepted lines (`Worker.lines`) and every line record the
/// ledger keeps for its card. What makes a queue row the worker's besides
/// the row's own delegation (`scheduler::worker_lines`).
fn known_of(doc: &DiskDoc, worker: &Worker) -> Vec<String> {
    let mut ids: Vec<String> = doc
        .operations
        .iter()
        .filter(|o| {
            o.client_id == worker.client_id
                && matches!(o.kind.as_str(), WORKER_SEND_KIND | WORKER_START_KIND)
                && o.result
                    .as_ref()
                    .and_then(|r| r.get("cardId"))
                    .and_then(Value::as_str)
                    == Some(worker.card_id.as_str())
        })
        .map(|o| o.operation_id.clone())
        .collect();
    ids.extend(worker.lines.iter().cloned());
    ids.push(worker.operation_id.clone());
    ids.sort();
    ids.dedup();
    ids
}

/// One card's lines from its client, as the queue holds them now (`None`:
/// Deck's queue is not available).
fn lines_of(
    runtime: &Runtime,
    worker: &Worker,
    known: &[String],
    recent: &[String],
    plans: Option<&crate::scheduler::Observations>,
) -> Option<crate::scheduler::WorkerLines> {
    queues(runtime).map(|q| {
        crate::scheduler::worker_lines(
            &q.q,
            &worker.card_id,
            &worker.client_id,
            known,
            recent,
            plans,
        )
    })
}

/// One worker line as a view or `deck_operation_get` carries it: its
/// operation, where it stands (`scheduler::line_state` words), how it goes
/// and — while queued and observed — where it waits.
fn line_view(line: &crate::scheduler::WorkerLine, mode: &str) -> Value {
    let mut view = json!({
        "operationId": line.id,
        "state": line.state,
        "mode": mode,
        "cancelable": line.cancelable,
    });
    if let Some(plan) = &line.plan {
        view["waiting"] = json!(plan.stage);
        view["checkedAt"] = json!(plan.checked_at);
        if let Some(quiet) = plan.quiet_remaining.filter(|q| *q > 0) {
            view["quietRemaining"] = json!(quiet);
        }
        if let Some(gap) = plan.gap_until {
            view["gapUntil"] = json!(gap);
        }
        if let Some(previous) = &plan.previous {
            view["previousOperationId"] = json!(previous);
        }
    }
    view
}

struct WorkerRow {
    worker: Worker,
    taken_over: bool,
    target: Option<crate::documents::CardTarget>,
    sends: Vec<String>,
    /// `known_of`
    known: Vec<String>,
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
            let known = known_of(doc, w);
            let interjected = doc
                .operations
                .iter()
                .filter(|o| known.contains(&o.operation_id) && is_interjected(o))
                .map(|o| o.operation_id.clone())
                .collect();
            Some(WorkerRow {
                worker: w.clone(),
                taken_over: super::origin::taken_over(w, Some(&targets)),
                target,
                sends,
                known,
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
    debug_assert_eq!(row.worker.client_id, client);
    let lines = lines_of(runtime, &row.worker, &row.known, &row.sends, plans);
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
            "why": unavailable_why(&seen),
        }),
        (Some(_), None) => json!({"state": "unavailable", "episode": 0, "why": "session-gone"}),
    };
    json!({
        "cardId": row.worker.card_id,
        "title": target.map(|t| t.title.as_str()).unwrap_or_default(),
        "automationId": row.worker.automation_id,
        "model": row.worker.model,
        "alive": observed.is_some_and(|o| o.alive),
        // what the listing proved: `alive` (the session exists), `gone`
        // (a listing that succeeded has no such session: a close may name
        // close_mode gone) or `unknown` (the listing failed: proves nothing)
        "liveness": liveness(seen.is_some(), observed),
        "foreground": foreground_view(observed, target.map(|t| t.session.as_str())),
        "agent": agent,
        "takenOver": row.taken_over,
        // what a close bound to this agent instance names
        // (`close_mode: instance`): the program in front of the card's pane
        // (null when Deck cannot tell) and the newest line accepted for it
        "instance": observed.as_ref().and_then(instance_token),
        "lastLine": row.worker.lines.last(),
        "mayInterject": row.may_interject,
        // lines still to be sent (pending or failed); null: the queue
        // could not be read
        "unsent": lines.as_ref().map(|l| l.queued),
        // every line still in the queue, by where it stands: these are
        // what the card's bound counts and what a close waits for
        "activeLines": lines.as_ref().map(|l| json!({
            "queued": l.queued,
            "sending": l.sending,
            "uncertain": l.uncertain,
        })),
        "reports": row.reports,
        // each line still in the queue, then the newest settled ones: its
        // state, how it goes (`queued` after the card is quiet, `interject`
        // without waiting for that, `start` the card's first line), whether
        // it can be canceled now and, while queued, where it waits
        "sends": lines
            .as_ref()
            .map(|l| l.lines.iter().map(|line| line_view(line, row.mode_of(&line.id))).collect::<Vec<_>>())
            .unwrap_or_default(),
    })
}

impl WorkerRow {
    /// How line `id` goes: the card's first line, an interjected one or a
    /// queued one.
    fn mode_of(&self, id: &str) -> &'static str {
        if id == self.worker.operation_id {
            "start"
        } else if self.interjected.contains(id) {
            INTERJECT_MODE
        } else {
            "queued"
        }
    }
}

/// What a listing proved about a card's session (`liveness` in the views).
fn liveness(listed: bool, now: Option<Seen>) -> &'static str {
    match (listed, now) {
        (false, _) => "unknown",
        (true, Some(_)) => "alive",
        (true, None) => "gone",
    }
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
        "quota": quota_view(&rows),
        "observed": seen.is_some(),
        // this Deck process's start (epoch seconds): a card whose
        // foreground.startedAt is older outlived a Deck restart
        "deckStartedAt": (deck_started_at() > 0).then(deck_started_at),
        "mayReadReplies": replies,
        "workerTypes": types,
        "nextSendSequence": next,
        "laneEpoch": lane_epoch(runtime, client_id),
    }))
}

/// The usage windows of the accounts the caller's own card and its listed
/// worker cards run under (`quota.rs` `projection`): deduplicated by
/// account, presentation only — nothing here reads it for a decision. The
/// accounts no such card runs under now are `deck_quota_snapshot`'s
/// (`quota.rs` in this folder, its own choice).
fn quota_view(rows: &[WorkerRow]) -> Vec<Value> {
    crate::quota::projection(&quota_cards_of(rows))
}

/// The cards a caller with the worker choice sees, as `quota.rs` takes
/// them (session name → card id): its listed worker cards (`rows_of`,
/// `origin::sees`) and its own calling card. `deck_quota_snapshot` names
/// these and no other card.
pub(super) fn quota_cards(
    doc: &DiskDoc,
    client_id: &str,
    orchestrator: Option<&str>,
) -> std::collections::HashMap<String, String> {
    quota_cards_of(&rows_of(doc, client_id, orchestrator))
}

fn quota_cards_of(rows: &[WorkerRow]) -> std::collections::HashMap<String, String> {
    let mut cards: std::collections::HashMap<String, String> = rows
        .iter()
        .filter_map(|row| {
            Some((
                row.target.as_ref()?.session.clone(),
                row.worker.card_id.clone(),
            ))
        })
        .collect();
    if let Some(own) = super::origin::orchestrator() {
        if let Some(target) =
            crate::documents::card_targets(false).and_then(|targets| targets.get(&own).cloned())
        {
            cards.insert(target.session, own);
        }
    }
    cards
}

/// What a `deck_worker_wait` returns for (`until`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitUntil {
    /// Any of: a new report (when the caller named its cursor), another
    /// episode, the card taken over, exited or gone (the original wait).
    Change,
    /// A new report, or the card leaving the caller's reach (taken over,
    /// exited, gone); another episode alone does not end it.
    Report,
}

/// The longest one backend wait blocks. A longer wait is the adapter's: it
/// strings bounded waits together inside one tool call
/// (`mcp-adapter` `long_wait`), so no connection is held longer than this
/// and the caller's identity is proven from the kernel again for each.
pub(super) const WORKER_WAIT_CHUNK_MS: u64 = 20_000;
/// Worker waits that may block at once, all clients together: well under
/// `MAX_CONNECTIONS`, so waiting orchestrators never take the slots a
/// report, a line or a control call needs. Past it a wait that would block
/// answers at once (`reason: "busy"`), and the adapter tries again shortly.
pub(super) const WAIT_SLOTS: usize = 12;
/// Report texts one wait returns inline (`include_reports`), all cards
/// together: the rest are left to `deck_worker_reports`.
const INLINE_REPORT_BYTES: usize = 64 * 1024;

static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One blocking wait's slot, given back when it ends.
struct WaitSlot;

impl WaitSlot {
    fn take() -> Option<Self> {
        use std::sync::atomic::Ordering;
        let mut now = WAITING.load(Ordering::SeqCst);
        loop {
            if now >= WAIT_SLOTS {
                return None;
            }
            match WAITING.compare_exchange(now, now + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(WaitSlot),
                Err(seen) => now = seen,
            }
        }
    }
}

impl Drop for WaitSlot {
    fn drop(&mut self) {
        WAITING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Why one asked card ends a wait, if it does: `report`, `taken-over`,
/// `exited`, `gone` or (only `WaitUntil::Change`) `episode`.
fn wait_reason(
    until: WaitUntil,
    asked: &WorkerSeen,
    view: Option<&Value>,
    observed: bool,
) -> Option<&'static str> {
    let Some(v) = view else {
        return Some("gone");
    };
    let cursor = match until {
        WaitUntil::Change => asked.reports,
        // a report wait without a cursor waits for any report
        WaitUntil::Report => Some(asked.reports.unwrap_or(0)),
    };
    if cursor.is_some_and(|seen| v["reports"]["latest"].as_u64() != Some(seen)) {
        return Some("report");
    }
    if v["takenOver"] == true {
        return Some("taken-over");
    }
    // an unread pane listing proves nothing: a report wait does not take
    // it for an exit (the original wait still does)
    if v["alive"] == false && (until == WaitUntil::Change || observed) {
        return Some("exited");
    }
    if until == WaitUntil::Change && v["agent"]["episode"].as_u64() != Some(asked.episode) {
        return Some("episode");
    }
    None
}

pub(super) fn worker_wait(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerWaitArgs = parse(arguments)?;
    let until = match args.until.as_deref() {
        None | Some("change") => WaitUntil::Change,
        Some("report") => WaitUntil::Report,
        Some(_) => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "until is `change` or `report`",
                "Use until: report to wait only for reports (and the card leaving your reach).",
            ))
        }
    };
    let wait = args.wait_ms.unwrap_or(DEFAULT_WAIT_MS);
    // one wait may follow every worker card the client may have
    let most = runtime.read(client_capacity).map_err(map_error)?;
    if wait > WORKER_WAIT_CHUNK_MS || args.cards.is_empty() || args.cards.len() > most {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            &format!(
                "cards must name 1 to {most} worker cards and wait_ms be at most {WORKER_WAIT_CHUNK_MS} per Deck call"
            ),
            "Name the cards and episodes deck_worker_list returned; call again after a timeout.",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(wait);
    let mut slot = None;
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
        let events: Vec<(String, &'static str)> = args
            .cards
            .iter()
            .filter_map(|asked| {
                let found = views.iter().find(|v| v["cardId"] == asked.card_id.as_str());
                wait_reason(until, asked, found, seen.is_some())
                    .map(|why| (asked.card_id.clone(), why))
            })
            .collect();
        let changed = !events.is_empty();
        let timed_out = Instant::now() >= deadline;
        // a wait that would block takes a slot; none free answers now
        let busy = !changed && !timed_out && slot.is_none() && {
            slot = WaitSlot::take();
            slot.is_none()
        };
        if changed || timed_out || busy {
            let reason = ["report", "taken-over", "exited", "gone", "episode"]
                .into_iter()
                .find(|r| events.iter().any(|(_, why)| why == r))
                .unwrap_or(if busy { "busy" } else { "timeout" });
            let mut answer = json!({
                "ok": true,
                "changed": changed,
                "reason": reason,
                "events": events
                    .iter()
                    .map(|(card, why)| json!({"cardId": card, "reason": why}))
                    .collect::<Vec<_>>(),
                "workers": views,
                "observed": seen.is_some(),
                "mayReadReplies": replies,
                "workerTypes": types,
                "nextSendSequence": next,
                "laneEpoch": lane_epoch(runtime, client_id),
            });
            if busy {
                answer["retryAfterMs"] = json!(1_000);
            }
            if args.include_reports {
                answer["reportsInline"] = inline_reports(runtime, client_id, &args.cards, replies);
            }
            return Ok(answer);
        }
        wait_change(mark, deadline);
        // at most five listings a second, however busy the other agents are
        std::thread::sleep(
            Duration::from_millis(200).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

/// The calling lane's epoch (`admit.rs`), for the views.
fn lane_epoch(runtime: &Runtime, client_id: &str) -> u64 {
    let orchestrator = super::origin::orchestrator();
    runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .find(|c| c.id == client_id)
                .map(|c| super::admit::epoch(c, orchestrator.as_deref()))
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// The reports newer than each asked card's cursor, for a reader with
/// `allow_replies` only; at most `INLINE_REPORT_BYTES` of them, the rest
/// named as left out. Never marks anything read.
fn inline_reports(
    runtime: &Runtime,
    client_id: &str,
    asked: &[WorkerSeen],
    replies: bool,
) -> Value {
    if !replies {
        return json!({
            "readable": false,
            "note": "This client may not read report texts (the local user's permission to read replies); the counts above are all a wait shows."
        });
    }
    let orchestrator = super::origin::orchestrator();
    let cards = runtime
        .read(|doc| {
            let mut budget = INLINE_REPORT_BYTES;
            asked
                .iter()
                .filter_map(|a| {
                    let worker =
                        own_card(doc, client_id, orchestrator.as_deref(), &a.card_id).ok()?;
                    let newer = super::reports::newer_than(doc, &worker, a.reports.unwrap_or(0));
                    if newer.is_empty() {
                        return None;
                    }
                    let size: usize = newer.iter().map(|r| r.to_string().len()).sum();
                    if size > budget {
                        return Some(json!({"cardId": a.card_id, "omitted": newer.len()}));
                    }
                    budget -= size;
                    Some(json!({"cardId": a.card_id, "reports": newer}))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "readable": true,
        "cards": cards,
        "note": super::reports::REPORT_NOTE,
    })
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

/// A replay of a worker request: its kept operation, or, for one Deck
/// accepted under `lane_epoch` and whose record it retired, the tombstone's
/// answer (`admit.rs`). Asked before every other check.
fn replay_worker(
    runtime: &Runtime,
    client_id: &str,
    orchestrator: Option<&str>,
    request_id: &str,
    hash: &str,
) -> Result<Option<Value>, Value> {
    runtime
        .read(|doc| -> Result<Option<Value>, DeckError> {
            let client = permitted(doc, client_id, Choice::Workers)?;
            let lane = super::admit::lane_view(client, orchestrator, None);
            if let Some(operation) = existing_operation(doc, client_id, request_id, hash)? {
                let mut view = operation_view(operation);
                view["replayed"] = json!(true);
                view["lane"] = lane;
                return Ok(Some(view));
            }
            Ok(
                super::admit::tombstone(doc, client_id, request_id, hash)?.map(|a| {
                    let mut view = super::admit::retired_view(a);
                    view["replayed"] = json!(true);
                    view["lane"] = lane;
                    view
                }),
            )
        })
        .map_err(map_error)?
        .map_err(map_error)
}

/// A write's other answer: a tombstone's view (a success: the request ran
/// once, never again) or a refusal.
fn settled_answer(answer: Value) -> Result<Value, Value> {
    if answer["ok"] == true {
        Ok(answer)
    } else {
        Err(answer)
    }
}

/// An accepted worker request's receipt: its operation and where the lane
/// stands after it (`admit::lane_view`).
fn receipt(
    runtime: &Runtime,
    client_id: &str,
    orchestrator: Option<&str>,
    binding: super::admit::Binding,
    operation: &Operation,
) -> Value {
    let mut view = operation_view(operation);
    if let Ok(Some(lane)) = runtime.read(|doc| {
        doc.config
            .clients
            .iter()
            .find(|c| c.id == client_id)
            .map(|c| super::admit::lane_view(c, orchestrator, Some(binding)))
    }) {
        view["lane"] = lane;
    }
    view
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
        "Queue the line with deck_worker_send if it can wait: for agy, Deck types it only after agy ends its turn (a working agy may be showing a permission question Deck cannot see; such a line waits at `agy-working`, and an agy waiting for the user's answer waits for the user).",
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
    let binding = super::admit::Binding::of(args.send_sequence, args.lane_epoch)?;
    let hash = request_hash(line.tool(), &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    if let Some(replayed) =
        replay_worker(runtime, client_id, orchestrator, &args.request_id, &hash)?
    {
        return Ok(replayed);
    }
    // an interjected line: only from the orchestrator card that started the
    // card, never from no card (the kernel's word, never the client's)
    let caller = match (line, orchestrator) {
        (Line::Interject, None) => return Err(not_interjectable(true)),
        (Line::Interject, Some(card)) => Some(card),
        (Line::Queued, _) => None,
    };
    let (worker, stale) = runtime
        .read(
            |doc| -> Result<Result<(Worker, Option<Value>), Value>, DeckError> {
                let client = permitted(doc, client_id, Choice::Workers)?;
                if !super::origin::lane_room(client, orchestrator) {
                    return Ok(Err(super::origin::no_lane()));
                }
                let stale = super::admit::check(client, orchestrator, binding).err();
                let worker = match caller {
                    Some(caller) => opened_by(doc, client_id, caller, &args.card_id)
                        .cloned()
                        .ok_or_else(|| not_interjectable(false)),
                    None => own_card(doc, client_id, orchestrator, &args.card_id),
                };
                if let Some(open) = open_close(doc, client_id, &args.card_id) {
                    return Ok(Err(worker_closing(&open.operation_id)));
                }
                Ok(worker.map(|w| (w, stale)))
            },
        )
        .map_err(map_error)?
        .map_err(map_error)??;
    if let Some(refusal) = stale {
        return Err(refusal);
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
    let known = runtime
        .read(|doc| known_of(doc, &worker))
        .map_err(map_error)?;
    // a first answer; the queue transaction that adds the row decides
    // (`scheduler::ActiveBound`)
    let active =
        crate::scheduler::worker_lines(&queue.q, &worker.card_id, client_id, &known, &[], None)
            .active();
    if active >= MAX_ACTIVE_PER_CARD {
        return Err(capacity(
            "the card already holds 4 lines from this client that are queued, being sent or of uncertain delivery",
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
            if let Some(a) = super::admit::tombstone(doc, client_id, &args.request_id, &hash)? {
                return Ok(Err(super::admit::retired_view(a)));
            }
            if let Err(refusal) = super::admit::check(client, orchestrator, binding) {
                return Ok(Err(refusal));
            }
            // the card handed back to nobody meanwhile: refused whole
            if let Some(caller) = caller {
                if opened_by(doc, client_id, caller, &worker.card_id).is_none() {
                    return Ok(Err(not_interjectable(false)));
                }
            }
            // a close of the card is under way: a line accepted now would
            // be reported as given to an instance about to go (the close
            // was accepted first, so it decides)
            if let Some(open) = open_close(doc, client_id, &worker.card_id) {
                return Ok(Err(worker_closing(&open.operation_id)));
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
            super::admit::accept(doc, client_id, orchestrator, binding, &operation);
            if let Some(record) = doc
                .workers
                .iter_mut()
                .find(|w| w.card_id == worker.card_id && w.client_id == client_id)
            {
                record.lines.push(operation.operation_id.clone());
                let over = record.lines.len().saturating_sub(LINES_KEPT);
                record.lines.drain(..over);
            }
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)?;
    let operation = match operation {
        Ok(operation) => operation,
        Err(answer) => return settled_answer(answer),
    };
    if operation.state != "accepted" || operation.operation_id != minted {
        return Ok(receipt(
            runtime,
            client_id,
            orchestrator,
            binding,
            &operation,
        ));
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
        Some(crate::scheduler::ActiveBound {
            known: &known,
            max: MAX_ACTIVE_PER_CARD,
        }),
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
                        match error.kind() {
                            ErrorKind::Invalid => "NOT_AN_AGENT_CARD",
                            // the card's bound, decided in the queue's own
                            // transaction (another line took the place)
                            ErrorKind::RequestConflict => "CAPACITY_EXCEEDED",
                            _ => "QUEUE_REFUSED",
                        }
                        .into(),
                    );
                }
            }
            record.updated_at = now_ms();
            Ok(record.clone())
        })
        .map_err(map_error)?;
    Ok(receipt(runtime, client_id, orchestrator, binding, &settled))
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
    close_refusal("worker-taken-over", None, json!(null))
}

fn needs_you_refusal() -> Value {
    close_refusal("worker-needs-you", None, json!(null))
}

/// How a close is bound to what its orchestrator decided on
/// (`close_mode`). Every mode stands on the same shared checks
/// (`shared_verdict`): ownership and the choice, the user's hold, no line
/// of the card still in its queue or accepted and not yet queued, the
/// program in front the one the decision saw (a shell back in front of the
/// same pane: that program ended), a running program's Agent observation
/// known (`OBSERVATION_UNKNOWN` otherwise, in every mode) and not asking the
/// user. A mode only adds what the decision was about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseMode {
    /// The original: the card's attention episode (`after_episode`). Any
    /// other accepted observation since — the turn ending included —
    /// refuses it. `after_episode: 0` is the episode of a card Deck has no
    /// observation of; it passes only where the shared checks do (a
    /// session gone, a shell in front), never past an unknown agent.
    Episode,
    /// Approved 2026-11-28: "I am done with THIS agent instance". Bound to
    /// the program instance in front of the card's pane (`instance`) and to
    /// the newest line accepted for the card (`last_line`): the instance's
    /// own turns (working → turn-done, a new episode) do not refuse it;
    /// another instance (an exit and a new program, a re-entry, a new
    /// session) or a line accepted since the decision do. An instance that
    /// ended (a shell back in front of the same pane, or its session gone)
    /// leaves the card to close.
    Instance,
    /// Approved 2026-11-28: "this card's session is gone; clear its
    /// record". For a card whose `liveness` read `gone` (a listing that
    /// succeeded has no such session; `instance` is then null, and no
    /// token from before is needed). Bound to the newest accepted line
    /// (`last_line`); refused at once, and at the admission, the moment the
    /// session exists again (`SESSION_PRESENT`): it never becomes a close
    /// of whatever runs there now, and the shell exception never applies.
    Gone,
}

impl CloseMode {
    fn word(self) -> &'static str {
        match self {
            CloseMode::Episode => "episode",
            CloseMode::Instance => "instance",
            CloseMode::Gone => "gone",
        }
    }
}

/// What a close's shared checks hold the card to (`shared_verdict`): the
/// program instance in front of the pane when it was decided on, a session
/// that was gone, or nothing (an episode close Deck could not read the
/// instance for, or one accepted by an older build).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decided {
    Instance { pane: u64, instance: u64 },
    Gone,
    Unbound,
}

impl Decided {
    /// What the card is now, as an episode close binds it at its check.
    fn of_now(now: Option<Seen>) -> Self {
        match now {
            None => Decided::Gone,
            Some(seen) if seen.pane != 0 && seen.instance != 0 => Decided::Instance {
                pane: seen.pane,
                instance: seen.instance,
            },
            Some(_) => Decided::Unbound,
        }
    }

    /// What an accepted close's plan says it was decided on.
    fn of_plan(plan: &Value) -> Self {
        let token = |key: &str| {
            plan.get(key)
                .and_then(Value::as_str)
                .and_then(parse_instance)
        };
        if plan.get("decidedGone").and_then(Value::as_bool) == Some(true) {
            return Decided::Gone;
        }
        let key = if plan.get("closeMode").and_then(Value::as_str) == Some("instance") {
            "instance"
        } else {
            "seenInstance"
        };
        match token(key) {
            Some((pane, instance)) => Decided::Instance { pane, instance },
            None => Decided::Unbound,
        }
    }

    /// What the plan records of it.
    fn record(self, plan: &mut Value) {
        match self {
            Decided::Gone => plan["decidedGone"] = json!(true),
            Decided::Instance { pane, instance } => {
                plan["seenInstance"] = json!(format!("{pane:016x}{instance:016x}"))
            }
            Decided::Unbound => plan["seenInstance"] = json!(null),
        }
    }
}

/// The checks every close mode stands on, at its check and again at its
/// admission: no line of the card still in its queue (`lines`: queued,
/// being sent or uncertain; `None` when the queue could not be read) or
/// accepted and not yet queued (`in_flight`); the session and the program
/// in front the ones `decided` names (a session that was gone must still
/// be; a program that ended, its session gone or its pane's own shell back
/// in front, leaves the card to close); a running program's Agent
/// observation known and not asking the user. `now` is the card's session
/// in a listing that succeeded (`None`: no such session).
fn shared_verdict(
    decided: Decided,
    now: Option<Seen>,
    lines: Option<&crate::scheduler::WorkerLines>,
    in_flight: bool,
) -> Result<(), &'static str> {
    match lines {
        None => return Err("lines-unknown"),
        Some(lines) if lines.active() > 0 => return Err("lines-pending"),
        Some(_) if in_flight => return Err("lines-pending"),
        Some(_) => {}
    }
    if decided == Decided::Gone {
        return match now {
            Some(_) => Err("session-present"),
            None => Ok(()),
        };
    }
    let Some(now) = now else {
        // the session left a listing that succeeded: its program ended
        return Ok(());
    };
    if let Decided::Instance { pane, instance } = decided {
        if now.pane == 0 || now.instance == 0 {
            return Err("instance-unknown");
        }
        if now.pane != pane {
            return Err("instance-changed");
        }
        if now.instance != instance {
            // the same pane's own shell back in front (the pane key names
            // that shell's process): the instance ended; anything else in
            // front is another program
            return if now.shell {
                Ok(())
            } else {
                Err("instance-changed")
            };
        }
    }
    if !now.shell && now.state.is_none() {
        return Err("observation-unknown");
    }
    if now.state == Some("needs-input") {
        return Err("worker-needs-you");
    }
    Ok(())
}

/// A line accepted for `card` whose queueing has not settled yet (its
/// operation still `accepted`): work the queue does not show yet, which a
/// close must not overtake.
fn line_in_flight(doc: &DiskDoc, client: &str, card: &str) -> bool {
    doc.operations.iter().any(|o| {
        o.client_id == client
            && o.kind == WORKER_SEND_KIND
            && o.state == "accepted"
            && o.result
                .as_ref()
                .and_then(|r| r.get("cardId"))
                .and_then(Value::as_str)
                == Some(card)
    })
}

/// One closed refusal word of a close (lowercase: the operation's code
/// once accepted; `close_refusal` gives its uppercase tool code), with what
/// the caller is told and should do.
fn close_words(code: &str) -> (&'static str, &'static str, &'static str) {
    match code {
        "episode-moved" => (
            "EPISODE_MOVED",
            "the card's Agent observation is no longer the episode this close was decided on; nothing was closed",
            "Read the card again (deck_worker_wait or deck_worker_list) and decide on its current state; to close once you are done with this agent whatever its turns do meanwhile, close with close_mode: instance and the card's instance and lastLine.",
        ),
        "instance-changed" => (
            "INSTANCE_CHANGED",
            "another program instance is in front of the card than the one this close was decided on (the agent was restarted or re-entered, or the session is a new one); nothing was closed",
            "Read the card again (deck_worker_list) and decide on the instance it runs now; that is a new decision with a new request id.",
        ),
        "instance-unknown" => (
            "INSTANCE_UNKNOWN",
            "Deck cannot read which program instance is in front of the card's pane right now, so it cannot prove it is the one this close was decided on; nothing was closed",
            "Read the card again in a moment (deck_worker_list instance) and close with what it says.",
        ),
        "observation-unknown" => (
            "OBSERVATION_UNKNOWN",
            "a program is still running in front of the card and Deck has no Agent observation of it (for instance after a Deck restart, or before its first observed turn), so it cannot prove the agent is not asking the user something; no close mode passes this; nothing was closed",
            "Wait until Deck observes the agent's next turn (deck_worker_wait until: change) and close then, or leave the card to the user (they can close it). Once the agent program ends (its pane back at the shell, or its session gone) the card can be closed.",
        ),
        "new-input" => (
            "NEW_INPUT",
            "a line was accepted for the card after the one this close was decided on: that is work the decision did not cover; nothing was closed",
            "Read the card again (deck_worker_list lastLine and sends) and decide whether you are still done with it.",
        ),
        "lines-pending" => (
            "LINES_PENDING",
            "the card still holds lines of yours that are queued, being sent, of uncertain delivery, or accepted and not yet queued; closing now would drop or overtake work the decision did not see settled; no close mode passes this; nothing was closed",
            "Read the card's sends (deck_worker_list): cancel a queued line you no longer want (deck_worker_cancel with its operationId) or wait until it is delivered, then decide again. A line of uncertain delivery is the user's to resolve in Deck's queue panel.",
        ),
        "lines-unknown" => (
            "LINES_UNKNOWN",
            "Deck could not read the card's queue, so it cannot prove no line of yours still waits there; nothing was closed",
            "Read the card again in a moment (deck_worker_list) and close with a new request.",
        ),
        "session-present" => (
            "SESSION_PRESENT",
            "the card's session exists (again): a close for a gone session never closes what runs there now; nothing was closed",
            "Read the card again (deck_worker_list): close what runs there by its instance (close_mode: instance) or its episode, which the shared checks decide on.",
        ),
        "worker-needs-you" => (
            "WORKER_NEEDS_YOU",
            "the card's Agent is asking the local user something: that question is theirs to answer",
            "Leave the card to the user; you may close it once its turn has ended.",
        ),
        "worker-taken-over" => (
            "WORKER_TAKEN_OVER",
            "the user took this card over: it is theirs until they hand it back",
            "Leave it to the user; once they hand it back you may close it.",
        ),
        "client-revoked" => (
            "PERMISSION_DENIED",
            "this client may no longer drive worker cards",
            "Nothing more can be closed; the local user withdrew the choice.",
        ),
        "worker-unobserved" => (
            "WORKER_UNOBSERVED",
            "Deck could not read the card's terminal when it came to close it",
            "Read the card again and close with a new request.",
        ),
        "close-in-progress" => (
            "CLOSE_IN_PROGRESS",
            "a close of this card is already under way; this one would close nothing more",
            "Follow that close (deck_operation_get with operationId); repeat it with its own request id when its answer was lost.",
        ),
        _ => (
            "WORKER_NOT_FOUND",
            "the card is no longer a worker card this orchestrator card started",
            "Read deck_worker_list again; a card already closed needs nothing more.",
        ),
    }
}

/// A refused close as the caller reads it, at the first check (nothing
/// accepted): the closed code, what Deck observed when it refused, and
/// what to do. A snapshot: it promises nothing about a later attempt.
fn close_refusal(code: &str, current_episode: Option<u64>, observed: Value) -> Value {
    let (upper, message, next) = close_words(code);
    let mut error = json!({
        "code": upper,
        "message": message,
        "nextAction": next,
        "stage": "precheck",
        "accepted": false,
    });
    if let Some(episode) = current_episode {
        error["currentEpisode"] = json!(episode);
    }
    if !observed.is_null() {
        error["observed"] = observed;
    }
    json!({"ok": false, "error": error})
}

/// The refusal an accepted close is rejected with at its admission
/// (`close_still_admissible`), kept in the operation's result
/// (`deck_operation_get` `result.refusal`) beside its code.
fn admission_refusal(code: &str, observed: Value) -> Value {
    let (upper, message, next) = close_words(code);
    let mut refusal = json!({
        "stage": "admission",
        "accepted": true,
        "code": upper,
        "message": message,
        "nextAction": next,
        "observed": observed,
    });
    if code == "episode-moved" {
        refusal["currentEpisode"] = observed["episode"].clone();
    }
    refusal
}

/// What Deck observed of a card when it decided on a close: its episode
/// and state, whether the listing has its session, the instance in front
/// of its pane, its newest accepted line and its lines still in the queue
/// (each null when unknown).
fn observed_view(
    now: Option<Seen>,
    listed: bool,
    worker: Option<&Worker>,
    lines: Option<&crate::scheduler::WorkerLines>,
) -> Value {
    json!({
        "episode": now.map(|s| s.episode),
        "state": match now {
            Some(Seen { state: Some(state), .. }) => state,
            Some(_) => "unavailable",
            None if listed => "gone",
            None => "unknown",
        },
        "liveness": liveness(listed, now),
        "instance": now.as_ref().and_then(instance_token),
        "lastLine": worker.and_then(|w| w.lines.last()),
        "unsent": lines.map(|l| l.queued),
        "activeLines": lines.map(|l| json!({
            "queued": l.queued,
            "sending": l.sending,
            "uncertain": l.uncertain,
        })),
    })
}

/// The worker card's lines from its client still in the queue (its start's
/// head row and its lines; `scheduler::worker_lines`), with whether one was
/// accepted and is not queued yet. `None` when the queue cannot be read.
pub(super) fn close_lines(
    runtime: &Runtime,
    worker: &Worker,
) -> Option<(crate::scheduler::WorkerLines, bool)> {
    #[cfg(test)]
    if let Some(forced) = UNSENT_FOR_TESTS.with(std::cell::Cell::get) {
        return forced.map(|n| {
            (
                crate::scheduler::WorkerLines {
                    queued: n,
                    ..Default::default()
                },
                false,
            )
        });
    }
    let (known, in_flight) = runtime
        .read(|doc| {
            (
                known_of(doc, worker),
                line_in_flight(doc, &worker.client_id, &worker.card_id),
            )
        })
        .ok()?;
    #[cfg(test)]
    if runtime.app.is_none() {
        // a unit test's runtime has no queue: an empty one, unless the test
        // names its own (`UNSENT_FOR_TESTS`, `Some(None)` = unreadable)
        return Some((crate::scheduler::WorkerLines::default(), in_flight));
    }
    lines_of(runtime, worker, &known, &[], None).map(|lines| (lines, in_flight))
}

/// What one close was decided on: its mode, the episode (episode mode),
/// the newest line (instance and gone modes) and what the shared checks
/// hold the card to.
struct Decision<'a> {
    mode: CloseMode,
    episode: u64,
    last_line: Option<&'a String>,
    decided: Decided,
}

/// Whether a close holds for the card as it is now (`Err`: the refusal
/// word): the mode's own binding first, then the shared checks.
fn close_verdict(
    decision: &Decision<'_>,
    worker: &Worker,
    now: Option<Seen>,
    lines: Option<&crate::scheduler::WorkerLines>,
    in_flight: bool,
) -> Result<(), &'static str> {
    match decision.mode {
        CloseMode::Episode => {
            if now.map_or(0, |s| s.episode) != decision.episode {
                return Err("episode-moved");
            }
        }
        CloseMode::Instance | CloseMode::Gone => {
            if worker.lines.last() != decision.last_line {
                return Err("new-input");
            }
        }
    }
    shared_verdict(decision.decided, now, lines, in_flight)
}

/// A line refused because a close of its card was accepted first.
fn worker_closing(close: &str) -> Value {
    json!({"ok": false, "error": {
        "code": "WORKER_CLOSING",
        "message": "a close of this card was accepted before this line; nothing was queued",
        "nextAction": "Follow the close (deck_operation_get with closeOperationId): once it is rejected the card takes lines again; once committed the card is gone.",
        "closeOperationId": close,
    }})
}

/// The worker-close operation still under way for `card` (accepted,
/// executing or admitted), if any: its card takes no further line or close.
pub(super) fn open_close<'a>(doc: &'a DiskDoc, client: &str, card: &str) -> Option<&'a Operation> {
    doc.operations.iter().find(|o| {
        o.client_id == client
            && o.kind == WORKER_CLOSE_KIND
            && matches!(o.state.as_str(), "accepted" | "executing" | "admitted")
            && o.result
                .as_ref()
                .and_then(|r| r.get("cardId"))
                .and_then(Value::as_str)
                == Some(card)
    })
}

/// The worker card `card` of `client` that orchestrator card `caller`
/// started (`Worker.orchestrator`, read from the kernel at its start):
/// never another card's, an unowned one, or any other card.
pub(super) fn opened_by<'a>(
    doc: &'a DiskDoc,
    client: &str,
    caller: &str,
    card: &str,
) -> Option<&'a Worker> {
    doc.workers.iter().find(|w| {
        w.card_id == card && w.client_id == client && w.orchestrator.as_deref() == Some(caller)
    })
}

/// Whether a planned close may still go ahead, read again where it is
/// admitted (`commands::close_admit`, inside the webview's Board
/// transaction, under the delivery lock): still this client's choice,
/// still the card the planning orchestrator card started (the same record:
/// `workerOperationId`) and nobody took over, its Agent not asking the
/// user, and — by the close's mode — its episode the one decided on, or its
/// instance and lines the ones decided on (`close_verdict`). This is
/// the close's linearization point: what changes before it decides the
/// outcome; once it admits, the card is closed through the webview's
/// ordinary close, which may still refuse for the user's own reasons
/// (shown, a Reminder, a kept scratchpad). `Err` is the closed code the
/// operation is rejected with and what Deck observed (`admission_refusal`).
pub(super) fn close_still_admissible(
    doc: &DiskDoc,
    operation: &Operation,
    targets: Option<&HashMap<String, crate::documents::CardTarget>>,
    seen: Option<&HashMap<String, Seen>>,
    lines: Option<&crate::scheduler::WorkerLines>,
) -> Result<(), (&'static str, Value)> {
    let bare = |code| (code, json!(null));
    let plan = operation.result.as_ref().ok_or(bare("worker-close-plan"))?;
    let text = |key: &str| plan.get(key).and_then(Value::as_str).unwrap_or_default();
    if permitted(doc, &operation.client_id, Choice::Workers).is_err() {
        return Err(bare("client-revoked"));
    }
    let worker = opened_by(
        doc,
        &operation.client_id,
        text("orchestratorCardId"),
        text("cardId"),
    )
    .filter(|w| {
        plan.get("workerOperationId")
            .and_then(Value::as_str)
            .is_none_or(|id| id == w.operation_id)
    })
    .ok_or(bare("worker-not-found"))?;
    if super::origin::taken_over(worker, targets) {
        return Err(bare("worker-taken-over"));
    }
    let seen = seen.ok_or(bare("worker-unobserved"))?;
    let now = seen.get(text("session")).copied();
    let observed = || observed_view(now, true, Some(worker), lines);
    let mode = match text("closeMode") {
        "instance" => CloseMode::Instance,
        "gone" => CloseMode::Gone,
        _ => CloseMode::Episode,
    };
    if mode == CloseMode::Instance && Decided::of_plan(plan) == Decided::Unbound {
        return Err(bare("worker-close-plan"));
    }
    let last_line = plan
        .get("lastLine")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let in_flight = line_in_flight(doc, &operation.client_id, &worker.card_id);
    close_verdict(
        &Decision {
            mode,
            episode: plan
                .get("afterEpisode")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            last_line: last_line.as_ref(),
            decided: Decided::of_plan(plan),
        },
        worker,
        now,
        lines,
        in_flight,
    )
    .map_err(|code| (code, observed()))
}

/// The refusal an admission stores in its operation's result.
pub(super) fn admission_record(code: &str, observed: Value) -> Value {
    admission_refusal(code, observed)
}

/// The tmux session an admitted worker close may end (`validate_close_admission`).
pub(super) fn close_session(operation: &Operation) -> Option<&str> {
    operation.result.as_ref()?.get("session")?.as_str()
}

/// The worker card a worker-close operation names, for its admission.
pub(super) fn close_worker(doc: &DiskDoc, operation: &Operation) -> Option<Worker> {
    let plan = operation.result.as_ref()?;
    let text = |key: &str| plan.get(key).and_then(Value::as_str).unwrap_or_default();
    opened_by(
        doc,
        &operation.client_id,
        text("orchestratorCardId"),
        text("cardId"),
    )
    .cloned()
}

/// `deck_worker_close`: the orchestrator card that started a worker card
/// asks Deck to close it. Refused unless the call comes from that very
/// card (the kernel's word, `origin.rs`; a caller from no card closes
/// nothing), the card is on the current Board, the user has not taken it
/// over and its Agent is not asking the user anything (an observation may
/// refuse a close, never release one), and the decision still holds by
/// its mode (`CloseMode`): the episode it named, or the instance and the
/// lines it named. Bound to the client's send sequence or lane epoch
/// exactly as a start or a line; journaled as a `worker-close` Board
/// intent the webview's one close commits or refuses (`mcp.js`
/// `closeWorker`: never while a pane shows the card, with a Reminder or a
/// kept scratchpad). From its acceptance until it ends, the card takes no
/// further line (`WORKER_CLOSING`) and no second close
/// (`CLOSE_IN_PROGRESS`); its admission decides again with the same rules
/// (`close_still_admissible`).
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
    let mode = match args.close_mode.as_deref() {
        None | Some("episode") => CloseMode::Episode,
        Some("instance") => CloseMode::Instance,
        Some("gone") => CloseMode::Gone,
        Some(_) => {
            return Err(error_value(
                "INVALID_ARGUMENTS",
                "close_mode is `episode`, `instance` or `gone`",
                "Use close_mode: instance with the card's instance and lastLine, close_mode: gone with its lastLine for a card whose liveness is gone, or close_mode: episode (the default) with after_episode.",
            ))
        }
    };
    let bound = match mode {
        CloseMode::Episode => {
            if args.after_episode.is_none() || args.instance.is_some() || args.last_line.is_some() {
                return Err(error_value(
                    "INVALID_ARGUMENTS",
                    "close_mode episode takes after_episode, and neither instance nor last_line",
                    "Name the card's agent.episode as after_episode, or use close_mode: instance with instance and last_line.",
                ));
            }
            None
        }
        CloseMode::Instance => {
            let parsed = args.instance.as_deref().and_then(parse_instance);
            if args.after_episode.is_some()
                || parsed.is_none()
                || args.last_line.as_deref().is_some_and(|l| !valid_id(l))
            {
                return Err(error_value(
                    "INVALID_ARGUMENTS",
                    "close_mode instance takes instance (and last_line when the card has one), never after_episode",
                    "Copy the card's instance and lastLine from deck_worker_list or deck_worker_wait; leave last_line out when lastLine is null. A card whose instance is null cannot be closed this way: if its liveness is gone, use close_mode: gone.",
                ));
            }
            parsed.map(|(pane, instance)| Decided::Instance { pane, instance })
        }
        CloseMode::Gone => {
            if args.after_episode.is_some()
                || args.instance.is_some()
                || args.last_line.as_deref().is_some_and(|l| !valid_id(l))
            {
                return Err(error_value(
                    "INVALID_ARGUMENTS",
                    "close_mode gone takes last_line when the card has one, and neither after_episode nor instance",
                    "Copy the card's lastLine from deck_worker_list (leave it out when null); use this mode only for a card whose liveness reads gone.",
                ));
            }
            Some(Decided::Gone)
        }
    };
    let binding = super::admit::Binding::of(args.send_sequence, args.lane_epoch)?;
    let hash = request_hash("deck_worker_close", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    if let Some(replayed) =
        replay_worker(runtime, client_id, orchestrator, &args.request_id, &hash)?
    {
        return Ok(replayed);
    }
    let Some(caller) = orchestrator else {
        return Err(not_opened_here(true));
    };
    let (worker, stale) = runtime
        .read(
            |doc| -> Result<Result<(Worker, Option<Value>), Value>, DeckError> {
                let client = permitted(doc, client_id, Choice::Workers)?;
                if !super::origin::lane_room(client, orchestrator) {
                    return Ok(Err(super::origin::no_lane()));
                }
                let stale = super::admit::check(client, orchestrator, binding).err();
                Ok(opened_by(doc, client_id, caller, &args.card_id)
                    .cloned()
                    .map(|w| (w, stale))
                    .ok_or_else(|| not_opened_here(false)))
            },
        )
        .map_err(map_error)?
        .map_err(map_error)??;
    if let Some(refusal) = stale {
        return Err(refusal);
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
    // what the close holds the card to: the instance or gone session it
    // names, or — an episode close — what is in front now
    let decided = bound.unwrap_or_else(|| Decided::of_now(now));
    let lines = close_lines(runtime, &worker);
    if let Err(code) = close_verdict(
        &Decision {
            mode,
            episode: args.after_episode.unwrap_or(0),
            last_line: args.last_line.as_ref(),
            decided,
        },
        &worker,
        now,
        lines.as_ref().map(|(l, _)| l),
        lines.as_ref().is_some_and(|(_, f)| *f),
    ) {
        let current = (code == "episode-moved").then(|| now.map_or(0, |s| s.episode));
        return Err(match code {
            "worker-needs-you" => needs_you_refusal(),
            _ => close_refusal(
                code,
                current,
                observed_view(now, true, Some(&worker), lines.as_ref().map(|(l, _)| l)),
            ),
        });
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
            if let Some(a) = super::admit::tombstone(doc, client_id, &args.request_id, &hash)? {
                return Ok(Err(super::admit::retired_view(a)));
            }
            if let Err(refusal) = super::admit::check(client, orchestrator, binding) {
                return Ok(Err(refusal));
            }
            // taken over meanwhile: refused whole
            let record = match opened_by(doc, client_id, caller, &worker.card_id) {
                None => return Ok(Err(not_opened_here(false))),
                Some(w) if super::origin::taken_over(w, targets.as_ref()) => {
                    return Ok(Err(taken_over_refusal()))
                }
                Some(w) => w,
            };
            // one close of a card at a time: a second would close nothing
            // more, and its own answer would say less than the first's
            if let Some(open) = open_close(doc, client_id, &worker.card_id) {
                let mut refusal = close_refusal("close-in-progress", None, json!(null));
                refusal["error"]["operationId"] = json!(open.operation_id);
                return Ok(Err(refusal));
            }
            // a line accepted since the check above is work the decision
            // did not see (instance and gone modes bind the newest line;
            // every mode refuses one still being queued)
            if mode != CloseMode::Episode && record.lines.last() != args.last_line.as_ref() {
                return Ok(Err(close_refusal(
                    "new-input",
                    None,
                    observed_view(now, true, Some(record), None),
                )));
            }
            if line_in_flight(doc, client_id, &worker.card_id) {
                return Ok(Err(close_refusal(
                    "lines-pending",
                    None,
                    observed_view(now, true, Some(record), None),
                )));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let mut plan = json!({
                "cardId": worker.card_id,
                "session": target.session,
                "projectId": target.project_id,
                "title": target.title,
                "orchestratorCardId": caller,
                // the record the close is about: a card id is never reused,
                // and its start operation says which worker it was
                "workerOperationId": worker.operation_id,
                "closeMode": mode.word(),
            });
            match mode {
                CloseMode::Episode => {
                    plan["afterEpisode"] = json!(args.after_episode.unwrap_or(0));
                    decided.record(&mut plan);
                }
                CloseMode::Instance => {
                    plan["instance"] = json!(args.instance);
                    plan["lastLine"] = json!(args.last_line);
                }
                CloseMode::Gone => {
                    plan["lastLine"] = json!(args.last_line);
                    decided.record(&mut plan);
                }
            }
            let operation = Operation {
                result: Some(plan),
                ..Operation::new(
                    minted.clone(),
                    client_id,
                    args.request_id.clone(),
                    hash.clone(),
                    WORKER_CLOSE_KIND,
                    "accepted",
                )
            };
            super::admit::accept(doc, client_id, orchestrator, binding, &operation);
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)?;
    let operation = match operation {
        Ok(operation) => operation,
        Err(answer) => return settled_answer(answer),
    };
    emit_changed(runtime);
    let mut view = receipt(runtime, client_id, orchestrator, binding, &operation);
    view["nextAction"] = json!("Accepted, not closed yet: deck_operation_get with this operationId says committed once the card is gone, or rejected with a code (result.refusal says at which stage and what Deck observed). Until then the card takes no further line or close.");
    Ok(view)
}

/* ---------- one line: its delivery, and canceling it ---------- */

/// A worker line's cancel request (`deck_worker_cancel`).
pub(super) const WORKER_CANCEL_KIND: &str = "worker-cancel";

/// The worker card a line operation is about, as the caller may see it:
/// the client's own worker (`own_card`'s reach), its known operations, and
/// whether the operation is one of its lines — known to the ledger, or a
/// queue row whose delegation is the client's.
fn line_owner(
    runtime: &Runtime,
    client_id: &str,
    orchestrator: Option<&str>,
    card: &str,
) -> Result<(Worker, Vec<String>), Value> {
    runtime
        .read(|doc| {
            own_card(doc, client_id, orchestrator, card).map(|w| {
                let known = known_of(doc, &w);
                (w, known)
            })
        })
        .map_err(map_error)?
}

/// The `delivery` of one worker line (`deck_operation_get`, the cancel's
/// answer): where the queue says it stands, how it goes, whether it can be
/// canceled now and, while queued and observed, where it waits. `ledger` is
/// the line's MCP operation when the ledger still keeps it: `accepted`
/// (not queued yet) and `rejected` (never queued) say so; a line the queue
/// no longer holds is `unknown`, never guessed `canceled` or `delivered`.
/// `delivered` says the text was handed to the pane, never that the agent
/// did anything with it.
fn delivery_view(
    runtime: &Runtime,
    worker: &Worker,
    known: &[String],
    id: &str,
    ledger: Option<&Operation>,
    mode: &str,
) -> Value {
    let observed = observe_for_views();
    let plans = observed.as_ref().map(|(_, plans)| plans);
    let line = queues(runtime).and_then(|q| {
        crate::scheduler::worker_line_of(&q.q, &worker.card_id, &worker.client_id, known, id, plans)
    });
    let mut view = match (&line, ledger.map(|o| o.state.as_str())) {
        (Some(line), _) => line_view(line, mode),
        (None, Some("accepted")) => {
            json!({"operationId": id, "state": "not-queued-yet", "mode": mode, "cancelable": false})
        }
        (None, Some("rejected")) => {
            json!({"operationId": id, "state": "not-queued", "mode": mode, "cancelable": false})
        }
        (None, _) => {
            json!({"operationId": id, "state": "unknown", "mode": mode, "cancelable": false})
        }
    };
    view["cardId"] = json!(worker.card_id);
    view["observed"] = json!(observed.is_some());
    view
}

/// `deck_operation_get` for a worker start, line or cancel: the operation's
/// own view (its `state` is the MCP decision: `committed` means the request
/// was carried out — a line queued, never delivered), plus the line's
/// `delivery`. With no ledger record (retired), the queue's own operation
/// of that id on one of the caller's worker cards still answers its
/// delivery, `record: retired`. `None`: not a worker line of this caller.
pub(super) fn worker_operation_view(
    runtime: &Runtime,
    client_id: &str,
    operation_id: &str,
    ledger: Option<&Operation>,
) -> Option<Value> {
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    let card = match ledger {
        Some(o) if matches!(o.kind.as_str(), WORKER_SEND_KIND | WORKER_START_KIND) => o
            .result
            .as_ref()
            .and_then(|r| r.get("cardId"))
            .and_then(Value::as_str)
            .map(str::to_owned)?,
        Some(_) => return None,
        None => {
            queues(runtime).and_then(|q| crate::scheduler::operation_card(&q.q, operation_id))?
        }
    };
    let (worker, known) = line_owner(runtime, client_id, orchestrator, &card).ok()?;
    let mode = match ledger {
        Some(o) if o.kind == WORKER_START_KIND => "start",
        Some(o) if is_interjected(o) => INTERJECT_MODE,
        Some(_) => "queued",
        None if operation_id == worker.operation_id => "start",
        None => "unknown",
    };
    let delivery = delivery_view(runtime, &worker, &known, operation_id, ledger, mode);
    if ledger.is_none() && delivery["state"] == "unknown" {
        // neither the ledger nor the queue holds it as this caller's line
        return None;
    }
    let mut view = match ledger {
        Some(o) => operation_view(o),
        None => json!({"ok": true, "operationId": operation_id, "record": "retired"}),
    };
    view["delivery"] = delivery;
    Some(view)
}

/// What a cancel's answer tells the caller to do next.
fn cancel_next(outcome: crate::scheduler::CancelOutcome) -> &'static str {
    use crate::scheduler::CancelOutcome::*;
    match outcome {
        Canceled | AlreadyCanceled => "Canceled: the line will not be sent. To send what it said another way (deck_worker_interject), use a NEW request_id and follow that new operation's delivery (deck_operation_get).",
        Delivered => "The line was already delivered: nothing was taken back. Do not send the same task again; read the card (deck_worker_wait, deck_worker_reply) to see what the agent does with it.",
        Sending => "The line is being sent right now and cannot be canceled. Follow it (deck_operation_get with targetOperationId) until it reads delivered or uncertain; do not send the same task again meanwhile.",
        Uncertain => "Deck cannot tell whether the line went: it stays for the user to resolve in Deck's queue panel. Do not send the same task again on your own.",
        UserEdited => "The user edited this line: it is theirs now and you cannot cancel it.",
        NotFound => "Deck's queue holds no such line of yours on that card (never queued, or retired long ago): nothing changed. Read deck_worker_list sends.",
    }
}

/// `deck_worker_cancel`: cancel ONE line of a worker card the caller drives
/// (`own_card`'s reach), named by the operation its `deck_worker_send` or
/// `deck_worker_interject` (or the start's first line) returned, while it
/// still waits in the queue. Decided in the queue's own transaction under
/// the lock the firing intent takes (`scheduler::worker_cancel`): a line is
/// canceled before its firing intent and never starts, or it is found
/// sending, delivered or uncertain and left exactly as it is (nothing is
/// taken back, an uncertain line is never guessed unsent). A line the user
/// edited, or a card the user took over, is theirs. It cancels that line
/// only: never the card, its start or its other lines. The request has its
/// own `request_id`: a repeat answers its recorded outcome; after a lost
/// answer the queue answers again (`already-canceled`). Never resends.
pub(super) fn worker_cancel(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerCancelArgs = parse(arguments)?;
    if !valid_id(&args.request_id) || !valid_id(&args.card_id) || !valid_id(&args.operation_id) {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id, card id or operation id is out of shape",
            "Name the cardId and the line's operationId from deck_worker_list sends, with a fresh request id.",
        ));
    }
    let hash = request_hash("deck_worker_cancel", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    if let Some(replayed) =
        replay_worker(runtime, client_id, orchestrator, &args.request_id, &hash)?
    {
        return Ok(replayed);
    }
    let (worker, known) = line_owner(runtime, client_id, orchestrator, &args.card_id)?;
    let targets = crate::documents::card_targets(false);
    if super::origin::taken_over(&worker, targets.as_ref()) {
        return Err(close_refusal("worker-taken-over", None, json!(null)));
    }
    let queue = queues(runtime).ok_or_else(|| {
        map_error(DeckError::new(
            ErrorKind::ContextChanged,
            "Deck's queue is not available",
        ))
    })?;
    // only a line of the caller's: known to the ledger, or a row whose
    // delegation is the caller's (the queue decides the rest)
    let line = crate::scheduler::worker_line_of(
        &queue.q,
        &worker.card_id,
        client_id,
        &known,
        &args.operation_id,
        None,
    );
    if line.is_some()
        && !known.contains(&args.operation_id)
        && !line.as_ref().is_some_and(|l| l.cancelable)
    {
        return Err(error_value(
            "LINE_NOT_FOUND",
            "that operation is not one of your lines on this card",
            "Name an operationId from this card's deck_worker_list sends.",
        ));
    }
    let outcome = crate::scheduler::worker_cancel(
        &queue,
        runtime.app.as_ref(),
        &worker.card_id,
        client_id,
        &args.operation_id,
    )
    .map_err(|_| {
        error_value(
            "CANCEL_NOT_SAVED",
            "Deck could not save the cancel: the line is unchanged and may still be sent",
            "Repeat the same request (same request_id) in a moment; read the line's delivery first (deck_operation_get with its operationId).",
        )
    })?;
    use crate::scheduler::CancelOutcome::*;
    let (state, code) = match outcome {
        Canceled | AlreadyCanceled => ("committed", None),
        Delivered => ("rejected", Some("ALREADY_DELIVERED")),
        Sending => ("rejected", Some("LINE_SENDING")),
        Uncertain => ("rejected", Some("DELIVERY_UNCERTAIN")),
        UserEdited => ("rejected", Some("LINE_EDITED_BY_USER")),
        NotFound => ("rejected", Some("LINE_NOT_FOUND")),
    };
    if outcome == Canceled {
        note_worker_change();
    }
    let minted = random_id("op_", 16).map_err(map_error)?;
    let recorded = runtime.write(|doc| -> Result<Operation, DeckError> {
        permitted(doc, client_id, Choice::Workers)?;
        if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
            return Ok(existing.clone());
        }
        reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
        let mut operation = Operation {
            result: Some(json!({
                "cardId": worker.card_id,
                "targetOperationId": args.operation_id,
                "outcome": outcome.word(),
            })),
            ..Operation::new(
                minted.clone(),
                client_id,
                args.request_id.clone(),
                hash.clone(),
                WORKER_CANCEL_KIND,
                state,
            )
        };
        operation.code = code.map(str::to_owned);
        doc.operations.push(operation.clone());
        Ok(operation)
    });
    let mode = runtime
        .read(|doc| {
            doc.operations
                .iter()
                .find(|o| o.operation_id == args.operation_id)
                .map(|o| {
                    if o.kind == WORKER_START_KIND {
                        "start"
                    } else if is_interjected(o) {
                        INTERJECT_MODE
                    } else {
                        "queued"
                    }
                })
        })
        .ok()
        .flatten()
        .unwrap_or(if args.operation_id == worker.operation_id {
            "start"
        } else {
            "unknown"
        });
    let delivery = delivery_view(runtime, &worker, &known, &args.operation_id, None, mode);
    let mut view = match recorded {
        Ok(operation) => operation_view(&operation),
        // the queue's answer stands; only its record is missing: the same
        // request asks the queue again
        Err(_) => json!({
            "ok": true,
            "kind": WORKER_CANCEL_KIND,
            "state": state,
            "code": code,
            "result": {"cardId": worker.card_id, "targetOperationId": args.operation_id, "outcome": outcome.word()},
            "recorded": false,
        }),
    };
    view["delivery"] = delivery;
    view["nextAction"] = json!(cancel_next(outcome));
    Ok(view)
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
    /// The newest report waits for the orchestrator card that started the
    /// card to acknowledge it (`reports::awaiting_ack`): the label says so
    /// until it does. Never a count, never the text.
    report_pending: bool,
    /// `reports::receipt_state` of the newest report (`none`, `unowned`,
    /// `unknown`, `pending`, `acknowledged`): the label tells an untracked
    /// receipt from a received one.
    receipt_state: &'static str,
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
                    report_pending: super::reports::awaiting_ack(w)
                        && !super::origin::taken_over(w, targets.as_ref()),
                    receipt_state: super::reports::receipt_state(w).word(),
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
            // unowned now: no orchestrator card acknowledges for it, and
            // what the old one acknowledged is nobody's word any more
            worker.orchestrator = None;
            worker.acknowledged = None;
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
/// decides nothing. The directory must exist; the candidates are the
/// projects this client reaches whose roots hold it, and — only when none
/// does and the client's linked-worktree switch is on — those whose
/// repository the directory's linked worktree belongs to
/// (`grants::auto_worktree_scope`, approved 2026-11-28; the scope's root is
/// then that worktree, the grant `AutoWorktree`). None:
/// `PATH_NOT_ACCESSIBLE` with a `reason` (`not-found`,
/// `volume-unavailable`, `not-accessible`, `outside-scope`, or
/// `worktree-unverified` with the rule's word in `detail`). One: that
/// project. Several (nested or shared directories, one repository in two
/// projects' rules): `PATH_AMBIGUOUS` with each candidate, never the
/// deepest one — the project also decides which environment variables the
/// shell gets, so Deck never picks among them. `project_id` only chooses
/// among the candidates: a reached project that is not one is
/// `PATH_NOT_ACCESSIBLE` too. Never falls back to another directory.
fn start_project(
    client: &Client,
    dir: &str,
    named: Option<&str>,
) -> Result<Result<(ProjectScope, super::grants::CwdGrant), Value>, DeckError> {
    let refuse = |reason: &str, message: &str, detail: Option<&str>| {
        let next = match reason {
            "volume-unavailable" => "The volume that directory is on is not mounted: ask the local user to connect it, then start again. Deck keeps the client's configuration as it is.",
            "not-found" => "Create the directory first (for a worktree: git worktree add), then start again with the same dir.",
            "not-accessible" => "Deck cannot read that directory (permissions or an unavailable volume); ask the local user, or use another directory.",
            "worktree-unverified" => "Deck could not verify that directory as a registered linked worktree of a repository one of your projects anchors (detail says why; deck_capabilities autoLinkedWorktrees lists each project's anchor). Create it with git worktree add from that repository, or ask the local user to add the directory to the project.",
            _ => "Use an existing directory inside one of the projects deck_capabilities authorizedWorkspaces lists (or, when autoLinkedWorktrees is enabled, a linked worktree of one of their repositories), or ask the local Deck user to give this client that project.",
        };
        let mut error = json!({
            "code": "PATH_NOT_ACCESSIBLE",
            "message": message,
            "reason": reason,
            "nextAction": next,
        });
        if let Some(detail) = detail {
            error["detail"] = json!(detail);
        }
        Ok(Err(json!({"ok": false, "error": error})))
    };
    let path = match std::fs::canonicalize(dir) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return refuse("not-found", "that path is not a directory", None),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return refuse("not-accessible", "Deck may not read that directory", None)
        }
        Err(_) => {
            // `/Volumes/<name>` missing: the disk, not the directory
            let volume = Path::new(dir)
                .strip_prefix("/Volumes")
                .ok()
                .and_then(|rest| rest.components().next())
                .map(|name| Path::new("/Volumes").join(name));
            return if volume.is_some_and(|v| !v.exists()) {
                refuse(
                    "volume-unavailable",
                    "the volume that directory is on is not mounted",
                    None,
                )
            } else {
                refuse("not-found", "that directory does not exist", None)
            };
        }
    };
    let holds = |scope: &ProjectScope| {
        scope.roots.iter().any(|root| {
            std::fs::canonicalize(root)
                .ok()
                .is_some_and(|root| path.starts_with(root))
        })
    };
    let mut candidates: Vec<ProjectScope> = match named {
        Some(id) => {
            let scope = scoped_project(client, id)?;
            [scope].into_iter().filter(holds).collect()
        }
        None => client_scopes(client).into_iter().filter(holds).collect(),
    };
    // the roots decide first; the switch only when they hold nothing
    let mut grant = super::grants::CwdGrant::Roots;
    let mut refused = None;
    if candidates.is_empty() && client.auto_worktrees {
        match super::grants::auto_worktree_scope(client, &path) {
            Ok(scopes) => {
                candidates = scopes
                    .into_iter()
                    .filter(|s| named.is_none_or(|id| id == s.project_id))
                    .collect();
                grant = super::grants::CwdGrant::AutoWorktree;
            }
            // a plain directory outside every root is just outside
            Err("not-a-worktree") => {}
            Err(why) => {
                debug_assert!(super::grants::WORKTREE_REASONS.contains(&why));
                refused = Some(why);
            }
        }
    }
    match candidates.len() {
        0 if refused.is_some() => refuse(
            "worktree-unverified",
            "that directory lies in a worktree folder the local user named, but Deck could not verify it as a linked worktree of that repository",
            refused,
        ),
        0 if named.is_some() => refuse(
            "outside-scope",
            "that directory is not inside the project named",
            None,
        ),
        0 => refuse(
            "outside-scope",
            "that directory is not inside any project this client reaches",
            None,
        ),
        1 => Ok(Ok((candidates.remove(0), grant))),
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
    let binding = super::admit::Binding::of(args.send_sequence, args.lane_epoch)?;
    let hash = request_hash("deck_worker_start", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    // the same request again answers its operation first, whatever the
    // scope says now: a replay never becomes a new decision
    if let Some(mut replayed) =
        replay_worker(runtime, client_id, orchestrator, &args.request_id, &hash)?
    {
        if let Some(result) = replayed.get_mut("result").and_then(Value::as_object_mut) {
            result.remove("text");
        }
        return Ok(replayed);
    }
    // the project comes from the directory, never from where the caller
    // runs (resolved before the write: a client of all projects reads the
    // project's directory from disk)
    let (scope, _) = runtime
        .read(
            |doc| -> Result<Result<(ProjectScope, super::grants::CwdGrant), Value>, DeckError> {
                let client = permitted(doc, client_id, Choice::Workers)?;
                start_project(client, &args.dir, args.project_id.as_deref())
            },
        )
        .map_err(map_error)?
        .map_err(map_error)??;
    if let Some(column) = args.column_id.as_deref() {
        named_column(runtime, client_id, &scope.project_id, column)?;
    }
    let operation = runtime
        .write(|doc| -> Result<Result<Operation, Value>, DeckError> {
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Ok(existing.clone()));
            }
            if let Some(a) = super::admit::tombstone(doc, client_id, &args.request_id, &hash)? {
                return Ok(Err(super::admit::retired_view(a)));
            }
            let client = permitted(doc, client_id, Choice::Workers)?.clone();
            // withdrawn since the check above: the start is refused whole
            if args.column_id.is_some() && permitted(doc, client_id, Choice::CardEdits).is_err() {
                return Ok(Err(column_not_permitted()));
            }
            if !super::origin::lane_room(&client, orchestrator) {
                return Ok(Err(super::origin::no_lane()));
            }
            if let Err(refusal) = super::admit::check(&client, orchestrator, binding) {
                return Ok(Err(refusal));
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
            // decided again under the write, on the client as it is now: a
            // scope or worktree rule withdrawn meanwhile refuses the start,
            // and the directory still lies where it was verified
            let (project, grant) = match start_project(&client, &args.dir, Some(&scope.project_id))? {
                Ok(again) if again.0.project_id == scope.project_id => again,
                Ok(_) => return Ok(Err(error_value(
                    "PATH_NOT_ACCESSIBLE",
                    "that directory no longer belongs to the project it was checked in",
                    "Read deck_capabilities and start again.",
                ))),
                Err(refusal) => return Ok(Err(refusal)),
            };
            let project = &project;
            let dir = canonical_scope(&args.dir, &project.roots)?;
            let identity = super::grants::dir_identity(&dir);
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
                    // how the directory was authorized and which directory it
                    // was: decided again at the launch (`commands::admit_launch`)
                    "cwdGrant": grant.word(),
                    "cwdIdentity": identity.map(|(dev, ino)| json!([dev, ino])),
                    "title": title,
                    "type": worker_type.name,
                    // the first line waits here until the card exists, then
                    // leaves the ledger for the queue (`settle_start`)
                    "text": args.text,
                    "orchestratorCardId": orchestrator,
                })), ..Operation::new(random_id("op_", 16)?, client_id, args.request_id.clone(), hash.clone(), WORKER_START_KIND, "accepted") };
            super::admit::accept(doc, client_id, orchestrator, binding, &operation);
            doc.operations.push(operation.clone());
            Ok(Ok(operation))
        })
        .map_err(map_error)?;
    let operation = match operation {
        Ok(operation) => operation,
        Err(answer) => return settled_answer(answer),
    };
    emit_changed(runtime);
    let mut view = receipt(runtime, client_id, orchestrator, binding, &operation);
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
            None,
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
    auto_worktrees: bool,
    quota: bool,
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
            auto_worktrees: client.auto_worktrees,
            quota: client.allow_quota,
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
            auto_worktrees: input.auto_worktrees,
            quota: input.allow_quota,
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
        client.auto_worktrees = self.auto_worktrees;
        client.allow_quota = self.quota;
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
        // a root the client holds now is kept as stored while it does not
        // resolve (an unmounted volume): saving must not drop it
        let kept = runtime.read(|doc| {
            doc.config
                .clients
                .iter()
                .find(|c| c.id == client_id)
                .map(|c| c.projects.clone())
                .unwrap_or_default()
        })?;
        let scopes = super::commands::validate_client_scopes(
            list,
            &kept,
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
            // the linked-worktree switch off: a start or create accepted on
            // it and not yet taken by the Board is rejected now; one the
            // Board took is decided at its launch (`commands::admit_launch`),
            // and cards already running stay as they are
            if before.auto_worktrees && !after.auto_worktrees {
                doc.reject_pending(
                    |operation| {
                        operation.client_id == client_id
                            && operation.state == "accepted"
                            && matches!(
                                operation.kind.as_str(),
                                WORKER_START_KIND | "session-create"
                            )
                            && operation
                                .result
                                .as_ref()
                                .and_then(|r| r.get("cwdGrant"))
                                .and_then(Value::as_str)
                                == Some(super::grants::CwdGrant::AutoWorktree.word())
                    },
                    "auto-worktrees-withdrawn",
                );
            }
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
