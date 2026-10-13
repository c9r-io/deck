use super::*;
use crate::agent_status::{self, ProcessTable, STORE_TEST_LOCK};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::Condvar;

const SIGNAL: Capabilities = Capabilities {
    signal: true,
    lifecycle: false,
};
const BOTH: Capabilities = Capabilities {
    signal: true,
    lifecycle: true,
};
const LIFECYCLE: Capabilities = Capabilities {
    signal: false,
    lifecycle: true,
};

type StandIns = HashMap<u32, (Arc<InstanceWatch>, crate::procinfo::InstanceStandIn)>;

/// The stand-in execution instance of every peer a test names, by pid.
static INSTANCES: Mutex<Option<StandIns>> = Mutex::new(None);

/// The watch of the program `pid` runs now: what the worker takes from the
/// kernel when that process says hello.
fn instance_of(pid: u32) -> Arc<InstanceWatch> {
    INSTANCES
        .lock_or_recover()
        .get_or_insert_with(HashMap::new)
        .entry(pid)
        .or_insert_with(InstanceWatch::stand_in)
        .0
        .clone()
}

/// Something happened to the process `pid`, said to the watch it has: an
/// exec, the exit, or a kernel that cannot be asked. A later hello of that
/// pid is watched afresh — it is the program that runs then.
fn became(pid: u32, what: impl FnOnce(&crate::procinfo::InstanceStandIn)) {
    let known = INSTANCES
        .lock_or_recover()
        .get_or_insert_with(HashMap::new)
        .remove(&pid);
    if let Some((_, switch)) = known {
        what(&switch);
    }
}

/// The process `pid` made a child: the kernel says so to the watch it has.
fn forked(pid: u32) {
    if let Some((_, switch)) = INSTANCES
        .lock_or_recover()
        .get_or_insert_with(HashMap::new)
        .get(&pid)
    {
        switch.forked();
    }
}

/// The listener's count of the connections it accepted, as these tests keep
/// it: every message is one more connection, and so is every mark the
/// worker takes.
static PLACES: AtomicU64 = AtomicU64::new(0);

/// A connection is made now: its place.
fn connected() -> u64 {
    PLACES.fetch_add(1, Ordering::SeqCst) + 1
}

/// What an observer holds between two messages, by its lease: its process
/// and the nonce of Deck's last answer to it.
static HELD: Mutex<Option<HashMap<u64, (u32, Nonce)>>> = Mutex::new(None);

fn hold(lease: u64, peer: u32, nonce: Nonce) {
    HELD.lock_or_recover()
        .get_or_insert_with(HashMap::new)
        .insert(lease, (peer, nonce));
}

fn held(lease: u64) -> (u32, Nonce) {
    HELD.lock_or_recover()
        .get_or_insert_with(HashMap::new)
        .get(&lease)
        .cloned()
        .expect("a lease this test bound")
}

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
    /// Processes the kernel gives no watch of.
    unwatched: Arc<Mutex<Vec<u32>>>,
    /// The worker's own connection gets no answer: it has no mark.
    unmarked: Arc<AtomicBool>,
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
            unwatched: Arc::new(Mutex::new(Vec::new())),
            unmarked: Arc::new(AtomicBool::new(false)),
        }
    }

    fn world(&self) -> World {
        let (table, rows, enabled, listings, during, gate, unwatched, unmarked) = (
            self.table.clone(),
            self.rows.clone(),
            self.enabled.clone(),
            self.listings.clone(),
            self.during.clone(),
            self.gate.clone(),
            self.unwatched.clone(),
            self.unmarked.clone(),
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
            watch: Box::new(move |pid| {
                (!unwatched.lock_or_recover().contains(&pid)).then(|| instance_of(pid))
            }),
            nonce: Box::new(Nonce::fresh),
            mark: Box::new(move || (!unmarked.load(Ordering::SeqCst)).then(connected)),
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

fn both(pane: &str) -> Hello {
    let mut hello = hello(pane);
    hello.capabilities = BOTH;
    hello
}

/// A long way off: a frame a test commits by hand is never late.
fn later() -> Instant {
    Instant::now() + Duration::from_secs(600)
}

/// A connection with `place`, whose peer the kernel names `peer`.
fn via(peer: Option<u32>, place: u64) -> Via {
    Via {
        peer,
        place,
        due: later(),
    }
}

/// The nonce a hello of `peer` is answered with.
fn challenged_as(world: &World, peer: Option<u32>) -> Result<Nonce, &'static str> {
    match challenge(world, &Bounds::LIVE, peer)? {
        Answer::Challenge(nonce) => Ok(nonce),
        other => panic!("a hello is answered with a nonce, not {other:?}"),
    }
}

/// One attest of `peer` with `nonce`, on a connection made now, as the
/// worker takes it.
fn attested(
    world: &World,
    hello: &Hello,
    nonce: &Nonce,
    peer: Option<u32>,
) -> Result<Answer, &'static str> {
    attested_on(world, hello, nonce, peer, connected())
}

/// The same on a connection that has `place`.
fn attested_on(
    world: &World,
    hello: &Hello,
    nonce: &Nonce,
    peer: Option<u32>,
    place: u64,
) -> Result<Answer, &'static str> {
    let mut listing = Listing { world, taken: None };
    let answer = bind(
        world,
        &Bounds::LIVE,
        hello,
        nonce,
        via(peer, place),
        &mut listing,
    )?;
    if let (Answer::Accepted { lease, next, .. }, Some(peer)) = (&answer, peer) {
        hold(*lease, peer, next.clone());
    }
    Ok(answer)
}

/// A hello and its attest: the lease, and what it may do.
fn bound_with(
    world: &World,
    hello: &Hello,
    peer: Option<u32>,
) -> Result<(u64, Capabilities), &'static str> {
    let nonce = challenged_as(world, peer)?;
    match attested(world, hello, &nonce, peer)? {
        Answer::Accepted { lease, usable, .. } => Ok((lease, usable)),
        Answer::Again(_) => Err("again"),
        other => panic!("an attest is not answered {other:?}"),
    }
}

fn bound_as(world: &World, hello: &Hello, peer: Option<u32>) -> Result<u64, &'static str> {
    bound_with(world, hello, peer).map(|(lease, _)| lease)
}

fn bound(world: &World, pane: &str, peer: u32) -> Result<u64, &'static str> {
    bound_as(world, &hello(pane), Some(peer))
}

/// What became of one frame of a proven sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Committed {
    Ack(u64),
    Revoked(u64),
    /// Not believed and not blamed: nothing was written.
    Again,
}

/// One frame of `lease` on a connection made now, whose peer the kernel
/// names `peer`, carrying `nonce`: exactly what the worker is handed.
fn sent_from(
    world: &World,
    lease: u64,
    frame: Frame,
    peer: Option<u32>,
    nonce: &Nonce,
) -> Result<Answer, &'static str> {
    sent_on(world, lease, frame, via(peer, connected()), nonce)
}

/// The same on a connection the test describes.
fn sent_on(
    world: &World,
    lease: u64,
    frame: Frame,
    via: Via,
    nonce: &Nonce,
) -> Result<Answer, &'static str> {
    let mut listing = Listing { world, taken: None };
    commit(world, lease, nonce, &Ok(frame), via, &mut listing)
}

/// One frame of the observer that holds `lease`: its own process, the nonce
/// it was last given. What the answer gives is what it holds next.
fn sent_as(world: &World, lease: u64, frame: Frame) -> Result<Committed, &'static str> {
    let (peer, nonce) = held(lease);
    Ok(match sent_from(world, lease, frame, Some(peer), &nonce)? {
        Answer::Ack { seq, next, .. } => {
            hold(lease, peer, next);
            Committed::Ack(seq)
        }
        Answer::Revoked { seq, next, .. } => {
            hold(lease, peer, next);
            Committed::Revoked(seq)
        }
        Answer::Again(next) => {
            hold(lease, peer, next);
            Committed::Again
        }
        other => panic!("a frame is not answered {other:?}"),
    })
}

/// The sequence number of a frame that was WRITTEN.
fn sent(world: &World, lease: u64, frame: Frame) -> Result<u64, &'static str> {
    match sent_as(world, lease, frame)? {
        Committed::Ack(seq) => Ok(seq),
        Committed::Revoked(_) => Err("revoked"),
        Committed::Again => Err("again"),
    }
}

/// What a live lease may still do, as Deck holds it.
fn usable(lease: u64) -> Option<Capabilities> {
    probe::leases()
        .into_iter()
        .find(|seen| seen.lease == lease)
        .map(|seen| seen.usable)
}

/// The observer of `lease` is gone without a word: what the worker's sweep
/// does when a lease says nothing for too long.
fn release(lease: u64) {
    end(lease, "stale");
}

/// Why a lease is over; `None` while it lives.
fn over(lease: u64) -> Option<&'static str> {
    with_registry(|registry| {
        registry
            .leases
            .get(&lease)
            .and_then(|lease| lease.over.map(|(code, _)| code))
    })
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

fn lifecycle_frame(seq: u64, event: crate::agent_pi::SessionEvent, id: &str) -> Frame {
    let body = match event {
        crate::agent_pi::SessionEvent::Start(_) => serde_json::json!({
            "seq": seq, "event": "start", "nativeSessionId": id, "reason": "startup"
        }),
        crate::agent_pi::SessionEvent::Shutdown(_) => serde_json::json!({
            "seq": seq, "event": "shutdown", "nativeSessionId": id, "reason": "quit"
        }),
        crate::agent_pi::SessionEvent::Snapshot => serde_json::json!({
            "seq": seq, "event": "snapshot", "nativeSessionId": id
        }),
    };
    match crate::agent_pi::parse_message(&frame_line(1, &"0".repeat(32), body)) {
        Ok(Message::Frame {
            frame: Ok(frame), ..
        }) => frame,
        other => panic!("a lifecycle frame, not {other:?}"),
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
    fresh_again();
    guard
}

/// The store, the notifications, the leases and what the test's observers
/// hold start empty again. The caller already holds the store lock.
fn fresh_again() {
    agent_status::reset_for_tests();
    crate::notify::reset_for_tests();
    probe::reset();
    *INSTANCES.lock_or_recover() = None;
    *HELD.lock_or_recover() = None;
}

fn sessions() -> Vec<(u64, String, crate::agent_pi::SessionEvent)> {
    crate::agent_reentry::lifecycle::pi_sessions_for_tests()
}

/// Everything a Pi observer's line can write, as numbers that only grow, and
/// every lease as it stands. A test asks before and after: an observation
/// that was written and taken back in between is heard of, not only one
/// that is still there.
#[derive(Debug, PartialEq, Eq)]
struct Written {
    /// Writes of recognition, words and withdrawals (`agent_status`).
    store: u64,
    /// Writes of observed sessions (`agent_reentry::lifecycle`).
    sessions: u64,
    /// The episode the next accepted observation gets.
    episode: agent_status::EpisodeId,
    /// What the attention layer announced, by session.
    announced: Vec<(String, u32)>,
    /// Every lease that ended, and why.
    ended: Vec<(String, &'static str)>,
    /// (lease, highest run, open, next sequence number, signal, lifecycle).
    leases: Vec<(u64, u64, bool, u64, bool, bool)>,
}

fn written() -> Written {
    let mut announced: Vec<_> = crate::notify::announced_for_tests().into_iter().collect();
    announced.sort();
    Written {
        store: agent_status::pi_writes_for_tests(),
        sessions: crate::agent_reentry::lifecycle::pi_writes_for_tests(),
        episode: agent_status::next_episode_for_tests(),
        announced,
        ended: probe::ended(),
        leases: probe::leases()
            .into_iter()
            .map(|seen| {
                (
                    seen.lease,
                    seen.run,
                    seen.open,
                    seen.next_seq,
                    seen.usable.signal,
                    seen.usable.lifecycle,
                )
            })
            .collect(),
    }
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
    // a frame of the old lease is told why that lease is over, whatever it says
    assert_eq!(sent(&world, first, working(3, 2)), Err("stale"));
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
    // nothing is on in Pi's settings (an observer loaded earlier): refused,
    // and that runtime does not come back before Pi reloads
    *shared.enabled.lock_or_recover() = Capabilities::NONE;
    assert!(!mismatched(SIGNAL));
    assert_eq!(bound(&world, "%2", 410), Err("capability-disabled"));
    assert_eq!(
        (
            shared.evidence("deck-b").pi,
            shared.evidence("deck-b").pi_signal
        ),
        (true, Some(PiCoverage::Unavailable))
    );
    assert!(
        mismatched(SIGNAL),
        "turned on again, that Pi still needs its reload"
    );
    // A capability the settings do not enable, beside one they do: the
    // lease is bound for the one and may not use the other. What its
    // runtime loaded is not what its lease may use.
    *shared.enabled.lock_or_recover() = SIGNAL;
    let mut more = hello("%2");
    more.capabilities.lifecycle = true;
    let (lease, may) = bound_with(&world, &more, Some(410)).expect("bound for what is enabled");
    assert_eq!(may, SIGNAL, "the acceptance says what the observer may do");
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Connected)
    );
    assert!(!mismatched(SIGNAL), "an attest that bound ends the wait");
    assert_eq!(
        sent_as(
            &world,
            lease,
            Frame::Lifecycle {
                seq: 1,
                event: crate::agent_pi::SessionEvent::Snapshot,
                native: crate::agent_pi::NativeId::Unsupported,
            }
        ),
        Ok(Committed::Revoked(1)),
        "claimed, and not enabled: not written, and no error"
    );
    assert_eq!(sent(&world, lease, working(2, 1)), Ok(2));
    release(lease);
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
    let refused = |frames: &[Frame], last: Frame, code: &'static str| {
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
        // and a later frame of it is told the same
        assert_eq!(sent(&world, lease, Frame::Ping { seq: 9 }), Err(code));
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

    // the switch is turned off in Deck: the lease ends now, not at its next frame
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
    // no lease is live, and the entry may be back: that Pi's observer is
    // not, until Pi loads its extensions again
    assert_eq!(probe::live(), 0);
    assert!(mismatched(SIGNAL), "a running Pi waits for its reload");
    // and a frame of a lease that outlived an external edit of the file
    let d = bound(&world, "%2", 410).unwrap();
    *shared.enabled.lock_or_recover() = Capabilities::NONE;
    assert_eq!(sent(&world, d, working(1, 1)), Err("capability-disabled"));
    assert_eq!(shared.state("deck-b"), None);
}

// ---------- whose bytes: the peer, the forks, the place and the nonce, together ----------

/// R01, with the kernel's answers described exactly. A line is the bound
/// process's own only when the peer of its connection is that process, it
/// carries the nonce of Deck's last answer to that process, and no fork of
/// that process was counted since the nonce was made — all three, on a
/// connection that was made after Deck last looked (every connection here
/// is; the test after the next takes that away). Take any one away and
/// nothing is written: not a word, not an episode, not a session, not the
/// end of the lease. Not even for a moment (`written`).
#[test]
fn a_line_is_the_observer_s_own_only_with_its_peer_its_nonce_and_no_fork_together() {
    use crate::agent_pi::{SessionEvent, StartReason};
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let world = shared.world();
    let a = bound_as(&world, &both("%1"), Some(310)).unwrap();
    let b = bound(&world, "%2", 410).unwrap();
    let start = SessionEvent::Start(StartReason::Startup);
    assert_eq!(
        sent(&world, a, lifecycle_frame(1, start, "session-a")),
        Ok(1)
    );
    assert_eq!(sent(&world, a, working(2, 1)), Ok(2));
    assert_eq!(sent(&world, b, working(1, 1)), Ok(1));
    // what somebody else would want A's observer to have said: its run
    // ended, it is in another session, it is gone, it is alive
    let forged = || {
        [
            done(3, 1),
            lifecycle_frame(3, SessionEvent::Snapshot, "session-x"),
            Frame::Reset {
                seq: 3,
                reason: ResetReason::Quit,
            },
            Frame::Ping { seq: 3 },
        ]
    };
    let unchanged = |shared: &Shared, before: &Written, why: &str| {
        assert_eq!(&written(), before, "{why}");
        assert_eq!(
            (shared.state("deck-a"), shared.state("deck-b")),
            (Some("working"), Some("working")),
            "{why}"
        );
        assert_eq!(sessions().len(), 1, "{why}");
        assert_eq!(sessions()[0].1, "session-a", "{why}");
    };
    let before = written();
    let (_, nonce) = held(a);

    // 1. Another process — a child of the observer, in the background of
    //    its pane or without a terminal; the other pane's observer; a process
    //    the kernel does not know — on a connection of its own. The kernel
    //    names THAT process, and whatever it knows, the very nonce included,
    //    nothing of the lease is touched.
    shared.put(process(320, 310, 7, 310, 3000));
    shared.put(process(321, 310, 0, 0, 3000));
    for stranger in [320, 321, 410, 999] {
        for frame in forged() {
            assert_eq!(
                sent_from(&world, a, frame, Some(stranger), &nonce),
                Err("sender-mismatch"),
                "{stranger}"
            );
        }
    }
    for frame in forged() {
        assert_eq!(sent_from(&world, a, frame, None, &nonce), Err("no-peer"));
    }
    unchanged(&shared, &before, "a connection of another process");
    assert_eq!(probe::agains(), 0);

    // 2. A connection the kernel names the observer for — it looked at a
    //    connection its child had written on — and the child's copy of the
    //    nonce. The fork that made the child was counted: neither believed
    //    nor blamed, and every answer uses a nonce up.
    forked(310);
    let mut known = nonce.clone();
    for frame in forged() {
        match sent_from(&world, a, frame, Some(310), &known) {
            Ok(Answer::Again(next)) => {
                assert!(!next.same(&known), "a nonce nobody has seen");
                // the child reads that answer too; it makes another child
                known = next;
                forked(310);
            }
            other => panic!("{other:?}"),
        }
    }
    unchanged(&shared, &before, "a child's copy of the nonce");
    assert_eq!(
        probe::again_reasons(),
        ["fork"; 4],
        "the nonce was the right one"
    );

    // 3. The observer's own process and no fork, without the nonce: one
    //    that was used up, one that was never given, the other lease's.
    match sent_from(&world, a, Frame::Ping { seq: 3 }, Some(310), &known) {
        // the last answer of (2) was read by the child, and a fork was counted since
        Ok(Answer::Again(next)) => known = next,
        other => panic!("{other:?}"),
    }
    let (_, of_b) = held(b);
    for wrong in [nonce.clone(), Nonce::of([7; 16]), of_b] {
        for frame in forged() {
            match sent_from(&world, a, frame, Some(310), &wrong) {
                Ok(Answer::Again(next)) => known = next,
                other => panic!("{other:?}"),
            }
        }
    }
    unchanged(&shared, &before, "without the nonce");
    assert_eq!(
        probe::again_reasons()[4..],
        [&["fork"][..], &["nonce"; 12][..]].concat()[..],
        "the process was the right one, and made no child"
    );

    // 4. All three: the observer's own frame, with the nonce of the answer
    //    before it. It is written — and it is the very next episode: nothing
    //    was written and taken back in all of the above.
    let episode = agent_status::next_episode_for_tests();
    hold(a, 310, known);
    assert_eq!(sent(&world, a, done(3, 1)), Ok(3));
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    assert_eq!(shared.episode("deck-a"), Some(episode));
    assert_eq!(
        crate::notify::announced_for_tests().get("deck-a").copied(),
        Some(1),
        "the one ending, announced once"
    );
    // B heard nothing of it, and its own nonce is still its own
    assert_eq!(sent(&world, b, done(2, 1)), Ok(2));

    // 5. A line that was written once is not written twice: the same bytes
    //    again carry a nonce that is used up.
    let (_, used) = held(a);
    assert_eq!(sent(&world, a, working(4, 2)), Ok(4));
    let again = written();
    match sent_from(&world, a, working(4, 2), Some(310), &used) {
        Ok(Answer::Again(next)) => hold(a, 310, next),
        other => panic!("{other:?}"),
    }
    assert_eq!(written(), again);
    // and the observer itself, sending a number twice, ends its lease
    assert_eq!(sent(&world, a, working(4, 2)), Err("bad-seq"));
    assert_eq!(over(a), Some("bad-seq"));
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// The same for the two messages that bind. A hello claims nothing and is
/// answered with a nonce for THAT process; an attest binds only with the
/// nonce its own process was given, once, in time, and when that process
/// made no child and executed nothing since.
#[test]
fn an_attest_binds_only_with_the_nonce_its_own_process_was_given() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let writes = agent_status::pi_writes_for_tests();
    let nothing = |shared: &Shared, why: &str| {
        assert_eq!(probe::live(), 0, "{why}");
        assert!(!shared.evidence("deck-a").pi, "{why}");
        assert_eq!(agent_status::pi_writes_for_tests(), writes, "{why}");
    };
    // no hello at all, and a nonce nobody gave
    assert_eq!(
        attested(&world, &hello("%1"), &Nonce::of([1; 16]), Some(310)),
        Err("no-challenge")
    );
    // a hello is answered whoever says it, and binds nothing
    let given = challenged_as(&world, Some(310)).unwrap();
    assert_eq!(probe::challenges(), 1);
    nothing(&shared, "a hello");
    // the nonce of one process is nothing in the mouth of another: a child
    // that read it from the connection of the hello, the other pane's Pi
    shared.put(process(320, 310, 7, 310, 3000));
    for stranger in [320, 410] {
        assert_eq!(
            attested(&world, &hello("%1"), &given, Some(stranger)),
            Err("no-challenge")
        );
    }
    assert_eq!(attested(&world, &hello("%1"), &given, None), Err("no-peer"));
    nothing(&shared, "another process's attest");
    // a wrong nonce uses the right one up: an attest is tried once
    assert_eq!(
        attested(&world, &hello("%1"), &Nonce::of([2; 16]), Some(310)),
        Err("no-challenge")
    );
    assert_eq!(
        attested(&world, &hello("%1"), &given, Some(310)),
        Err("no-challenge")
    );
    assert_eq!(probe::challenges(), 0);
    nothing(&shared, "a nonce that is used up");
    // a second hello of one process replaces what its first was answered
    let first = challenged_as(&world, Some(310)).unwrap();
    let second = challenged_as(&world, Some(310)).unwrap();
    assert_eq!(probe::challenges(), 1);
    assert_eq!(
        attested(&world, &hello("%1"), &first, Some(310)),
        Err("no-challenge")
    );
    assert_eq!(
        attested(&world, &hello("%1"), &second, Some(310)),
        Err("no-challenge"),
        "used up by the attempt before it"
    );
    nothing(&shared, "an earlier hello's nonce");
    // the process made a child between its hello and its attest: the child
    // has a copy of the nonce. Neither believed nor blamed; the answer gives
    // a new nonce, and with it — and no fork since — the attest binds.
    let given = challenged_as(&world, Some(310)).unwrap();
    forked(310);
    let next = match attested(&world, &hello("%1"), &given, Some(310)) {
        Ok(Answer::Again(next)) => next,
        other => panic!("{other:?}"),
    };
    nothing(&shared, "a fork since the hello");
    assert_eq!(probe::agains(), 1);
    forked(310);
    let next = match attested(&world, &hello("%1"), &next, Some(310)) {
        Ok(Answer::Again(next)) => next,
        other => panic!("{other:?}"),
    };
    nothing(&shared, "and another");
    let lease = match attested(&world, &hello("%1"), &next, Some(310)) {
        Ok(Answer::Accepted { lease, usable, .. }) => {
            assert_eq!(usable, SIGNAL);
            lease
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    // the lease's first frame carries the nonce of the acceptance
    assert_eq!(sent(&world, lease, working(1, 1)), Ok(1));
    release(lease);
}

/// R01, the fourth thing. The peer, the nonce and the fork count together do
/// not say that a connection is the observer's alone. One that existed when
/// Deck last looked at the process may be held by a child from before that
/// look, which no later count shows; and a nonce can be read by a child from
/// a connection the observer left, and told to another. So a line counts
/// only on a connection that was made AFTER that look: one whose place among
/// the accepted connections is behind the worker's own, made at the look.
/// With the kernel's answers described exactly — the right process, the
/// lease's own nonce, no fork since — a line on a connection from before the
/// look writes nothing: not a word, not an episode, not a session, not the
/// end of the lease, not for a moment.
#[test]
fn a_line_counts_only_on_a_connection_made_after_deck_last_looked() {
    use crate::agent_pi::{SessionEvent, StartReason};
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let world = shared.world();
    let a = bound_as(&world, &both("%1"), Some(310)).unwrap();
    let start = SessionEvent::Start(StartReason::Startup);
    assert_eq!(
        sent(&world, a, lifecycle_frame(1, start, "session-a")),
        Ok(1)
    );
    assert_eq!(sent(&world, a, working(2, 1)), Ok(2));

    // The observer has a connection on which nothing was said yet, and
    // makes a child: the child holds that connection too.
    let kept = connected();
    forked(310);
    // The observer's own next frame, on a connection of its own, meets that
    // fork: Deck looks again, and marks the look.
    assert_eq!(
        sent_as(&world, a, Frame::Ping { seq: 3 }),
        Ok(Committed::Again)
    );
    assert_eq!(probe::again_reasons(), ["fork"]);
    assert_eq!(sent(&world, a, Frame::Ping { seq: 3 }), Ok(3));
    // Now a line on the connection from before that look: the kernel names
    // the observer (it looked at its connection), the nonce is the one the
    // lease has NOW — however a child came by it — and no fork was counted
    // since it was made.
    let before = written();
    for frame in [
        done(4, 1),
        lifecycle_frame(4, SessionEvent::Snapshot, "session-x"),
        Frame::Reset {
            seq: 4,
            reason: ResetReason::Quit,
        },
        Frame::Ping { seq: 4 },
    ] {
        let (_, nonce) = held(a);
        match sent_on(&world, a, frame, via(Some(310), kept), &nonce) {
            // every answer uses a nonce up, this one as well
            Ok(Answer::Again(next)) => hold(a, 310, next),
            other => panic!("{other:?}"),
        }
        assert_eq!(written(), before);
        assert_eq!(shared.state("deck-a"), Some("working"));
    }
    assert_eq!(
        probe::again_reasons()[1..],
        ["place"; 4],
        "the process, the nonce and the forks were right"
    );
    assert_eq!(sessions().len(), 1);
    // Everything the same on a connection that was made after the look: it
    // is written, and it is the very next episode.
    let episode = agent_status::next_episode_for_tests();
    assert_eq!(sent(&world, a, done(4, 1)), Ok(4));
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    assert_eq!(shared.episode("deck-a"), Some(episode));

    // The same before there is a lease. An attest on a connection that was
    // made before its hello's look — the right process, the nonce of that
    // hello's answer, no fork since — recognizes nothing; on a connection
    // made afterwards it binds.
    let writes = agent_status::pi_writes_for_tests();
    let early = connected();
    let mut given = challenged_as(&world, Some(410)).unwrap();
    for _ in 0..2 {
        given = match attested_on(&world, &hello("%2"), &given, Some(410), early) {
            Ok(Answer::Again(next)) => next,
            other => panic!("{other:?}"),
        };
        assert_eq!(probe::again_reasons().last(), Some(&"place"));
        assert_eq!(probe::live(), 1, "A's lease, and no other");
        assert_eq!(agent_status::pi_writes_for_tests(), writes);
    }
    assert!(matches!(
        attested(&world, &hello("%2"), &given, Some(410)),
        Ok(Answer::Accepted { .. })
    ));
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Connected)
    );
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// A look is marked by the worker's own connection. When that connection
/// gets no answer there is no look a later line could be judged against:
/// no nonce is made, the message is refused `busy`, and nothing changes —
/// not the nonce the observer holds, and not the count its next line is
/// judged against.
#[test]
fn without_a_mark_no_nonce_is_made_and_nothing_changes() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    // a hello
    shared.unmarked.store(true, Ordering::SeqCst);
    assert_eq!(challenged_as(&world, Some(310)), Err("busy"));
    assert_eq!(probe::challenges(), 0);
    shared.unmarked.store(false, Ordering::SeqCst);
    let lease = bound(&world, "%1", 310).unwrap();
    assert_eq!(sent(&world, lease, working(1, 1)), Ok(1));
    // a frame of a process that made a child: the look would have to move
    forked(310);
    shared.unmarked.store(true, Ordering::SeqCst);
    let before = written();
    let (_, nonce) = held(lease);
    assert_eq!(
        sent_from(&world, lease, done(2, 1), Some(310), &nonce),
        Err("busy")
    );
    assert_eq!(written(), before);
    assert_eq!(over(lease), None);
    assert_eq!(probe::agains(), 0);
    // the fork is still counted against the same nonce ...
    shared.unmarked.store(false, Ordering::SeqCst);
    assert_eq!(sent_as(&world, lease, done(2, 1)), Ok(Committed::Again));
    assert_eq!(probe::again_reasons(), ["fork"]);
    // ... and with the next one, and no fork since, the frame is written
    assert_eq!(sent(&world, lease, done(2, 1)), Ok(2));
    // a frame whose process made no child needs no new mark
    shared.unmarked.store(true, Ordering::SeqCst);
    assert_eq!(sent(&world, lease, working(3, 2)), Ok(3));
    // an attest whose process made a child since its hello: refused, and
    // the nonce it carried is used up like any other
    shared.unmarked.store(false, Ordering::SeqCst);
    let given = challenged_as(&world, Some(410)).unwrap();
    forked(410);
    shared.unmarked.store(true, Ordering::SeqCst);
    let writes = agent_status::pi_writes_for_tests();
    assert_eq!(
        attested(&world, &hello("%2"), &given, Some(410)),
        Err("busy")
    );
    assert_eq!(probe::challenges(), 0);
    assert_eq!(probe::live(), 1, "the first lease, and no other");
    assert_eq!(agent_status::pi_writes_for_tests(), writes);
    release(lease);
}

/// A nonce waits for its attest for a bounded time, and at most
/// `MAX_CHALLENGES` of them wait at once.
#[test]
fn a_nonce_waits_a_bounded_time_and_a_bounded_number_wait() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let quick = Bounds {
        challenge: Duration::from_millis(60),
        ..Bounds::LIVE
    };
    assert!(!watching(), "nothing is kept: the worker has no timer");
    let given = match challenge(&world, &quick, Some(310)) {
        Ok(Answer::Challenge(nonce)) => nonce,
        other => panic!("{other:?}"),
    };
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(
        attested(&world, &hello("%1"), &given, Some(310)),
        Err("no-challenge"),
        "too late"
    );
    assert!(!shared.evidence("deck-a").pi);
    // as many processes as nonces may wait, and one more
    for pid in 0..MAX_CHALLENGES as u32 {
        assert!(challenge(&world, &quick, Some(5000 + pid)).is_ok());
    }
    assert_eq!(probe::challenges(), MAX_CHALLENGES);
    assert_eq!(challenge(&world, &quick, Some(9000)), Err("busy"));
    // one that already waits may ask again
    assert!(challenge(&world, &quick, Some(5000)).is_ok());
    assert_eq!(probe::challenges(), MAX_CHALLENGES);
    // what nobody answered is forgotten, by the next hello or by the sweep
    std::thread::sleep(Duration::from_millis(120));
    assert!(challenge(&world, &quick, Some(9000)).is_ok());
    assert_eq!(probe::challenges(), 1);
    assert!(watching(), "an open nonce is looked after");
    std::thread::sleep(Duration::from_millis(120));
    sweep(&quick);
    assert_eq!(probe::challenges(), 0);
    assert!(!watching(), "and once it ran out, nothing is");
    // a lease is looked after while it lives, and for as long as its end
    // is remembered
    let lease = bound(&world, "%1", 310).unwrap();
    assert!(watching());
    release(lease);
    assert!(watching(), "a frame of it is still told why it ended");
}

/// R02, with the kernel's answers described exactly. What an earlier image
/// said is not the new image's: a hello it wrote is answered to whatever
/// program runs in that process now, which has to say everything itself; a
/// nonce and a lease it was given are worth nothing after its exec, whoever
/// presents them.
#[test]
fn what_an_earlier_image_said_or_was_given_is_not_the_new_image_s() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let writes = agent_status::pi_writes_for_tests();
    // A hello that waited unread while its image executed another is read
    // when Deck looks at the process: the watch that begins there is of the
    // NEW image, the nonce goes to whoever reads that connection, and
    // nothing is bound by a hello — a new image that says nothing is nothing.
    let given = challenged_as(&world, Some(310)).unwrap();
    assert_eq!(probe::live(), 0);
    assert!(!shared.evidence("deck-a").pi);
    // An image that was given a nonce and THEN executed another, which was
    // handed that nonce (an argument, the environment, a file, the
    // connection itself): the kernel reported the exec after the nonce was
    // made, and a nonce is not who a process is.
    became(310, |process| process.executed());
    assert_eq!(
        attested(&world, &hello("%1"), &given, Some(310)),
        Err("peer-executed")
    );
    assert!(!shared.evidence("deck-a").pi);
    assert_eq!(agent_status::pi_writes_for_tests(), writes);
    // The old image was bound and reported a run; a frame it wrote waited
    // unread while it executed another image. Deck reads it afterwards: the
    // right process, the right nonce, no fork — and not that program.
    let lease = bound(&world, "%1", 310).unwrap();
    assert_eq!(sent(&world, lease, working(1, 1)), Ok(1));
    let (_, nonce) = held(lease);
    let before = written();
    became(310, |process| process.executed());
    assert_eq!(
        shared.state("deck-a"),
        None,
        "withdrawn from every reader before any frame is read"
    );
    assert_eq!(
        sent_from(&world, lease, done(2, 1), Some(310), &nonce),
        Err("peer-executed")
    );
    assert_eq!(over(lease), Some("peer-executed"));
    assert_eq!(
        agent_status::next_episode_for_tests(),
        before.episode,
        "no ending was written, not even for a moment"
    );
    assert_eq!(
        crate::notify::announced_for_tests().get("deck-a").copied(),
        before.announced.first().map(|(_, count)| *count)
    );
    assert_eq!(
        (
            shared.evidence("deck-a").pi,
            shared.evidence("deck-a").pi_signal
        ),
        (false, None),
        "the new image is no recognized Pi and has no coverage"
    );
    // the new image, once it says hello and attests ITSELF, is bound as
    // itself: a new lease, and nothing of the old one
    let own = bound(&world, "%1", 310).expect("the program that runs now");
    assert_ne!(own, lease);
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(sent(&world, own, working(1, 1)), Ok(1));
    release(own);
}

// ---------- one execution instance ------------------------------------------------------

/// A lease is of ONE execution instance. A program that executes another
/// image keeps its pid, its birth instant, its terminal and — when it keeps
/// the descriptor — the connection; none of that makes the new program the
/// observer. Asked for every frame, at the bind and by the poll.
#[test]
fn a_peer_that_executed_another_image_is_not_the_observer_any_more() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let now = |shared: &Shared| {
        let table = shared.table();
        move |pane: u32| agent_status::foreground_generation(&table, pane)
    };
    let a = bound(&world, "%1", 310).unwrap();
    assert_eq!(sent(&world, a, working(1, 1)), Ok(1));
    assert_eq!(shared.state("deck-a"), Some("working"));

    // the same pid, the same birth instant, another program: an exec
    became(310, |process| process.executed());
    // what is READ is withdrawn before any frame or poll notices
    assert_eq!(shared.state("deck-a"), None, "its word is not spoken");
    assert_eq!(
        (
            shared.evidence("deck-a").pi,
            shared.evidence("deck-a").pi_signal
        ),
        (false, None),
        "what the attest proved is proven of no program that runs there"
    );
    // a frame the new program sends, with the nonce the old one was given
    assert_eq!(sent(&world, a, done(2, 1)), Err("peer-executed"));
    assert_eq!(over(a), Some("peer-executed"));
    assert_eq!(shared.state("deck-a"), None);
    // an old callback of that lease, however late, is told the same
    assert_eq!(sent(&world, a, done(2, 1)), Err("peer-executed"));
    assert_eq!(
        sent(&world, a, Frame::Ping { seq: 2 }),
        Err("peer-executed")
    );
    // the new program is nothing until it says hello itself; then it has a
    // lease of its own, and nothing of the old one
    let again = bound(&world, "%1", 310).expect("the program that runs now may bind");
    assert_ne!(again, a);
    assert_eq!(shared.state("deck-a"), None, "no word is inherited");
    assert_eq!(sent(&world, again, working(1, 1)), Ok(1));
    assert_eq!(shared.state("deck-a"), Some("working"));

    // the poll notices by itself: no frame is needed
    let b = bound(&world, "%2", 410).unwrap();
    assert_eq!(sent(&world, b, working(1, 1)), Ok(1));
    became(410, |process| process.executed());
    reconcile_with(&shared.rows(), now(&shared));
    assert_eq!(over(b), Some("peer-executed"));
    assert_eq!(shared.state("deck-b"), None);
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable),
        "B is `pi` by name: a name says no more than that"
    );
    assert_eq!(over(again), None, "A's new lease is not B's");
    assert_eq!(shared.state("deck-a"), Some("working"));

    // an old lease that was not noticed yet does not stand in the way of
    // the program that runs now: it ends first, for its own reason
    became(310, |process| process.executed());
    let third = bound(&world, "%1", 310).expect("not a duplicate of a program that is gone");
    assert_eq!(over(again), Some("peer-executed"));
    assert_eq!(shared.state("deck-a"), None);
    release(third);

    // an exec between the hello and the attest: the nonce was given to a
    // program that is gone — nothing is bound, and nothing is recognized
    agent_status::reset_for_tests();
    let nonce = challenged_as(&world, Some(310)).unwrap();
    became(310, |process| process.executed());
    assert_eq!(
        attested(&world, &hello("%1"), &nonce, Some(310)),
        Err("peer-executed")
    );
    assert!(!shared.evidence("deck-a").pi);
    // no watch at all: nothing could notice a later exec, so nothing is
    // asked of that process, and nothing is bound
    shared.unwatched.lock_or_recover().push(310);
    assert_eq!(challenged_as(&world, Some(310)), Err("no-instance"));
    assert_eq!(bound(&world, "%1", 310), Err("no-instance"));
    assert!(!shared.evidence("deck-a").pi);
    assert_eq!(probe::live(), 0);
    assert_eq!(probe::challenges(), 0);
}

/// The two other things a watch can say. A process that is gone names
/// nothing (`generation-mismatch`). A kernel that cannot be asked proves
/// nothing: the observation is withdrawn for a closed reason, and only the
/// recognition stays, which never does more than add a hold.
#[test]
fn a_peer_that_is_gone_or_cannot_be_asked_about_is_not_trusted() {
    let _guard = fresh();
    let shared = two_panes();
    let world = shared.world();
    let a = bound(&world, "%1", 310).unwrap();
    let b = bound(&world, "%2", 410).unwrap();
    assert_eq!(sent(&world, a, working(1, 1)), Ok(1));
    assert_eq!(sent(&world, b, working(1, 1)), Ok(1));

    became(310, |process| process.unreadable());
    assert_eq!(shared.state("deck-a"), None, "no word without continuity");
    assert_eq!(
        (
            shared.evidence("deck-a").pi,
            shared.evidence("deck-a").pi_signal
        ),
        (true, Some(PiCoverage::Unavailable)),
        "recognized still, and never `connected`"
    );
    assert_eq!(sent(&world, a, done(2, 1)), Err("no-instance"));
    assert_eq!(over(a), Some("no-instance"));
    assert_eq!(shared.state("deck-b"), Some("working"), "B is not touched");

    became(410, |process| process.exited());
    assert_eq!(sent(&world, b, done(2, 1)), Err("generation-mismatch"));
    assert_eq!(shared.state("deck-b"), None);
    assert_eq!(probe::live(), 0);
}

/// Turning one capability off takes exactly that one from a lease that has
/// both: the other goes on, on the same lease, with the same run and the
/// same unread episode. A frame of the lost capability that was on its way
/// uses up its number and writes nothing. Nothing gives it back.
#[test]
fn turning_one_capability_off_leaves_the_other_and_the_lease() {
    use crate::agent_pi::{SessionEvent, ShutdownReason, StartReason};
    use crate::agent_reentry::lifecycle::PiSession;
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let world = shared.world();
    let start = SessionEvent::Start(StartReason::Startup);
    let quit = SessionEvent::Shutdown(ShutdownReason::Quit);

    // ---- session observation off, in the middle of a run ----
    let (a, may) = bound_with(&world, &both("%1"), Some(310)).unwrap();
    assert_eq!(may, BOTH, "nothing less than it claimed");
    assert_eq!(
        sent(&world, a, lifecycle_frame(1, start, "session-a")),
        Ok(1)
    );
    assert_eq!(sent(&world, a, working(2, 1)), Ok(2));
    let episode = shared.episode("deck-a");
    assert_eq!(
        shared.evidence("deck-a").pi_session,
        Some(PiSession::Observed)
    );
    enabled_changed(SIGNAL);
    assert_eq!(over(a), None, "the lease stands");
    assert!(sessions().is_empty(), "its session is forgotten at once");
    assert_eq!(shared.evidence("deck-a").pi_session, None);
    assert_eq!(
        (shared.state("deck-a"), shared.episode("deck-a")),
        (Some("working"), episode),
        "the run, and its episode, are not touched"
    );
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    assert_eq!(usable(a), Some(SIGNAL), "taken from the lease at once");
    // a session frame that was on its way: its number is used, nothing is
    // written, and its answer is where the observer hears what is left
    let (peer, nonce) = held(a);
    match sent_from(
        &world,
        a,
        lifecycle_frame(3, SessionEvent::Snapshot, "session-a"),
        Some(peer),
        &nonce,
    ) {
        Ok(Answer::Revoked { seq, next, usable }) => {
            assert_eq!((seq, usable), (3, SIGNAL));
            hold(a, peer, next);
        }
        other => panic!("{other:?}"),
    }
    assert!(sessions().is_empty());
    assert_eq!(over(a), None, "and that is no reason to end what is left");
    // the run goes on to its end, numbered without a gap
    assert_eq!(sent(&world, a, done(4, 1)), Ok(4));
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    let ending = shared.episode("deck-a");
    assert!(!mismatched(SIGNAL), "what runs is what is configured");
    // turned on again: not given back to a running observer
    assert!(mismatched(BOTH), "a reload is what gives it back");
    enabled_changed(BOTH);
    assert_eq!(usable(a), Some(SIGNAL));
    assert_eq!(
        sent_as(
            &world,
            a,
            lifecycle_frame(5, SessionEvent::Snapshot, "session-a")
        ),
        Ok(Committed::Revoked(5)),
        "a capability is never widened on a live lease"
    );
    assert!(sessions().is_empty());
    assert_eq!(
        (shared.state("deck-a"), shared.episode("deck-a")),
        (Some("turn-done"), ending),
        "the unread ending is still there"
    );
    release(a);

    // ---- Signal off, in the middle of a run ----
    let b = bound_as(&world, &both("%2"), Some(410)).unwrap();
    assert_eq!(
        sent(&world, b, lifecycle_frame(1, start, "session-b")),
        Ok(1)
    );
    assert_eq!(sent(&world, b, working(2, 1)), Ok(2));
    assert!(shared.episode("deck-b").is_some());
    enabled_changed(LIFECYCLE);
    assert_eq!(over(b), None, "the lease stands");
    assert_eq!(
        (shared.state("deck-b"), shared.episode("deck-b")),
        (None, None),
        "that lease's word and episode are withdrawn"
    );
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert_eq!(
        shared.evidence("deck-b").pi_session,
        Some(PiSession::Observed),
        "and its session is not"
    );
    assert_eq!(usable(b), Some(LIFECYCLE));
    // the run's ending was on its way: not written, now or later
    assert_eq!(sent_as(&world, b, done(3, 1)), Ok(Committed::Revoked(3)));
    assert_eq!(sent_as(&world, b, working(4, 2)), Ok(Committed::Revoked(4)));
    assert_eq!(shared.state("deck-b"), None);
    // what is left goes on
    assert_eq!(
        sent(&world, b, lifecycle_frame(5, quit, "session-b")),
        Ok(5)
    );
    assert_eq!(shared.evidence("deck-b").pi_session, None);
    assert!(mismatched(BOTH) && !mismatched(LIFECYCLE));
    // the other one off as well: nothing is left, and the lease is over
    enabled_changed(Capabilities::NONE);
    assert_eq!(over(b), Some("capability-disabled"));
    assert!(sessions().is_empty());
    release(b);

    // ---- an edit of the file nobody announced: noticed by the next frame ----
    *shared.enabled.lock_or_recover() = BOTH;
    let c = bound_as(&world, &both("%1"), Some(310)).unwrap();
    assert_eq!(
        sent(&world, c, lifecycle_frame(1, start, "session-c")),
        Ok(1)
    );
    *shared.enabled.lock_or_recover() = SIGNAL;
    assert_eq!(sent(&world, c, working(2, 1)), Ok(2), "Signal is still on");
    assert!(sessions().is_empty(), "narrowed by that very commit");
    assert_eq!(usable(c), Some(SIGNAL));
    assert_eq!(
        sent_as(&world, c, lifecycle_frame(3, quit, "session-c")),
        Ok(Committed::Revoked(3))
    );
    // a capability the attest never claimed is a forgery, as before
    release(c);
    let d = bound(&world, "%1", 310).unwrap();
    assert_eq!(
        sent(&world, d, lifecycle_frame(1, start, "session-d")),
        Err("capability-disabled")
    );
    assert_eq!(over(d), Some("capability-disabled"));
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

// ---------- messages: the service as the listener uses it -------------------------------

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
    challenge: CHALLENGE_BUDGET,
};

fn until(what: &str, done: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// One line of the family: its common part and `body`.
fn message(body: serde_json::Value) -> String {
    let mut line = serde_json::json!({ "protocol": "pi-observer", "v": 2, "source": "pi" });
    line.as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    line.to_string()
}

/// A frame of `lease` that carries `nonce`.
fn frame_line(lease: u64, nonce: &str, body: serde_json::Value) -> String {
    let mut frame = serde_json::json!({ "lease": lease, "nonce": nonce });
    frame
        .as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    message(frame)
}

/// What an attest claims, without its nonce.
fn claims(pane: &str, signal: bool, lifecycle: bool) -> serde_json::Value {
    serde_json::json!({
        "socket": "deck", "server_pid": 42, "pane": pane, "observerVersion": 2,
        "piVersion": "1.1.0", "mode": "tui",
        "capabilities": { "signal": signal, "lifecycle": lifecycle }
    })
}

/// One line on a connection of its own, handed to the service as `peer`'s.
/// What comes back is where the test reads Deck's answer.
fn handed(service: &Service, line: &str, peer: Option<u32>) -> UnixStream {
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    ours.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    service.hand_as(theirs, line.to_string(), peer, connected());
    ours
}

/// Deck's one line of answer, or `None` when the connection ended without.
fn answer_of(stream: UnixStream) -> Option<serde_json::Value> {
    let mut text = String::new();
    match BufReader::new(stream).read_line(&mut text) {
        Ok(read) if read > 0 => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

fn asked(service: &Service, line: &str, peer: Option<u32>) -> Option<serde_json::Value> {
    answer_of(handed(service, line, peer))
}

/// The two capability bits of an answer: (signal, lifecycle).
fn bits(reply: &serde_json::Value) -> Option<(bool, bool)> {
    let bits = reply.get("capabilities")?;
    Some((bits["signal"].as_bool()?, bits["lifecycle"].as_bool()?))
}

fn refusal(reply: &serde_json::Value) -> Option<String> {
    reply["refused"].as_str().map(str::to_owned)
}

/// The nonce a hello of `peer` is answered with, through the service.
fn hello_through(service: &Service, peer: u32) -> String {
    let reply = asked(service, &message(serde_json::json!({})), Some(peer))
        .expect("an answer to the hello");
    assert_eq!(reply.as_object().unwrap().len(), 3, "{reply}");
    reply["nonce"]
        .as_str()
        .unwrap_or_else(|| panic!("no nonce: {reply}"))
        .to_string()
}

/// One observer as a test plays it through the service: every message on a
/// connection of its own, handed over as its process's, with the nonce of
/// the answer before it.
struct Client<'a> {
    service: &'a Service,
    peer: u32,
    lease: u64,
    nonce: String,
    /// What the last answer said the lease may do.
    may: (bool, bool),
}

impl<'a> Client<'a> {
    /// A hello and an attest of `peer`. `Err` is Deck's answer to an attest
    /// that did not bind.
    fn open_with(
        service: &'a Service,
        claims: serde_json::Value,
        peer: u32,
    ) -> Result<Self, serde_json::Value> {
        let mut attest = claims;
        attest["nonce"] = hello_through(service, peer).into();
        let reply = asked(service, &message(attest), Some(peer)).expect("an answer to the attest");
        match (
            reply["accepted"].as_u64(),
            reply["nonce"].as_str(),
            bits(&reply),
        ) {
            (Some(lease), Some(nonce), Some(may)) => {
                assert_eq!(reply.as_object().unwrap().len(), 5, "{reply}");
                Ok(Self {
                    service,
                    peer,
                    lease,
                    nonce: nonce.to_string(),
                    may,
                })
            }
            _ => Err(reply),
        }
    }

    /// A Signal observer of `pane`, bound.
    fn open(service: &'a Service, pane: &str, peer: u32) -> Self {
        Self::open_with(service, claims(pane, true, false), peer)
            .unwrap_or_else(|reply| panic!("not bound: {reply}"))
    }

    /// The refusal an attest ends in.
    fn refused_at_once(
        service: &'a Service,
        claims: serde_json::Value,
        peer: u32,
    ) -> Option<String> {
        Self::open_with(service, claims, peer)
            .err()
            .and_then(|reply| refusal(&reply))
    }

    /// The line of one frame of this lease, with the nonce it holds.
    fn line(&self, body: serde_json::Value) -> String {
        frame_line(self.lease, &self.nonce, body)
    }

    /// What an answer gives is what the observer holds next.
    fn heard(&mut self, reply: &serde_json::Value) {
        if let Some(nonce) = reply["nonce"].as_str() {
            self.nonce = nonce.to_string();
        }
        if let Some(may) = bits(reply) {
            self.may = may;
        }
    }

    /// One frame, and Deck's answer to it.
    fn send(&mut self, body: serde_json::Value) -> serde_json::Value {
        let reply = asked(self.service, &self.line(body), Some(self.peer)).expect("an answer");
        self.heard(&reply);
        reply
    }

    fn ack(&mut self, body: serde_json::Value) -> Option<u64> {
        self.send(body)["ack"].as_u64()
    }

    /// The number of a frame that was answered `revoked`.
    fn revoked(&mut self, body: serde_json::Value) -> Option<u64> {
        self.send(body)["revoked"].as_u64()
    }

    fn refused(&mut self, body: serde_json::Value) -> Option<String> {
        refusal(&self.send(body))
    }
}

#[test]
fn a_message_is_answered_on_its_own_connection_after_the_commit() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open(&service, "%1", 310);
    assert_eq!(probe::live(), 1);
    let mut nonces = vec![client.nonce.clone()];
    assert_eq!(client.ack(serde_json::json!({"seq": 1})), Some(1));
    nonces.push(client.nonce.clone());
    assert_eq!(
        client.ack(serde_json::json!({"seq": 2, "run": 1, "state": "working"})),
        Some(2)
    );
    nonces.push(client.nonce.clone());
    // the answer follows the commit: the word is there already
    assert_eq!(shared.state("deck-a"), Some("working"));
    assert_eq!(
        client.ack(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
        Some(3)
    );
    nonces.push(client.nonce.clone());
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    // every answer gave a nonce nobody had seen
    nonces.sort();
    nonces.dedup();
    assert_eq!(nonces.len(), 4);
    assert!(nonces.iter().all(|nonce| nonce.len() == 32));
    // Pi's own teardown: acknowledged, and the lease is simply over
    assert_eq!(
        client.ack(serde_json::json!({"seq": 4, "reason": "reload"})),
        Some(4)
    );
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(probe::live(), 0);
    assert_eq!(probe::ended(), [("deck-a".to_string(), "reset-reload")]);
    assert!(reasons.lock_or_recover().is_empty(), "nothing was refused");
    // whatever follows a reset, with the nonce of its answer, is late
    assert_eq!(
        client.refused(serde_json::json!({"seq": 5})).as_deref(),
        Some("late")
    );
    // the reloaded runtime is a new lease, with nothing replayed
    let client = Client::open(&service, "%1", 310);
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    release(client.lease);
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert_eq!(*reasons.lock_or_recover(), ["pi-late"]);
    assert_eq!(probe::pending(), 0);
}

/// A quitting Pi sends its last frames one after the other and is gone: they
/// are committed in order, and the lease ends for the reason its reset names.
/// Deck's own line is answered where it is handed over, with the place of
/// its connection: it waits for no worker — the worker is the one that
/// asks, in the middle of a message — and reaches none.
#[test]
fn deck_s_own_line_is_answered_with_its_place_and_reaches_no_worker() {
    let _guard = fresh();
    let shared = two_panes();
    // the attest that keeps the worker busy below is never late
    let (service, reasons) = service(&shared, Bounds::LIVE);
    // the worker is inside a listing that does not return
    shared.gate.set(false);
    let nonce = hello_through(&service, 310);
    let attest = handed(
        &service,
        &message({
            let mut claims = claims("%1", true, false);
            claims["nonce"] = nonce.into();
            claims
        }),
        Some(310),
    );
    until("the worker to be inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) == 1
    });
    // this process, on a connection of its own (a pair's peer is its maker)
    let (ours, theirs) = UnixStream::pair().expect("a socket pair");
    ours.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let asked_at = Instant::now();
    service.hand(theirs, crate::agent_pi::place_line(), 41);
    assert_eq!(
        answer_of(ours),
        Some(serde_json::json!({"protocol": "pi-observer", "v": 2, "place": 41}))
    );
    assert!(asked_at.elapsed() < Duration::from_secs(1));
    assert_eq!(probe::pending(), 1, "only the attest waits for the worker");
    assert!(reasons.lock_or_recover().is_empty());
    shared.gate.set(true);
    let accepted = answer_of(attest).expect("an answer to the attest");
    release(
        accepted["accepted"]
            .as_u64()
            .unwrap_or_else(|| panic!("no lease: {accepted}")),
    );
}

#[test]
fn the_last_frames_of_a_quitting_pi_are_committed_in_order() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open(&service, "%1", 310);
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    let announced = crate::notify::announced_for_tests()
        .get("deck-a")
        .copied()
        .unwrap_or(0);
    assert_eq!(
        client.ack(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"})),
        Some(2)
    );
    assert_eq!(
        client.ack(serde_json::json!({"seq": 3, "reason": "quit"})),
        Some(3)
    );
    assert_eq!(probe::ended(), [("deck-a".to_string(), "reset-quit")]);
    // the ending was committed before the reset withdrew it: announced once
    assert_eq!(
        crate::notify::announced_for_tests().get("deck-a").copied(),
        Some(announced + 1)
    );
    assert_eq!(
        agent_status::projections(&shared.rows()).get("deck-a"),
        None,
        "and the reset took the observation with it"
    );
    assert!(reasons.lock_or_recover().is_empty(), "nothing was refused");
}

/// Session observation is its own capability of the same lease: a lease may
/// report only what its attest claimed AND the settings enable, a
/// lifecycle-only lease reports no run, a signal-only lease names no
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

    // A carries both capabilities under ONE lease; B only session observation
    let mut a = Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    let mut b = Client::open_with(&service, claims("%2", false, true), 410).unwrap();
    assert_eq!(a.ack(start(1, "session-one")), Some(1));
    assert_eq!(
        a.ack(serde_json::json!({"seq": 2, "run": 1, "state": "working"})),
        Some(2)
    );
    assert_eq!(b.ack(start(1, "session-one")), Some(1));
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
    // a lifecycle-only lease may report no run
    assert_eq!(
        b.refused(serde_json::json!({"seq": 2, "run": 1, "state": "working"}))
            .as_deref(),
        Some("capability-disabled")
    );
    until("B's record to go with its lease", || sessions().len() == 1);
    assert_eq!(observed("deck-b"), None);
    assert!(!agent_status::projections(&rows()).contains_key("deck-b"));

    // the teardown, then the reset: the reason is kept until the lease ends
    assert_eq!(a.ack(serde_json::json!({"seq": 3, "event": "shutdown", "nativeSessionId": "session-one", "reason": "quit"})), Some(3));
    assert_eq!(
        sessions()[0].2,
        crate::agent_pi::SessionEvent::Shutdown(ShutdownReason::Quit)
    );
    assert_eq!(
        observed("deck-a"),
        None,
        "a runtime that said it is going observes no session"
    );
    assert_eq!(
        a.ack(serde_json::json!({"seq": 4, "reason": "quit"})),
        Some(4)
    );
    until("A's lease to end", || probe::live() == 0);
    assert!(
        sessions().is_empty(),
        "nothing of a lease that is over is kept"
    );

    // a signal-only lease names no session
    *shared.enabled.lock_or_recover() = BOTH;
    let mut signal_only = Client::open(&service, "%1", 310);
    assert_eq!(
        signal_only.refused(start(1, "session-one")).as_deref(),
        Some("capability-disabled")
    );
    assert!(sessions().is_empty());
    until("its lease to end", || probe::live() == 0);

    // An attest that claims session observation while the settings enable
    // Signal alone: bound for Signal, told so in its acceptance, and a
    // session frame it sends all the same is not written — without ending
    // the runs it may report.
    *shared.enabled.lock_or_recover() = SIGNAL;
    let mut narrowed = Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    assert_eq!(narrowed.may, (true, false));
    assert_eq!(
        narrowed.revoked(start(1, "session-one")),
        Some(1),
        "not committed, and no refusal"
    );
    assert!(sessions().is_empty());
    assert_eq!(observed("deck-a"), None);
    assert_eq!(
        narrowed.ack(serde_json::json!({"seq": 2, "run": 1, "state": "working"})),
        Some(2)
    );
    release(narrowed.lease);
    // an observer none of whose capabilities are enabled is refused
    assert_eq!(
        Client::refused_at_once(&service, claims("%1", false, true), 310).as_deref(),
        Some("capability-disabled")
    );
    // and with it enabled alone, a lifecycle-only observer binds and Signal does not
    *shared.enabled.lock_or_recover() = LIFECYCLE;
    assert_eq!(
        Client::refused_at_once(&service, claims("%1", true, false), 310).as_deref(),
        Some("capability-disabled")
    );
    let mut only = Client::open_with(&service, claims("%1", false, true), 310).unwrap();
    assert_eq!(only.ack(start(1, "session-two")), Some(1));
    assert_eq!(
        observed("deck-a"),
        Some(crate::agent_reentry::lifecycle::PiSession::Observed)
    );
    // turned off in Settings: the record goes at once, not at the next
    // frame — which is where the observer hears why its lease is over
    enabled_changed(Capabilities::NONE);
    assert!(sessions().is_empty());
    assert_eq!(observed("deck-a"), None);
    assert_eq!(probe::live(), 0);
    assert_eq!(
        only.refused(serde_json::json!({"seq": 2})).as_deref(),
        Some("capability-disabled")
    );
    assert!(reasons
        .lock_or_recover()
        .iter()
        .all(|reason| *reason == "pi-capability-disabled"));
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// What a runtime reports about its session is checked against what the
/// same lease reported before, and an id is Pi's own shape or nothing.
#[test]
fn a_session_frame_that_contradicts_its_lease_ends_the_lease() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let open = || Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    // a second start in one runtime
    let mut client = open();
    assert_eq!(client.ack(serde_json::json!({"seq": 1, "event": "start", "nativeSessionId": "one", "reason": "startup"})), Some(1));
    assert_eq!(client.refused(serde_json::json!({"seq": 2, "event": "start", "nativeSessionId": "two", "reason": "resume"})).as_deref(), Some("bad-lifecycle"));
    until("the lease to end", || probe::live() == 0);
    assert!(sessions().is_empty(), "neither reading is kept");
    // a teardown of a session it never reported
    let mut client = open();
    assert_eq!(client.refused(serde_json::json!({"seq": 1, "event": "shutdown", "nativeSessionId": "one", "reason": "quit"})).as_deref(), Some("bad-lifecycle"));
    until("the lease to end", || probe::live() == 0);
    // an id that is not Pi's shape is no frame at all
    let mut client = open();
    assert_eq!(client.refused(
        serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": "../sessions/x"}),
    ).as_deref(), Some("bad-native-id"));
    until("the lease to end", || probe::live() == 0);
    assert_eq!(
        *reasons.lock_or_recover(),
        ["pi-bad-lifecycle", "pi-bad-lifecycle", "pi-bad-native-id"]
    );
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// An id this build does not keep is not cut short, not stood in for and
/// not passed over: the lease's session observation becomes `unsupported`,
/// which the card can say, and stays so for the lease — a short id sent
/// afterwards is not a first observation. The runs of the same lease go
/// on. A new lease observes again. An id that is not Pi's shape at all is
/// a refusal, never this state.
#[test]
fn an_id_this_build_does_not_keep_makes_sessions_unsupported_and_leaves_the_runs() {
    use crate::agent_reentry::lifecycle::{pi_unsupported_for_tests, PiSession};
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let open = || Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    let session = || shared.evidence("deck-a").pi_session;
    let start = |seq: u64, id: &str| serde_json::json!({"seq": seq, "event": "start", "nativeSessionId": id, "reason": "startup"});
    let snapshot = |seq: u64, id: &str| serde_json::json!({"seq": seq, "event": "snapshot", "nativeSessionId": id});

    // the boundary: 256 bytes are a session, 257 are not
    let mut client = open();
    assert_eq!(client.ack(start(1, &"a".repeat(256))), Some(1));
    assert_eq!(
        (sessions()[0].1.len(), session()),
        (256, Some(PiSession::Observed))
    );
    release(client.lease);

    // an unsupported start, then a short snapshot, on ONE lease
    let mut client = open();
    assert_eq!(
        client.ack(start(1, &"a".repeat(257))),
        Some(1),
        "acknowledged: the lease goes on"
    );
    assert!(sessions().is_empty(), "nothing is kept, cut short or not");
    assert_eq!(pi_unsupported_for_tests().len(), 1);
    assert_eq!(session(), Some(PiSession::Unsupported));
    assert_eq!(client.ack(snapshot(2, &"a".repeat(256))), Some(2));
    assert!(
        sessions().is_empty(),
        "a short id afterwards is not a first observation"
    );
    assert_eq!(session(), Some(PiSession::Unsupported));
    // the runs of the same lease are untouched
    assert_eq!(
        client.ack(serde_json::json!({"seq": 3, "run": 1, "state": "working"})),
        Some(3)
    );
    assert_eq!(shared.state("deck-a"), Some("working"));
    assert_eq!(
        shared.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    assert_eq!(
        client.ack(serde_json::json!({"seq": 4, "run": 1, "state": "turn-done"})),
        Some(4)
    );
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    release(client.lease);
    assert!(pi_unsupported_for_tests().is_empty(), "gone with its lease");
    assert_eq!(session(), None);

    // A, then an id that is not kept, then B: A is no longer current and B
    // is not taken up; a teardown that names B is acknowledged and is nothing
    let mut client = open();
    assert_eq!(client.ack(start(1, "session-a")), Some(1));
    assert_eq!(session(), Some(PiSession::Observed));
    assert_eq!(client.ack(snapshot(2, &"b".repeat(512))), Some(2));
    assert!(sessions().is_empty());
    assert_eq!(session(), Some(PiSession::Unsupported));
    assert_eq!(client.ack(snapshot(3, "session-b")), Some(3));
    assert_eq!(client.ack(serde_json::json!({"seq": 4, "event": "shutdown", "nativeSessionId": "session-b", "reason": "quit"})), Some(4));
    assert!(sessions().is_empty());
    assert_eq!(session(), Some(PiSession::Unsupported));
    release(client.lease);

    // the observer's own word for an id that does not fit a line (513 bytes
    // and more are never sent): the same state, and nothing else in the frame
    let mut client = open();
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id"})),
        Some(1)
    );
    assert_eq!(session(), Some(PiSession::Unsupported));
    assert_eq!(client.ack(snapshot(2, "session-a")), Some(2));
    assert_eq!(session(), Some(PiSession::Unsupported));
    release(client.lease);
    // ... which a lease without the capability may not say,
    let mut signal_only = Client::open(&service, "%1", 310);
    assert_eq!(
        signal_only
            .refused(serde_json::json!({"seq": 1, "unavailable": "unsupported-native-id"}))
            .as_deref(),
        Some("capability-disabled")
    );
    until("the lease to end", || probe::live() == 0);
    // ... and which is one closed word
    let mut client = open();
    assert_eq!(
        client
            .refused(serde_json::json!({"seq": 1, "unavailable": "anything-else"}))
            .as_deref(),
        Some("bad-frame")
    );
    until("the lease to end", || probe::live() == 0);

    // a consistency failure is still a failure: a teardown of ANOTHER session
    let mut client = open();
    assert_eq!(client.ack(start(1, "session-a")), Some(1));
    assert_eq!(client.refused(serde_json::json!({"seq": 2, "event": "shutdown", "nativeSessionId": "session-b", "reason": "quit"})).as_deref(), Some("bad-lifecycle"));
    until("the lease to end", || probe::live() == 0);
    // and an id that is not Pi's shape is refused whatever its length
    let mut client = open();
    assert_eq!(
        client
            .refused(snapshot(1, &format!("{}/x", "c".repeat(300))))
            .as_deref(),
        Some("bad-native-id")
    );
    until("the lease to end", || probe::live() == 0);
    assert!(pi_unsupported_for_tests().is_empty());

    // a new lease observes again
    let mut client = open();
    assert_eq!(client.ack(snapshot(1, "session-a")), Some(1));
    assert_eq!(session(), Some(PiSession::Observed));
    release(client.lease);
    assert_eq!(
        *reasons.lock_or_recover(),
        [
            "pi-capability-disabled",
            "pi-bad-frame",
            "pi-bad-lifecycle",
            "pi-bad-native-id"
        ]
    );
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// A line that breaks the wire is refused, whoever sent it. It ends a lease
/// only when the line names that lease AND its sender is proven: then it is
/// the bound observer that said it. A line nobody can be shown to have
/// written changes nothing for anybody.
#[test]
fn a_line_that_breaks_the_wire_is_refused_and_ends_only_a_proven_observer_s_lease() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    // attests that never bind: each with the nonce its hello was given
    let attest = |change: &dyn Fn(String) -> String| {
        let mut claimed = claims("%1", true, false);
        claimed["nonce"] = hello_through(&service, 310).into();
        asked(&service, &change(message(claimed)), Some(310)).expect("an answer")
    };
    for (change, code) in [
        (
            (&|line: String| line.replace("\"mode\":\"tui\"", "\"mode\":\"rpc\""))
                as &dyn Fn(String) -> String,
            "bad-mode",
        ),
        (
            &|line: String| line.replace("\"observerVersion\":2", "\"observerVersion\":1"),
            "bad-version",
        ),
        (
            &|line: String| line.replace("\"pane\":\"%1\"", "\"pane\":\"%1\",\"title\":\"x\""),
            "bad-frame",
        ),
        (
            &|line: String| line.replace("\"pane\":\"%1\"", "\"pane\":\"%1\",\"pane\":\"%2\""),
            "bad-frame",
        ),
        (
            &|line: String| line.replace("\"mode\":\"tui\"", "\"mode\":\"tui\",\"seq\":1"),
            "bad-frame",
        ),
        (
            &|line: String| format!("{line}{}", " ".repeat(1024)),
            "bad-frame",
        ),
        (
            &|line: String| line.replace("1.1.0", "2.0.0"),
            "unsupported-pi-version",
        ),
        (
            &|line: String| line.replace("\"pane\":\"%1\"", "\"pane\":\"%9\""),
            "no-such-pane",
        ),
    ] {
        let reply = attest(change);
        assert_eq!(refusal(&reply).as_deref(), Some(code), "{reply}");
        assert_eq!(reply["v"], 2);
        assert_eq!(reply.as_object().unwrap().len(), 3, "{reply}");
    }
    // a hello that carries anything is no hello
    let reply = asked(&service, &message(serde_json::json!({"seq": 1})), Some(310)).unwrap();
    assert_eq!(refusal(&reply).as_deref(), Some("bad-frame"));
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
            "pi-no-such-pane",
            "pi-bad-frame"
        ]
    );
    reasons.lock_or_recover().clear();

    // An observer of the wire as it was (a Pi that loaded it before Deck was
    // replaced): told so in ITS framing, which it takes as final. Nothing of
    // it is recognized, with or without what it used to send.
    let writes = agent_status::pi_writes_for_tests();
    for old in [
        serde_json::json!({
            "protocol": "pi-observer", "v": 1, "source": "pi", "socket": "deck", "server_pid": 42,
            "pane": "%1", "observerVersion": 1, "piVersion": "1.1.0", "mode": "tui",
            "capabilities": { "signal": true, "lifecycle": false }
        }),
        serde_json::json!({"protocol": "pi-observer", "v": 1, "source": "pi", "seq": 1, "run": 1, "state": "working"}),
    ] {
        let reply = asked(&service, &old.to_string(), Some(310)).unwrap();
        assert_eq!(
            reply,
            serde_json::json!({"protocol": "pi-observer", "v": 1, "refused": "bad-version"})
        );
    }
    assert_eq!(
        agent_status::pi_writes_for_tests(),
        writes,
        "nothing was written for it"
    );
    assert_eq!(
        probe::challenges(),
        0,
        "a line of another version asks nothing"
    );
    reasons.lock_or_recover().clear();

    // frames of a PROVEN observer that break the wire: what it reported goes
    for (body, code) in [
        (
            serde_json::json!({"seq": 2, "run": 2, "state": "needs-input"}),
            "bad-frame",
        ),
        (
            serde_json::json!({"seq": 2, "title": "Allow?"}),
            "bad-frame",
        ),
        (serde_json::json!({"seq": 2, "seq": 3}), "bad-frame"),
        (serde_json::json!({"seq": 7}), "bad-seq"),
        (
            serde_json::json!({"seq": 2, "run": 1, "state": "working"}),
            "bad-run",
        ),
    ] {
        let mut client = Client::open(&service, "%1", 310);
        assert_eq!(
            client.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
            Some(1)
        );
        assert_eq!(shared.state("deck-a"), Some("working"));
        // a repeated key does not survive a JSON value: the line is written by hand
        let line = if body == serde_json::json!({"seq": 3}) {
            client
                .line(serde_json::json!({"seq": 2}))
                .replace("\"seq\":2", "\"seq\":2,\"seq\":3")
        } else {
            client.line(body.clone())
        };
        let reply = asked(&service, &line, Some(310)).unwrap();
        assert_eq!(refusal(&reply).as_deref(), Some(code), "{body}");
        assert_eq!(shared.state("deck-a"), None, "{body}");
        assert_eq!(probe::live(), 0, "{body}");
    }
    reasons.lock_or_recover().clear();

    // The same lines when their sender is NOT proven — another process, or
    // no nonce — and lines that name no lease that can be read: refused or
    // asked for again, and the lease they aim at stands, with its word.
    let mut client = Client::open(&service, "%1", 310);
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    let before = written();
    let bad = client.line(serde_json::json!({"seq": 2, "title": "Allow?"}));
    assert_eq!(
        refusal(&asked(&service, &bad, Some(410)).unwrap()).as_deref(),
        Some("sender-mismatch")
    );
    let stale = frame_line(
        client.lease,
        &"0".repeat(32),
        serde_json::json!({"seq": 2, "title": "Allow?"}),
    );
    let reply = asked(&service, &stale, Some(310)).unwrap();
    assert_eq!(reply["again"], true, "{reply}");
    client.heard(&reply);
    for line in [
        // longer than the wire allows, whole
        format!(
            "{{\"protocol\":\"pi-observer\",\"pad\":\"{}\"}}",
            "x".repeat(1100)
        ),
        // not JSON at all
        "y".repeat(1100),
        // a lease that is no number, a nonce that is none
        client
            .line(serde_json::json!({"seq": 2}))
            .replace(&format!("\"lease\":{}", client.lease), "\"lease\":\"1\""),
        frame_line(client.lease, "nonce", serde_json::json!({"seq": 2})),
        message(serde_json::json!({"lease": client.lease, "seq": 2})),
    ] {
        let reply = asked(&service, &line, Some(310)).unwrap();
        assert_eq!(refusal(&reply).as_deref(), Some("bad-frame"), "{line}");
    }
    assert_eq!(written(), before, "nothing was written for any of them");
    assert_eq!(shared.state("deck-a"), Some("working"));
    // and the observer's own next frame is its next frame
    assert_eq!(
        client.ack(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"})),
        Some(2)
    );
    release(client.lease);
    assert_eq!(probe::pending(), 0);
}

/// A lease that says nothing is over after the stale bound — the worker
/// looks by itself — and that only withdraws: nothing is reported in the
/// observation's place. A heartbeat is what keeps a lease.
#[test]
fn a_lease_that_says_nothing_is_ended_and_only_withdrawn() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, QUICK);
    let mut quiet = Client::open(&service, "%2", 410);
    let mut alive = Client::open(&service, "%1", 310);
    assert_eq!(
        quiet.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    assert_eq!(
        alive.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    let announced = crate::notify::announced_for_tests();
    let started = Instant::now();
    let mut seq = 1;
    while started.elapsed() < Duration::from_millis(1500) {
        seq += 1;
        assert_eq!(alive.ack(serde_json::json!({ "seq": seq })), Some(seq));
        std::thread::sleep(Duration::from_millis(150));
    }
    assert_eq!(probe::ended(), [("deck-b".to_string(), "stale")]);
    assert_eq!(
        shared.state("deck-b"),
        None,
        "withdrawn, not turned into an ending"
    );
    assert_eq!(
        crate::notify::announced_for_tests(),
        announced,
        "and nobody was told anything"
    );
    assert_eq!(
        shared.evidence("deck-b").pi_signal,
        Some(PiCoverage::Unavailable)
    );
    assert_eq!(shared.state("deck-a"), Some("working"));
    // a frame of the lease that went stale is told so
    assert_eq!(
        quiet
            .refused(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"}))
            .as_deref(),
        Some("stale")
    );
    assert_eq!(shared.state("deck-b"), None);
    assert_eq!(*reasons.lock_or_recover(), ["pi-stale"]);
    release(alive.lease);
}

/// At most `MAX_PENDING` messages wait for the worker; one more is refused
/// at once, on the thread that handed it over, which never waits.
#[test]
fn messages_that_wait_are_counted_and_bounded_and_the_listener_never_waits() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let client = Client::open(&service, "%1", 310);
    assert_eq!(probe::pending(), 0);
    // the worker is held in a listing by one frame
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    let first = handed(
        &service,
        &client.line(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(310),
    );
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    let hello = message(serde_json::json!({}));
    let started = Instant::now();
    let queued: Vec<UnixStream> = (0..MAX_PENDING)
        .map(|_| handed(&service, &hello, Some(410)))
        .collect();
    assert_eq!(probe::pending(), MAX_PENDING + 1);
    let extra = handed(&service, &hello, Some(410));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "handing over never waits"
    );
    assert_eq!(
        refusal(&answer_of(extra).unwrap()).as_deref(),
        Some("busy"),
        "answered while the worker is still held"
    );
    assert_eq!(*reasons.lock_or_recover(), ["pi-busy"]);
    assert_eq!(probe::pending(), MAX_PENDING + 1);
    shared.gate.set(true);
    assert_eq!(answer_of(first).unwrap()["ack"], 1);
    for stream in queued {
        assert!(answer_of(stream).unwrap()["nonce"].is_string());
    }
    until("every answer", || probe::pending() == 0);
    assert_eq!(probe::challenges(), 1, "one process, one open nonce");
    release(client.lease);
}

#[test]
fn the_service_holds_a_bounded_number_of_leases() {
    let _guard = fresh();
    // as many panes as the service holds leases, plus one
    let count = MAX_LEASES as u32 + 1;
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
    let clients: Vec<Client> = (0..MAX_LEASES as u32)
        .map(|index| Client::open(&service, &format!("%{index}"), 1001 + index * 10))
        .collect();
    assert_eq!(probe::live(), MAX_LEASES);
    // one more is refused
    let last = MAX_LEASES as u32;
    assert_eq!(
        Client::refused_at_once(
            &service,
            claims(&format!("%{last}"), true, false),
            1001 + last * 10
        )
        .as_deref(),
        Some("busy")
    );
    assert_eq!(reasons.lock_or_recover().last(), Some(&"pi-busy"));
    assert_eq!(
        shared.evidence(&format!("deck-{last}")).pi_signal,
        Some(PiCoverage::Unavailable),
        "`pi` by name, and no more than that"
    );
    // a lease that ends gives its place to the next
    release(clients[0].lease);
    let next = Client::open(&service, &format!("%{last}"), 1001 + last * 10);
    assert_eq!(probe::live(), MAX_LEASES);
    release(next.lease);
    for client in &clients {
        release(client.lease);
    }
    assert_eq!(probe::live(), 0);
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
    let mut a = Client::open(&service, "%1", 310);
    let b = Client::open(&service, "%2", 410);
    let c = Client::open(&service, "%3", 510);
    // heartbeats ask tmux nothing, however many there are
    let before = shared.listings.load(Ordering::SeqCst);
    for seq in 1..=5 {
        assert_eq!(a.ack(serde_json::json!({ "seq": seq })), Some(seq));
    }
    assert_eq!(shared.listings.load(Ordering::SeqCst), before);
    // A's frame holds the worker in a listing; B's and C's frames queue
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    let run = serde_json::json!({"seq": 1, "run": 1, "state": "working"});
    let of_a = handed(
        &service,
        &a.line(serde_json::json!({"seq": 6, "run": 1, "state": "working"})),
        Some(310),
    );
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    let of_b = handed(&service, &b.line(run.clone()), Some(410));
    let of_c = handed(&service, &c.line(run), Some(510));
    std::thread::sleep(Duration::from_millis(150));
    // meanwhile B's Pi leaves the foreground: a shared listing cannot hide it
    shared.put(process(450, 400, 8, 450, 4000));
    shared.foreground(8, 450);
    shared.gate.set(true);
    assert_eq!(answer_of(of_a).unwrap()["ack"], 6);
    assert_eq!(
        refusal(&answer_of(of_b).unwrap()).as_deref(),
        Some("generation-mismatch")
    );
    assert_eq!(answer_of(of_c).unwrap()["ack"], 1);
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
    release(a.lease);
    release(c.lease);
}

/// A message whose answer is not ready in time writes nothing — not even
/// for a moment — and ends its lease: the observer has stopped waiting.
#[test]
fn an_answer_that_would_be_late_writes_nothing_and_ends_the_lease() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, QUICK);
    let client = Client::open(&service, "%1", 310);
    let before = written();
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    let frame = handed(
        &service,
        &client.line(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(310),
    );
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    std::thread::sleep(QUICK.answer + Duration::from_millis(100));
    shared.gate.set(true);
    assert_eq!(refusal(&answer_of(frame).unwrap()).as_deref(), Some("late"));
    assert_eq!(probe::live(), 0);
    assert_eq!(shared.state("deck-a"), None);
    assert_eq!(
        agent_status::next_episode_for_tests(),
        before.episode,
        "the word was never written"
    );
    assert_eq!(probe::ended(), [("deck-a".to_string(), "late")]);
    assert_eq!(reasons.lock_or_recover().as_slice(), ["pi-late"]);
    // an attest whose answer would be late binds nothing either
    let nonce = hello_through(&service, 310);
    let writes = agent_status::pi_writes_for_tests();
    shared.gate.set(false);
    let waiting = shared.gate.waiting.load(Ordering::SeqCst);
    let mut claimed = claims("%1", true, false);
    claimed["nonce"] = nonce.into();
    let attest = handed(&service, &message(claimed), Some(310));
    until("the worker inside the listing", || {
        shared.gate.waiting.load(Ordering::SeqCst) > waiting
    });
    std::thread::sleep(QUICK.answer + Duration::from_millis(100));
    shared.gate.set(true);
    assert_eq!(
        refusal(&answer_of(attest).unwrap()).as_deref(),
        Some("late")
    );
    assert_eq!(probe::live(), 0);
    assert_eq!(
        agent_status::pi_writes_for_tests(),
        writes,
        "not recognized, not for a moment"
    );
    // and the same Pi may bind afterwards
    let client = Client::open(&service, "%1", 310);
    release(client.lease);
}

/// The refusal a program reads that executed another image and then sends a
/// frame with what the old one held, and the drop reason production counts.
#[test]
fn a_frame_of_a_process_that_executed_another_image_is_refused() {
    let _guard = fresh();
    let shared = two_panes();
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open(&service, "%1", 310);
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    assert_eq!(shared.state("deck-a"), Some("working"));
    became(310, |process| process.executed());
    assert_eq!(
        client
            .refused(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"}))
            .as_deref(),
        Some("peer-executed")
    );
    assert_eq!(probe::live(), 0);
    assert_eq!(shared.state("deck-a"), None, "no ending was written");
    assert_eq!(*reasons.lock_or_recover(), ["pi-peer-executed"]);
    assert_eq!(
        probe::ended(),
        [("deck-a".to_string(), "peer-executed")],
        "what app.log says"
    );
}

/// The same through the service: Deck acts on a change of the settings at
/// once; the observer hears what is left in the answer to its next frame, a
/// frame that crossed the change is answered `revoked`, what is left is
/// acknowledged as before, and losing the last capability ends the lease.
#[test]
fn a_lease_hears_what_it_may_still_do_in_its_next_answer_and_goes_on() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = BOTH;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    assert_eq!(client.may, (true, true));
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "event": "start", "nativeSessionId": "one", "reason": "startup"})),
        Some(1)
    );
    assert_eq!(
        client.ack(serde_json::json!({"seq": 2, "run": 1, "state": "working"})),
        Some(2)
    );
    assert_eq!(client.may, (true, true));

    // the session switch is turned off in Settings while the Pi is working
    *shared.enabled.lock_or_recover() = SIGNAL;
    enabled_changed(SIGNAL);
    assert!(sessions().is_empty(), "forgotten at once");
    // a session frame that crossed the change, then the run's ending
    assert_eq!(
        client
            .revoked(serde_json::json!({"seq": 3, "event": "snapshot", "nativeSessionId": "one"})),
        Some(3)
    );
    assert_eq!(client.may, (true, false), "said in that answer");
    assert!(sessions().is_empty());
    assert_eq!(
        client.ack(serde_json::json!({"seq": 4, "run": 1, "state": "turn-done"})),
        Some(4)
    );
    assert_eq!(shared.state("deck-a"), Some("turn-done"));
    // one more whole run under the same lease
    assert_eq!(
        client.ack(serde_json::json!({"seq": 5, "run": 2, "state": "working"})),
        Some(5)
    );
    assert_eq!(
        client.ack(serde_json::json!({"seq": 6})),
        Some(6),
        "the heartbeat goes on"
    );
    assert_eq!(client.may, (true, false));
    assert_eq!(probe::live(), 1);
    // the last capability goes: that is the end of the lease, now
    *shared.enabled.lock_or_recover() = Capabilities::NONE;
    enabled_changed(Capabilities::NONE);
    assert_eq!(probe::live(), 0);
    assert_eq!(shared.state("deck-a"), None);
    // and the observer hears why at its next frame
    assert_eq!(
        client.refused(serde_json::json!({"seq": 7})).as_deref(),
        Some("capability-disabled")
    );
    assert_eq!(*reasons.lock_or_recover(), ["pi-capability-disabled"]);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// A runtime that loaded more than the settings enable now — a switch was
/// turned off and that Pi has not reloaded — is bound for what is left, and
/// its ACCEPTANCE says so: an observer knows what it may do before it may
/// send anything, so no frame of the rest, and no session id, ever crosses
/// the socket. One that sends such a frame all the same gets `revoked`, and
/// the lease goes on.
#[test]
fn a_lease_bound_with_less_than_its_runtime_loaded_hears_so_in_its_acceptance() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let shared = two_panes();
    *shared.enabled.lock_or_recover() = SIGNAL;
    let (service, reasons) = service(&shared, Bounds::LIVE);
    let mut client = Client::open_with(&service, claims("%1", true, true), 310).unwrap();
    assert_eq!(client.may, (true, false), "what is left, in the acceptance");
    assert_eq!(
        client.ack(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
        Some(1)
    );
    assert_eq!(shared.state("deck-a"), Some("working"));
    // a session frame under a lease that never had the capability in use
    assert_eq!(
        client
            .revoked(serde_json::json!({"seq": 2, "event": "snapshot", "nativeSessionId": "one"})),
        Some(2)
    );
    assert!(sessions().is_empty());
    assert_eq!(probe::revoked(), [("deck-a".to_string(), "lifecycle")]);
    assert_eq!(
        client.ack(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
        Some(3)
    );
    assert_eq!(probe::live(), 1);
    assert!(reasons.lock_or_recover().is_empty());
    // an attest that claims exactly what is enabled is told the same two bits
    let exact = Client::open(&service, "%2", 410);
    assert_eq!(exact.may, (true, false));
    release(client.lease);
    release(exact.lease);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
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

    /// The attest of an observer in `session`'s pane, as a step of the
    /// peer: the nonce is put in by the peer that sends it.
    fn attest_with(&self, session: &str, signal: bool, lifecycle: bool) -> String {
        format!(
            "'{}'",
            message(serde_json::json!({
                "nonce": "@NONCE@",
                "socket": "t", "server_pid": self.fact(session, "#{pid}").parse::<u32>().unwrap(),
                "pane": self.fact(session, "#{pane_id}"), "observerVersion": 2,
                "piVersion": "1.1.0", "mode": "tui",
                "capabilities": { "signal": signal, "lifecycle": lifecycle }
            }))
        )
    }

    fn attest(&self, session: &str) -> String {
        self.attest_with(session, true, false)
    }

    /// Run `command` as the foreground job of `session`'s shell. It is
    /// read from a file, so that its length is no matter to the line editor.
    fn run(&self, session: &str, name: &str, command: &str) {
        let script = self.base.join(format!("{name}.cmd"));
        std::fs::write(&script, format!("{command}\n")).unwrap();
        self.type_line(session, &format!(". {}", script.to_str().unwrap()));
    }

    fn there(&self, name: &str) -> bool {
        self.base.join(name).exists()
    }

    /// Create the file a waiting peer waits for.
    fn go(&self, name: &str) {
        std::fs::write(self.base.join(name), "").unwrap();
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.base.join(name)).unwrap_or_default()
    }

    fn screen(&self, session: &str) -> String {
        self.tmux(&["capture-pane", "-p", "-t", session])
            .unwrap_or_default()
    }

    /// Wait for the file a peer creates; say what the pane shows when it
    /// does not come.
    fn wait(&self, session: &str, name: &str) {
        let end = Instant::now() + Duration::from_secs(15);
        while !self.there(name) {
            assert!(
                Instant::now() < end,
                "{name} never came; the pane shows:\n{}",
                self.screen(session)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The pane's shell is in front again.
    fn idle(&self, session: &str) {
        until("the shell in front again", || {
            self.fact(session, "#{pane_current_command}") == "zsh"
        });
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

/// A frame as a step of the peer: the peer puts in the lease and the nonce
/// it holds when it sends the line.
fn step(body: serde_json::Value) -> String {
    format!("'{}'", step_line(body))
}

/// The same without the quotes: the text of the line.
fn step_line(body: serde_json::Value) -> String {
    let mut frame = serde_json::json!({ "lease": "@LEASE@", "nonce": "@NONCE@" });
    frame
        .as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    message(frame).replace("\"@LEASE@\"", "@LEASE@")
}

/// The time bounds of the tests that run real processes: none that a slow
/// machine could reach. Their stand-in observers send no heartbeat (the real
/// one's is every five seconds), and some stay bound for the whole of a long
/// test. What these tests prove is whose bytes a line is, never how fast the
/// machine was: on a hosted runner one of them outlasted `STALE_AFTER`, and
/// the silent lease in pane B ended in the middle of it. None of them expects
/// a lease to go stale, an answer to come late or a nonce to run out; those
/// have their own tests, under bounds of their own.
const UNHURRIED: Bounds = Bounds {
    answer: Duration::from_secs(120),
    stale: Duration::from_secs(3600),
    challenge: Duration::from_secs(120),
};

/// The production listener — accept, the first line under its deadline, the
/// dispatcher — on a real server's status socket, with a Pi service that
/// asks that server's tmux and the real kernel.
struct Accepting {
    stop: Arc<AtomicBool>,
    /// How many connections Deck may still accept; negative is any.
    pass: Arc<std::sync::atomic::AtomicI64>,
    thread: Option<std::thread::JoinHandle<()>>,
    counts: agent_status::DropCounts,
}

impl Accepting {
    fn start(server: &Server, enabled: Capabilities) -> Self {
        let listener =
            agent_status::listen_at(&server.base.join("status.sock")).expect("a status socket");
        let counts = agent_status::DropCounts::new();
        let runner = server.runner();
        let service = Service::start_in(
            counts.reporter(),
            World {
                process: Box::new(crate::procinfo::process),
                listing: Box::new(move || crate::tmux::list_panes_with(&runner).ok()),
                enabled: Box::new(move || enabled),
                watch: Box::new(InstanceWatch::of),
                nonce: Box::new(Nonce::fresh),
                mark: Box::new({
                    let status = server.base.join("status.sock");
                    move || mark_at(&status)
                }),
                socket: "t".into(),
            },
            UNHURRIED,
        );
        let stop = Arc::new(AtomicBool::new(false));
        let pass = Arc::new(std::sync::atomic::AtomicI64::new(-1));
        let thread = std::thread::spawn({
            let (stop, pass, counts) = (stop.clone(), pass.clone(), counts.clone());
            move || agent_status::serve_pi_for_tests(listener, stop, &pass, &counts, service)
        });
        Self {
            stop,
            pass,
            thread: Some(thread),
            counts,
        }
    }

    /// How many messages were refused for `reason` so far.
    fn refused(&self, reason: &str) -> u32 {
        self.counts.counts().get(reason).copied().unwrap_or(0)
    }

    /// While held Deck reads nothing: what a client connects and writes
    /// waits in the kernel, so a test decides what has happened by the time
    /// Deck reads its first byte.
    fn held(&self, held: bool) {
        self.pass
            .store(if held { 0 } else { -1 }, Ordering::Release);
    }

    /// While held: Deck accepts the connection that has waited longest,
    /// and no other.
    fn one(&self) {
        self.pass.store(1, Ordering::Release);
    }
}

impl Drop for Accepting {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A program for a real pane that speaks to the status socket as an observer
/// does, makes children that keep what it had, and can replace its own
/// image: the test-only fixture the native run uses too (its header says
/// what its steps are).
const PEER_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/pi/exec-peer.c"
));

/// Build the peer at `path`; `tag` makes two builds two different images.
fn build_peer(path: &std::path::Path, tag: &str) {
    let source = path.with_extension("c");
    std::fs::write(&source, PEER_SOURCE).unwrap();
    let built = std::process::Command::new("/usr/bin/cc")
        .args(["-O1", &format!("-DBUILD=\"{tag}\""), "-o"])
        .arg(path)
        .arg(&source)
        .output()
        .expect("the C compiler");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
}

/// The kernel's own answers, with real processes in two real panes: the peer
/// of a real socket, its group, its terminal's foreground and its ancestry.
/// The peer program stands in for the observer: it is the pane's foreground
/// program and connects by itself, exactly the topology the observer has
/// inside Pi.
#[test]
fn real_processes_bind_only_as_their_own_pane_s_foreground_program() {
    let _guard = fresh();
    let server = Server::start("real");
    let accepting = Accepting::start(&server, SIGNAL);
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "observer");
    let peer =
        |log: &str, steps: &str| format!("{base}/observer {base}/status.sock {base}/{log} {steps}");
    let working = step(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    let ending = step(serde_json::json!({"seq": 2, "run": 1, "state": "turn-done"}));

    // A: the pane's foreground program says hello and attests for its own pane
    server.run(
        "deck-a",
        "a",
        &peer(
            "log-a",
            &format!(
                "@hello {attest} @mark:{base}/a1 @wait:{base}/a2 {working} @mark:{base}/a3 \
                 @wait:{base}/a4 {ending} @mark:{base}/a5 @sleep",
                attest = server.attest("deck-a")
            ),
        ),
    );
    server.wait("deck-a", "a1");
    assert!(
        server.log("log-a").contains("\"accepted\":"),
        "{}",
        server.log("log-a")
    );
    assert_eq!(probe::live(), 1);
    assert_eq!(server.fact("deck-a", "#{pane_current_command}"), "observer");
    assert_eq!(
        (
            server.evidence("deck-a").pi,
            server.evidence("deck-a").pi_signal
        ),
        (true, Some(PiCoverage::Connected))
    );
    assert_eq!(server.evidence("deck-b"), agent_status::Evidence::default());
    server.go("a2");
    server.wait("deck-a", "a3");
    assert_eq!(server.state("deck-a"), Some("working"));

    // B's foreground program names A's pane: the kernel says whose it is
    server.run(
        "deck-b",
        "b1",
        &peer(
            "log-b1",
            &format!("@hello {} @mark:{base}/b1", server.attest("deck-a")),
        ),
    );
    server.wait("deck-b", "b1");
    assert!(server
        .log("log-b1")
        .contains("\"refused\":\"foreign-pane\""));
    assert_eq!(accepting.refused("pi-foreign-pane"), 1);
    server.idle("deck-b");
    assert_eq!(probe::live(), 1);
    assert_eq!(server.state("deck-a"), Some("working"), "A is not touched");
    assert!(!server.evidence("deck-b").pi);

    // a background job of B's shell that inherited the pane's environment
    let own = server.attest("deck-b");
    server.run(
        "deck-b",
        "b2",
        &format!(
            "{} &",
            peer("log-b2", &format!("@hello {own} @mark:{base}/b2"))
        ),
    );
    server.wait("deck-b", "b2");
    assert!(server
        .log("log-b2")
        .contains("\"refused\":\"not-foreground\""));
    assert_eq!(probe::live(), 1);
    // a wrapper leads the foreground group and the client is its child
    std::fs::write(
        server.base.join("wrapped.sh"),
        format!(
            "{}\ntrue\n",
            peer("log-b3", &format!("@hello {own} @mark:{base}/b3"))
        ),
    )
    .unwrap();
    server.type_line("deck-b", &format!("/bin/zsh -f {base}/wrapped.sh"));
    server.wait("deck-b", "b3");
    assert!(server
        .log("log-b3")
        .contains("\"refused\":\"not-foreground\""));
    assert_eq!(accepting.refused("pi-not-foreground"), 2);
    server.idle("deck-b");
    assert!(
        !server.evidence("deck-b").pi,
        "none of them recognized B as Pi"
    );
    assert_eq!(server.state("deck-b"), None);

    // B's own foreground program, for B's own pane: a second, independent lease
    server.run(
        "deck-b",
        "b",
        &peer(
            "log-b",
            &format!("@hello {own} {working} @mark:{base}/b4 @sleep"),
        ),
    );
    server.wait("deck-b", "b4");
    assert_eq!(probe::live(), 2);
    assert_eq!(
        server.evidence("deck-b").pi_signal,
        Some(PiCoverage::Connected)
    );
    assert_eq!(server.state("deck-b"), Some("working"));
    server.go("a4");
    server.wait("deck-a", "a5");
    assert_eq!(server.state("deck-a"), Some("turn-done"));
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

    // A's program ends without a word. No connection says so any more: the
    // kernel does, every reader hears it at once, and the worker ends the
    // lease the next time it looks.
    server.tmux(&["send-keys", "-t", "deck-a", "C-c"]).unwrap();
    server.idle("deck-a");
    assert_eq!(server.state("deck-a"), None);
    until("A's lease to end", || probe::live() == 1);
    assert_eq!(
        probe::ended(),
        [("deck-a".to_string(), "generation-mismatch")]
    );
    assert_eq!(server.state("deck-b"), Some("working"));
    assert!(!server.evidence("deck-a").pi, "a shell in front is no Pi");
    assert!(!agent_status::pi_foreground(
        a.server_pid,
        &a.pane_id,
        a.pane_pid,
        agent_status::live_generation,
        agent_status::foreground_name,
    ));

    drop(accepting);
    drop(server);
}

/// A real exec inside the observer's pid, with the real kernel's watch.
///
/// The peer is a program in a real pane's foreground that binds and reports
/// a run; between two frames it starts a child (a fork, and an exec in that
/// child), which costs it one retry; then it EXECUTES ANOTHER IMAGE in its
/// own pid. Pane A's new program is told everything the old one held — the
/// lease and the nonce — and sends the run's ending; pane B's says nothing.
/// In both the pid, the birth instant, the terminal and the foreground group
/// are what they were: only the execution instance tells the two programs
/// apart.
#[test]
fn a_real_exec_inside_the_observer_s_pid_ends_its_lease() {
    let _guard = fresh();
    let server = Server::start("exec");
    let accepting = Accepting::start(&server, SIGNAL);
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "before");
    build_peer(&server.base.join("other"), "after");
    let working = step(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    let ping = step(serde_json::json!({"seq": 2}));
    let ending = step(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"}));
    let generation = |session: &str| {
        let pane = server.fact(session, "#{pane_pid}").parse::<u32>().unwrap();
        agent_status::live_generation(pane)
    };
    // A: the new image is told the lease and the nonce, and sends a frame
    // B: the new image says nothing
    let lasts = [
        (
            "deck-a",
            format!("@exec+:{base}/other {ending} @mark:{base}/after-deck-a @sleep"),
        ),
        (
            "deck-b",
            format!("@exec:{base}/other @mark:{base}/after-deck-b @sleep"),
        ),
    ];
    for (session, last) in &lasts {
        server.run(
            session,
            session,
            &format!(
                "{base}/observer {base}/status.sock {base}/log-{session} @hello {attest} {working} \
                 @fork {ping} @mark:{base}/before-{session} @wait:{base}/go-{session} {last}",
                attest = server.attest(session),
            ),
        );
        server.wait(session, &format!("before-{session}"));
        let said = server.log(&format!("log-{session}"));
        assert!(
            said.contains("\"accepted\":")
                && said.contains("\"ack\":1")
                && said.contains("\"again\":true")
                && said.contains("\"ack\":2"),
            "{session}: a fork and a child's exec between two frames cost one retry and end nothing: {said}"
        );
        assert_eq!(server.state(session), Some("working"));
        assert_eq!(
            server.evidence(session).pi_signal,
            Some(PiCoverage::Connected)
        );
        assert_eq!(server.fact(session, "#{pane_current_command}"), "observer");
    }
    assert_eq!(probe::live(), 2);
    assert_eq!(probe::agains(), 2, "one for each pane's fork");
    let (a_before, b_before) = (generation("deck-a"), generation("deck-b"));
    assert!(a_before.is_some() && b_before.is_some());

    // A executes another image, which sends the run's ending with all the
    // old one knew
    let before = written();
    server.go("go-deck-a");
    server.wait("deck-a", "after-deck-a");
    assert!(
        server
            .log("log-deck-a")
            .contains("\"refused\":\"peer-executed\""),
        "{}",
        server.log("log-deck-a")
    );
    assert_eq!(accepting.refused("pi-peer-executed"), 1);
    assert_eq!(probe::live(), 1);
    assert_eq!(
        generation("deck-a"),
        a_before,
        "the same pid and the same birth instant: neither tells an exec"
    );
    assert_eq!(server.fact("deck-a", "#{pane_current_command}"), "other");
    assert_eq!(server.state("deck-a"), None);
    assert_eq!(
        agent_status::next_episode_for_tests(),
        before.episode,
        "no ending was written, not for a moment"
    );
    assert_eq!(
        (
            server.evidence("deck-a").pi,
            server.evidence("deck-a").pi_signal
        ),
        (false, None),
        "nothing of the old observer describes the program that runs now"
    );
    let rows = server.rows();
    let row = rows
        .iter()
        .find(|row| row.session_name == "deck-a")
        .unwrap();
    assert!(!agent_status::pi_foreground(
        row.server_pid,
        &row.pane_id,
        row.pane_pid,
        agent_status::live_generation,
        agent_status::foreground_name,
    ));
    assert_eq!(server.state("deck-b"), Some("working"), "B is not A");

    // B executes another image and says nothing: every reader sees it at
    // once, and the worker ends the lease by itself — no frame, no poll
    server.go("go-deck-b");
    server.wait("deck-b", "after-deck-b");
    until("B's word to be withdrawn from every reader", || {
        server.state("deck-b").is_none()
    });
    assert_eq!(generation("deck-b"), b_before);
    assert_eq!(
        (
            server.evidence("deck-b").pi,
            server.evidence("deck-b").pi_signal
        ),
        (false, None)
    );
    until("B's lease to end", || probe::live() == 0);
    assert_eq!(
        probe::ended()
            .iter()
            .map(|(_, code)| *code)
            .collect::<Vec<_>>(),
        ["peer-executed", "peer-executed"]
    );

    for session in ["deck-a", "deck-b"] {
        server.tmux(&["send-keys", "-t", session, "C-c"]).unwrap();
    }
    drop(accepting);
    drop(server);
}

// ---------- follow-up 02: R01 and R02, with real processes and the real kernel ----------

/// One scenario in pane A of a server whose pane B holds an observer of its
/// own. A's foreground program binds with both capabilities, reports its
/// session and a run, and then does what the round is about.
struct Round<'a> {
    server: &'a Server,
    name: String,
}

impl Round<'_> {
    /// The path of one of this round's marks, for a step of the peer.
    fn mark(&self, tag: &str) -> String {
        format!("{}/{}-{tag}", self.server.base.to_str().unwrap(), self.name)
    }

    fn wait(&self, tag: &str) {
        self.server.wait("deck-a", &format!("{}-{tag}", self.name));
    }

    fn go(&self, tag: &str) {
        self.server.go(&format!("{}-{tag}", self.name));
    }

    fn log(&self) -> String {
        self.server.log(&format!("{}.log", self.name))
    }

    /// Start the round: the parent binds, reports, and waits for `go`.
    fn start(&self, then: &str) {
        use crate::agent_reentry::lifecycle::PiSession;
        let base = self.server.base.to_str().unwrap();
        let start = step(serde_json::json!({
            "seq": 1, "event": "start", "nativeSessionId": "session-a", "reason": "startup"
        }));
        let working = step(serde_json::json!({"seq": 2, "run": 1, "state": "working"}));
        self.server.run(
            "deck-a",
            &self.name,
            &format!(
                "{base}/observer {base}/status.sock {base}/{name}.log @hello {attest} {start} \
                 {working} @mark:{ready} @wait:{go} {then}",
                name = self.name,
                attest = self.server.attest_with("deck-a", true, true),
                ready = self.mark("ready"),
                go = self.mark("go"),
            ),
        );
        self.wait("ready");
        assert_eq!(
            self.server.state("deck-a"),
            Some("working"),
            "{}",
            self.name
        );
        assert_eq!(
            self.server.evidence("deck-a").pi_session,
            Some(PiSession::Observed),
            "{}",
            self.name
        );
        assert_eq!(probe::live(), 2, "{}", self.name);
    }

    /// The parent's own frame was written, and nothing else was: the run's
    /// ending is the very next episode after `before`, announced once.
    fn the_parent_s_own_ending(&self, before: &Written) {
        assert_eq!(
            self.server.state("deck-a"),
            Some("turn-done"),
            "{}",
            self.name
        );
        assert_eq!(
            agent_status::next_episode_for_tests(),
            before.episode + 1,
            "{}: exactly one observation was written",
            self.name
        );
        let told = |announced: &[(String, u32)]| {
            announced
                .iter()
                .find(|(session, _)| session == "deck-a")
                .map_or(0, |(_, count)| *count)
        };
        assert_eq!(
            told(&written().announced),
            told(&before.announced) + 1,
            "{}",
            self.name
        );
        assert_eq!(sessions().len(), 1, "{}", self.name);
        assert_eq!(sessions()[0].1, "session-a", "{}", self.name);
    }

    /// The parent says goodbye, and the pane's shell is in front again.
    fn end(&self) {
        self.go("end");
        until("the round's lease to end", || probe::live() == 1);
        self.server.idle("deck-a");
        assert_eq!(
            self.server.state("deck-b"),
            Some("working"),
            "{}: B is not A",
            self.name
        );
    }
}

/// R01. The pane's foreground program is a bound observer. A child of it —
/// in its process group, in a session of its own, or as another program it
/// executed — has what a child has: a copy of the parent's memory, the
/// lease and the nonce in it, and every connection that existed when it was
/// made. With that it tries to say that the parent's run ended, that the
/// parent is in another session, that the parent is gone. In every order of
/// its writes and the parent's looks at the connection the kernel can give,
/// nothing is written for the parent: not a word, not an episode, not a
/// session, not the end of its lease — not for a moment (`written`) — and
/// the parent's own next frame is written as if nothing had happened. Pane B
/// never hears of any of it.
#[test]
fn r01_no_child_of_the_observer_writes_anything_for_it() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let server = Server::start("r01");
    let accepting = Accepting::start(&server, BOTH);
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "parent");
    build_peer(&server.base.join("other"), "another program");

    // B: an observer of its own, bound and working for the whole test
    server.run(
        "deck-b",
        "b",
        &format!(
            "{base}/observer {base}/status.sock {base}/b.log @hello {attest} {working} \
             @mark:{base}/b-ready @wait:{base}/b-end {reset}",
            attest = server.attest("deck-b"),
            working = step(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
            reset = step(serde_json::json!({"seq": 2, "reason": "quit"})),
        ),
    );
    server.wait("deck-b", "b-ready");
    assert_eq!(server.state("deck-b"), Some("working"));

    // what a child would want its parent to have said, as the next frame
    let forged = [
        (
            "run",
            step_line(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
        ),
        (
            "session",
            step_line(
                serde_json::json!({"seq": 3, "event": "snapshot", "nativeSessionId": "session-x"}),
            ),
        ),
        (
            "reset",
            step_line(serde_json::json!({"seq": 3, "reason": "quit"})),
        ),
    ];
    let own_ping = step_line(serde_json::json!({"seq": 3}));
    let done = |seq: u64| step(serde_json::json!({"seq": seq, "run": 1, "state": "turn-done"}));
    let reset = |seq: u64| step(serde_json::json!({"seq": seq, "reason": "quit"}));
    let kinds = [
        ("kid", "@kid".to_string()),
        ("setsid", "@child".to_string()),
        ("exec", format!("@kidexec:{base}/other")),
    ];
    let round = |name: String| Round {
        server: &server,
        name,
    };

    // 1. The child writes on the connection it inherited, and the parent
    //    then LOOKS at that connection, as an event loop does: the kernel
    //    names the parent for it. The line carries the nonce the parent
    //    holds. Every kind of child, every kind of frame.
    for (kind, child) in &kinds {
        for (what, frame) in &forged {
            let r = round(format!("looked-{kind}-{what}"));
            // The child reads its answer only when the test says so: a read
            // is a use of the connection too, and would name the child again.
            r.start(&format!(
                "@open {child} '@line:{frame}' @mark:{c1} @wait:{read} @read @mark:{c2} @parent \
                 @wait:{c1} @touch @mark:{p1} @wait:{p2} @close {own} @mark:{p3} @wait:{end} {bye}",
                c1 = r.mark("c1"),
                read = r.mark("read"),
                c2 = r.mark("c2"),
                p1 = r.mark("p1"),
                p2 = r.mark("p2"),
                p3 = r.mark("p3"),
                end = r.mark("end"),
                own = done(3),
                bye = reset(4),
            ));
            accepting.held(true);
            r.go("go");
            r.wait("p1");
            let (before, agains, mismatched) = (
                written(),
                probe::agains(),
                accepting.refused("pi-sender-mismatch"),
            );
            accepting.held(false);
            until("the answer to the child's line", || {
                probe::agains() == agains + 1
                    || accepting.refused("pi-sender-mismatch") > mismatched
            });
            r.go("read");
            r.wait("c2");
            assert!(
                r.log().contains("\"again\":true"),
                "{}: {}",
                r.name,
                r.log()
            );
            assert!(
                !r.log().contains("sender-mismatch"),
                "{}: {}",
                r.name,
                r.log()
            );
            assert_eq!(written(), before, "{}", r.name);
            // the kernel named the parent and the nonce was the parent's:
            // what stood in the way is the fork that made the child
            assert_eq!(probe::again_reasons()[agains..], ["fork"], "{}", r.name);
            assert_eq!(server.state("deck-a"), Some("working"), "{}", r.name);
            // the parent's own frame, after all that
            r.go("p2");
            r.wait("p3");
            r.the_parent_s_own_ending(&before);
            // the child read the answer the parent's nonce was replaced in:
            // the parent was asked for its frame again, once, for that alone
            assert_eq!(
                probe::again_reasons()[agains..],
                ["fork", "nonce"],
                "{}",
                r.name
            );
            r.end();
        }
    }

    // 2. The same, and nobody looks at the connection afterwards: the
    //    kernel names the child.
    {
        let r = round("unlooked".into());
        let (_, frame) = &forged[0];
        r.start(&format!(
            "@open @child '@line:{frame}' @mark:{c1} @read @mark:{c2} @parent @wait:{c1} @close \
             @mark:{p1} @wait:{p2} {own} @mark:{p3} @wait:{end} {bye}",
            c1 = r.mark("c1"),
            c2 = r.mark("c2"),
            p1 = r.mark("p1"),
            p2 = r.mark("p2"),
            p3 = r.mark("p3"),
            end = r.mark("end"),
            own = done(3),
            bye = reset(4),
        ));
        accepting.held(true);
        r.go("go");
        r.wait("p1");
        let (before, mismatched) = (written(), accepting.refused("pi-sender-mismatch"));
        accepting.held(false);
        r.wait("c2");
        assert!(
            r.log().contains("\"refused\":\"sender-mismatch\""),
            "{}",
            r.log()
        );
        assert_eq!(accepting.refused("pi-sender-mismatch"), mismatched + 1);
        assert_eq!(written(), before);
        r.go("p2");
        r.wait("p3");
        r.the_parent_s_own_ending(&before);
        r.end();
    }

    // 3. The child on a connection of its own, with everything it knows.
    for (kind, child) in [&kinds[1], &kinds[2]] {
        for (what, frame) in &forged {
            let r = round(format!("own-{kind}-{what}"));
            r.start(&format!(
                "{child} '@once:{frame}' @mark:{c1} @parent @wait:{c1} @wait:{p2} {own} @mark:{p3} \
                 @wait:{end} {bye}",
                c1 = r.mark("c1"),
                p2 = r.mark("p2"),
                p3 = r.mark("p3"),
                end = r.mark("end"),
                own = done(3),
                bye = reset(4),
            ));
            let (before, mismatched) = (written(), accepting.refused("pi-sender-mismatch"));
            r.go("go");
            r.wait("c1");
            assert!(
                r.log().contains("\"refused\":\"sender-mismatch\""),
                "{}",
                r.log()
            );
            assert_eq!(accepting.refused("pi-sender-mismatch"), mismatched + 1);
            assert_eq!(written(), before, "{}", r.name);
            r.go("p2");
            r.wait("p3");
            r.the_parent_s_own_ending(&before);
            r.end();
        }
    }

    // 4. The child was made while a message of the parent's was on its way,
    //    and READS the answer from that connection: a nonce that was made
    //    after its own fork was counted. It has no connection the kernel
    //    names the parent for — the parent has one at a time — and on one
    //    of its own the nonce is worth nothing.
    {
        let r = round("stolen".into());
        let (_, frame) = &forged[0];
        r.start(&format!(
            "@open '@line:{own_ping}' @child @wait:{steal} @read @mark:{c1} '@once:{frame}' \
             @mark:{c2} @parent @mark:{p1} @wait:{p2} @close {own} @mark:{p3} @wait:{end} {bye}",
            steal = r.mark("steal"),
            c1 = r.mark("c1"),
            c2 = r.mark("c2"),
            p1 = r.mark("p1"),
            p2 = r.mark("p2"),
            p3 = r.mark("p3"),
            end = r.mark("end"),
            own = done(3),
            bye = reset(4),
        ));
        accepting.held(true);
        r.go("go");
        r.wait("p1");
        let (before, agains, mismatched) = (
            written(),
            probe::agains(),
            accepting.refused("pi-sender-mismatch"),
        );
        accepting.held(false);
        // the parent's own message met the fork: neither believed nor blamed
        until("the answer to the parent's message", || {
            probe::agains() == agains + 1
        });
        assert_eq!(probe::again_reasons()[agains..], ["fork"]);
        assert_eq!(written(), before, "the heartbeat was not written either");
        r.go("steal");
        r.wait("c2");
        assert!(r.log().contains("\"again\":true"), "{}", r.log());
        assert!(
            r.log().contains("\"refused\":\"sender-mismatch\""),
            "{}",
            r.log()
        );
        assert_eq!(accepting.refused("pi-sender-mismatch"), mismatched + 1);
        assert_eq!(written(), before);
        // the parent never read its answer: its next frame is asked for
        // again, once, and then written
        r.go("p2");
        r.wait("p3");
        r.the_parent_s_own_ending(&before);
        assert_eq!(probe::again_reasons()[agains..], ["fork", "nonce"]);
        r.end();
    }

    // 5. The child's line arrives in two parts, and the parent looks at the
    //    connection after each.
    {
        let r = round("split".into());
        let (_, frame) = &forged[0];
        let at = frame.find("\"protocol\"").unwrap() + 5;
        let (head, tail) = frame.split_at(at);
        r.start(&format!(
            "@open @kid '@part:{head}' @mark:{c1} @wait:{c2} '@line:{tail}' @mark:{c3} \
             @wait:{read} @read @mark:{c4} @parent @wait:{c1} @touch @mark:{p1} @wait:{c3} \
             @touch @mark:{p2} @wait:{p3} @close {own} @mark:{p4} @wait:{end} {bye}",
            c1 = r.mark("c1"),
            c2 = r.mark("c2"),
            c3 = r.mark("c3"),
            read = r.mark("read"),
            c4 = r.mark("c4"),
            p1 = r.mark("p1"),
            p2 = r.mark("p2"),
            p3 = r.mark("p3"),
            p4 = r.mark("p4"),
            end = r.mark("end"),
            own = done(3),
            bye = reset(4),
        ));
        accepting.held(true);
        r.go("go");
        r.wait("p1");
        r.go("c2");
        r.wait("p2");
        let (before, agains, mismatched) = (
            written(),
            probe::agains(),
            accepting.refused("pi-sender-mismatch"),
        );
        accepting.held(false);
        until("the answer to the child's line", || {
            probe::agains() == agains + 1 || accepting.refused("pi-sender-mismatch") > mismatched
        });
        r.go("read");
        r.wait("c4");
        assert!(r.log().contains("\"again\":true"), "{}", r.log());
        assert_eq!(written(), before);
        assert_eq!(probe::again_reasons()[agains..], ["fork"]);
        r.go("p3");
        r.wait("p4");
        r.the_parent_s_own_ending(&before);
        r.end();
    }

    // 6. The child's line, and behind it on the same connection the
    //    parent's own: two lines are nobody's message.
    {
        let r = round("merged".into());
        let (_, frame) = &forged[0];
        r.start(&format!(
            "@open @kid '@line:{frame}' @mark:{c1} @parent @wait:{c1} '@line:{own_ping}' \
             @mark:{p1} @read @mark:{p2} @close @wait:{p3} {own} @mark:{p4} @wait:{end} {bye}",
            c1 = r.mark("c1"),
            p1 = r.mark("p1"),
            p2 = r.mark("p2"),
            p3 = r.mark("p3"),
            p4 = r.mark("p4"),
            end = r.mark("end"),
            own = done(3),
            bye = reset(4),
        ));
        accepting.held(true);
        r.go("go");
        r.wait("p1");
        let (before, trailing) = (written(), accepting.refused("trailing-bytes"));
        accepting.held(false);
        r.wait("p2");
        assert_eq!(accepting.refused("trailing-bytes"), trailing + 1);
        assert!(r.log().contains("(no answer)"), "{}", r.log());
        assert_eq!(written(), before);
        r.go("p3");
        r.wait("p4");
        r.the_parent_s_own_ending(&before);
        r.end();
    }

    // 7. The child writes on the connection it inherited; the parent lets
    //    go of that connection and sends its ordinary heartbeat on a new
    //    one before Deck has read either.
    {
        let r = round("heartbeat".into());
        let (_, frame) = &forged[0];
        r.start(&format!(
            "@open @child '@line:{frame}' @mark:{c1} @read @mark:{c2} @parent @wait:{c1} @close \
             @sent:{sent} '{own_ping}' @mark:{p1} \
             @wait:{p2} {own} @mark:{p3} @wait:{end} {bye}",
            c1 = r.mark("c1"),
            c2 = r.mark("c2"),
            sent = r.mark("sent"),
            p1 = r.mark("p1"),
            p2 = r.mark("p2"),
            p3 = r.mark("p3"),
            end = r.mark("end"),
            own = done(4),
            bye = reset(5),
        ));
        accepting.held(true);
        r.go("go");
        r.wait("c1");
        // The parent has connected AND written its heartbeat; the marker
        // precedes its blocking read. Elapsed time proves neither fact.
        r.wait("sent");
        let (before, mismatched) = (written(), accepting.refused("pi-sender-mismatch"));
        accepting.held(false);
        r.wait("c2");
        r.wait("p1");
        assert!(
            r.log().contains("\"refused\":\"sender-mismatch\""),
            "{}",
            r.log()
        );
        assert!(r.log().contains("\"ack\":3"), "{}", r.log());
        assert_eq!(accepting.refused("pi-sender-mismatch"), mismatched + 1);
        // the heartbeat used its number up; nothing else changed
        let after = written();
        assert_eq!(
            (
                after.store,
                after.sessions,
                after.episode,
                &after.announced,
                &after.ended
            ),
            (
                before.store,
                before.sessions,
                before.episode,
                &before.announced,
                &before.ended
            )
        );
        let seq = |written: &Written| -> Vec<u64> {
            written.leases.iter().map(|lease| lease.3).collect()
        };
        assert_eq!(
            seq(&after).iter().sum::<u64>(),
            seq(&before).iter().sum::<u64>() + 1
        );
        r.go("p2");
        r.wait("p3");
        r.the_parent_s_own_ending(&before);
        r.end();
    }

    // B went on through all of it, under the lease it had
    server.go("b-end");
    until("B's lease to end", || probe::live() == 0);
    // B writes down the answer after Deck has given it
    until("B's goodbye to be answered", || {
        server.log("b.log").contains("\"ack\":2")
    });
    assert_eq!(
        probe::ended()
            .iter()
            .filter(|(session, _)| session == "deck-b")
            .collect::<Vec<_>>(),
        [&("deck-b".to_string(), "reset-quit")]
    );
    drop(accepting);
    drop(server);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// Pane B's observer, bound and working for as long as a test of pane A runs.
fn pane_b_works(server: &Server) {
    let base = server.base.to_str().unwrap();
    server.run(
        "deck-b",
        "b",
        &format!(
            "{base}/observer {base}/status.sock {base}/b.log @hello {attest} {working} \
             @mark:{base}/b-ready @wait:{base}/b-end {reset}",
            attest = server.attest("deck-b"),
            working = step(serde_json::json!({"seq": 1, "run": 1, "state": "working"})),
            reset = step(serde_json::json!({"seq": 2, "reason": "quit"})),
        ),
    );
    server.wait("deck-b", "b-ready");
    assert_eq!(server.state("deck-b"), Some("working"));
}

/// B went on through all of it, under the lease it had, and says goodbye.
fn pane_b_ends(server: &Server) {
    server.go("b-end");
    until("B's lease to end", || probe::live() == 0);
    // B writes down the answer after Deck has given it
    until("B's goodbye to be answered", || {
        server.log("b.log").contains("\"ack\":2")
    });
    assert_eq!(
        probe::ended()
            .iter()
            .filter(|(session, _)| session == "deck-b")
            .collect::<Vec<_>>(),
        [&("deck-b".to_string(), "reset-quit")]
    );
}

/// R01, the connection an observer left behind. An observer that stops
/// waiting for an answer lets go of that connection and makes another. A
/// child made while the first existed still holds it and reads there what
/// Deck answers late — a nonce that was made after that child, and every
/// other child up to then, had been counted. A second child, made while the
/// second connection existed, holds that one, on which nothing has been
/// said yet, and is told by the first what it read. It writes that the
/// parent's run ended; the parent looks at its connection, as an event loop
/// does; Deck reads. The kernel names the parent, and the child wrote what
/// it had been told. Nothing is written for the parent, not for a moment,
/// and the parent's own next frame is written as if nothing had happened.
///
/// Deck reads the first connection alone here, and the second only after
/// everything above has happened: the order is the test's, not the
/// scheduler's.
#[test]
fn r01_what_one_child_read_on_a_connection_its_parent_left_writes_nothing_on_another() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let server = Server::start("r01l");
    let accepting = Accepting::start(&server, BOTH);
    build_peer(&server.base.join("observer"), "parent");
    pane_b_works(&server);
    let r = Round {
        server: &server,
        name: "left".into(),
    };
    r.start(&format!(
        "@open @child @wait:{x1} @read @save:{told} @mark:{x2} @parent '@line:{heartbeat}' @close \
         @open @child @wait:{y1} @load:{told} '@line:{ending}' @mark:{y2} @parent @mark:{p1} \
         @wait:{p2} @touch @mark:{p3} @wait:{p4} @close {own} @mark:{p5} @wait:{end} {bye}",
        x1 = r.mark("x1"),
        x2 = r.mark("x2"),
        y1 = r.mark("y1"),
        y2 = r.mark("y2"),
        told = r.mark("told"),
        p1 = r.mark("p1"),
        p2 = r.mark("p2"),
        p3 = r.mark("p3"),
        p4 = r.mark("p4"),
        p5 = r.mark("p5"),
        end = r.mark("end"),
        heartbeat = step_line(serde_json::json!({"seq": 3})),
        ending = step_line(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
        own = step(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
        bye = step(serde_json::json!({"seq": 4, "reason": "quit"})),
    ));
    accepting.held(true);
    r.go("go");
    // Both connections exist and both children were made. The parent wrote
    // its heartbeat on the first and let go of it; the second is silent.
    r.wait("p1");
    let (before, agains, busy) = (written(), probe::agains(), accepting.refused("pi-busy"));
    // Deck reads the heartbeat, and nothing else yet
    accepting.one();
    until("the answer to the parent's heartbeat", || {
        probe::agains() > agains || accepting.refused("pi-busy") > busy
    });
    // the first child reads that answer; the second writes with what it says
    r.go("x1");
    r.wait("x2");
    r.go("y1");
    r.wait("y2");
    // the parent looks at its connection: the kernel names the parent for it
    r.go("p2");
    r.wait("p3");
    let (asked, mismatched) = (probe::agains(), accepting.refused("pi-sender-mismatch"));
    accepting.held(false);
    // One `sender-mismatch` is not the child's. While Deck read nothing but
    // the heartbeat, the worker's own connection for a mark waited at the
    // closed door and was given up; it is accepted now, and its peer is
    // gone. That refusal comes BEFORE the answer to the child's line, which
    // needs a mark of its own from a connection behind it in the same queue:
    // it is no sign of that answer. A second one would be.
    until("the answer to the child's line", || {
        probe::agains() > asked
            || accepting.refused("pi-sender-mismatch") > mismatched + 1
            || written() != before
    });
    assert_eq!(
        written(),
        before,
        "what a child wrote was taken for its parent's"
    );
    assert_eq!(server.state("deck-a"), Some("working"));
    // Why: the heartbeat met the forks that made the two children, and a
    // look that moves is marked by a connection of the worker's own — which
    // could not be accepted ahead of the one that was already waiting. So
    // no nonce was made at all: there was nothing for the first child to
    // read but a refusal, and the second wrote with the nonce it was born
    // with, which met those same forks.
    assert_eq!(accepting.refused("pi-busy"), busy + 1);
    assert!(r.log().contains("\"refused\":\"busy\""), "{}", r.log());
    assert_eq!(probe::again_reasons()[agains..], ["fork"]);
    // the parent's own frame, after all that: its nonce was used up by the
    // answer to the child's line, so it is asked for once more
    r.go("p4");
    r.wait("p5");
    r.the_parent_s_own_ending(&before);
    assert_eq!(probe::again_reasons()[agains..], ["fork", "nonce"]);
    r.end();
    pane_b_ends(&server);
    drop(accepting);
    drop(server);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// R01, a connection that existed before Deck last looked. A child holds a
/// connection its parent made, on which nothing has been said; the parent
/// goes on with its heartbeat — which meets the fork that made the child,
/// is asked for again and is written — and then the child is TOLD the nonce
/// the parent holds now: more than any child can find out, since that nonce
/// came on a connection made after the child. It writes the ending of the
/// parent's run with it, all but the end of the line; the parent looks at
/// the connection and ends the line itself. The kernel names the parent, the
/// nonce is the lease's own, and no fork was counted since it was made.
/// Nothing is written: that connection was made before the look after which
/// the nonce was made, and a child from before that look may hold it.
#[test]
fn r01_a_connection_made_before_deck_last_looked_is_nobody_s_whatever_it_carries() {
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let server = Server::start("r01o");
    let accepting = Accepting::start(&server, BOTH);
    build_peer(&server.base.join("observer"), "parent");
    pane_b_works(&server);
    let r = Round {
        server: &server,
        name: "before".into(),
    };
    r.start(&format!(
        "@open @child @wait:{y1} @load:{told} '@part:{ending}' @mark:{y2} @parent {heartbeat} \
         @save:{told} @mark:{p1} @wait:{p2} @touch '@line:' @mark:{p3} @wait:{p4} @close {own} \
         @mark:{p5} @wait:{end} '@once:{decks}' {bye}",
        y1 = r.mark("y1"),
        y2 = r.mark("y2"),
        told = r.mark("told"),
        p1 = r.mark("p1"),
        p2 = r.mark("p2"),
        p3 = r.mark("p3"),
        p4 = r.mark("p4"),
        p5 = r.mark("p5"),
        end = r.mark("end"),
        heartbeat = step(serde_json::json!({"seq": 3})),
        ending = step_line(serde_json::json!({"seq": 4, "run": 1, "state": "turn-done"})),
        own = step(serde_json::json!({"seq": 4, "run": 1, "state": "turn-done"})),
        decks = crate::agent_pi::place_line(),
        bye = step(serde_json::json!({"seq": 5, "reason": "quit"})),
    ));
    let asked = probe::agains();
    r.go("go");
    r.wait("p1");
    // the heartbeat met the fork that made the child, and was then written
    assert_eq!(probe::again_reasons()[asked..], ["fork"]);
    assert!(r.log().contains("\"ack\":3"), "{}", r.log());
    let (before, agains, mismatched) = (
        written(),
        probe::agains(),
        accepting.refused("pi-sender-mismatch"),
    );
    // the child writes with the nonce it was told; the parent ends the line
    r.go("y1");
    r.wait("y2");
    r.go("p2");
    r.wait("p3");
    until("the answer to the line the child wrote", || {
        probe::agains() > agains
            || accepting.refused("pi-sender-mismatch") > mismatched
            || written() != before
    });
    assert_eq!(
        written(),
        before,
        "what a child wrote was taken for its parent's"
    );
    assert_eq!(server.state("deck-a"), Some("working"));
    // the process, the nonce and the forks were right: the connection was not
    assert_eq!(probe::again_reasons()[agains..], ["place"]);
    // the parent's own frame, after all that: its nonce was used up by the
    // answer to that line, so it is asked for once more
    r.go("p4");
    r.wait("p5");
    r.the_parent_s_own_ending(&before);
    assert_eq!(probe::again_reasons()[agains..], ["place", "nonce"]);
    // Deck's own line is Deck's alone: in anybody else's mouth it asks for
    // nothing and is told nothing
    let mismatched = accepting.refused("pi-sender-mismatch");
    r.end();
    assert_eq!(accepting.refused("pi-sender-mismatch"), mismatched + 1);
    assert!(
        r.log().contains("\"refused\":\"sender-mismatch\""),
        "{}",
        r.log()
    );
    assert!(!r.log().contains("\"place\""), "{}", r.log());
    pane_b_ends(&server);
    drop(accepting);
    drop(server);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// R02. What an earlier image wrote before Deck looked at its process, and
/// what it was given, is not the new image's. The orders are decided here:
/// Deck reads nothing until the test lets it.
#[test]
fn r02_what_an_earlier_image_wrote_or_was_given_is_not_the_new_image_s() {
    use crate::agent_reentry::lifecycle::PiSession;
    let _operation = crate::agent_reentry::TEST_LOCK.lock_or_recover();
    let _guard = fresh();
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
    let server = Server::start("r02");
    let accepting = Accepting::start(&server, BOTH);
    let base = server.base.to_str().unwrap().to_string();
    build_peer(&server.base.join("observer"), "before");
    build_peer(&server.base.join("other"), "after");
    let hello = message(serde_json::json!({}));
    let attest = server.attest("deck-a");
    let working = step(serde_json::json!({"seq": 1, "run": 1, "state": "working"}));
    let stop = |name: &str| {
        server.tmux(&["send-keys", "-t", "deck-a", "C-c"]).unwrap();
        server.idle("deck-a");
        assert_eq!(probe::live(), 0, "{name}");
    };
    let unrecognized = |name: &str| {
        assert_eq!(
            (
                server.evidence("deck-a").pi,
                server.evidence("deck-a").pi_signal
            ),
            (false, None),
            "{name}: the new image is no recognized Pi and has no coverage"
        );
        assert_eq!(server.state("deck-a"), None, "{name}");
    };

    // 1. A hello waits unread; its image executes another, which says
    //    nothing. Deck then reads the hello: it watches the program that
    //    runs NOW and answers a nonce nobody attests with.
    accepting.held(true);
    server.run(
        "deck-a",
        "r1",
        &format!(
            "{base}/observer {base}/status.sock {base}/r1.log @open '@line:{hello}' \
             @exec:{base}/other @mark:{base}/r1-after @sleep"
        ),
    );
    server.wait("deck-a", "r1-after");
    assert_eq!(server.fact("deck-a", "#{pane_current_command}"), "other");
    let (before, waiting) = (written(), probe::challenges());
    accepting.held(false);
    until("the hello to be read", || {
        probe::challenges() == waiting + 1
    });
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(written(), before, "a hello writes nothing");
    unrecognized("a hello that waited");
    stop("a hello that waited");

    // 2. A hello and a frame behind it on one connection, then the exec:
    //    two lines are nobody's message, whoever runs there now.
    accepting.held(true);
    server.run(
        "deck-a",
        "r2",
        &format!(
            "{base}/observer {base}/status.sock {base}/r2.log @open '@line:{hello}' \
             '@line:{forged}' @exec:{base}/other @mark:{base}/r2-after @sleep",
            forged = frame_line(
                1,
                &"0".repeat(32),
                serde_json::json!({"seq": 1, "run": 1, "state": "working"})
            ),
        ),
    );
    server.wait("deck-a", "r2-after");
    let (before, waiting, trailing) = (
        written(),
        probe::challenges(),
        accepting.refused("trailing-bytes"),
    );
    accepting.held(false);
    until("the two lines to be refused", || {
        accepting.refused("trailing-bytes") == trailing + 1
    });
    assert_eq!(written(), before);
    assert!(
        probe::challenges() <= waiting,
        "and nothing was asked of that process"
    );
    unrecognized("a hello with a frame behind it");
    stop("a hello with a frame behind it");

    // 3. A BOUND observer reports its session and a run. The run's ending
    //    and another session's name wait unread, each on a connection of
    //    its own, while its image executes another. Deck reads them
    //    afterwards: the right process, the right lease, the right nonce,
    //    no fork — and not that program.
    server.run(
        "deck-a",
        "r3",
        &format!(
            "{base}/observer {base}/status.sock {base}/r3.log @hello {both} {start} {working} \
             @mark:{base}/r3-ready @wait:{base}/r3-go @open '@line:{ending}' @open \
             '@line:{other}' @exec:{base}/other @mark:{base}/r3-after @sleep",
            both = server.attest_with("deck-a", true, true),
            start = step(serde_json::json!({
                "seq": 1, "event": "start", "nativeSessionId": "session-a", "reason": "startup"
            })),
            working = step(serde_json::json!({"seq": 2, "run": 1, "state": "working"})),
            ending = step_line(serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})),
            other = step_line(
                serde_json::json!({"seq": 3, "event": "snapshot", "nativeSessionId": "session-x"})
            ),
        ),
    );
    server.wait("deck-a", "r3-ready");
    assert_eq!(server.state("deck-a"), Some("working"));
    assert_eq!(
        server.evidence("deck-a").pi_session,
        Some(PiSession::Observed)
    );
    assert_eq!(sessions().len(), 1);
    accepting.held(true);
    server.go("r3-go");
    server.wait("deck-a", "r3-after");
    assert_eq!(
        (server.state("deck-a"), server.evidence("deck-a").pi_session),
        (None, None),
        "withdrawn from every reader before a frame is read"
    );
    let (before, executed) = (written(), accepting.refused("pi-peer-executed"));
    accepting.held(false);
    until("both frames to be refused", || {
        accepting.refused("pi-peer-executed") == executed + 2
    });
    assert_eq!(
        probe::ended().last(),
        Some(&("deck-a".to_string(), "peer-executed"))
    );
    assert_eq!(
        agent_status::next_episode_for_tests(),
        before.episode,
        "no ending was written, not for a moment"
    );
    assert_eq!(written().announced, before.announced);
    assert!(
        sessions().is_empty(),
        "neither the old session nor the one the frame named"
    );
    assert_eq!(server.evidence("deck-a").pi_session, None);
    unrecognized("frames that waited");
    stop("frames that waited");

    // 4. An image is given a nonce and THEN executes another, which is
    //    handed that nonce and attests with it: a nonce is not who a
    //    process is.
    let executed = accepting.refused("pi-peer-executed");
    let writes = agent_status::pi_writes_for_tests();
    server.run(
        "deck-a",
        "r4",
        &format!(
            "{base}/observer {base}/status.sock {base}/r4.log @hello @exec+:{base}/other \
             {attest} @mark:{base}/r4-after @sleep"
        ),
    );
    server.wait("deck-a", "r4-after");
    assert!(
        server
            .log("r4.log")
            .contains("\"refused\":\"peer-executed\""),
        "{}",
        server.log("r4.log")
    );
    assert_eq!(accepting.refused("pi-peer-executed"), executed + 1);
    assert_eq!(agent_status::pi_writes_for_tests(), writes);
    unrecognized("a nonce that was handed on");
    stop("a nonce that was handed on");

    // 5. Half a hello, the exec, and the new image writes the other half:
    //    a hello claims nothing, so nothing of the old image is in what
    //    follows. The new image reads the nonce, attests ITSELF and is bound
    //    as itself — a program in the pane's foreground that says so.
    let (head, tail) = hello.split_at(20);
    server.run(
        "deck-a",
        "r5",
        &format!(
            "{base}/observer {base}/status.sock {base}/r5.log @open '@part:{head}' \
             @exec:{base}/other '@line:{tail}' @read {attest} {working} @mark:{base}/r5-bound \
             @wait:{base}/r5-end {reset}",
            reset = step(serde_json::json!({"seq": 2, "reason": "quit"})),
        ),
    );
    server.wait("deck-a", "r5-bound");
    assert_eq!(server.fact("deck-a", "#{pane_current_command}"), "other");
    assert!(server.log("r5.log").contains("\"accepted\":"));
    assert_eq!(server.state("deck-a"), Some("working"));
    assert_eq!(
        server.evidence("deck-a").pi_signal,
        Some(PiCoverage::Connected)
    );
    server.go("r5-end");
    until("its lease to end", || probe::live() == 0);
    server.idle("deck-a");

    drop(accepting);
    drop(server);
    crate::agent_reentry::lifecycle::reset_pi_for_tests();
}

/// The observer that ships in the bundle, as text: two of its times are
/// written down on this side as well.
const OBSERVER_SOURCE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/resources/pi/observer.ts"
));

/// The whole number `observer.ts` gives `name`: `const <name> = <n>;`, on a
/// line of its own and once.
fn observer_constant(name: &str) -> u128 {
    let declaration = format!("const {name} = ");
    let mut found = OBSERVER_SOURCE
        .lines()
        .filter_map(|line| line.strip_prefix(declaration.as_str()));
    let value = found
        .next()
        .unwrap_or_else(|| panic!("observer.ts declares no {name}"));
    assert!(found.next().is_none(), "observer.ts declares {name} twice");
    value
        .strip_suffix(';')
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{name} is no whole number: {value}"))
}

/// Two times are agreed between the observer and the transport and written
/// on both sides: how long the observer waits for an answer, and how often
/// it says that it is still there. A change on one side alone fails here.
#[test]
fn observer_constants_match_the_transport() {
    let ack_timeout = observer_constant("ACK_TIMEOUT_MS");
    let ping_every = observer_constant("PING_EVERY_MS");
    // an answer has exactly as long as the observer waits for it
    assert_eq!(ack_timeout, ANSWER_BUDGET.as_millis());
    // a lease outlives one lost heartbeat, and the next one after it
    assert!(ping_every * 2 <= STALE_AFTER.as_millis());
    assert!(ping_every < STALE_AFTER.as_millis());
}
