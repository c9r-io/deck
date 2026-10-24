//! Recover interrupted Agent sessions: after the shell service was LOST — a
//! frozen machine, a power cut, a tmux server that died — offer to re-enter
//! the exact Claude Code session each card was in. This is a separate
//! operation from re-entry on a restart (`mod.rs`), with its own record,
//! command, dialog and log tag (`[agent-recovery]`); it shares only the
//! launch and its confirmation (`Plan::recovered` → `Plan::reopen`).
//!
//! # Contract
//! - **A record, written on events.** A Claude Code `SessionStart` that the
//!   kernel attributes to a card's pane writes one record: the card, its
//!   session, the native session id, the client's generation, the server's
//!   generation, the pane, the client's executable path and working
//!   directory. A `SessionEnd` the user caused (`prompt-input-exit`,
//!   `logout`) deletes it; a `clear` or `resume` end is followed by the
//!   next start, which replaces it; an `other` end deletes nothing (a client
//!   killed with its server says that). Nothing is polled for a session id.
//!   The file is `~/.deck/agent-recovery.json`, bounded (`MAX_BYTES`,
//!   `MAX_RECORDS`), written atomically with fsync, 0600 like the rest of
//!   the tree. A record is recovery material, never authority: it names
//!   what to OFFER; only the user's confirmation launches anything.
//! - **Attribution.** The status listener reads, while the helper is still
//!   connected, one process table and the peer: the peer executes the
//!   bundled lifecycle helper and its DIRECT parent is the client
//!   (`observation`). The recorder worker then requires the claimed pane
//!   on Deck's own server, the pane's shell in the helper's ancestry, the
//!   client as the pane's foreground generation in a terminal-continuous
//!   chain, and — under its own deadline — the client's running code as
//!   Claude Code in a supported launch shape (`procinfo::standing_client`),
//!   and the client still the pane's foreground after that call. The card
//!   is the one card the committed Board binds to that session. Anything
//!   less records nothing. An end deletes only the record of the very client
//!   generation and native id that reported it.
//! - **Pruning.** While the record's server still runs, a record whose pane
//!   is gone or no longer has that client in its foreground is deleted once
//!   that was seen on two prune ticks at least `PRUNE_GRACE` apart: a
//!   client that died alone must not be offered after a later crash, and a
//!   shutdown that takes everything down within seconds prunes nothing.
//! - **Lost, and only lost.** A record is offered when its server
//!   generation AND its client generation are gone (`Liveness::Gone`; an
//!   unreadable one is not gone) and the committed Board still binds its
//!   card to its session alone. A Deck that crashed while tmux went on has
//!   live clients in their panes: nothing is offered. A card whose shell is
//!   already open, whose directory or program is missing, or whose
//!   adapter's lifecycle hooks are off is listed and cannot be ticked.
//! - **Once only.** Confirming or declining an offer deletes every lost
//!   record the offer read, and a confirmed recovery deletes them BEFORE
//!   the first shell is created: a crash during the recovery never offers
//!   the same session again. If that write fails nothing is launched.
//! - **The launch is re-entry's.** The executable at the recorded path is
//!   read and validated in full as Claude Code now (the old image is gone;
//!   this is a weaker identity than a restart's, which binds the running
//!   client). The request is always the plain `--resume <exact id>`: no
//!   bypass, whatever the old client was started with — a record written
//!   before a crash is no authorization. Shell, readiness, Board fence,
//!   single slot, receipts and confirmation are `Plan::reopen` unchanged.
//! - **Privacy.** The file holds native ids and paths; views and logs carry
//!   card id, session, closed codes and counts only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::claude::{EndReason, Told};
use super::lifecycle::{Event, Wire};
use super::{Env, Reason, Recovered, ReentryResult};
use crate::agent_status::ProcessTable;
use crate::applog::{applog, session_tag};
use crate::error::{DeckError, ErrorKind};
use crate::procinfo::{Liveness, StandingClient};
use crate::sync::LockRecover;
use crate::tmux::PaneRow;

const FILE: &str = "agent-recovery.json";
const LABEL: &str = "agent recovery records";
const VERSION: u64 = 1;
const MAX_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RECORDS: usize = 64;
/// The source word of the one adapter whose sessions are recorded.
const SOURCE: &str = "claude-code";
/// How long the recorder may take to qualify one client (one dynamic
/// validation and kernel reads).
const STANDING_BUDGET: Duration = Duration::from_secs(2);
/// How often the recorder looks for records whose client ended alone.
const PRUNE_EVERY: Duration = Duration::from_secs(30);
/// How long a record must look stale before it is deleted.
const PRUNE_GRACE: Duration = Duration::from_secs(60);
/// How many observations may wait for the recorder.
const QUEUE: usize = 64;
/// How long an offer's token can be confirmed.
const OFFER_TTL: Duration = Duration::from_secs(3600);
/// What reading and validating the recorded programs may take, together.
pub(crate) const PREPARE_SHARE: Duration = Duration::from_secs(10);

// ---------- the record --------------------------------------------------------------

/// A process generation: pid and birth instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Gen {
    pub(crate) pid: u32,
    pub(crate) secs: u64,
    pub(crate) micros: u32,
}

impl Gen {
    pub(crate) fn of(pid: u32, birth: (u64, u32)) -> Self {
        Self {
            pid,
            secs: birth.0,
            micros: birth.1,
        }
    }
    pub(crate) fn birth(self) -> (u64, u32) {
        (self.secs, self.micros)
    }
}

/// One card's interrupted-session candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Record {
    pub(crate) card_id: String,
    pub(crate) session: String,
    pub(crate) source: String,
    pub(crate) native_id: String,
    pub(crate) client: Gen,
    pub(crate) server: Gen,
    pub(crate) pane: String,
    pub(crate) executable: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) recorded_at: u64,
}

impl Record {
    /// The same session of the same client, whenever it was written.
    fn same(&self, other: &Record) -> bool {
        Record {
            recorded_at: other.recorded_at,
            ..self.clone()
        } == *other
    }

    fn valid(&self) -> bool {
        let text = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
        text(&self.card_id)
            && text(&self.session)
            && self.source == SOURCE
            && super::claude::native_id_ok(&self.native_id)
            && (2..=10).contains(&self.pane.len())
            && self.pane.starts_with('%')
            && self.pane[1..].bytes().all(|b| b.is_ascii_digit())
            && self.executable.is_absolute()
            && self.cwd.is_absolute()
            && self.executable.as_os_str().len() <= 1024
            && self.cwd.as_os_str().len() <= 1024
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    v: u64,
    records: Vec<Record>,
}

/// One writer of the file at a time: the recorder and the commands.
static STORE: Mutex<()> = Mutex::new(());

/// The records file in the data directory.
pub(crate) fn store() -> PathBuf {
    crate::datadir::deck_dir().join(FILE)
}

/// The records in `path`, or why they cannot be used. A file of a later
/// version, or one that cannot be read, is left untouched and used for
/// nothing: no record is offered from it and none is written over it.
fn load_at(path: &Path) -> Result<Vec<Record>, &'static str> {
    let value = match crate::ledger::load_bounded::<serde_json::Value>(path, MAX_BYTES, LABEL) {
        Ok(None) => return Ok(Vec::new()),
        Ok(Some(value)) => value,
        Err(_) => return Err("recovery-records-unreadable"),
    };
    match value.get("v").and_then(serde_json::Value::as_u64) {
        Some(VERSION) => {}
        Some(v) if v > VERSION => return Err("recovery-records-future"),
        _ => return Err("recovery-records-unreadable"),
    }
    let doc: Doc = serde_json::from_value(value).map_err(|_| "recovery-records-unreadable")?;
    if doc.records.len() > MAX_RECORDS || !doc.records.iter().all(Record::valid) {
        return Err("recovery-records-unreadable");
    }
    Ok(doc.records)
}

/// Load, change and — when `change` says it changed something — write
/// back, under the one store lock. `Ok(changed)`.
fn update_at(
    path: &Path,
    change: impl FnOnce(&mut Vec<Record>) -> bool,
) -> Result<bool, &'static str> {
    let _store = STORE.lock_or_recover();
    let mut records = load_at(path)?;
    if !change(&mut records) {
        return Ok(false);
    }
    let bytes = crate::ledger::encode(
        &Doc {
            v: VERSION,
            records,
        },
        LABEL,
    )
    .map_err(|_| "recovery-records-encode")?;
    crate::ledger::write_bounded(path, &bytes, MAX_BYTES, LABEL)
        .map_err(|_| "recovery-records-write")?;
    Ok(true)
}

/// Keep `record` as its card's one candidate: it replaces any record of
/// the same card, the same client generation or the same native session,
/// and the oldest record goes when the file is full. `false` when the same
/// session of the same client is already recorded.
pub(crate) fn upsert(records: &mut Vec<Record>, record: Record) -> bool {
    if records.iter().any(|known| known.same(&record)) {
        return false;
    }
    records.retain(|known| {
        known.card_id != record.card_id
            && known.client != record.client
            && known.native_id != record.native_id
    });
    records.push(record);
    while records.len() > MAX_RECORDS {
        let oldest = records
            .iter()
            .enumerate()
            .min_by_key(|(_, known)| known.recorded_at)
            .map(|(at, _)| at)
            .unwrap_or(0);
        records.remove(oldest);
    }
    true
}

/// A user-caused end of `client`'s session `native_id` deletes its record;
/// any other end deletes nothing.
pub(crate) fn ended(
    records: &mut Vec<Record>,
    client: Gen,
    native_id: &str,
    reason: EndReason,
) -> bool {
    if !matches!(reason, EndReason::PromptInputExit | EndReason::Logout) {
        return false;
    }
    let before = records.len();
    records.retain(|known| !(known.client == client && known.native_id == native_id));
    records.len() != before
}

// ---------- the recorder ------------------------------------------------------------

/// What the status listener read of one lifecycle line while its helper was
/// still connected: still only a claim about a pane.
pub(crate) struct Observation {
    wire: Wire,
    client: Gen,
    chain: Vec<u32>,
    table: ProcessTable,
}

fn generation(table: &ProcessTable, pid: u32) -> Option<Gen> {
    table
        .get(&pid)
        .filter(|info| info.start_seconds != 0)
        .map(|info| Gen::of(pid, (info.start_seconds, info.start_micros)))
}

/// The listener's part, on its own thread while the helper is connected:
/// a Claude Code line, a peer that executes the bundled helper, and that
/// peer's direct parent — read from one process table taken now. Kernel
/// reads only; `None` records nothing.
pub(crate) fn observation(
    wire: Wire,
    peer: Option<u32>,
    helper: Option<&Path>,
    table: impl FnOnce() -> ProcessTable,
    runs_helper: &dyn Fn(u32, &Path) -> bool,
) -> Option<Observation> {
    if !matches!(wire.event, Event::Claude(_)) {
        return None;
    }
    let peer = peer?;
    let table = table();
    generation(&table, peer)?;
    if !runs_helper(peer, helper?) {
        return None;
    }
    let parent = table.get(&peer)?.ppid;
    let client = generation(&table, parent)?;
    let chain = crate::procinfo::ancestry_in(&table, peer, crate::agent_origin::ORIGIN_HOPS);
    Some(Observation {
        wire,
        client,
        chain,
        table,
    })
}

/// Everything the recorder reaches. Production is `Recording::live`; tests
/// describe a topology with closures.
pub(crate) struct Recording<'a> {
    pub(crate) socket: &'a str,
    /// Deck's server's panes; `None` when the listing failed.
    pub(crate) rows: &'a dyn Fn() -> Option<Vec<PaneRow>>,
    pub(crate) cards: &'a dyn Fn() -> Option<Vec<(String, String)>>,
    pub(crate) table: &'a dyn Fn() -> ProcessTable,
    pub(crate) standing: &'a dyn Fn(Gen) -> Option<StandingClient>,
    pub(crate) liveness: &'a dyn Fn(Gen) -> Liveness,
    pub(crate) birth: &'a dyn Fn(u32) -> Option<(u64, u32)>,
    pub(crate) store: &'a Path,
    pub(crate) now: &'a dyn Fn() -> u64,
}

fn foreground(table: &ProcessTable, pane_pid: u32) -> Option<Gen> {
    crate::agent_status::foreground_generation(table, pane_pid)
        .map(|leader| Gen::of(leader.pid(), leader.birth()))
}

/// The one card the Board binds to `session`.
fn card_of(cards: &[(String, String)], session: &str) -> Option<String> {
    let bound: Vec<&String> = cards
        .iter()
        .filter(|(_, s)| s == session)
        .map(|(id, _)| id)
        .collect();
    match bound.as_slice() {
        [card] => Some((*card).clone()),
        _ => None,
    }
}

/// Settle one observation: write, delete or refuse. `Ok` names what was
/// done; both sides are closed words.
pub(crate) fn observe(
    observation: Observation,
    rec: &Recording,
) -> Result<&'static str, &'static str> {
    let Observation {
        wire,
        client,
        chain,
        table,
    } = observation;
    if wire.socket != rec.socket {
        return Err("recovery-other-socket");
    }
    let told = match wire.event {
        Event::Claude(told) => told,
        _ => return Err("recovery-other-source"),
    };
    if let Told::End(reason) = told {
        let changed = update_at(rec.store, |records| {
            ended(records, client, &wire.native_id, reason)
        })?;
        return Ok(if changed { "ended" } else { "end-kept" });
    }
    let rows = (rec.rows)().ok_or("recovery-no-listing")?;
    let row = rows
        .iter()
        .find(|row| row.pane_id == wire.pane && row.server_pid == wire.server_pid)
        .ok_or("recovery-no-pane")?;
    if !chain.contains(&row.pane_pid)
        || foreground(&table, row.pane_pid) != Some(client)
        || !crate::agent_origin::terminal_continuous(&table, &chain, client.pid, row.pane_pid)
    {
        return Err("recovery-foreign-client");
    }
    let standing = {
        let _deadline = crate::session_runtime::Deadline::until(Instant::now() + STANDING_BUDGET);
        (rec.standing)(client)
    }
    .ok_or("recovery-not-supported")?;
    // what was true before a call of unknown length is read again after it
    if foreground(&(rec.table)(), row.pane_pid) != Some(client) {
        return Err("recovery-late");
    }
    let card_id = (rec.cards)()
        .and_then(|cards| card_of(&cards, &row.session_name))
        .ok_or("recovery-no-card")?;
    let server = (rec.birth)(row.server_pid)
        .map(|birth| Gen::of(row.server_pid, birth))
        .ok_or("recovery-no-server")?;
    let record = Record {
        card_id,
        session: row.session_name.clone(),
        source: SOURCE.into(),
        native_id: wire.native_id,
        client,
        server,
        pane: row.pane_id.clone(),
        executable: standing.image,
        cwd: standing.cwd.path().to_path_buf(),
        recorded_at: (rec.now)(),
    };
    if !record.valid() {
        return Err("recovery-not-supported");
    }
    let session = record.session.clone();
    let changed = update_at(rec.store, |records| upsert(records, record))?;
    if changed {
        applog(&format!(
            "[agent-recovery] recorded {}",
            session_tag(&session)
        ));
    }
    Ok(if changed { "recorded" } else { "unchanged" })
}

/// The records of a server that still runs and whose client is no longer
/// in its pane's foreground. `suspects` remembers when each was first seen
/// so; a record seen so again `PRUNE_GRACE` later is deleted. Returns how
/// many were deleted.
pub(crate) fn prune(
    rec: &Recording,
    suspects: &mut HashMap<(String, Gen), Instant>,
    now: Instant,
) -> usize {
    let Ok(records) = load_at(rec.store) else {
        return 0;
    };
    if records.is_empty() {
        suspects.clear();
        return 0;
    }
    let Some(rows) = (rec.rows)() else {
        return 0;
    };
    let table = (rec.table)();
    let mut stale = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for record in &records {
        if (rec.liveness)(record.server) != Liveness::Alive {
            continue; // a lost server is the offer's, not this
        }
        let healthy = rows
            .iter()
            .find(|row| row.pane_id == record.pane && row.server_pid == record.server.pid)
            .is_some_and(|row| foreground(&table, row.pane_pid) == Some(record.client));
        let key = (record.card_id.clone(), record.client);
        seen.insert(key.clone());
        if healthy {
            suspects.remove(&key);
            continue;
        }
        let since = *suspects.entry(key).or_insert(now);
        if now.saturating_duration_since(since) >= PRUNE_GRACE {
            stale.push(record.clone());
        }
    }
    suspects.retain(|key, _| seen.contains(key));
    if stale.is_empty() {
        return 0;
    }
    let removed = update_at(rec.store, |records| {
        let before = records.len();
        records.retain(|known| !stale.iter().any(|old| old.same(known)));
        records.len() != before
    })
    .unwrap_or(false);
    if removed {
        for record in &stale {
            suspects.remove(&(record.card_id.clone(), record.client));
        }
        applog(&format!("[agent-recovery] pruned count={}", stale.len()));
        stale.len()
    } else {
        0
    }
}

impl Recording<'static> {
    fn live() -> Self {
        fn rows() -> Option<Vec<PaneRow>> {
            match crate::tmux::list_panes() {
                Ok(rows) => Some(rows),
                Err(error) if error.message() == "tmux list-panes failed: no current target" => {
                    Some(Vec::new())
                }
                Err(_) => None,
            }
        }
        fn standing(client: Gen) -> Option<StandingClient> {
            let home = dirs::home_dir()?;
            let capability = &super::claude::CAPABILITY;
            crate::procinfo::standing_client(
                client.pid,
                client.birth(),
                &crate::procinfo::ClientSpec {
                    product: capability.product,
                    launcher: capability.launcher,
                    grammar: capability.grammar,
                    home: &home,
                },
            )
        }
        static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
        Self {
            socket: crate::tmux::socket(),
            rows: &rows,
            cards: &crate::tmux_lifecycle::board_cards,
            table: &crate::procinfo::processes,
            standing: &standing,
            liveness: &|g| crate::procinfo::generation_liveness(g.pid, g.birth()),
            birth: &crate::procinfo::process_start,
            store: PATH.get_or_init(store),
            now: &crate::datadir::now_epoch,
        }
    }
}

static RECORDER: std::sync::OnceLock<Option<std::sync::mpsc::SyncSender<Observation>>> =
    std::sync::OnceLock::new();

/// Hand one observation to the recorder without waiting; the worker starts
/// with the first one. A full queue drops it: a session not recorded.
pub(crate) fn hand(observation: Observation) -> Result<(), &'static str> {
    RECORDER
        .get_or_init(start)
        .as_ref()
        .ok_or("recovery-unavailable")?
        .try_send(observation)
        .map_err(|_| "recovery-busy")
}

fn start() -> Option<std::sync::mpsc::SyncSender<Observation>> {
    let (queue, waiting) = std::sync::mpsc::sync_channel::<Observation>(QUEUE);
    std::thread::Builder::new()
        .name("deck-agent-recovery".into())
        .spawn(move || {
            use std::sync::mpsc::RecvTimeoutError;
            let rec = Recording::live();
            let mut suspects = HashMap::new();
            let mut pruned_at = Instant::now();
            loop {
                match waiting.recv_timeout(PRUNE_EVERY) {
                    Ok(observation) => {
                        if let Err(reason) = observe(observation, &rec) {
                            if !matches!(reason, "recovery-no-card" | "recovery-other-socket") {
                                applog(&format!("[agent-recovery] not-recorded reason={reason}"));
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                if pruned_at.elapsed() >= PRUNE_EVERY {
                    pruned_at = Instant::now();
                    prune(&rec, &mut suspects, pruned_at);
                }
            }
        })
        .ok()?;
    Some(queue)
}

// ---------- the offer ---------------------------------------------------------------

/// Why an offered card cannot be ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Blocked {
    /// Claude's lifecycle hooks are not installed now: nothing could
    /// confirm the session was entered.
    ObservationOff,
    /// The card's shell is already open.
    ShellOpen,
    /// The recorded working directory no longer exists.
    DirectoryMissing,
    /// The recorded program is no longer there.
    ProgramMissing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfferEntry {
    pub(crate) card_id: String,
    pub(crate) session: String,
    pub(crate) source: &'static str,
    pub(crate) blocked: Option<Blocked>,
    /// When the session was recorded (epoch seconds).
    pub(crate) recorded_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OfferView {
    /// `None`: nothing to ask.
    pub(crate) token: Option<String>,
    /// `false`: there is no current Board yet, so nothing can be offered
    /// now; ask again after the Board loaded.
    pub(crate) board_known: bool,
    pub(crate) entries: Vec<OfferEntry>,
}

struct Offer {
    token: String,
    made: Instant,
    /// Every lost record this offer read: what answering it deletes.
    lost: Vec<Record>,
    /// The cards that could be ticked.
    shown: Vec<String>,
}

static OFFER: Mutex<Option<Offer>> = Mutex::new(None);

/// What one offer reads. `liveness` answers for a generation; `open` is
/// the sessions Deck's server has now.
pub(crate) struct OfferInput<'a> {
    pub(crate) records: &'a [Record],
    pub(crate) cards: Option<&'a [(String, String)]>,
    pub(crate) open: &'a dyn Fn(&str) -> bool,
    pub(crate) liveness: &'a dyn Fn(Gen) -> Liveness,
    pub(crate) observation: bool,
    pub(crate) is_dir: &'a dyn Fn(&Path) -> bool,
    pub(crate) is_file: &'a dyn Fn(&Path) -> bool,
}

/// The pure offer: the lost records, and the entries the user is shown.
pub(crate) fn offer_of(input: &OfferInput) -> (Vec<Record>, Vec<OfferEntry>) {
    let lost: Vec<Record> = input
        .records
        .iter()
        .filter(|record| {
            (input.liveness)(record.server) == Liveness::Gone
                && (input.liveness)(record.client) == Liveness::Gone
        })
        .cloned()
        .collect();
    let Some(cards) = input.cards else {
        return (lost, Vec::new());
    };
    let mut entries: Vec<OfferEntry> = lost
        .iter()
        .filter(|record| card_of(cards, &record.session).as_deref() == Some(&record.card_id))
        .map(|record| OfferEntry {
            card_id: record.card_id.clone(),
            session: record.session.clone(),
            source: SOURCE,
            blocked: if !input.observation {
                Some(Blocked::ObservationOff)
            } else if (input.open)(&record.session) {
                Some(Blocked::ShellOpen)
            } else if !(input.is_dir)(&record.cwd) {
                Some(Blocked::DirectoryMissing)
            } else if !(input.is_file)(&record.executable) {
                Some(Blocked::ProgramMissing)
            } else {
                None
            },
            recorded_at: record.recorded_at,
        })
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.recorded_at));
    (lost, entries)
}

fn expired() -> DeckError {
    DeckError::new(ErrorKind::ContextChanged, "recovery-offer-expired")
}

fn records_error(reason: &'static str) -> DeckError {
    DeckError::new(ErrorKind::Recovery, reason)
}

/// Read the records and make the offer `env` shows: read-only, except that
/// lost records no card can take any more are deleted when nothing else is
/// left to ask about.
pub(crate) fn offer(env: &Env, store: &Path) -> Result<OfferView, DeckError> {
    let records = match load_at(store) {
        Ok(records) => records,
        Err(reason) => {
            applog(&format!("[agent-recovery] offer-refused reason={reason}"));
            return Err(records_error(reason));
        }
    };
    let cards = (env.cards)();
    let open: Vec<String> = match env.rows() {
        Ok(rows) => rows.into_iter().map(|row| row.session_name).collect(),
        Err(error) if error.message() == "tmux list-panes failed: no current target" => Vec::new(),
        Err(error) => return Err(error),
    };
    let observation = env
        .adapter(SOURCE)
        .is_some_and(|adapter| adapter.observation && adapter.baseline);
    let (lost, entries) = offer_of(&OfferInput {
        records: &records,
        cards: cards.as_deref(),
        open: &|session| open.iter().any(|s| s == session),
        liveness: &|g| crate::procinfo::generation_liveness(g.pid, g.birth()),
        observation,
        is_dir: &|path| path.is_absolute() && path.is_dir(),
        is_file: &|path| path.is_absolute() && path.is_file(),
    });
    if cards.is_none() {
        return Ok(OfferView {
            token: None,
            board_known: false,
            entries: Vec::new(),
        });
    }
    if entries.is_empty() {
        if !lost.is_empty() {
            let _ = consume(store, &lost);
        }
        *OFFER.lock_or_recover() = None;
        return Ok(OfferView {
            token: None,
            board_known: true,
            entries,
        });
    }
    let token = super::opaque_token()?;
    applog(&format!(
        "[agent-recovery] offer entries={} available={}",
        entries.len(),
        entries.iter().filter(|e| e.blocked.is_none()).count()
    ));
    *OFFER.lock_or_recover() = Some(Offer {
        token: token.clone(),
        made: Instant::now(),
        lost,
        shown: entries
            .iter()
            .filter(|entry| entry.blocked.is_none())
            .map(|entry| entry.card_id.clone())
            .collect(),
    });
    Ok(OfferView {
        token: Some(token),
        board_known: true,
        entries,
    })
}

/// Delete exactly these records.
fn consume(store: &Path, lost: &[Record]) -> Result<(), DeckError> {
    update_at(store, |records| {
        let before = records.len();
        records.retain(|known| !lost.iter().any(|old| old.same(known)));
        records.len() != before
    })
    .map(|_| ())
    .map_err(records_error)
}

fn take_offer(token: &str) -> Result<Offer, DeckError> {
    OFFER
        .lock_or_recover()
        .take()
        .filter(|offer| offer.token == token && offer.made.elapsed() < OFFER_TTL)
        .ok_or_else(expired)
}

/// The user declined: the offer's records are deleted and nothing starts.
pub(crate) fn decline(token: &str, store: &Path) -> Result<(), DeckError> {
    let offer = take_offer(token)?;
    consume(store, &offer.lost)?;
    applog(&format!(
        "[agent-recovery] declined records={}",
        offer.lost.len()
    ));
    Ok(())
}

/// The user's confirmation of one offer.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecoveryApproval {
    pub(crate) token: String,
    pub(crate) cards: Vec<String>,
}

/// One confirmed recovery, inside the caller's guards (the lifecycle
/// operation gate and the exclusive activity guard, `held`). `server_pid`
/// is Deck's server as read just now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    env: &Env,
    store: &Path,
    request_id: &str,
    approval: &RecoveryApproval,
    server_pid: u32,
    restore_shells: bool,
    held: &std::sync::RwLockWriteGuard<'static, ()>,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<Option<ReentryResult>, DeckError> {
    let offer = take_offer(&approval.token)?;
    let mut seen = std::collections::HashSet::new();
    if approval.cards.is_empty()
        || !approval
            .cards
            .iter()
            .all(|card| seen.insert(card.as_str()) && offer.shown.contains(card))
    {
        return Err(expired());
    }
    // what the file says now, for each ticked card: the very record the
    // offer read, its server and client still gone
    let now = load_at(store).map_err(records_error)?;
    let chosen: Vec<(String, Option<Record>)> = approval
        .cards
        .iter()
        .map(|card| {
            let record = offer
                .lost
                .iter()
                .find(|record| record.card_id == *card)
                .filter(|record| now.iter().any(|known| known.same(record)))
                .filter(|record| {
                    let gone = |g: Gen| {
                        crate::procinfo::generation_liveness(g.pid, g.birth()) == Liveness::Gone
                    };
                    gone(record.server) && gone(record.client)
                })
                .cloned();
            (card.clone(), record)
        })
        .collect();
    // Once only: every record the offer read is deleted BEFORE anything is
    // created. If that fails, nothing is launched.
    consume(store, &offer.lost)?;
    let preparing = crate::session_runtime::Deadline::until(Instant::now() + PREPARE_SHARE);
    // whose code the recorded program must be: the adapter of the recorded
    // source, as this environment has it
    let product = env
        .adapter(SOURCE)
        .map(|adapter| adapter.capability.product)
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "reentry-unavailable"))?;
    let mut read: Vec<(PathBuf, crate::procinfo::ExecutableRef)> = Vec::new();
    let mut entries = Vec::new();
    for (card, record) in chosen {
        let Some(record) = record else {
            entries.push(Recovered::refused(card, Reason::IdentityUnavailable));
            continue;
        };
        let cwd = crate::procinfo::DirRef::recorded(&record.cwd).filter(|cwd| cwd.is_dir());
        let Some(cwd) = cwd else {
            entries.push(Recovered::refused_session(
                card,
                record.session,
                Reason::UserActionRequired,
            ));
            continue;
        };
        let known = read
            .iter()
            .find(|(path, executable)| *path == record.executable && executable.same_state())
            .map(|(_, executable)| executable.clone());
        let executable = match known {
            Some(executable) => Ok(executable),
            None => crate::procinfo::ExecutableRef::read(&record.executable, product)
                .inspect(|executable| read.push((record.executable.clone(), executable.clone()))),
        };
        // a check that answered after the share is no answer
        let executable = executable.and_then(|executable| {
            crate::session_runtime::check_deadline()
                .map(|()| executable)
                .map_err(|_| crate::procinfo::Unavailable::Unverified)
        });
        match executable {
            Ok(executable) => entries.push(Recovered::ready(
                card,
                record.session,
                record.native_id,
                (record.client.pid, record.client.birth()),
                executable,
                cwd,
            )),
            Err(why) => entries.push(Recovered::refused_session(
                card,
                record.session,
                super::unavailable(why),
            )),
        }
    }
    drop(preparing);
    applog(&format!(
        "[agent-recovery] begin entries={} ready={}",
        entries.len(),
        entries.iter().filter(|entry| entry.is_ready()).count()
    ));
    let mut plan = super::Plan::recovered(env, request_id, entries)?;
    let post_end = Instant::now() + super::POST_BUDGET;
    plan.reopen(env, server_pid, restore_shells, post_end, held, progress);
    drop(plan);
    Ok(super::known(request_id))
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *OFFER.lock_or_recover() = None;
}

#[cfg(test)]
pub(crate) fn load_for_tests(path: &Path) -> Result<Vec<Record>, &'static str> {
    load_at(path)
}

#[cfg(test)]
pub(crate) fn write_for_tests(path: &Path, records: Vec<Record>) {
    update_at(path, |all| {
        *all = records;
        true
    })
    .expect("records written");
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
