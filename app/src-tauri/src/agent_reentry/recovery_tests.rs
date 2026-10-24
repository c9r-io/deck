//! The record, the recorder's attribution and pruning, and the pure offer.
//! The whole operation against a real tmux server, helper and stand-in
//! client is in `agent_reentry/tests.rs` (`a_lost_server_is_recovered_…`).

use super::*;
use crate::agent_reentry::claude::{self, StartSource};
use crate::procinfo::{DirRef, ProcessInfo};
use crate::test_dir::TestDir;
use std::cell::{Cell, RefCell};

const ID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";
const OTHER: &str = "8e8889c6-251a-4388-b3a3-4719ad442bce";

const SHELL: u32 = 100;
const CLIENT: u32 = 200;
const HELPER: u32 = 300;
const SERVER: u32 = 50;
const TTY: u32 = 7;

fn info(pid: u32, ppid: u32, pgid: u32, tty: u32, tty_pgid: u32, start: u64) -> ProcessInfo {
    ProcessInfo {
        pid,
        ppid,
        pgid,
        tty,
        tty_pgid,
        start_seconds: start,
        start_micros: 0,
    }
}

/// A pane's shell, the client in its foreground and the hook helper the
/// client ran: the topology a real `SessionStart` has.
fn table(foreground: u32) -> ProcessTable {
    [
        info(SHELL, 1, SHELL, TTY, foreground, 10),
        info(CLIENT, SHELL, CLIENT, TTY, foreground, 20),
        info(HELPER, CLIENT, HELPER, 0, 0, 30),
    ]
    .into_iter()
    .map(|info| (info.pid, info))
    .collect()
}

fn client() -> Gen {
    Gen::of(CLIENT, (20, 0))
}

fn wire(event: Event, native_id: &str) -> Wire {
    Wire {
        event,
        native_id: native_id.into(),
        socket: "deck".into(),
        server_pid: SERVER,
        pane: "%3".into(),
    }
}

fn row(session: &str) -> PaneRow {
    PaneRow {
        server_pid: SERVER,
        session_name: session.into(),
        pane_id: "%3".into(),
        pane_pid: SHELL,
        ..PaneRow::default()
    }
}

fn record(card: &str, native_id: &str, client: Gen) -> Record {
    Record {
        card_id: card.into(),
        session: format!("deck-{card}"),
        source: "claude-code".into(),
        native_id: native_id.into(),
        client,
        server: Gen::of(SERVER, (5, 0)),
        pane: "%3".into(),
        executable: "/Applications/claude".into(),
        cwd: "/work".into(),
        recorded_at: 1_800_000_000,
    }
}

/// The recorder's world: one pane `%3` of session `deck-A` on server 50,
/// card `A` bound to it, a client that qualifies, a server that runs.
struct Fake {
    dir: TestDir,
    rows: RefCell<Option<Vec<PaneRow>>>,
    cards: RefCell<Option<Vec<(String, String)>>>,
    now_table: RefCell<ProcessTable>,
    standing: Cell<bool>,
    alive: RefCell<Vec<Gen>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            dir: TestDir::new("agent-recovery"),
            rows: RefCell::new(Some(vec![row("deck-A")])),
            cards: RefCell::new(Some(vec![("A".into(), "deck-A".into())])),
            now_table: RefCell::new(table(CLIENT)),
            standing: Cell::new(true),
            alive: RefCell::new(vec![Gen::of(SERVER, (5, 0)), client()]),
        }
    }

    fn store(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    fn with<R>(&self, work: impl FnOnce(&Recording) -> R) -> R {
        let store = self.store();
        let rows = || self.rows.borrow().clone();
        let cards = || self.cards.borrow().clone();
        let now_table = || self.now_table.borrow().clone();
        let standing = |_: Gen| {
            self.standing.get().then(|| StandingClient {
                image: "/Applications/claude".into(),
                cwd: DirRef::for_tests(Path::new("/work")),
            })
        };
        let liveness = |g: Gen| {
            if self.alive.borrow().contains(&g) {
                Liveness::Alive
            } else {
                Liveness::Gone
            }
        };
        let birth = |pid: u32| (pid == SERVER).then_some((5, 0));
        let now = || 1_800_000_000;
        work(&Recording {
            socket: "deck",
            rows: &rows,
            cards: &cards,
            table: &now_table,
            standing: &standing,
            liveness: &liveness,
            birth: &birth,
            store: &store,
            now: &now,
        })
    }

    fn observe(&self, event: Event, native_id: &str) -> Result<&'static str, &'static str> {
        self.observe_on(event, native_id, table(CLIENT))
    }

    fn observe_on(
        &self,
        event: Event,
        native_id: &str,
        seen: ProcessTable,
    ) -> Result<&'static str, &'static str> {
        let observation = observation(
            wire(event, native_id),
            Some(HELPER),
            Some(Path::new("/bundle/deck-lifecycle-helper")),
            || seen,
            &|pid, _| pid == HELPER,
        )
        .ok_or("no-observation")?;
        self.with(|rec| observe(observation, rec))
    }

    fn records(&self) -> Vec<Record> {
        load_at(&self.store()).expect("readable records")
    }
}

fn start(source: StartSource) -> Event {
    claude::started(source)
}

fn end(reason: EndReason) -> Event {
    claude::ended(reason)
}

#[test]
fn a_start_records_the_cards_current_session_and_a_user_exit_deletes_it() {
    let fake = Fake::new();
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Ok("recorded")
    );
    let records = fake.records();
    assert_eq!(records.len(), 1);
    let kept = &records[0];
    assert_eq!(
        (
            kept.card_id.as_str(),
            kept.session.as_str(),
            kept.native_id.as_str()
        ),
        ("A", "deck-A", ID)
    );
    assert_eq!(kept.client, client());
    assert_eq!(kept.server, Gen::of(SERVER, (5, 0)));
    assert_eq!(kept.executable, Path::new("/Applications/claude"));
    assert_eq!(kept.cwd, Path::new("/work"));
    // the same start again writes nothing
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Ok("unchanged")
    );
    // `/clear`: the end names nothing, the next start replaces the session
    assert_eq!(fake.observe(end(EndReason::Clear), ID), Ok("end-kept"));
    assert_eq!(
        fake.observe(start(StartSource::Clear), OTHER),
        Ok("recorded")
    );
    assert_eq!(
        fake.records()
            .iter()
            .map(|r| r.native_id.as_str())
            .collect::<Vec<_>>(),
        [OTHER]
    );
    // an end the user did not cause (a killed server says `other`) keeps it
    assert_eq!(fake.observe(end(EndReason::Other), OTHER), Ok("end-kept"));
    // an end of another session of this client deletes nothing
    assert_eq!(
        fake.observe(end(EndReason::PromptInputExit), ID),
        Ok("end-kept")
    );
    assert_eq!(fake.records().len(), 1);
    // the user's own exit deletes it
    assert_eq!(
        fake.observe(end(EndReason::PromptInputExit), OTHER),
        Ok("ended")
    );
    assert!(fake.records().is_empty());
    assert_eq!(fake.observe(start(StartSource::Resume), ID), Ok("recorded"));
    assert_eq!(fake.observe(end(EndReason::Logout), ID), Ok("ended"));
    assert!(fake.records().is_empty());
}

#[test]
fn only_the_panes_own_foreground_client_of_a_bound_card_is_recorded() {
    let fake = Fake::new();
    // another socket's line
    let mut other = wire(start(StartSource::Startup), ID);
    other.socket = "deck-dev".into();
    let observation = observation(
        other,
        Some(HELPER),
        Some(Path::new("/h")),
        || table(CLIENT),
        &|_, _| true,
    )
    .unwrap();
    assert_eq!(
        fake.with(|rec| observe(observation, rec)),
        Err("recovery-other-socket")
    );
    // a peer that is not the bundled helper, or no peer at all
    assert!(observation_of(start(StartSource::Startup), Some(HELPER), &|_, _| false).is_none());
    assert!(observation_of(start(StartSource::Startup), None, &|_, _| true).is_none());
    // a Codex line is not this record's
    let codex = crate::agent_reentry::codex::started(StartSource::Startup);
    assert!(observation_of(codex, Some(HELPER), &|_, _| true).is_none());
    // the client is not the pane's foreground (a subagent's or a tool's hook)
    assert_eq!(
        fake.observe_on(start(StartSource::Startup), ID, table(SHELL)),
        Err("recovery-foreign-client")
    );
    // the pane the line names is not on Deck's server
    *fake.rows.borrow_mut() = Some(vec![PaneRow {
        server_pid: SERVER + 1,
        ..row("deck-A")
    }]);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-no-pane")
    );
    *fake.rows.borrow_mut() = None;
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-no-listing")
    );
    *fake.rows.borrow_mut() = Some(vec![row("deck-A")]);
    // not the product, or not a supported launch
    fake.standing.set(false);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-not-supported")
    );
    fake.standing.set(true);
    // the client left the foreground during its check
    *fake.now_table.borrow_mut() = table(SHELL);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-late")
    );
    *fake.now_table.borrow_mut() = table(CLIENT);
    // no card, or two cards, name the session
    *fake.cards.borrow_mut() = Some(vec![("B".into(), "deck-B".into())]);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-no-card")
    );
    *fake.cards.borrow_mut() = Some(vec![
        ("A".into(), "deck-A".into()),
        ("A2".into(), "deck-A".into()),
    ]);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-no-card")
    );
    *fake.cards.borrow_mut() = None;
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Err("recovery-no-card")
    );
    assert!(fake.records().is_empty(), "nothing was written");
}

fn observation_of(
    event: Event,
    peer: Option<u32>,
    runs_helper: &dyn Fn(u32, &Path) -> bool,
) -> Option<Observation> {
    observation(
        wire(event, ID),
        peer,
        Some(Path::new("/bundle/deck-lifecycle-helper")),
        || table(CLIENT),
        runs_helper,
    )
}

#[test]
fn one_record_per_card_client_and_session_and_the_oldest_goes_when_full() {
    let mut records = Vec::new();
    assert!(upsert(&mut records, record("A", ID, client())));
    // the same card in another session: replaced
    assert!(upsert(
        &mut records,
        record("A", OTHER, Gen::of(201, (1, 0)))
    ));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].native_id, OTHER);
    // the same session moved to another card: replaced
    assert!(upsert(
        &mut records,
        record("B", OTHER, Gen::of(202, (1, 0)))
    ));
    assert_eq!(
        records
            .iter()
            .map(|r| r.card_id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
    let mut full: Vec<Record> = (0..MAX_RECORDS)
        .map(|n| {
            let mut kept = record(&format!("C{n}"), ID, Gen::of(1000 + n as u32, (1, 0)));
            kept.native_id = format!("{:08x}-251a-4388-b3a3-4719ad442bce", n);
            kept.recorded_at = 100 + n as u64;
            kept
        })
        .collect();
    let mut newest = record("new", ID, Gen::of(5000, (1, 0)));
    newest.native_id = OTHER.into();
    newest.recorded_at = 10_000;
    assert!(upsert(&mut full, newest));
    assert_eq!(full.len(), MAX_RECORDS);
    assert!(!full.iter().any(|r| r.card_id == "C0"), "the oldest went");
    assert!(full.iter().any(|r| r.card_id == "new"));
}

#[test]
fn a_file_that_cannot_be_used_is_left_untouched_and_used_for_nothing() {
    let dir = TestDir::new("agent-recovery-load");
    let path = dir.join(FILE);
    assert_eq!(load_at(&path), Ok(Vec::new()), "no file: no records");
    write_for_tests(&path, vec![record("A", ID, client())]);
    assert_eq!(load_at(&path).unwrap().len(), 1);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for (bytes, reason) in [
        (
            r#"{"v":2,"records":[]}"#.to_string(),
            "recovery-records-future",
        ),
        ("not json".to_string(), "recovery-records-unreadable"),
        (
            r#"{"v":1,"records":[],"extra":1}"#.to_string(),
            "recovery-records-unreadable",
        ),
        (
            serde_json::json!({"v": 1, "records": [{"cardId": "A"}]}).to_string(),
            "recovery-records-unreadable",
        ),
        ("x".repeat(MAX_BYTES + 1), "recovery-records-unreadable"),
    ] {
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(load_at(&path), Err(reason));
        // nothing is written over it
        assert_eq!(
            update_at(&path, |records| upsert(records, record("A", ID, client()))),
            Err(reason)
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
    // a record that is not one this build writes makes the file unusable
    let mut bad = record("A", ID, client());
    bad.native_id = "latest".into();
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"v": 1, "records": [bad]})).unwrap(),
    )
    .unwrap();
    assert_eq!(load_at(&path), Err("recovery-records-unreadable"));
}

#[test]
fn a_client_that_ended_alone_is_pruned_after_the_grace_and_a_lost_server_is_left_alone() {
    let fake = Fake::new();
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Ok("recorded")
    );
    let mut suspects = HashMap::new();
    let t0 = Instant::now();
    assert_eq!(fake.with(|rec| prune(rec, &mut suspects, t0)), 0, "healthy");
    assert!(suspects.is_empty());
    // the client ended while its shell lives on
    *fake.now_table.borrow_mut() = table(SHELL);
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t0)),
        0,
        "first sighting"
    );
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t0 + PRUNE_GRACE / 2)),
        0
    );
    // it came back before the grace (another sighting): the suspicion ends
    *fake.now_table.borrow_mut() = table(CLIENT);
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t0 + PRUNE_GRACE / 2)),
        0
    );
    assert!(suspects.is_empty());
    *fake.now_table.borrow_mut() = table(SHELL);
    let t1 = t0 + PRUNE_GRACE;
    assert_eq!(fake.with(|rec| prune(rec, &mut suspects, t1)), 0);
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t1 + PRUNE_GRACE)),
        1
    );
    assert!(fake.records().is_empty());
    // a server that is gone: the record waits for the offer, however long
    *fake.now_table.borrow_mut() = table(CLIENT);
    assert_eq!(
        fake.observe(start(StartSource::Startup), ID),
        Ok("recorded")
    );
    fake.alive.borrow_mut().clear();
    *fake.rows.borrow_mut() = Some(Vec::new());
    let t2 = t1 + PRUNE_GRACE * 3;
    assert_eq!(fake.with(|rec| prune(rec, &mut suspects, t2)), 0);
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t2 + PRUNE_GRACE * 2)),
        0
    );
    assert_eq!(fake.records().len(), 1);
    // a listing that failed decides nothing
    fake.alive.borrow_mut().push(Gen::of(SERVER, (5, 0)));
    *fake.rows.borrow_mut() = None;
    assert_eq!(
        fake.with(|rec| prune(rec, &mut suspects, t2 + PRUNE_GRACE * 5)),
        0
    );
    assert_eq!(fake.records().len(), 1);
}

/// The pure offer over `records`, with deck-B's shell open.
fn offer_with(
    records: &[Record],
    cards: Option<&[(String, String)]>,
    liveness: &dyn Fn(Gen) -> Liveness,
    observation: bool,
    paths: (bool, bool),
) -> (Vec<Record>, Vec<OfferEntry>) {
    offer_of(&OfferInput {
        records,
        cards,
        open: &|session| session == "deck-B",
        liveness,
        observation,
        is_dir: &|_| paths.0,
        is_file: &|_| paths.1,
    })
}

#[test]
fn only_a_record_whose_server_and_client_are_both_gone_is_offered() {
    let a = record("A", ID, client());
    let mut b = record("B", OTHER, Gen::of(201, (1, 0)));
    b.recorded_at = a.recorded_at + 60;
    let records = vec![a.clone(), b.clone()];
    let pair = |id: &str, session: &str| (id.to_string(), session.to_string());
    let cards = vec![pair("A", "deck-A"), pair("B", "deck-B")];
    let alive = RefCell::new(vec![a.server]);
    let liveness = |g: Gen| {
        if alive.borrow().contains(&g) {
            Liveness::Alive
        } else {
            Liveness::Gone
        }
    };
    // the server still runs: nothing is lost
    let (lost, entries) = offer_with(&records, Some(&cards), &liveness, true, (true, true));
    assert!(lost.is_empty() && entries.is_empty());
    alive.borrow_mut().clear();
    let (lost, entries) = offer_with(&records, Some(&cards), &liveness, true, (true, true));
    assert_eq!(lost.len(), 2);
    // newest first; an open shell is listed and cannot be ticked
    assert_eq!(
        entries
            .iter()
            .map(|e| (e.card_id.as_str(), e.blocked))
            .collect::<Vec<_>>(),
        [("B", Some(Blocked::ShellOpen)), ("A", None)]
    );
    // without the observation nothing can be confirmed
    let (_, entries) = offer_with(&records, Some(&cards), &liveness, false, (true, true));
    assert!(entries
        .iter()
        .all(|e| e.blocked == Some(Blocked::ObservationOff)));
    // a client generation that may still run is not lost
    alive.borrow_mut().push(a.client);
    let (lost, _) = offer_with(&records, Some(&cards), &liveness, true, (true, true));
    assert_eq!(lost, vec![b.clone()]);
    alive.borrow_mut().clear();
    // the Board no longer binds a card to its session: lost, not offered
    let rebound = vec![pair("A", "deck-A"), pair("C", "deck-B")];
    let (lost, entries) = offer_with(&records, Some(&rebound), &liveness, true, (true, true));
    assert_eq!(lost.len(), 2);
    assert_eq!(
        entries
            .iter()
            .map(|e| e.card_id.as_str())
            .collect::<Vec<_>>(),
        ["A"]
    );
    // no current Board: nothing can be offered
    let (_, entries) = offer_with(&records, None, &liveness, true, (true, true));
    assert!(entries.is_empty());
    // the recorded directory or program is gone
    for (paths, want) in [
        ((false, true), Blocked::DirectoryMissing),
        ((true, false), Blocked::ProgramMissing),
    ] {
        let (_, entries) = offer_with(&records, Some(&cards), &liveness, true, paths);
        assert_eq!(entries[1].blocked, Some(want));
    }
}

#[test]
fn an_offer_is_answered_once_by_its_own_token() {
    // the one offer slot is process-wide; the integration tests hold this too
    let _lock = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let dir = TestDir::new("agent-recovery-offer");
    let path = dir.join(FILE);
    let a = record("A", ID, client());
    let fresh = record("F", OTHER, Gen::of(201, (1, 0)));
    write_for_tests(&path, vec![a.clone(), fresh.clone()]);
    let set = |token: &str, made: Instant| {
        *OFFER.lock_or_recover() = Some(Offer {
            token: token.into(),
            made,
            lost: vec![a.clone()],
            shown: vec!["A".into()],
        });
    };
    set("T1", Instant::now());
    assert_eq!(
        decline("T2", &path).unwrap_err().message(),
        "recovery-offer-expired"
    );
    // a wrong token spends the offer too: it is single-use
    assert_eq!(
        decline("T1", &path).unwrap_err().message(),
        "recovery-offer-expired"
    );
    set("T1", Instant::now());
    decline("T1", &path).expect("declined");
    // only what the offer read is deleted
    assert_eq!(load_at(&path).unwrap(), vec![fresh.clone()]);
    assert_eq!(
        decline("T1", &path).unwrap_err().message(),
        "recovery-offer-expired"
    );
    if let Some(old) = Instant::now().checked_sub(OFFER_TTL) {
        set("T1", old);
        assert!(decline("T1", &path).is_err(), "an old offer is expired");
    }
    reset_for_tests();
}
