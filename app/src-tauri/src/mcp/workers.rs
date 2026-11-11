//! Worker cards: the cards a delegated client started through its Agent
//! automations, which the user also let it drive. The four tools it uses
//! (list, wait, reply, send), the ledger records that say which cards are
//! its own and which ones the user took over, and the facts the scheduler,
//! the reply store and the webview read.
//!
//! The contract lives in `mcp/mod.rs` ("Worker cards"). In short:
//! - A worker card is recorded when an Agent trigger of a client with
//!   `allow_workers` commits its card, or when the webview commits a
//!   `deck_worker_start` Board intent (`record_worker`); nothing else makes
//!   one, and a client sees only its own.
//! - Worker types (`WorkerType`): the user's list, per client, of the kinds
//!   of worker card it may start — a name, the user's own command (`claude`
//!   or `codex --no-daemon` with simple arguments, `validate_worker_types`),
//!   a column and a description the orchestrator reads. `deck_worker_start`
//!   names a type (never a command) and a directory inside the client's
//!   authorized root, and journals a `worker-start` intent with the first
//!   line; the webview creates and starts the card (`mcp.js`), and the
//!   commit queues that line as the run's head row (`settle_start`), which
//!   then leaves the ledger. Its first-send override rests on the client's
//!   `worker_first_send` choice alone (`worker_first_send_key`,
//!   `scheduler/first_send.rs`).
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
//! - The user takes a card over by typing into its pane (`worker_hold`,
//!   from the webview) and only hands it back by hand (`worker_release`).
//!   While held, the card's delegated lines hold at `human`.
//! - Within one client, a worker started from an orchestrator card
//!   (`origin.rs`) is that card's: only calls from it (and from no card) see
//!   and drive it, its sequences are that card's lane, and once the card
//!   left the Board the user holds it (`origin::taken_over`, read wherever
//!   `held` is) until handing it back makes it unowned.
//! - Withdrawing `allow_workers`, revoking the client or disabling MCP takes
//!   the client out of `automation_delegations` under the settings fence the
//!   scheduler's firing intent also takes: no delegated line starts after
//!   that returns, and the tick strips their authority.

use super::*;

pub(super) const WORKER_SEND_KIND: &str = "worker-send";
/// Worker cards one client may have on the Board at once; a trigger beyond
/// it is refused (`CAPACITY_EXCEEDED`).
pub(super) const MAX_LIVE_WORKERS: usize = 8;
/// Worker records kept in all (closed cards are pruned first).
const MAX_WORKER_RECORDS: usize = 256;
/// Unsent delegated lines one card may hold.
const MAX_UNSENT_PER_CARD: usize = 4;
/// Sends one client may make per minute.
const SENDS_PER_MINUTE: usize = 30;
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
        || doc
            .operations
            .iter()
            .any(|o| matches!(o.kind.as_str(), WORKER_SEND_KIND | WORKER_START_KIND))
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
    }
    doc.workers.push(Worker {
        card_id: card.into(),
        client_id: operation.client_id.clone(),
        operation_id: operation.operation_id.clone(),
        automation_id: automation,
        created_at: now_ms(),
        held: false,
        orchestrator: super::origin::of_operation(operation),
    });
}

/// How many of `client`'s worker cards are on the Board now (all of its
/// records while no Board is known).
pub(super) fn live_workers(doc: &DiskDoc, client: &str) -> usize {
    let cards = crate::documents::card_targets(false);
    doc.workers
        .iter()
        .filter(|w| w.client_id == client)
        .filter(|w| cards.as_ref().is_none_or(|c| c.contains_key(&w.card_id)))
        .count()
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
}

/// One pane listing: for each session, whether it is alive and what its
/// Signal target reported for the current generation. `None` when tmux
/// could not be read.
fn observe_sessions() -> Option<HashMap<String, Seen>> {
    #[cfg(test)]
    if let Some(forced) = OBSERVE_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced;
    }
    let rows = crate::tmux::list_panes().ok()?;
    let agents = crate::agent_status::projections(&rows);
    let current = crate::agent_status::reporting(&rows, crate::agent_status::live_generation);
    let mut seen: HashMap<String, Seen> = rows
        .iter()
        .map(|row| {
            (
                row.session_name.clone(),
                Seen {
                    alive: true,
                    state: None,
                    episode: 0,
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
    Some(seen)
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
}

fn rows_of(doc: &DiskDoc, client: &str, orchestrator: Option<&str>) -> Vec<WorkerRow> {
    let targets = crate::documents::card_targets(false).unwrap_or_default();
    doc.workers
        .iter()
        .filter(|w| w.client_id == client && super::origin::sees(w, orchestrator))
        .filter_map(|w| {
            let target = targets.get(&w.card_id).cloned();
            target.as_ref()?;
            Some(WorkerRow {
                worker: w.clone(),
                taken_over: super::origin::taken_over(w, Some(&targets)),
                target,
                sends: sends_of(doc, client, &w.card_id),
            })
        })
        .collect()
}

fn view(
    runtime: &Runtime,
    client: &str,
    row: &WorkerRow,
    seen: Option<&HashMap<String, Seen>>,
) -> Value {
    let target = row.target.as_ref();
    let observed = target.and_then(|t| seen.and_then(|s| s.get(&t.session)).copied());
    let (unsent, states) = queues(runtime)
        .map(|q| crate::scheduler::worker_rows(&q.q, &row.worker.card_id, client, &row.sends))
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
        (Some(_), _) => json!({"state": "unavailable", "episode": 0}),
    };
    json!({
        "cardId": row.worker.card_id,
        "title": target.map(|t| t.title.as_str()).unwrap_or_default(),
        "automationId": row.worker.automation_id,
        "alive": observed.is_some_and(|o| o.alive),
        "agent": agent,
        "takenOver": row.taken_over,
        "unsent": unsent,
        "sends": states
            .iter()
            .map(|(id, state)| json!({"operationId": id, "state": state}))
            .collect::<Vec<_>>(),
    })
}

fn driving<'a>(doc: &'a DiskDoc, client_id: &str) -> Result<&'a Client, DeckError> {
    let client = client(doc, client_id)?;
    if !client.allow_workers {
        return Err(DeckError::new(
            ErrorKind::Perm,
            "client may not drive worker cards",
        ));
    }
    Ok(client)
}

type Snapshot = (Vec<WorkerRow>, u64, bool, Vec<Value>);

fn snapshot(runtime: &Runtime, client_id: &str) -> Result<Snapshot, Value> {
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    runtime
        .read(|doc| {
            driving(doc, client_id).map(|client| {
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
    let seen = observe_sessions();
    Ok(json!({
        "ok": true,
        "workers": rows.iter().map(|row| view(runtime, client_id, row, seen.as_ref())).collect::<Vec<_>>(),
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
    if wait > MAX_WAIT_MS || args.cards.is_empty() || args.cards.len() > MAX_LIVE_WORKERS * 4 {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "cards must name 1 to 32 worker cards and wait_ms be at most 5000",
            "Name the cards and episodes deck_worker_list returned; call again after a timeout.",
        ));
    }
    let deadline = Instant::now() + Duration::from_millis(wait);
    loop {
        let mark = change_count();
        let (rows, next, replies, types) = snapshot(runtime, client_id)?;
        let seen = observe_sessions();
        let views: Vec<Value> = rows
            .iter()
            .filter(|row| args.cards.iter().any(|c| c.card_id == row.worker.card_id))
            .map(|row| view(runtime, client_id, row, seen.as_ref()))
            .collect();
        let changed = args.cards.iter().any(|asked| {
            match views.iter().find(|v| v["cardId"] == asked.card_id.as_str()) {
                None => true,
                Some(v) => {
                    v["takenOver"] == true
                        || v["alive"] == false
                        || v["agent"]["episode"].as_u64() != Some(asked.episode)
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

fn own_card(
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
            let client = driving(doc, client_id)?;
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
            "Only Claude Code turn endings Deck saw since it started are kept (the newest 32); ask the worker to write durable results to a file instead.",
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

fn within_rate(client: &str) -> bool {
    let mut window = send_window().lock_or_recover();
    let sent = window.entry(client.to_string()).or_default();
    let now = Instant::now();
    sent.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
    if sent.len() >= SENDS_PER_MINUTE {
        return false;
    }
    sent.push(now);
    true
}

fn capacity(message: &str) -> Value {
    error_value(
        "CAPACITY_EXCEEDED",
        message,
        "Wait for the card's unsent lines to go out (deck_worker_list `unsent`) or slow down, then send again with a new request.",
    )
}

pub(super) fn worker_send(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
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
    let hash = request_hash("deck_worker_send", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    let replay = runtime
        .read(|doc| {
            driving(doc, client_id)?;
            Ok(existing_operation(doc, client_id, &args.request_id, &hash)?.cloned())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    let (worker, stale) = runtime
        .read(|doc| -> Result<Result<(Worker, bool), Value>, DeckError> {
            let client = driving(doc, client_id)?;
            let stale = super::origin::send_sequence(client, orchestrator) != args.send_sequence;
            Ok(own_card(doc, client_id, orchestrator, &args.card_id).map(|w| (w, stale)))
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
    let (unsent, _) = crate::scheduler::worker_rows(&queue.q, &worker.card_id, client_id, &sends);
    if unsent >= MAX_UNSENT_PER_CARD {
        return Err(capacity(
            "the card already holds 4 unsent lines from this client",
        ));
    }
    if !within_rate(client_id) {
        return Err(capacity("this client sent 30 lines in the last minute"));
    }
    let minted = random_id("op_", 16).map_err(map_error)?;
    let operation = runtime
        .write(|doc| {
            let client = driving(doc, client_id)?;
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(existing.clone());
            }
            if args.send_sequence != super::origin::send_sequence(client, orchestrator) {
                return Err(DeckError::new(ErrorKind::ContextChanged, STALE_REQUEST));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let operation = Operation {
                operation_id: minted.clone(),
                client_id: client_id.into(),
                request_id: args.request_id.clone(),
                request_hash: hash.clone(),
                kind: WORKER_SEND_KIND.into(),
                state: "accepted".into(),
                code: None,
                result: Some(json!({"cardId": worker.card_id, "afterEpisode": args.after_episode})),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: None,
                control_epoch: None,
                control_sequence: None,
            };
            if let Some(client) = doc.config.clients.iter_mut().find(|c| c.id == client_id) {
                super::origin::bump_send(client, orchestrator);
            }
            doc.operations.push(operation.clone());
            Ok(operation)
        })
        .map_err(map_error)?;
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
        },
        None,
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

/* ---------- local commands ---------- */

/// Bring the Stop hook in line with whether any client may read replies.
pub(super) fn sync_reply_hook() {
    crate::agent_status::sync_worker_replies(replies_wanted_any());
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
/// start, the column they open in, and a line the orchestrator reads to
/// choose between types. The client only ever names a type; it never writes
/// a command.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkerType {
    pub(crate) name: String,
    pub(crate) cmd: String,
    pub(crate) column_id: String,
    #[serde(default)]
    pub(crate) description: String,
}

/// The closed reasons a worker type list is refused (stable `mcp-*` codes the
/// webview maps to one sentence each).
pub(super) const TYPE_NAME_INVALID: &str = "mcp-worker-type-name";
pub(super) const TYPE_COMMAND_INVALID: &str = "mcp-worker-type-command";
pub(super) const TYPE_CODEX_DAEMON: &str = "mcp-worker-type-codex-daemon";
pub(super) const TYPE_LIST_INVALID: &str = "mcp-worker-type-list";

fn valid_type_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

/// A worker type list as the user saved it: at most `MAX_WORKER_TYPES`,
/// unique names, each command one the external admission accepts (an
/// interactive `claude` or `codex` with simple arguments), a Codex command
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
        let agent = crate::admission::channel_agent_command(&worker_type.cmd)
            .ok_or_else(|| invalid(TYPE_COMMAND_INVALID))?;
        if agent == "codex" && !worker_type.cmd.split(' ').any(|word| word == "--no-daemon") {
            return Err(invalid(TYPE_CODEX_DAEMON));
        }
        if !valid_id(&worker_type.column_id)
            || worker_type.description.chars().count() > MAX_TYPE_DESCRIPTION_CHARS
            || worker_type.description.chars().any(char::is_control)
        {
            return Err(invalid(TYPE_LIST_INVALID));
        }
    }
    Ok(())
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
        .map(|t| json!({"type": t.name, "description": t.description}))
        .collect()
}

pub(super) fn worker_start(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let args: WorkerStartArgs = parse(arguments)?;
    if !valid_id(&args.request_id)
        || args.text.trim().is_empty()
        || args.text.len() > MAX_SEND_BYTES
        || args
            .title
            .as_ref()
            .is_some_and(|t| t.trim().is_empty() || t.chars().count() > MAX_WORKER_TITLE_CHARS)
    {
        return Err(error_value(
            "INVALID_ARGUMENTS",
            "request id, text or title is out of shape or bounds",
            "Send a first line of at most 16 KiB, an optional title of at most 80 characters, and a fresh request id.",
        ));
    }
    let hash = request_hash("deck_worker_start", &args);
    let orchestrator = super::origin::orchestrator();
    let orchestrator = orchestrator.as_deref();
    let replay = runtime
        .read(|doc| {
            driving(doc, client_id)?;
            Ok(existing_operation(doc, client_id, &args.request_id, &hash)?.cloned())
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    if let Some(operation) = replay {
        return Ok(operation_view(&operation));
    }
    let operation = runtime
        .write(|doc| -> Result<Result<Operation, Value>, DeckError> {
            if let Some(existing) = existing_operation(doc, client_id, &args.request_id, &hash)? {
                return Ok(Ok(existing.clone()));
            }
            let client = driving(doc, client_id)?.clone();
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
            let Some(project) = client.projects.first() else {
                return Err(DeckError::new(ErrorKind::Perm, "client has no project"));
            };
            let dir = match args.dir.as_deref() {
                Some(dir) => canonical_scope(dir, &project.roots)?,
                None => {
                    let root = project
                        .roots
                        .first()
                        .ok_or_else(|| DeckError::new(ErrorKind::Perm, "client has no root"))?;
                    canonical_scope(root, &project.roots)?
                }
            };
            let pending_starts = doc
                .operations
                .iter()
                .filter(|o| {
                    o.client_id == client_id
                        && o.kind == WORKER_START_KIND
                        && matches!(o.state.as_str(), "accepted" | "executing")
                })
                .count();
            if live_workers(doc, client_id) + pending_starts >= MAX_LIVE_WORKERS {
                return Ok(Err(capacity(
                    "this client already has 8 worker cards on the Board",
                )));
            }
            reserve_operation(runtime, doc, client_id, Slot::Ordinary)?;
            let title = args
                .title
                .clone()
                .unwrap_or_else(|| format!("{} worker", worker_type.name));
            let operation = Operation {
                operation_id: random_id("op_", 16)?,
                client_id: client_id.into(),
                request_id: args.request_id.clone(),
                request_hash: hash.clone(),
                kind: WORKER_START_KIND.into(),
                state: "accepted".into(),
                code: None,
                result: Some(json!({
                    "cardId": random_id("W", 16)?,
                    "projectId": project.project_id,
                    "columnId": super::origin::beside(&client, orchestrator, &project.project_id)
                        .unwrap_or_else(|| worker_type.column_id.clone()),
                    "cmd": worker_type.cmd,
                    "dir": dir,
                    "title": title,
                    "type": worker_type.name,
                    // the first line waits here until the card exists, then
                    // leaves the ledger for the queue (`settle_start`)
                    "text": args.text,
                    "orchestratorCardId": orchestrator,
                })),
                accepted_at: now_ms(),
                updated_at: now_ms(),
                admission_hash: None,
                session_id: None,
                control_epoch: None,
                control_sequence: None,
            };
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
    let first_send = runtime
        .read(|doc| {
            doc.config
                .clients
                .iter()
                .any(|c| c.id == client && c.allow_workers && c.worker_first_send)
        })
        .unwrap_or(false);
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
            },
            Some(first_send),
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
    let withdrew_automations;
    let mut withdrew_drafts = false;
    {
        let _fence = crate::storage::settings_fence();
        let _delivery = runtime.delivery.lock_or_recover();
        withdrew_automations = runtime.write(|doc| {
            let client = doc
                .config
                .clients
                .iter_mut()
                .find(|client| client.id == client_id && client.revoked_at.is_none())
                .ok_or_else(|| DeckError::new(ErrorKind::Missing, "MCP client not found"))?;
            let before = (
                client.allow_create,
                client.allow_automations,
                client.allow_workers,
                client.allow_replies,
                client.worker_first_send,
                client.worker_types.clone(),
                client.allow_drafts,
                client.allow_mentions,
                client.rule_column,
            );
            client.allow_create = permissions.allow_create;
            client.allow_automations = permissions.allow_automations;
            client.allow_workers = permissions.allow_workers;
            client.allow_replies = permissions.allow_workers && permissions.allow_replies;
            client.worker_first_send = permissions.allow_workers && permissions.worker_first_send;
            client.worker_types = permissions.worker_types.clone();
            client.allow_drafts = permissions.allow_drafts;
            client.allow_mentions = permissions.allow_mentions;
            client.rule_column = permissions.rule_column;
            let after = (
                client.allow_create,
                client.allow_automations,
                client.allow_workers,
                client.allow_replies,
                client.worker_first_send,
                client.worker_types.clone(),
                client.allow_drafts,
                client.allow_mentions,
                client.rule_column,
            );
            if before == after {
                return Ok(false);
            }
            let withdrew_automations = before.1 && !after.1;
            let withdrew_create = before.0 && !after.0;
            let withdrew_workers = before.2 && !after.2;
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
            if before.6 && !after.6 && super::proposals::withdraw_pending(doc, client_id) {
                withdrew_drafts = true;
            }
            if withdrew_workers {
                doc.reject_pending(
                    |operation| {
                        operation.client_id == client_id && operation.kind == WORKER_START_KIND
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
    if withdrew_drafts {
        super::proposals::note_withdrawn(runtime);
    }
    sync_reply_hook();
    note_worker_change();
    Ok(())
}
