use super::*;
use crate::agent_status::{self, ProcessTable, STORE_TEST_LOCK};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::AtomicUsize;
use std::sync::Condvar;

const SIGNAL: Capabilities = Capabilities {
    signal: true,
    lifecycle: false,
};

fn process(pid: u32, ppid: u32, tty: u32, tty_pgid: u32, start: u64) -> ProcessInfo {
    ProcessInfo {
        pid,
        ppid,
        pgid: pid, // every process here leads its own group unless a test says otherwise
        tty,
        tty_pgid,
        start_seconds: start,
        start_micros: 7,
    }
}

/// A pane: its shell `pane_pid` on `tty` (a child of the tmux server 42) and
/// the terminal's foreground program `leader`, a child of that shell.
fn pane(pane_pid: u32, tty: u32, leader: u32, start: u64) -> Vec<ProcessInfo> {
    vec![
        process(pane_pid, 42, tty, leader, 1000),
        process(leader, pane_pid, tty, leader, start),
    ]
}

fn table(parts: &[Vec<ProcessInfo>]) -> ProcessTable {
    let mut table: ProcessTable = parts
        .iter()
        .flatten()
        .map(|info| (info.pid, *info))
        .collect();
    table.insert(42, process(42, 1, 0, 0, 1));
    table
}

fn row(session: &str, session_id: &str, pane: &str, pane_pid: u32, command: &str) -> PaneRow {
    PaneRow {
        server_pid: 42,
        session_id: session_id.into(),
        session_name: session.into(),
        window_id: "@1".into(),
        pane_id: pane.into(),
        pane_pid,
        window_active: true,
        pane_active: true,
        command: command.into(),
        ..PaneRow::default()
    }
}

/// A gate a listing waits at until the test opens it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
    waiting: AtomicUsize,
}

impl Gate {
    fn pass(&self) {
        let mut open = self.open.lock_or_recover();
        self.waiting.fetch_add(1, Ordering::SeqCst);
        while !*open {
            open = crate::sync::wait_or_recover(&self.changed, open);
        }
    }

    fn set(&self, open: bool) {
        *self.open.lock_or_recover() = open;
        self.changed.notify_all();
    }
}

/// What a test runs inside a listing, before it answers.
type During = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// A world a test can change while admission runs.
#[derive(Clone)]
struct Shared {
    table: Arc<Mutex<ProcessTable>>,
    rows: Arc<Mutex<Option<Vec<PaneRow>>>>,
    enabled: Arc<Mutex<Capabilities>>,
    listings: Arc<AtomicUsize>,
    during: During,
    gate: Arc<Gate>,
}

impl Shared {
    fn new(rows: Vec<PaneRow>, table: ProcessTable) -> Self {
        let gate = Gate::default();
        gate.set(true);
        Self {
            table: Arc::new(Mutex::new(table)),
            rows: Arc::new(Mutex::new(Some(rows))),
            enabled: Arc::new(Mutex::new(SIGNAL)),
            listings: Arc::new(AtomicUsize::new(0)),
            during: Arc::new(Mutex::new(None)),
            gate: Arc::new(gate),
        }
    }

    fn world(&self) -> World {
        let (table, rows, enabled, listings, during, gate) = (
            self.table.clone(),
            self.rows.clone(),
            self.enabled.clone(),
            self.listings.clone(),
            self.during.clone(),
            self.gate.clone(),
        );
        World {
            process: Box::new(move |pid| table.lock_or_recover().get(&pid).copied()),
            listing: Box::new(move || {
                listings.fetch_add(1, Ordering::SeqCst);
                gate.pass();
                if let Some(event) = during.lock_or_recover().as_ref() {
                    event();
                }
                rows.lock_or_recover().clone()
            }),
            enabled: Box::new(move || *enabled.lock_or_recover()),
            socket: "deck".into(),
        }
    }

    fn rows(&self) -> Vec<PaneRow> {
        self.rows.lock_or_recover().clone().unwrap_or_default()
    }

    fn table(&self) -> ProcessTable {
        self.table.lock_or_recover().clone()
    }

    fn put(&self, info: ProcessInfo) {
        self.table.lock_or_recover().insert(info.pid, info);
    }

    fn gone(&self, pid: u32) {
        self.table.lock_or_recover().remove(&pid);
    }

    /// `leader` is the foreground of `tty` now: every process on that
    /// terminal says so, as the kernel does.
    fn foreground(&self, tty: u32, leader: u32) {
        for info in self.table.lock_or_recover().values_mut() {
            if info.tty == tty {
                info.tty_pgid = leader;
            }
        }
    }

    /// What the scheduler and the poll would read for `session` right now.
    fn evidence(&self, session: &str) -> agent_status::Evidence {
        let table = self.table();
        agent_status::generation_evidence(
            &self.rows(),
            |pane| agent_status::foreground_generation(&table, pane),
            |_| None,
        )
        .get(session)
        .copied()
        .unwrap_or_default()
    }

    fn state(&self, session: &str) -> Option<&'static str> {
        agent_status::projections(&self.rows())
            .get(session)
            .map(|observation| observation.state)
    }

    fn episode(&self, session: &str) -> Option<agent_status::EpisodeId> {
        agent_status::projections(&self.rows())
            .get(session)
            .map(|observation| observation.episode)
    }
}

fn hello(pane: &str) -> Hello {
    Hello {
        socket: "deck".into(),
        server_pid: 42,
        pane: pane.into(),
        version: PiVersion::Supported,
        capabilities: SIGNAL,
    }
}

fn bound_as(world: &World, hello: &Hello, peer: Option<u32>) -> Result<u64, &'static str> {
    let mut listing = Listing { world, taken: None };
    bind(world, hello, peer, &mut listing).map(|bound| bound.lease)
}

fn bound(world: &World, pane: &str, peer: u32) -> Result<u64, &'static str> {
    bound_as(world, &hello(pane), Some(peer))
}

fn sent(world: &World, lease: u64, frame: Frame) -> Result<u64, &'static str> {
    let mut listing = Listing { world, taken: None };
    commit(world, lease, &frame, &mut listing)
}

fn working(seq: u64, run: u64) -> Frame {
    Frame::Signal {
        seq,
        run,
        state: RunState::Working,
    }
}

fn done(seq: u64, run: u64) -> Frame {
    Frame::Signal {
        seq,
        run,
        state: RunState::TurnDone,
    }
}

/// Two Pi panes of one project: A is `%1` (shell 300, tty 7, Pi 310, an npm
/// install that tmux calls `node`) and B is `%2` (shell 400, tty 8, Pi 410).
fn two_panes() -> Shared {
    Shared::new(
        vec![
            row("deck-a", "$1", "%1", 300, "node"),
            row("deck-b", "$2", "%2", 400, "pi"),
        ],
        table(&[pane(300, 7, 310, 2000), pane(400, 8, 410, 2100)]),
    )
}

fn fresh() -> std::sync::MutexGuard<'static, ()> {
    let guard = STORE_TEST_LOCK.lock_or_recover();
    agent_status::reset_for_tests();
    crate::notify::reset_for_tests();
    probe::reset();
    guard
}

#[test]
fn a_bound_observer_reports_run_boundaries_into_the_shared_store() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    assert!(
        !shared.evidence("deck-a").pi,
        "tmux calls it node: nothing says Pi yet"
    );
    let lease = bound(&world, "%1", 310).expect("the pane's own foreground program");
    assert_eq!(shared.listings.load(Ordering::SeqCst), 1);
    // recognized, connected with limited coverage — and no agent state: a
    // loaded, idle observer is not a turn that ended
    assert_eq!(
        (
            shared.evidence("deck-a").pi,
            shared.evidence("deck-a").pi_signal
        ),
        (true, Some(PiCoverage::Connected))
    );
    assert_eq!(shared.state("deck-a"), None);
    assert!(
        !shared.evidence("deck-a").claude_interaction,
        "never interaction evidence"
    );
    // a heartbeat commits nothing and asks tmux nothing
    assert_eq!(sent(&world, lease, Frame::Ping { seq: 1 }), Ok(1));
    assert_eq!(shared.listings.load(Ordering::SeqCst), 1);
    assert_eq!(shared.state("deck-a"), None);

    assert_eq!(sent(&world, lease, working(2, 1)), Ok(2));
    let first = shared.episode("deck-a").expect("an accepted observation");
    assert_eq!(shared.state("deck-a"), Some("working"));
    assert_eq!(sent(&world, lease, done(3, 1)), Ok(3));
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    let ended = shared.episode("deck-a").unwrap();
    assert_ne!(first, ended, "an ending is a new episode");
    // the same attention layer every agent's ending reaches
    let (words, unread, _) = crate::notify::snapshot_for_tests();
    assert_eq!(words.get("deck-a"), Some(&"turn-done"));
    assert!(unread.contains("deck-a"));
    // the next run is another interaction: two more episodes, never merged
    assert_eq!(sent(&world, lease, working(4, 2)), Ok(4));
    assert_eq!(sent(&world, lease, done(5, 2)), Ok(5));
    let again = shared.episode("deck-a").unwrap();
    assert!(again > ended);
    assert!(agent_status::mark_viewed("deck-a", again));
    // B heard nothing of it
    assert_eq!(shared.state("deck-b"), None);
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert_eq!(probe::live(), 1);
}

#[test]
fn two_panes_of_one_project_never_share_a_state() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let a = bound(&world, "%1", 310).unwrap();
    let b = bound(&world, "%2", 410).unwrap();
    assert_ne!(a, b);
    assert_eq!(sent(&world, a, working(1, 1)), Ok(1));
    assert_eq!(
        (shared.state("deck-a"), shared.state("deck-b")),
        (Some("working"), None)
    );
    assert_eq!(sent(&world, b, working(1, 1)), Ok(1));
    assert_eq!(sent(&world, a, done(2, 1)), Ok(2));
    assert_eq!(
        (shared.state("deck-a"), shared.state("deck-b")),
        (Some("turn-done"), Some("working")),
        "run 1 of A and run 1 of B are two leases' runs"
    );
    assert_ne!(shared.episode("deck-a"), shared.episode("deck-b"));
    // a pane that is not its session's Signal target keeps its word to itself
    {
        let mut rows = shared.rows.lock_or_recover();
        let rows = rows.as_mut().unwrap();
        rows[1].pane_active = false;
        rows.push(PaneRow {
            pane_id: "%3".into(),
            pane_pid: 500,
            ..row("deck-b", "$2", "%3", 500, "zsh")
        });
    }
    assert_eq!(sent(&world, b, done(2, 1)), Ok(2));
    assert_eq!(
        shared.state("deck-b"),
        None,
        "the card shows its active pane"
    );
    let (words, _, _) = crate::notify::snapshot_for_tests();
    assert_eq!(words.get("deck-b"), Some(&"working"), "and nobody was told");
}

#[test]
fn only_the_pane_s_own_foreground_program_may_bind() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let untouched = |shared: &Shared| {
        assert_eq!(probe::live(), 0);
        assert!(
            !shared.evidence("deck-a").pi,
            "the named pane is not recognized"
        );
        assert_eq!(shared.state("deck-a"), None);
    };
    // no kernel peer, a peer the kernel does not know
    assert_eq!(bound_as(&world, &hello("%1"), None), Err("no-peer"));
    assert_eq!(bound(&world, "%1", 999), Err("no-peer"));
    // B's Pi names A's pane (an inherited or forged $TMUX_PANE)
    assert_eq!(bound(&world, "%1", 410), Err("foreign-pane"));
    untouched(&shared);
    // a background job of A's Pi that inherited its environment
    shared.put(process(320, 310, 7, 310, 3000));
    assert_eq!(bound(&world, "%1", 320), Err("not-foreground"));
    // a child inside the foreground group (a wrapper leads, the client follows)
    shared.put(ProcessInfo {
        pgid: 310,
        ..process(321, 310, 7, 310, 3000)
    });
    assert_eq!(bound(&world, "%1", 321), Err("not-foreground"));
    // a detached process without a terminal
    shared.put(process(322, 310, 0, 0, 3000));
    assert_eq!(bound(&world, "%1", 322), Err("not-foreground"));
    untouched(&shared);
    // a foreground leader of another terminal that descends from nothing here
    shared.put(process(600, 1, 9, 600, 3000));
    assert_eq!(bound(&world, "%1", 600), Err("foreign-pane"));
    // on the pane's terminal and in front, yet no descendant of its shell
    shared.put(process(601, 1, 7, 601, 3000));
    shared.foreground(7, 601);
    assert_eq!(bound(&world, "%1", 601), Err("foreign-pane"));
    shared.gone(601);
    // a process off the terminal between the client and the pane's shell
    shared.put(process(330, 300, 0, 0, 2500));
    shared.put(process(331, 330, 7, 331, 2600));
    shared.foreground(7, 331);
    assert_eq!(bound(&world, "%1", 331), Err("terminal-discontinuity"));
    shared.gone(330);
    shared.gone(331);
    shared.foreground(7, 310);
    untouched(&shared);
    // claims about the server
    let other = |change: &dyn Fn(&mut Hello)| {
        let mut hello = hello("%1");
        change(&mut hello);
        bound_as(&world, &hello, Some(310))
    };
    assert_eq!(
        other(&|h| h.socket = "deck-dev".into()),
        Err("other-server")
    );
    assert_eq!(other(&|h| h.server_pid = 43), Err("no-such-pane"));
    assert_eq!(other(&|h| h.pane = "%9".into()), Err("no-such-pane"));
    untouched(&shared);
    // a plain shell in front: nothing to bind an observation's lifetime to
    shared.rows.lock_or_recover().as_mut().unwrap()[0].command = "zsh".into();
    assert_eq!(bound(&world, "%1", 310), Err("not-foreground"));
    shared.rows.lock_or_recover().as_mut().unwrap()[0].command = "node".into();
    // a session whose name is not one of Deck's
    shared.rows.lock_or_recover().as_mut().unwrap()[0].session_name = "bad name".into();
    assert_eq!(bound(&world, "%1", 310), Err("no-such-pane"));
    shared.rows.lock_or_recover().as_mut().unwrap()[0].session_name = "deck-a".into();
    // a listing that failed proves no pane
    let rows = shared.rows.lock_or_recover().take();
    assert_eq!(bound(&world, "%1", 310), Err("no-such-pane"));
    *shared.rows.lock_or_recover() = rows;
    untouched(&shared);
    // and the same hello from the pane's own foreground program is bound
    assert!(bound(&world, "%1", 310).is_ok());
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert!(!agent_status::generation_evidence(&shared.rows(), |_| None, |_| None)["deck-a"].pi);
}

#[test]
fn one_observer_per_foreground_generation_and_the_first_stays() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let first = bound(&world, "%1", 310).unwrap();
    assert_eq!(sent(&world, first, working(1, 1)), Ok(1));
    let listings = shared.listings.load(Ordering::SeqCst);
    assert_eq!(bound(&world, "%1", 310), Err("duplicate-observer"));
    assert_eq!(
        shared.listings.load(Ordering::SeqCst),
        listings,
        "refused before any tmux call"
    );
    // the first is neither replaced nor disturbed
    assert_eq!(shared.state("deck-a"), Some("working"));
    assert_eq!(sent(&world, first, done(2, 1)), Ok(2));
    // once it is over, the same Pi may bind again — as another lease
    release(first);
    assert_eq!(shared.state("deck-a"), None);
    let second = bound(&world, "%1", 310).unwrap();
    assert!(second > first, "a lease number is never used again");
    // a frame of the old lease is late, whatever it says
    assert_eq!(sent(&world, first, working(3, 2)), Err("late"));
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(sent(&world, second, working(1, 1)), Ok(1));
    assert_eq!(shared.state("deck-a"), Some("working"));
}

#[test]
fn compatibility_and_the_settings_are_asked_after_identity() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    // a release this build does not know: recognized, said, not observed
    let mut old = hello("%1");
    old.version = PiVersion::Unsupported;
    assert_eq!(
        bound_as(&world, &old, Some(310)),
        Err("unsupported-pi-version")
    );
    assert_eq!(
        (
            shared.evidence("deck-a").pi,
            shared.evidence("deck-a").pi_signal
        ),
        (true, Some(PiCoverage::Incompatible))
    );
    assert_eq!(probe::live(), 0);
    // the capability is off in Pi's settings (an observer loaded earlier)
    *shared.enabled.lock_or_recover() = Capabilities::NONE;
    assert_eq!(bound(&world, "%2", 410), Err("capability-disabled"));
    assert_eq!(
        (
            shared.evidence("deck-b").pi,
            shared.evidence("deck-b").pi_signal
        ),
        (true, Some(PiCoverage::Unavailable))
    );
    // a capability the settings do not enable, beside one they do
    *shared.enabled.lock_or_recover() = SIGNAL;
    let mut more = hello("%2");
    more.capabilities.lifecycle = true;
    assert_eq!(
        bound_as(&world, &more, Some(410)),
        Err("capability-disabled")
    );
    // an incompatible identity from ANOTHER pane's process proves nothing
    agent_status::reset_for_tests();
    assert_eq!(bound_as(&world, &old, Some(410)), Err("foreign-pane"));
    assert!(!shared.evidence("deck-a").pi);
}

#[test]
fn frames_are_numbered_without_a_gap_and_runs_never_go_back() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    // every refusal ends the lease and withdraws what it reported: the next
    // case starts on a new one
    let lease = || {
        let lease = bound(&world, "%1", 310).unwrap();
        assert_eq!(shared.state("deck-a"), None);
        lease
    };
    let refused = |frames: &[Frame], last: Frame, code: &str| {
        let lease = lease();
        for frame in frames {
            assert!(sent(&world, lease, frame.clone()).is_ok(), "{frame:?}");
        }
        assert_eq!(sent(&world, lease, last.clone()), Err(code), "{last:?}");
        assert_eq!(
            shared.state("deck-a"),
            None,
            "{last:?}: withdrawn, not replaced"
        );
        assert_eq!(
            shared.evidence("deck-a").pi_signal,
            Some(PiCoverage::Unavailable),
            "{last:?}"
        );
        assert_eq!(sent(&world, lease, Frame::Ping { seq: 9 }), Err("late"));
    };
    // a repeated number, a gap, a first frame that is not 1
    refused(&[Frame::Ping { seq: 1 }], Frame::Ping { seq: 1 }, "bad-seq");
    refused(&[Frame::Ping { seq: 1 }], Frame::Ping { seq: 3 }, "bad-seq");
    refused(&[], Frame::Ping { seq: 2 }, "bad-seq");
    refused(&[working(1, 1)], working(1, 2), "bad-seq");
    // a first run that is not 1, a run that skips
    refused(&[], working(1, 2), "bad-run");
    refused(&[working(1, 1), done(2, 1)], working(3, 3), "bad-run");
    // a second start while a run is open, of the same or of another run
    refused(&[working(1, 1)], working(2, 1), "bad-run");
    refused(&[working(1, 1)], working(2, 2), "bad-run");
    // a run that goes back
    refused(&[working(1, 1), done(2, 1)], working(3, 1), "bad-run");
    // an ending without a start, a second ending
    refused(&[], done(1, 1), "bad-run");
    refused(&[working(1, 1), done(2, 1)], done(3, 1), "bad-run");
    // a late ending of run 1 while run 2 is open never closes run 2
    refused(
        &[working(1, 1), done(2, 1), working(3, 2)],
        done(4, 1),
        "bad-run",
    );
    // and an ending of a run that never started on this lease
    refused(&[working(1, 1)], done(2, 2), "bad-run");
}

#[test]
fn every_frame_is_judged_by_what_the_kernel_says_now() {
    let _guard = fresh();
    // another program took the foreground; the Pi process is still alive
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    assert_eq!(sent(&world, lease, working(1, 1)), Ok(1));
    shared.put(process(350, 300, 7, 350, 4000));
    shared.foreground(7, 350);
    assert_eq!(
        sent(&world, lease, Frame::Ping { seq: 2 }),
        Err("generation-mismatch")
    );
    assert_eq!(
        shared.state("deck-a"),
        None,
        "the old program's word is gone"
    );
    assert!(
        !shared.evidence("deck-a").pi,
        "and the new foreground is not Pi"
    );

    // the pid is another process now (a later birth)
    fresh_again();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    shared.put(process(310, 300, 7, 310, 9000));
    assert_eq!(
        sent(&world, lease, working(1, 1)),
        Err("generation-mismatch")
    );
    assert_eq!(shared.state("deck-a"), None);

    // the peer is gone
    fresh_again();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    shared.gone(310);
    shared.foreground(7, 300);
    assert_eq!(
        sent(&world, lease, Frame::Ping { seq: 1 }),
        Err("generation-mismatch")
    );

    // the pane's own process was replaced under the same pane id
    fresh_again();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    shared.put(process(300, 42, 7, 310, 5000));
    assert_eq!(
        sent(&world, lease, Frame::Ping { seq: 1 }),
        Err("pane-changed")
    );

    // the pane now belongs to another session
    fresh_again();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    shared.rows.lock_or_recover().as_mut().unwrap()[0].session_id = "$7".into();
    assert_eq!(sent(&world, lease, working(1, 1)), Err("pane-changed"));
    assert_eq!(shared.state("deck-a"), None);

    // the pane is gone from the listing
    fresh_again();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    shared.rows.lock_or_recover().as_mut().unwrap().remove(0);
    assert_eq!(sent(&world, lease, working(1, 1)), Err("pane-changed"));
}

/// The store, the notifications and the leases start empty again. The caller
/// already holds the store lock.
fn fresh_again() {
    agent_status::reset_for_tests();
    crate::notify::reset_for_tests();
    probe::reset();
}

#[test]
fn a_foreground_that_changes_during_the_listing_writes_nothing() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let lease = bound(&world, "%1", 310).unwrap();
    // the listing is slow, and while it runs another program takes the
    // foreground: what was true before it is not what is checked
    let table = shared.table.clone();
    *shared.during.lock_or_recover() = Some(Box::new(move || {
        let mut table = table.lock_or_recover();
        table.insert(350, process(350, 300, 7, 350, 4000));
        for info in table.values_mut().filter(|info| info.tty == 7) {
            info.tty_pgid = 350;
        }
    }));
    assert_eq!(
        sent(&world, lease, working(1, 1)),
        Err("generation-mismatch")
    );
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(probe::live(), 0);
}

#[test]
fn ending_a_lease_withdraws_its_own_observation_and_nothing_else() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let a = bound(&world, "%1", 310).unwrap();
    let b = bound(&world, "%2", 410).unwrap();
    for lease in [a, b] {
        assert_eq!(sent(&world, lease, working(1, 1)), Ok(1));
        assert_eq!(sent(&world, lease, done(2, 1)), Ok(2));
    }
    let (_, unread, _) = crate::notify::snapshot_for_tests();
    assert_eq!(unread.len(), 2);
    // A's Pi quits: its reset is acknowledged, and its ending is not kept
    assert_eq!(
        sent(
            &world,
            a,
            Frame::Reset {
                seq: 3,
                reason: ResetReason::Quit
            }
        ),
        Ok(3)
    );
    assert_eq!(
        (shared.state("deck-a"), shared.state("deck-b")),
        (None, Some("turn-done"))
    );
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert!(
        shared.evidence("deck-a").pi,
        "still a recognized Pi: the hold stays"
    );
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Connected)
    );
    let (words, unread, _) = crate::notify::snapshot_for_tests();
    assert!(!words.contains_key("deck-a") && !unread.contains("deck-a"));
    assert!(unread.contains("deck-b"));
    assert_eq!(
        sent(&world, a, working(4, 2)),
        Err("late"),
        "nothing after a reset"
    );

    // a word a hook reported for the same pane is not the lease's to take
    let hook = serde_json::json!({
        "v": 1, "source": "claude-code", "state": "needs-input",
        "socket": crate::tmux::socket(), "server_pid": 42, "pane": "%2"
    })
    .to_string();
    let mut table = shared.table();
    table.insert(420, process(420, 410, 7, 410, 5000));
    table.get_mut(&420).unwrap().tty = 8;
    let origin = agent_status::Origin {
        peer: Some(420),
        table,
    };
    assert_eq!(
        agent_status::ingest(&hook, &origin, || Some(shared.rows())),
        Ok(())
    );
    assert_eq!(shared.state("deck-b"), Some("needs-input"));
    release(b);
    assert_eq!(
        shared.state("deck-b"),
        Some("needs-input"),
        "not Pi's word any more"
    );
    assert_eq!(probe::live(), 0);
}

#[test]
fn the_poll_and_the_settings_end_leases_that_no_longer_hold() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let a = bound(&world, "%1", 310).unwrap();
    let b = bound(&world, "%2", 410).unwrap();
    assert_eq!(sent(&world, a, working(1, 1)), Ok(1));
    assert_eq!(sent(&world, b, working(1, 1)), Ok(1));
    let now = |shared: &Shared| {
        let table = shared.table();
        move |pane: u32| agent_status::foreground_generation(&table, pane)
    };
    // nothing changed: both stay, whatever the rows of an older listing say
    reconcile_with(&shared.rows(), now(&shared));
    reconcile_with(&[], now(&shared));
    assert_eq!(probe::live(), 2);
    assert!(!mismatched(SIGNAL));
    assert!(mismatched(Capabilities {
        signal: true,
        lifecycle: true
    }));
    // A's foreground is a shell again (Pi exited without a word)
    shared.gone(310);
    shared.foreground(7, 300);
    reconcile_with(&shared.rows(), now(&shared));
    assert_eq!(over(a), Some("generation-mismatch"));
    assert_eq!(
        (shared.state("deck-a"), shared.state("deck-b")),
        (None, Some("working"))
    );
    // B's pane was moved into another session
    let mut moved = shared.rows();
    moved[1].session_id = "$9".into();
    reconcile_with(&moved, now(&shared));
    assert_eq!(over(b), Some("pane-changed"));
    assert_eq!(probe::live(), 0);

    // the switch is turned off in Deck: the stream ends now, not at its next frame
    let c = bound(&world, "%2", 410).unwrap();
    assert_eq!(sent(&world, c, working(1, 1)), Ok(1));
    enabled_changed(SIGNAL);
    assert_eq!(probe::live(), 1, "still enabled: nothing ends");
    enabled_changed(Capabilities::NONE);
    assert_eq!(over(c), Some("capability-disabled"));
    assert_eq!(shared.state("deck-b"), None);
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    // and a frame of a stream that outlived an external edit of the file
    let d = bound(&world, "%2", 410).unwrap();
    *shared.enabled.lock_or_recover() = Capabilities::NONE;
    assert_eq!(sent(&world, d, working(1, 1)), Err("capability-disabled"));
    assert_eq!(shared.state("deck-b"), None);
}

#[test]
fn pending_lines_are_counted_and_bounded() {
    let _guard = fresh();
    assert_eq!(probe::pending(), 0);
    let mut streams: Vec<Pending> = (0..5).map(|_| Pending::default()).collect();
    for stream in streams.iter_mut().take(4) {
        for _ in 0..MAX_PENDING_PER_STREAM {
            stream.push("x".into()).expect("inside both bounds");
        }
        assert_eq!(stream.push("x".into()), Err("busy"), "one stream's bound");
    }
    assert_eq!(probe::pending(), MAX_PENDING);
    assert_eq!(
        streams[4].push("x".into()),
        Err("busy"),
        "the bound of all streams"
    );
    assert_eq!(streams[0].pop().as_deref(), Some("x"));
    assert_eq!(streams[4].push("x".into()), Ok(()));
    assert_eq!(probe::pending(), MAX_PENDING);
    streams.clear();
    assert_eq!(
        probe::pending(),
        0,
        "a stream that ends gives back what it held"
    );
}

// ---------- streams: the service as the listener uses it ----------------------------

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    /// One stream handed to `service` as the peer `peer`, its hello sent.
    fn open(service: &Service, hello: &str, peer: u32) -> Self {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        ours.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        service.hand_as(theirs, hello.to_string(), Some(peer));
        Self {
            reader: BufReader::new(ours.try_clone().unwrap()),
            writer: ours,
        }
    }

    fn send(&mut self, body: serde_json::Value) {
        self.writer
            .write_all(format!("{}\n", frame_line(body)).as_bytes())
            .expect("the frame is written");
    }

    /// The next reply, or `None` when the stream ended.
    fn reply(&mut self) -> Option<serde_json::Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => serde_json::from_str(&line).ok(),
        }
    }

    fn accepted(&mut self) -> bool {
        self.reply().is_some_and(|reply| reply["accepted"] == true)
    }

    fn ack(&mut self) -> Option<u64> {
        self.reply().and_then(|reply| reply["ack"].as_u64())
    }

    fn refused(&mut self) -> Option<String> {
        self.reply()
            .and_then(|reply| reply["refused"].as_str().map(str::to_owned))
    }

    fn ended(&mut self) -> bool {
        self.reply().is_none()
    }

    /// The refusal that ends the stream, whatever was acknowledged before it.
    fn outcome(&mut self) -> Option<String> {
        let mut refusal = None;
        while let Some(reply) = self.reply() {
            if let Some(code) = reply["refused"].as_str() {
                refusal = Some(code.to_string());
            }
        }
        refusal
    }
}

fn frame_line(body: serde_json::Value) -> String {
    let mut line = serde_json::json!({ "protocol": "pi-observer", "v": 1, "source": "pi" });
    line.as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    line.to_string()
}

fn hello_line(pane: &str) -> String {
    frame_line(serde_json::json!({
        "socket": "deck", "server_pid": 42, "pane": pane, "observerVersion": 1,
        "piVersion": "1.1.0", "mode": "tui",
        "capabilities": { "signal": true, "lifecycle": false }
    }))
}

type Reasons = Arc<Mutex<Vec<&'static str>>>;

fn service(shared: &Shared, bounds: Bounds) -> (Service, Reasons) {
    let reasons: Reasons = Arc::new(Mutex::new(Vec::new()));
    let report = reasons.clone();
    (
        Service::start_in(
            Arc::new(move |reason| report.lock_or_recover().push(reason)),
            shared.world(),
            bounds,
        ),
        reasons,
    )
}

const QUICK: Bounds = Bounds {
    answer: Duration::from_millis(400),
    stale: Duration::from_millis(600),
};

fn until(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_stream_is_answered_frame_by_frame_after_each_commit() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    assert_eq!(probe::live(), 1);
    client.send(serde_json::json!({"seq": 1}));
    assert_eq!(client.ack(), Some(1));
    client.send(serde_json::json!({"seq": 2, "run": 1, "state": "working"}));
    assert_eq!(client.ack(), Some(2));
    // the acknowledgement follows the commit: the word is there already
    assert_eq!(shared.state("deck-a"), Some("working"));
    client.send(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"}));
    assert_eq!(client.ack(), Some(3));
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    // Pi's own teardown: acknowledged, then the stream is simply over
    client.send(serde_json::json!({"seq": 4, "reason": "reload"}));
    assert_eq!(client.ack(), Some(4));
    assert!(client.ended());
    assert_eq!(shared.state("deck-a"), None);
    until("the stream's slot", || probe::streams() == 0);
    assert_eq!(probe::live(), 0);
    assert!(reasons.lock_or_recover().is_empty(), "nothing was refused");
    // the reloaded runtime is a new stream and a new lease, with nothing replayed
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    drop(client);
    until("the end of the stream", || {
        probe::live() == 0 && probe::streams() == 0
    });
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Unavailable)
    );
}

/// A quitting Pi writes its last frames and is gone: whole lines that arrive
/// together with the end of the stream are still its frames. They are
/// committed in order, the lease ends for the reason the reset names, and
/// the end of the stream is not what ended it.
#[test]
fn frames_sent_just_before_the_stream_ends_are_answered_before_the_end() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    client.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    assert_eq!(client.ack(), Some(1));
    let announced = crate::notify::announced_for_tests()
        .get("deck-a")
        .copied()
        .unwrap_or(0);
    // the ending and the reset in one write, then the client is gone
    let last = format!(
        "{}\n{}\n",
        frame_line(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"})),
        frame_line(serde_json::json!({"seq": 3, "reason": "quit"}))
    );
    client.writer.write_all(last.as_bytes()).unwrap();
    client
        .writer
        .shutdown(std::net::Shutdown::Both)
        .expect("the client closes");
    drop(client);
    until("the lease to end", || {
        probe::live() == 0 && probe::streams() == 0
    });
    assert_eq!(
        probe::ended(),
        [("deck-a".to_string(), "reset-quit")],
        "ended by the reset it sent, not by the end of the stream"
    );
    // the ending was committed before the reset withdrew it: announced once
    assert_eq!(
        crate::notify::announced_for_tests().get("deck-a").copied(),
        Some(announced + 1)
    );
    assert_eq!(
        agent_status::projections(&shared.rows.lock_or_recover().clone().unwrap()).get("deck-a"),
        None,
        "and the reset took the observation with it"
    );
    assert!(reasons.lock_or_recover().is_empty(), "nothing was refused");
    // a stream that ends in the middle of a line has no last frame
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    client
        .writer
        .write_all(
            b"{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":1,\"reason\":\"qu",
        )
        .unwrap();
    client.writer.shutdown(std::net::Shutdown::Both).unwrap();
    drop(client);
    until("the second lease to end", || probe::ended().len() == 2);
    assert_eq!(probe::ended()[1], ("deck-a".to_string(), "eof"));
}

fn hello_with(pane: &str, signal: bool, lifecycle: bool) -> String {
    frame_line(serde_json::json!({
        "socket": "deck", "server_pid": 42, "pane": pane, "observerVersion": 1,
        "piVersion": "1.1.0", "mode": "tui",
        "capabilities": { "signal": signal, "lifecycle": lifecycle }
    }))
}

const BOTH: Capabilities = Capabilities {
    signal: true,
    lifecycle: true,
};
const LIFECYCLE: Capabilities = Capabilities {
    signal: false,
    lifecycle: true,
};

fn sessions() -> Vec<(u64, String, crate::agent_pi::SessionEvent)> {
    crate::agent_reentry::lifecycle::pi_sessions_for_tests()
}

/// Session observation is its own capability on the same stream: a stream
/// may report only what its hello claimed AND the settings enable, a
/// lifecycle-only stream reports no run, a signal-only stream names no
/// session, and what a lease observed is gone with the lease.
#[test]
fn session_frames_need_their_own_capability_and_are_gone_with_the_lease() {
    use crate::agent_pi::{ShutdownReason, StartReason};
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let rows = || shared.rows.lock_or_recover().clone().unwrap();
    let observed = |session: &str| {
        agent_status::generation_evidence(
            &rows(),
            |pane| agent_status::foreground_generation(&shared.table.lock_or_recover(), pane),
            |_| None,
        )
        .get(session)
        .and_then(|evidence| evidence.pi_session)
    };
    let start = |seq: u64, id: &str| serde_json::json!({"seq": seq, "event": "start", "nativeSessionId": id, "reason": "startup"});

    // A carries both capabilities on ONE stream; B only session observation
    let mut a = Client::open(&service, &hello_with("%1", true, true), 310);
    assert!(a.accepted());
    let mut b = Client::open(&service, &hello_with("%2", false, true), 410);
    assert!(b.accepted());
    a.send(start(1, "session-one"));
    assert_eq!(a.ack(), Some(1));
    a.send(serde_json::json!({"seq": 2, "run": 1, "state": "working"}));
    assert_eq!(a.ack(), Some(2));
    b.send(start(1, "session-one"));
    assert_eq!(b.ack(), Some(1));
    // one session in two panes: two leases, two records, nothing merged
    let known = sessions();
    assert_eq!(known.len(), 2);
    assert!(known[0].0 != known[1].0 && known.iter().all(|k| k.1 == "session-one"));
    assert_eq!(
        known[0].2,
        crate::agent_pi::SessionEvent::Start(StartReason::Startup)
    );
    assert_eq!(
        observed("deck-a"),
        Some(crate::agent_reentry::lifecycle::PiSession::Observed)
    );
    assert_eq!(
        observed("deck-b"),
        Some(crate::agent_reentry::lifecycle::PiSession::Observed)
    );
    // a session is no Signal: B's card has no word, A's word came from its run
    let projected = agent_status::projections(&rows());
    assert_eq!(projected.get("deck-a").map(|o| o.state), Some("working"));
    assert!(!projected.contains_key("deck-b"));
    // a lifecycle-only stream may report no run
    b.send(serde_json::json!({"seq": 2, "run": 1, "state": "working"}));
    assert_eq!(b.reply().unwrap()["refused"], "capability-disabled");
    until("B's record to go with its lease", || sessions().len() == 1);
    assert_eq!(observed("deck-b"), None);
    assert!(!agent_status::projections(&rows()).contains_key("deck-b"));

    // the teardown, then the reset: the reason is kept until the lease ends
    a.send(serde_json::json!({"seq": 3, "event": "shutdown", "nativeSessionId": "session-one", "reason": "quit"}));
    assert_eq!(a.ack(), Some(3));
    assert_eq!(
        sessions()[0].2,
        crate::agent_pi::SessionEvent::Shutdown(ShutdownReason::Quit)
    );
    assert_eq!(
        observed("deck-a"),
        None,
        "a runtime that said it is going observes no session"
    );
    a.send(serde_json::json!({"seq": 4, "reason": "quit"}));
    assert_eq!(a.ack(), Some(4));
    until("A's lease to end", || probe::live() == 0);
    assert!(
        sessions().is_empty(),
        "nothing of a lease that is over is kept"
    );

    // a signal-only stream names no session
    *shared.enabled.lock_or_recover() = BOTH;
    let mut signal_only = Client::open(&service, &hello_with("%1", true, false), 310);
    assert!(signal_only.accepted());
    signal_only.send(start(1, "session-one"));
    assert_eq!(
        signal_only.reply().unwrap()["refused"],
        "capability-disabled"
    );
    assert!(sessions().is_empty());
    until("its lease to end", || probe::live() == 0);

    // a hello may claim session observation only while the settings enable it
    *shared.enabled.lock_or_recover() = SIGNAL;
    let mut refused = Client::open(&service, &hello_with("%1", true, true), 310);
    assert_eq!(refused.reply().unwrap()["refused"], "capability-disabled");
    let mut refused = Client::open(&service, &hello_with("%1", false, true), 310);
    assert_eq!(refused.reply().unwrap()["refused"], "capability-disabled");
    // and with it enabled alone, a lifecycle-only stream binds and Signal does not
    *shared.enabled.lock_or_recover() = LIFECYCLE;
    let mut refused = Client::open(&service, &hello_with("%1", true, false), 310);
    assert_eq!(refused.reply().unwrap()["refused"], "capability-disabled");
    let mut only = Client::open(&service, &hello_with("%1", false, true), 310);
    assert!(only.accepted());
    only.send(start(1, "session-two"));
    assert_eq!(only.ack(), Some(1));
    assert_eq!(
        observed("deck-a"),
        Some(crate::agent_reentry::lifecycle::PiSession::Observed)
    );
    // turned off in Settings: the record goes at once, not at the next frame
    enabled_changed(Capabilities::NONE);
    assert!(sessions().is_empty());
    assert_eq!(observed("deck-a"), None);
    assert_eq!(only.reply().unwrap()["refused"], "capability-disabled");
    assert!(reasons
        .lock_or_recover()
        .iter()
        .all(|reason| *reason == "pi-capability-disabled"));
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// What a runtime reports about its session is checked against what the
/// same lease reported before, and an id is Pi's own shape or nothing.
#[test]
fn a_session_frame_that_contradicts_its_lease_ends_the_stream() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let open = || {
        let mut client = Client::open(&service, &hello_with("%1", true, true), 310);
        assert!(client.accepted());
        client
    };
    // a second start in one runtime
    let mut client = open();
    client.send(serde_json::json!({"seq": 1, "event": "start", "nativeSessionId": "one", "reason": "startup"}));
    assert_eq!(client.ack(), Some(1));
    client.send(serde_json::json!({"seq": 2, "event": "start", "nativeSessionId": "two", "reason": "resume"}));
    assert_eq!(client.reply().unwrap()["refused"], "bad-lifecycle");
    until("the lease to end", || probe::live() == 0);
    assert!(sessions().is_empty(), "neither reading is kept");
    // a teardown of a session it never reported
    let mut client = open();
    client.send(serde_json::json!({"seq": 1, "event": "shutdown", "nativeSessionId": "one", "reason": "quit"}));
    assert_eq!(client.reply().unwrap()["refused"], "bad-lifecycle");
    until("the lease to end", || probe::live() == 0);
    // an id that is not Pi's shape is no frame at all
    let mut client = open();
    client.send(
        serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": "../sessions/x"}),
    );
    assert_eq!(client.reply().unwrap()["refused"], "bad-native-id");
    until("the lease to end", || probe::live() == 0);
    // an id longer than this build keeps is acknowledged and not kept: never cut short
    let mut client = open();
    client.send(serde_json::json!({"seq": 1, "event": "start", "nativeSessionId": "a".repeat(257), "reason": "startup"}));
    assert_eq!(client.ack(), Some(1));
    assert!(sessions().is_empty());
    client.send(
        serde_json::json!({"seq": 2, "event": "snapshot", "nativeSessionId": "a".repeat(256)}),
    );
    assert_eq!(client.ack(), Some(2));
    assert_eq!(sessions()[0].1.len(), 256);
    drop(client);
    until("the lease to end", || probe::live() == 0);
    assert!(sessions().is_empty());
    assert_eq!(
        *reasons.lock_or_recover(),
        ["pi-bad-lifecycle", "pi-bad-lifecycle", "pi-bad-native-id"]
    );
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

#[test]
fn a_line_that_breaks_the_wire_is_refused_and_ends_the_stream() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    // hellos that never bind
    for (line, code) in [
        (
            hello_line("%1").replace("\"mode\":\"tui\"", "\"mode\":\"rpc\""),
            "bad-mode",
        ),
        (
            hello_line("%1").replace("\"v\":1", "\"v\":2"),
            "bad-version",
        ),
        (
            hello_line("%1").replace("\"pane\":\"%1\"", "\"pane\":\"%1\",\"title\":\"x\""),
            "bad-frame",
        ),
        (
            hello_line("%1").replace("\"pane\":\"%1\"", "\"pane\":\"%1\",\"pane\":\"%2\""),
            "bad-frame",
        ),
        (frame_line(serde_json::json!({"seq": 1})), "bad-frame"),
        (
            format!("{}{}", hello_line("%1"), " ".repeat(1024)),
            "bad-frame",
        ),
        (
            hello_line("%1").replace("1.1.0", "2.0.0"),
            "unsupported-pi-version",
        ),
        (hello_line("%9"), "no-such-pane"),
    ] {
        let mut client = Client::open(&service, &line, 310);
        assert_eq!(client.refused().as_deref(), Some(code), "{line}");
        assert!(client.ended());
    }
    until("every refused stream's slot", || probe::streams() == 0);
    assert_eq!(probe::live(), 0);
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(
        *reasons.lock_or_recover(),
        [
            "pi-bad-mode",
            "pi-bad-version",
            "pi-bad-frame",
            "pi-bad-frame",
            "pi-bad-frame",
            "pi-bad-frame",
            "pi-unsupported-pi-version",
            "pi-no-such-pane"
        ]
    );
    // frames that break the wire after a bound hello: what was reported goes
    for (body, code) in [
        (
            serde_json::json!({"seq": 2, "run": 2, "state": "needs-input"}),
            "bad-frame",
        ),
        (
            serde_json::json!({"seq": 2, "title": "Allow?"}),
            "bad-frame",
        ),
        (serde_json::json!({"seq": 7}), "bad-seq"),
        (
            serde_json::json!({"seq": 2, "run": 1, "state": "working"}),
            "bad-run",
        ),
    ] {
        let mut client = Client::open(&service, &hello_line("%1"), 310);
        assert!(client.accepted());
        client.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
        assert_eq!(client.ack(), Some(1));
        assert_eq!(shared.state("deck-a"), Some("working"));
        client.send(body.clone());
        assert_eq!(client.refused().as_deref(), Some(code), "{body}");
        assert!(client.ended());
        assert_eq!(shared.state("deck-a"), None, "{body}");
        until("the stream's slot", || probe::streams() == 0);
    }
    // a line longer than the wire allows, whole or never ended
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    client
        .writer
        .write_all(
            format!(
                "{{\"protocol\":\"pi-observer\",\"pad\":\"{}\"}}\n",
                "x".repeat(1100)
            )
            .as_bytes(),
        )
        .unwrap();
    assert_eq!(client.refused().as_deref(), Some("bad-frame"));
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    client
        .writer
        .write_all("y".repeat(1100).as_bytes())
        .unwrap();
    assert_eq!(client.refused().as_deref(), Some("bad-frame"));
    until("both streams' slots", || {
        probe::streams() == 0 && probe::pending() == 0
    });
}

#[test]
fn a_stream_that_floods_or_stalls_is_ended_inside_its_bounds() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, _) = service(&shared, QUICK);
    // more unanswered lines than one stream may hold: the worker is kept
    // busy by a slow listing while the client writes ahead
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    client.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    for seq in 2..=(MAX_PENDING_PER_STREAM as u64 + 8) {
        client.send(serde_json::json!({ "seq": seq }));
    }
    shared.gate.set(true);
    // the first frame's answer is late (the listing outlasted the budget) or
    // the flood is refused: either way the stream ends and nothing stays
    let reply = client.outcome();
    assert!(
        matches!(reply.as_deref(), Some("late" | "busy")),
        "{reply:?}"
    );
    until("the flooding stream's end", || {
        probe::live() == 0 && probe::streams() == 0 && probe::pending() == 0
    });
    assert_eq!(
        shared.state("deck-a"),
        None,
        "a late commit wrote nothing that stays"
    );

    // read ahead beyond the bound without a slow worker: refused as busy
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    let mut burst = String::new();
    for seq in 1..=(MAX_PENDING_PER_STREAM as u64 + 30) {
        burst.push_str(&frame_line(serde_json::json!({ "seq": seq })));
        burst.push('\n');
    }
    client.writer.write_all(burst.as_bytes()).unwrap();
    assert_eq!(client.outcome().as_deref(), Some("busy"));
    until("the burst stream's end", || {
        probe::streams() == 0 && probe::pending() == 0
    });

    // silence: no valid frame for the stale bound ends the stream — and
    // only withdraws; nothing is reported in the observation's place
    let mut client = Client::open(&service, &hello_line("%2"), 410);
    assert!(client.accepted());
    client.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    assert_eq!(client.ack(), Some(1));
    assert_eq!(shared.state("deck-b"), Some("working"));
    // a line that never ends is not a frame and does not keep it alive
    client
        .writer
        .write_all(b"{\"protocol\":\"pi-observer\"")
        .unwrap();
    assert_eq!(client.refused().as_deref(), Some("stale"));
    assert!(client.ended());
    assert_eq!(
        shared.state("deck-b"),
        None,
        "withdrawn, not turned into an ending"
    );
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    until("the stale stream's end", || probe::streams() == 0);

    // a client that never reads its acknowledgements cannot hold the service
    let mut client = Client::open(&service, &hello_line("%2"), 410);
    assert!(client.accepted());
    client
        .writer
        .set_write_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut seq = 0u64;
    let end = Instant::now() + Duration::from_secs(20);
    while probe::live() == 1 && Instant::now() < end {
        seq += 1;
        if client
            .writer
            .write_all(format!("{}\n", frame_line(serde_json::json!({ "seq": seq }))).as_bytes())
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    until("the unread stream's end", || {
        probe::live() == 0 && probe::streams() == 0
    });
}

#[test]
fn the_service_holds_a_bounded_number_of_streams_and_never_makes_the_listener_wait() {
    let _guard = fresh();
    // as many panes as the service holds streams, plus one
    let count = MAX_STREAMS as u32 + 1;
    let mut rows = Vec::new();
    let mut parts = Vec::new();
    for index in 0..count {
        let shell = 1000 + index * 10;
        rows.push(row(
            &format!("deck-{index}"),
            &format!("${index}"),
            &format!("%{index}"),
            shell,
            "pi",
        ));
        parts.push(pane(shell, 100 + index, shell + 1, 2000));
    }
    let shared = Shared::new(rows, table(&parts));
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut clients: Vec<Client> = (0..MAX_STREAMS as u32)
        .map(|index| {
            Client::open(
                &service,
                &hello_line(&format!("%{index}")),
                1001 + index * 10,
            )
        })
        .collect();
    for client in clients.iter_mut() {
        assert!(client.accepted());
    }
    assert_eq!(
        (probe::streams(), probe::live()),
        (MAX_STREAMS, MAX_STREAMS)
    );
    // one more is refused at once, on the thread that handed it over
    let last = MAX_STREAMS as u32;
    let started = Instant::now();
    let mut extra = Client::open(&service, &hello_line(&format!("%{last}")), 1001 + last * 10);
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(extra.refused().as_deref(), Some("busy"));
    assert_eq!(reasons.lock_or_recover().last(), Some(&"pi-busy"));
    // a stream that ends gives its place to the next
    clients.pop();
    until("a free slot", || probe::streams() == MAX_STREAMS - 1);
    // a worker that is stuck in a listing keeps nobody on the listener's
    // thread: the hand-over returns at once and the hello waits its turn
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    clients[0].send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    let started = Instant::now();
    let mut next = Client::open(&service, &hello_line(&format!("%{last}")), 1001 + last * 10);
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "handing over never waits"
    );
    shared.gate.set(true);
    assert_eq!(clients[0].ack(), Some(1));
    assert!(next.accepted());
    drop(next);
    drop(clients);
    until("every stream's end", || {
        probe::streams() == 0 && probe::live() == 0
    });
}

#[test]
fn frames_that_wait_together_share_one_listing_and_keep_their_own_checks() {
    let _guard = fresh();
    let shared = Shared::new(
        vec![
            row("deck-a", "$1", "%1", 300, "pi"),
            row("deck-b", "$2", "%2", 400, "pi"),
            row("deck-c", "$3", "%3", 500, "pi"),
        ],
        table(&[
            pane(300, 7, 310, 2000),
            pane(400, 8, 410, 2100),
            pane(500, 9, 510, 2200),
        ]),
    );
    let (service, _) = service(&shared, Bounds::LIVE);
    let mut a = Client::open(&service, &hello_line("%1"), 310);
    let mut b = Client::open(&service, &hello_line("%2"), 410);
    let mut c = Client::open(&service, &hello_line("%3"), 510);
    assert!(a.accepted() && b.accepted() && c.accepted());
    // heartbeats ask tmux nothing, however many there are
    let before = shared.listings.load(Ordering::SeqCst);
    for seq in 1..=5 {
        a.send(serde_json::json!({ "seq": seq }));
        assert_eq!(a.ack(), Some(seq));
    }
    assert_eq!(shared.listings.load(Ordering::SeqCst), before);
    // A's frame holds the worker in a listing; B's and C's frames queue
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    a.send(serde_json::json!({"seq": 6, "run": 1, "state": "working"}));
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    b.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    c.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    std::thread::sleep(Duration::from_millis(150));
    // meanwhile B's Pi leaves the foreground: a shared listing cannot hide it
    shared.put(process(450, 400, 8, 450, 4000));
    shared.foreground(8, 450);
    shared.gate.set(true);
    assert_eq!(a.ack(), Some(6));
    assert_eq!(b.refused().as_deref(), Some("generation-mismatch"));
    assert_eq!(c.ack(), Some(1));
    assert_eq!(
        shared.listings.load(Ordering::SeqCst),
        before + 2,
        "one listing for A's frame, one for B's and C's together"
    );
    assert_eq!(
        (
            shared.state("deck-a"),
            shared.state("deck-b"),
            shared.state("deck-c")
        ),
        (Some("working"), None, Some("working"))
    );
    drop((a, b, c));
    until("every stream's end", || probe::streams() == 0);
}

#[test]
fn an_answer_that_outlasts_its_budget_is_late_and_commits_nothing_that_stays() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, QUICK);
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted());
    shared.gate.set(false);
    client.send(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    assert_eq!(client.refused().as_deref(), Some("late"));
    assert!(client.ended());
    assert_eq!(
        probe::live(),
        0,
        "the lease ended before the worker came back"
    );
    shared.gate.set(true);
    until("the stream's end", || probe::streams() == 0);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(reasons.lock_or_recover().as_slice(), ["pi-late"]);
    // a hello whose answer is late binds nothing that stays either
    shared.gate.set(false);
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert_eq!(client.refused().as_deref(), Some("late"));
    shared.gate.set(true);
    until("the late hello's lease", || {
        probe::live() == 0 && probe::streams() == 0
    });
    let mut client = Client::open(&service, &hello_line("%1"), 310);
    assert!(client.accepted(), "and the same Pi may bind afterwards");
    drop(client);
    until("the last stream's end", || {
        probe::streams() == 0 && probe::live() == 0
    });
}

// ---------- real processes: the kernel's own answers --------------------------------

/// A throwaway tmux server of the bundled tmux on a socket inside its own
/// directory, and a status socket beside it. Everything is this test's.
struct Server {
    base: std::path::PathBuf,
}

impl Server {
    fn start(name: &str) -> Self {
        // short: a socket address holds about a hundred bytes
        let base = std::path::PathBuf::from(format!("/tmp/deck-pi-t{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .unwrap();
        let server = Self {
            base: std::fs::canonicalize(&base).unwrap(),
        };
        for session in ["deck-a", "deck-b"] {
            server
                .tmux(&[
                    "new-session",
                    "-d",
                    "-s",
                    session,
                    "-x",
                    "200",
                    "-y",
                    "50",
                    "/bin/zsh",
                    "-f",
                ])
                .expect("the bundled tmux starts a session");
        }
        server
    }

    fn runner(
        &self,
    ) -> impl Fn(&[&str]) -> Result<String, crate::error::DeckError> + Send + Sync + 'static {
        let base = self.base.clone();
        move |args| {
            let mut command = std::process::Command::new(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries/tmux-aarch64-apple-darwin"),
            );
            command
                .env_clear()
                .env("HOME", &base)
                .env("PATH", "/usr/bin:/bin")
                .env("TERM", "xterm-256color")
                .env("SHELL", "/bin/zsh")
                .arg("-f")
                .arg("/dev/null")
                .arg("-S")
                .arg(base.join("t"));
            crate::tmux::captured_output(&mut command, args)
        }
    }

    fn tmux(&self, args: &[&str]) -> Result<String, crate::error::DeckError> {
        (self.runner())(args)
    }

    fn fact(&self, session: &str, format: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", session, format])
            .unwrap()
            .trim()
            .to_string()
    }

    fn type_line(&self, session: &str, text: &str) {
        self.tmux(&["send-keys", "-t", session, "-l", text])
            .unwrap();
        self.tmux(&["send-keys", "-t", session, "Enter"]).unwrap();
    }

    fn rows(&self) -> Vec<PaneRow> {
        crate::tmux::list_panes_with(&self.runner()).unwrap_or_default()
    }

    fn hello(&self, session: &str) -> String {
        frame_line(serde_json::json!({
            "socket": "t", "server_pid": self.fact(session, "#{pid}").parse::<u32>().unwrap(),
            "pane": self.fact(session, "#{pane_id}"), "observerVersion": 1,
            "piVersion": "1.1.0", "mode": "tui",
            "capabilities": { "signal": true, "lifecycle": false }
        }))
    }

    fn state(&self, session: &str) -> Option<&'static str> {
        agent_status::projections(&self.rows())
            .get(session)
            .map(|observation| observation.state)
    }

    fn evidence(&self, session: &str) -> agent_status::Evidence {
        agent_status::generation_evidence(
            &self.rows(),
            agent_status::live_generation,
            agent_status::foreground_name,
        )
        .get(session)
        .copied()
        .unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // this test's own server, by its full socket path
        let _ = self.tmux(&["kill-server"]);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// The kernel's own answers, with real processes in two real panes: the peer
/// of a real socket, its group, its terminal's foreground and its ancestry.
/// `nc` stands in for the observer: it is the pane's foreground program and
/// connects by itself, exactly the topology the observer has inside Pi.
#[test]
fn real_processes_bind_only_as_their_own_pane_s_foreground_program() {
    let _guard = fresh();
    let server = Server::start("real");
    let listener =
        agent_status::listen_at(&server.base.join("status.sock")).expect("a status socket");
    let reasons: Reasons = Arc::new(Mutex::new(Vec::new()));
    let runner = server.runner();
    let service = {
        let report = reasons.clone();
        Service::start_in(
            Arc::new(move |reason| report.lock_or_recover().push(reason)),
            World {
                process: Box::new(crate::procinfo::process),
                listing: Box::new(move || crate::tmux::list_panes_with(&runner).ok()),
                enabled: Box::new(|| SIGNAL),
                socket: "t".into(),
            },
            Bounds::LIVE,
        )
    };
    // the listener's own step: the first line, then the hand-over
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let accepting = std::thread::spawn({
        let stop = stop.clone();
        move || {
            listener.set_nonblocking(true).unwrap();
            while !stop.load(Ordering::Acquire) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut line = String::new();
                if BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .is_ok_and(|read| read > 0)
                {
                    service.hand(stream, line.trim_end().to_string());
                }
            }
        }
    });
    let socket = server.base.join("status.sock");
    let socket = socket.to_str().unwrap();
    let screen = |session: &str| {
        server
            .tmux(&["capture-pane", "-p", "-t", session])
            .unwrap_or_default()
    };

    // A: the pane's foreground program connects and says hello for its own pane
    server.type_line("deck-a", &format!("/usr/bin/nc -U {socket}"));
    until("nc in A's foreground", || {
        server.fact("deck-a", "#{pane_current_command}") == "nc"
    });
    server.type_line("deck-a", &server.hello("deck-a"));
    until("A's lease", || probe::live() == 1);
    assert!(screen("deck-a").contains("\"accepted\":true"));
    assert_eq!(
        (
            server.evidence("deck-a").pi,
            server.evidence("deck-a").pi_signal
        ),
        (true, Some(PiCoverage::Connected))
    );
    assert_eq!(server.evidence("deck-b"), agent_status::Evidence::default());
    server.type_line(
        "deck-a",
        &frame_line(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
    );
    until("A's word", || server.state("deck-a") == Some("working"));

    // B's foreground program names A's pane: the kernel says whose it is
    server.type_line("deck-b", &format!("/usr/bin/nc -U {socket}"));
    until("nc in B's foreground", || {
        server.fact("deck-b", "#{pane_current_command}") == "nc"
    });
    server.type_line("deck-b", &server.hello("deck-a"));
    until("the foreign hello's refusal", || {
        reasons.lock_or_recover().contains(&"pi-foreign-pane")
    });
    until("B's shell again", || {
        server.fact("deck-b", "#{pane_current_command}") == "zsh"
    });
    assert_eq!(probe::live(), 1);
    assert_eq!(server.state("deck-a"), Some("working"), "A is not touched");
    assert!(!server.evidence("deck-b").pi);

    // a background job of B's shell that inherited the pane's environment
    let own = server.hello("deck-b");
    let hello_file = server.base.join("hello-b");
    std::fs::write(&hello_file, format!("{own}\n")).unwrap();
    let hello_file = hello_file.to_str().unwrap();
    server.type_line(
        "deck-b",
        &format!("(cat {hello_file}; sleep 2) | /usr/bin/nc -U {socket} &"),
    );
    until("the background job's refusal", || {
        reasons.lock_or_recover().contains(&"pi-not-foreground")
    });
    assert_eq!(probe::live(), 1);
    // a wrapper leads the foreground group and the client is its child
    reasons.lock_or_recover().clear();
    server.type_line(
        "deck-b",
        &format!("/bin/zsh -fc '(cat {hello_file}; sleep 2) | /usr/bin/nc -U {socket}; true'"),
    );
    until("the wrapped client's refusal", || {
        reasons.lock_or_recover().contains(&"pi-not-foreground")
    });
    until("B's shell again", || {
        server.fact("deck-b", "#{pane_current_command}") == "zsh"
    });
    assert!(
        !server.evidence("deck-b").pi,
        "none of them recognized B as Pi"
    );
    assert_eq!(server.state("deck-b"), None);

    // B's own foreground program, for B's own pane: a second, independent lease
    server.type_line("deck-b", &format!("/usr/bin/nc -U {socket}"));
    until("nc in B's foreground", || {
        server.fact("deck-b", "#{pane_current_command}") == "nc"
    });
    server.type_line("deck-b", &own);
    until("B's lease", || probe::live() == 2);
    assert_eq!(
        server.evidence("deck-b").pi_signal,
        Some(PiCoverage::Connected)
    );
    server.type_line(
        "deck-b",
        &frame_line(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
    );
    until("B's word", || server.state("deck-b") == Some("working"));
    server.type_line(
        "deck-a",
        &frame_line(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"})),
    );
    until("A's ending", || server.state("deck-a") == Some("turn-done"));
    assert_eq!(server.state("deck-b"), Some("working"));

    // the scheduler's pre-fire question, asked of the live kernel
    let rows = server.rows();
    let a = rows
        .iter()
        .find(|row| row.session_name == "deck-a")
        .unwrap();
    assert!(agent_status::pi_foreground(
        a.server_pid,
        &a.pane_id,
        a.pane_pid,
        agent_status::live_generation,
        agent_status::foreground_name,
    ));

    // A's program ends without a word: the stream's end withdraws its ending
    server.tmux(&["send-keys", "-t", "deck-a", "C-c"]).unwrap();
    until("A's lease to end", || probe::live() == 1);
    assert_eq!(server.state("deck-a"), None);
    assert_eq!(server.state("deck-b"), Some("working"));
    until("A's shell again", || {
        server.fact("deck-a", "#{pane_current_command}") == "zsh"
    });
    assert!(!server.evidence("deck-a").pi, "a shell in front is no Pi");
    assert!(!agent_status::pi_foreground(
        a.server_pid,
        &a.pane_id,
        a.pane_pid,
        agent_status::live_generation,
        agent_status::foreground_name,
    ));

    stop.store(true, Ordering::Release);
    accepting.join().unwrap();
    drop(server);
    until("the last stream's end", || {
        probe::live() == 0 && probe::streams() == 0
    });
}
