//! Orchestrator cards: which Deck card a delegated client's call comes from,
//! and the grouping of its worker cards that follows from it.
//!
//! The contract lives in `mcp/mod.rs` ("Orchestrator cards"). In short:
//! - Deck reads it from the kernel, never from the client: the connecting
//!   adapter's pid (`procinfo::peer_pid`, read while the adapter is still
//!   connected — it waits for its answer) and ONE process-table snapshot,
//!   checked as an Agent Signal's origin is (`agent_origin`): the nearest
//!   pane of this Deck's tmux server on the adapter's parent chain, the
//!   pane's current foreground leader on that chain, and the chain on the
//!   pane's terminal apart from one terminal-less group (an agent's Bash
//!   tool). The pane's session names the card. Unlike Signal, a shell as
//!   the foreground leader is no refusal: a wrapper script that runs the
//!   agent is the pane's foreground, and the caller still runs in that
//!   card (the 2026-11-11 run: `zsh script.sh` → `claude -p` → adapter).
//!   Proven for Claude Code 2.1.322 (its stdio MCP server is in its process
//!   group on the pane's terminal; a subagent uses the same server). A
//!   caller none of this proves (outside Deck, detached, a shared daemon)
//!   has no orchestrator card and is served as before.
//! - Read once per request (`enter` / `orchestrator`, per connection
//!   thread), never stored as identity: the card id is the key, so an agent
//!   restarted or resumed in the same card is the same orchestrator.
//! - What it changes, all within one client's own authority (it grants
//!   nothing, a Signal may refuse and never release):
//!   * a worker card it starts with `deck_worker_start` opens in the
//!     orchestrator card's project (refused when the client does not reach
//!     it and no reachable project is named);
//!   * a worker card it starts (`deck_worker_start`, an Agent handover,
//!     a Slack mention handover) opens in the orchestrator card's column
//!     at that moment, when both are in one project and the client follows
//!     its orchestrators (`Client.rule_column` off, the default); never
//!     moved later;
//!   * the worker record keeps the orchestrator card (`Worker.orchestrator`)
//!     and a call from that card sees and drives only its own worker cards
//!     and the client's unowned ones (`sees`); a caller without a card sees
//!     all of the client's, as before;
//!   * a worker whose orchestrator card is gone from the Board is taken
//!     over by the user (`taken_over`, derived, never written): its lines
//!     hold at `human`; handing it back makes it unowned;
//!   * trigger and send sequences are kept per orchestrator card
//!     (`Client.lanes`), so two orchestrators of one client do not refuse
//!     each other's calls; a caller without a card keeps the client's own.
//! - A ledger holding any of it is written at `STATE_VERSION_ORCHESTRATORS`
//!   (sticky).

use super::*;
use crate::agent_status::ProcessTable;
use crate::tmux::PaneRow;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// Orchestrator cards whose sequences one client keeps; the lanes of cards
/// gone from the Board are dropped first when a new one would pass it.
const MAX_LANES: usize = 32;
/// The key an orchestrator card's id takes in an operation's result.
pub(super) const RESULT_KEY: &str = "orchestratorCardId";

/// One orchestrator card's sequences, bound like the client's own.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Lane {
    #[serde(default)]
    pub(super) trigger: u64,
    #[serde(default)]
    pub(super) send: u64,
}

pub(super) type Lanes = BTreeMap<String, Lane>;

/* ---------- the proof ---------- */

/// The session of the card whose pane `peer` runs in, from one snapshot and
/// one pane listing: `None` unless every step of the module header holds.
pub(crate) fn pane_session(peer: u32, table: &ProcessTable, rows: &[PaneRow]) -> Option<String> {
    let chain = crate::procinfo::ancestry_in(table, peer, crate::agent_origin::ORIGIN_HOPS);
    let row = chain
        .iter()
        .find_map(|pid| rows.iter().find(|row| row.pane_pid == *pid))?;
    crate::tmux::validate_session_name(&row.session_name).ok()?;
    let generation = crate::agent_status::foreground_generation(table, row.pane_pid)?;
    if !chain.contains(&generation.pid())
        || !crate::agent_origin::terminal_continuous(table, &chain, generation.pid(), row.pane_pid)
    {
        return None;
    }
    Some(row.session_name.clone())
}

/// The card whose session is `session` on the committed Board.
fn card_of(session: &str) -> Option<String> {
    crate::documents::card_targets(false)?
        .into_iter()
        .find(|(_, target)| target.session == session)
        .map(|(id, _)| id)
}

/* ---------- once per request ---------- */

thread_local! {
    static PEER: Cell<Option<u32>> = const { Cell::new(None) };
    /// The answer, once asked for in this request.
    static CARD: RefCell<Option<Option<String>>> = const { RefCell::new(None) };
}

/// The request on this thread comes from `peer`; it is forgotten when the
/// returned guard drops.
pub(super) struct Scope;

impl Drop for Scope {
    fn drop(&mut self) {
        PEER.with(|p| p.set(None));
        CARD.with(|c| *c.borrow_mut() = None);
    }
}

pub(super) fn enter(peer: Option<u32>) -> Scope {
    PEER.with(|p| p.set(peer));
    CARD.with(|c| *c.borrow_mut() = None);
    Scope
}

/// A test's request from `card` (or from no card).
#[cfg(test)]
pub(super) fn enter_as(card: Option<&str>) -> Scope {
    PEER.with(|p| p.set(None));
    CARD.with(|c| *c.borrow_mut() = Some(card.map(str::to_owned)));
    Scope
}

/// The orchestrator card of the request on this thread, read on first use.
pub(super) fn orchestrator() -> Option<String> {
    if let Some(known) = CARD.with(|c| c.borrow().clone()) {
        return known;
    }
    let found = PEER.with(Cell::get).and_then(|peer| {
        let table = crate::procinfo::processes();
        let rows = crate::tmux::list_panes().ok()?;
        card_of(&pane_session(peer, &table, &rows)?)
    });
    CARD.with(|c| *c.borrow_mut() = Some(found.clone()));
    found
}

/// What `deck_capabilities` says of the caller's card.
pub(super) fn card_view(card: Option<&str>) -> Value {
    let Some(card) = card else {
        return Value::Null;
    };
    let title = crate::documents::card_targets(false)
        .and_then(|targets| targets.get(card).map(|t| t.title.clone()))
        .unwrap_or_default();
    json!({"cardId": card, "title": title})
}

/* ---------- placement ---------- */

/// The column a worker card started from `orchestrator` in `project` opens
/// in: the orchestrator card's own, now, when the client follows its
/// orchestrators and both cards are in that project.
pub(super) fn beside(client: &Client, orchestrator: Option<&str>, project: &str) -> Option<String> {
    if client.rule_column {
        return None;
    }
    let target = crate::documents::card_targets(false)?.remove(orchestrator?)?;
    (target.project_id == project && !target.column_id.is_empty()).then_some(target.column_id)
}

/// The orchestrator card an operation was started from.
pub(super) fn of_operation(operation: &Operation) -> Option<String> {
    operation
        .result
        .as_ref()?
        .get(RESULT_KEY)?
        .as_str()
        .map(str::to_owned)
}

/// For an Agent item about to become a card (`inbound_pending`): the column
/// its card opens in instead of the rule's, when one applies. The item's key
/// is its operation's id.
pub(crate) fn column_for_item(item_key: &str, project: &str) -> Option<String> {
    column_for_item_in(RUNTIME.get()?, item_key, project)
}

pub(super) fn column_for_item_in(
    runtime: &Runtime,
    item_key: &str,
    project: &str,
) -> Option<String> {
    runtime
        .read(|doc| {
            let operation = doc.operations.iter().find(|o| o.operation_id == item_key)?;
            let client = doc
                .config
                .clients
                .iter()
                .find(|c| c.id == operation.client_id && c.revoked_at.is_none())?;
            beside(client, of_operation(operation).as_deref(), project)
        })
        .ok()
        .flatten()
}

/* ---------- scope ---------- */

/// Whether a call from `orchestrator` sees worker `worker`: its own and the
/// client's unowned ones; a call from no card sees every one.
pub(super) fn sees(worker: &super::workers::Worker, orchestrator: Option<&str>) -> bool {
    match (orchestrator, worker.orchestrator.as_deref()) {
        (Some(caller), Some(owner)) => caller == owner,
        _ => true,
    }
}

/// Whether worker `worker`'s orchestrator card is gone from the Board.
pub(super) fn orphaned(
    worker: &super::workers::Worker,
    targets: Option<&HashMap<String, crate::documents::CardTarget>>,
) -> bool {
    match (worker.orchestrator.as_deref(), targets) {
        (Some(owner), Some(cards)) => !cards.contains_key(owner),
        _ => false,
    }
}

/// Whether the user holds worker `worker`: taken over by hand, or its
/// orchestrator card is gone.
pub(super) fn taken_over(
    worker: &super::workers::Worker,
    targets: Option<&HashMap<String, crate::documents::CardTarget>>,
) -> bool {
    worker.held || orphaned(worker, targets)
}

/* ---------- sequences ---------- */

pub(super) fn trigger_sequence(client: &Client, orchestrator: Option<&str>) -> u64 {
    match orchestrator {
        Some(card) => client.lanes.get(card).map_or(0, |lane| lane.trigger),
        None => client.trigger_sequence,
    }
}

pub(super) fn send_sequence(client: &Client, orchestrator: Option<&str>) -> u64 {
    match orchestrator {
        Some(card) => client.lanes.get(card).map_or(0, |lane| lane.send),
        None => client.send_sequence,
    }
}

fn lane<'a>(client: &'a mut Client, card: &str) -> &'a mut Lane {
    if !client.lanes.contains_key(card) && client.lanes.len() >= MAX_LANES {
        if let Some(cards) = crate::documents::card_targets(false) {
            client.lanes.retain(|id, _| cards.contains_key(id));
        }
        while client.lanes.len() >= MAX_LANES {
            let Some(first) = client.lanes.keys().next().cloned() else {
                break;
            };
            client.lanes.remove(&first);
        }
    }
    client.lanes.entry(card.to_owned()).or_default()
}

pub(super) fn bump_trigger(client: &mut Client, orchestrator: Option<&str>) {
    match orchestrator {
        Some(card) => {
            let lane = lane(client, card);
            lane.trigger = lane.trigger.saturating_add(1);
        }
        None => client.trigger_sequence = client.trigger_sequence.saturating_add(1),
    }
}

pub(super) fn bump_send(client: &mut Client, orchestrator: Option<&str>) {
    match orchestrator {
        Some(card) => {
            let lane = lane(client, card);
            lane.send = lane.send.saturating_add(1);
        }
        None => client.send_sequence = client.send_sequence.saturating_add(1),
    }
}

/// Whether the ledger holds anything a v10 reader would misread.
pub(super) fn uses_orchestrators(doc: &DiskDoc) -> bool {
    doc.workers.iter().any(|w| w.orchestrator.is_some())
        || doc
            .config
            .clients
            .iter()
            .any(|c| c.rule_column || !c.lanes.is_empty())
        || doc.operations.iter().any(|o| of_operation(o).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procinfo::ProcessInfo;

    fn p(pid: u32, ppid: u32, pgid: u32, tty: u32, fg: u32) -> (u32, ProcessInfo) {
        (
            pid,
            ProcessInfo {
                pid,
                ppid,
                pgid,
                tty,
                tty_pgid: fg,
                start_seconds: 1000 + u64::from(pid),
                start_micros: 3,
            },
        )
    }

    fn row(session: &str, pane_pid: u32, command: &str) -> PaneRow {
        PaneRow {
            server_pid: 42,
            session_id: "$1".into(),
            session_name: session.into(),
            window_id: "@1".into(),
            pane_id: "%1".into(),
            pane_pid,
            window_active: true,
            pane_active: true,
            command: command.into(),
            ..PaneRow::default()
        }
    }

    /// Two panes as the 2026-11-11 probe saw them: each a zsh on its own
    /// terminal running Claude Code in the foreground; Claude's stdio MCP
    /// server in Claude's group on that terminal; a Bash-tool command in a
    /// terminal-less group of its own under Claude; a detached process
    /// reparented to launchd.
    fn world() -> (ProcessTable, Vec<PaneRow>) {
        let table: ProcessTable = [
            p(1, 0, 1, 0, 0),
            p(42, 1, 42, 0, 0),
            // pane A: zsh 100 on tty 7, claude 101 foreground
            p(100, 42, 100, 7, 101),
            p(101, 100, 101, 7, 101),
            p(102, 101, 101, 7, 101), // MCP server
            p(103, 101, 103, 0, 0),   // Bash tool: zsh
            p(104, 103, 103, 0, 0),   // its python
            p(105, 1, 104, 0, 0),     // detached, reparented
            // pane B: zsh 200 on tty 8, claude 201 foreground
            p(200, 42, 200, 8, 201),
            p(201, 200, 201, 8, 201),
            p(202, 201, 201, 8, 201),
            // pane C: a wrapper script's shell in the foreground, its agent a child
            p(300, 42, 300, 9, 310),
            p(310, 300, 310, 9, 310),
            p(311, 310, 310, 9, 310),
            p(312, 311, 310, 9, 310),
            // pane D: a plain shell, and a job it put in the background
            p(500, 42, 500, 10, 500),
            p(501, 500, 501, 10, 500),
            // a daemon between two terminal-less groups under pane B's claude
            p(400, 201, 400, 0, 0),
            p(401, 400, 401, 0, 0),
        ]
        .into_iter()
        .collect();
        let rows = vec![
            row("deck-a", 100, "claude"),
            row("deck-b", 200, "claude"),
            row("deck-c", 300, "zsh"),
            row("deck-d", 500, "zsh"),
        ];
        (table, rows)
    }

    #[test]
    fn a_call_is_bound_to_the_card_whose_pane_its_agent_runs_in() {
        let (table, rows) = world();
        let session = |peer| pane_session(peer, &table, &rows);
        assert_eq!(session(102).as_deref(), Some("deck-a"), "the MCP server");
        assert_eq!(session(202).as_deref(), Some("deck-b"), "the other pane");
        assert_eq!(session(104).as_deref(), Some("deck-a"), "a Bash-tool child");
        assert_eq!(session(101).as_deref(), Some("deck-a"), "the agent itself");
        assert_eq!(session(105), None, "detached: no pane on its chain");
        assert_eq!(
            session(312).as_deref(),
            Some("deck-c"),
            "an agent under a wrapper script"
        );
        assert_eq!(
            session(501).as_deref(),
            Some("deck-d"),
            "a job the card's own shell put in the background runs in that card"
        );
        assert_eq!(session(401), None, "two terminal-less groups");
        assert_eq!(session(999), None, "an unknown peer");
    }

    #[test]
    fn a_pane_whose_foreground_leader_is_not_on_the_chain_proves_nothing() {
        let (mut table, rows) = world();
        // pane A's foreground moved to another program (not the caller's)
        for pid in [100, 101, 102] {
            table.get_mut(&pid).unwrap().tty_pgid = 150;
        }
        table.extend([p(150, 100, 150, 7, 150)]);
        assert_eq!(pane_session(102, &table, &rows), None);
        // and a row with a name tmux would refuse is no card
        let (table, mut rows) = world();
        rows[0].session_name = "-bad".into();
        assert_eq!(pane_session(102, &table, &rows), None);
    }

    #[test]
    fn sequences_are_kept_per_orchestrator_card_and_bounded() {
        let mut client: Client = serde_json::from_value(json!({
            "id": "c1", "name": "c", "revokedAt": null, "allowCreate": false, "projects": [],
            "triggerSequence": 5, "sendSequence": 2
        }))
        .unwrap();
        assert_eq!(trigger_sequence(&client, None), 5);
        assert_eq!(trigger_sequence(&client, Some("A")), 0);
        bump_trigger(&mut client, Some("A"));
        bump_send(&mut client, Some("A"));
        bump_send(&mut client, Some("B"));
        bump_trigger(&mut client, None);
        assert_eq!(trigger_sequence(&client, Some("A")), 1);
        assert_eq!(send_sequence(&client, Some("A")), 1);
        assert_eq!(send_sequence(&client, Some("B")), 1);
        assert_eq!(trigger_sequence(&client, None), 6);
        assert_eq!(send_sequence(&client, None), 2);
        for i in 0..(MAX_LANES * 2) {
            bump_trigger(&mut client, Some(&format!("card{i:03}")));
        }
        assert!(client.lanes.len() <= MAX_LANES);
        assert_eq!(
            trigger_sequence(&client, Some("card063")),
            1,
            "the newest kept"
        );
    }
}
