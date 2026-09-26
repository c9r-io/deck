//! Signal Trace harness, Rust half (FR-SI-05). Test-only.
//!
//! ONE trace file (`ui/test/fixtures/signal-traces.json`) describes Agent
//! lifecycles as steps — hook events (v1 or v2), polls, view/dismiss calls,
//! active-pane switches, generation replacement, exits, stops, hooks sent
//! through a Codex shared daemon (`"from": "daemon:<pane>"`) — plus
//! `expect` steps. This runner replays each trace against the REAL backend
//! derivations over a deterministic synthetic world (tmux server 42, one
//! process table): `agent_status::ingest` admission and the interaction
//! tracker, `commands::poll_from_listing` (and through it `reconcile`,
//! projection and `finish_fg`), `scheduler::observe` + `agent_holds`, and
//! the real `notify_dismiss` command with the notification state.
//!
//! Every successful poll is normalized into a Signal-only record — session
//! label, alive, agent word, episode label (`e1`, `e2`, … by first
//! appearance, never the process-global token), `episode_viewed` and the
//! retirement evidence as `shell`/`agent`/null — and compared with the
//! checked-in golden `ui/test/fixtures/signal-trace-projection.json`. The
//! JS half (`ui/test/signal-trace.test.mjs`) feeds exactly that golden
//! stream into the real attention/notify-model/finish derivations and
//! checks the same `expect` steps, so the two layers cannot drift apart
//! while both suites pass. A stale golden fails; the run never writes it
//! unless a maintainer asks: `DECK_SIGNAL_GOLDEN=update cargo test --bin
//! deck-app signal_trace`.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::agent_status::{Origin, ProcessTable, STORE_TEST_LOCK};
use crate::procinfo::ProcessInfo;
use crate::sync::LockRecover;
use crate::tmux::PaneRow;

const TRACES: &str = include_str!("../../ui/test/fixtures/signal-traces.json");
const GOLDEN: &str = include_str!("../../ui/test/fixtures/signal-trace-projection.json");
const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../ui/test/fixtures/signal-trace-projection.json"
);
const SERVER: u32 = 42;

fn process(pid: u32, ppid: u32, tty: u32, fg: u32, start: u64) -> ProcessInfo {
    ProcessInfo {
        pid,
        ppid,
        pgid: pid,
        tty,
        tty_pgid: fg,
        start_seconds: start,
        start_micros: 0,
    }
}

struct Pane {
    label: String,
    session: String,
    pane_id: String,
    pane_pid: u32,
    tty: u32,
    /// the tty foreground leader: the agent, or the shell itself
    leader: u32,
    start: u64,
    /// a replaced generation's leader, still alive outside the foreground
    old: Option<(u32, u64)>,
    active: bool,
    stopped: bool,
    /// the foreground program's name when it is not the shell
    agent: String,
}

struct World {
    sessions: Vec<String>,
    panes: Vec<Pane>,
    next_helper: u32,
    next_start: u64,
    ids: HashMap<String, String>,
}

impl World {
    fn new(trace: &Value) -> Self {
        let sessions: Vec<String> = trace["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        let mut panes = Vec::new();
        let spec = trace["panes"].as_object().expect("panes");
        for (i, (label, pane)) in spec.iter().enumerate() {
            let pane_pid = 1000 + 10 * i as u32;
            let shell = pane["shell"].as_bool().unwrap_or(false);
            panes.push(Pane {
                label: label.clone(),
                session: pane["session"].as_str().expect("pane session").to_string(),
                pane_id: format!("%{}", i + 1),
                pane_pid,
                tty: 100 + i as u32,
                leader: if shell { pane_pid } else { pane_pid + 1 },
                start: 2000 + i as u64,
                old: None,
                active: false,
                stopped: false,
                agent: pane["agent"].as_str().unwrap_or("claude").to_string(),
            });
        }
        let mut world = Self {
            sessions,
            panes,
            next_helper: 50_000,
            next_start: 3000,
            ids: HashMap::new(),
        };
        // each session's first pane (label order) is its active target
        for session in world.sessions.clone() {
            if let Some(first) = world.panes.iter_mut().find(|p| p.session == session) {
                first.active = true;
            }
        }
        world
    }

    fn name(session: &str) -> String {
        format!("deck-trace-{session}")
    }

    fn pane(&mut self, label: &str) -> &mut Pane {
        self.panes
            .iter_mut()
            .find(|p| p.label == label)
            .unwrap_or_else(|| panic!("pane {label}"))
    }

    fn table(&self) -> ProcessTable {
        let mut t = ProcessTable::new();
        t.insert(SERVER, process(SERVER, 1, 0, 0, 1));
        for p in self.panes.iter().filter(|p| !p.stopped) {
            t.insert(
                p.pane_pid,
                process(p.pane_pid, SERVER, p.tty, p.leader, 1000),
            );
            if p.leader != p.pane_pid {
                t.insert(
                    p.leader,
                    process(p.leader, p.pane_pid, p.tty, p.leader, p.start),
                );
            }
            if let Some((old, start)) = p.old {
                t.insert(old, process(old, p.pane_pid, p.tty, p.leader, start));
            }
        }
        t
    }

    fn rows(&self) -> Vec<PaneRow> {
        self.panes
            .iter()
            .filter(|p| !p.stopped)
            .map(|p| {
                let index = self.sessions.iter().position(|s| *s == p.session).unwrap();
                PaneRow {
                    server_pid: SERVER,
                    session_id: format!("${}", index + 1),
                    session_name: Self::name(&p.session),
                    window_id: "@1".into(),
                    pane_id: p.pane_id.clone(),
                    pane_pid: p.pane_pid,
                    window_active: true,
                    pane_active: p.active,
                    command: if p.leader == p.pane_pid {
                        "zsh".into()
                    } else {
                        p.agent.clone()
                    },
                    ..PaneRow::default()
                }
            })
            .collect()
    }

    /// A deterministic lowercase UUID per interaction label.
    fn interaction(&mut self, label: &str) -> String {
        let next = self.ids.len() + 1;
        self.ids
            .entry(label.to_string())
            .or_insert_with(|| format!("0199aaaa-0000-7000-8000-{next:012x}"))
            .clone()
    }
}

/// Episode normalization: process-global tokens → `e1`, `e2`, … per trace.
#[derive(Default)]
struct Episodes {
    labels: HashMap<u64, String>,
}

impl Episodes {
    fn label(&mut self, token: u64) -> String {
        let next = self.labels.len() + 1;
        self.labels
            .entry(token)
            .or_insert_with(|| format!("e{next}"))
            .clone()
    }

    fn token(&self, label: &str) -> u64 {
        self.labels
            .iter()
            .find(|(_, l)| *l == label)
            .map(|(t, _)| *t)
            .unwrap_or_else(|| panic!("episode {label} was never polled"))
    }
}

/// Replay one trace; returns its normalized golden records.
fn run(trace: &Value) -> Value {
    let name = trace["name"].as_str().unwrap();
    crate::agent_status::reset_for_tests();
    crate::notify::reset_for_tests();
    let mut world = World::new(trace);
    let mut episodes = Episodes::default();
    let mut records = Vec::new();
    let mut last: HashMap<String, Value> = HashMap::new();
    for (i, step) in trace["steps"].as_array().unwrap().iter().enumerate() {
        let at = format!("{name} step {i}: {step}");
        if let Some(state) = step["event"].as_str() {
            let label = step["pane"].as_str().unwrap();
            let id = step["id"].as_str().map(|l| world.interaction(l));
            let hook = world.next_helper;
            world.next_helper += 2;
            let mut table = world.table();
            let from = step["from"].as_str();
            let parent = match from.and_then(|f| f.strip_prefix("daemon:")) {
                // a Codex 0.157 shared app-server started by that pane's
                // agent: no terminal, its own group; it spawns the hooks
                // of EVERY client with the starter's `$TMUX_PANE`
                Some(starter) => {
                    let starter = world.pane(starter).leader;
                    let daemon = 40_000 + starter;
                    table.insert(daemon, process(daemon, starter, 0, 0, 4000));
                    daemon
                }
                None if from == Some("old") => world.pane(label).old.expect("an old generation").0,
                None => world.pane(label).leader,
            };
            let pane_id = world.pane(label).pane_id.clone();
            // the probed hook shape: a fresh terminal-less group (`sh`)
            // holding the helper
            table.insert(hook, process(hook, parent, 0, 0, 5000));
            let helper = hook + 1;
            table.insert(
                helper,
                ProcessInfo {
                    pgid: hook,
                    ..process(helper, hook, 0, 0, 5000)
                },
            );
            let line = match &id {
                Some(id) => format!(
                    "{{\"v\":2,\"source\":\"codex\",\"state\":\"{state}\",\"socket\":\"{}\",\"server_pid\":{SERVER},\"pane\":\"{pane_id}\",\"interaction\":\"{id}\"}}",
                    crate::tmux::socket()
                ),
                None => format!(
                    "{{\"v\":1,\"source\":\"claude-code\",\"state\":\"{state}\",\"socket\":\"{}\",\"server_pid\":{SERVER},\"pane\":\"{pane_id}\"}}",
                    crate::tmux::socket()
                ),
            };
            let rows = world.rows();
            let got = crate::agent_status::ingest(
                &line,
                &Origin {
                    peer: Some(helper),
                    table,
                },
                || Some(rows),
            );
            let want = step["result"].as_str();
            assert_eq!(got.err(), want, "{at}");
        } else if let Some(label) = step["focus"].as_str() {
            let session = world.pane(label).session.clone();
            for p in world.panes.iter_mut().filter(|p| p.session == session) {
                p.active = p.label == label;
            }
        } else if let Some(session) = step["unfocus"].as_str() {
            for p in world.panes.iter_mut().filter(|p| p.session == session) {
                p.active = false;
            }
        } else if let Some(label) = step["replace"].as_str() {
            let start = world.next_start;
            world.next_start += 1;
            let pane = world.pane(label);
            pane.old = Some((pane.leader, pane.start));
            pane.leader = pane.pane_pid + 2 + (start as u32 % 7);
            pane.start = start;
        } else if let Some(label) = step["exit"].as_str() {
            let pane = world.pane(label);
            pane.leader = pane.pane_pid;
            pane.old = None;
        } else if let Some(session) = step["stop"].as_str() {
            for p in world.panes.iter_mut().filter(|p| p.session == session) {
                p.stopped = true;
            }
        } else if step.get("poll").is_some() {
            if step["poll"] == "fail" {
                records.push(json!({"step": i, "poll": "fail"}));
                continue;
            }
            let table = world.table();
            let names: Vec<String> = world.sessions.iter().map(|s| World::name(s)).collect();
            let infos = crate::commands::poll_from_listing(
                names,
                vec![],
                false,
                Ok(world.rows()),
                move || table,
            )
            .unwrap();
            let mut sessions = serde_json::Map::new();
            for (session, info) in world.sessions.iter().zip(infos) {
                let info = serde_json::to_value(info).unwrap();
                let finish = info["finish_fg"].as_str().map(|fg| {
                    if crate::context::shell_process(Some(fg)) {
                        "shell"
                    } else {
                        "agent"
                    }
                });
                let record = json!({
                    "alive": info["alive"],
                    "agent": info["agent"],
                    "episode": info["episode"].as_u64().map(|t| episodes.label(t)),
                    "episode_viewed": info["episode_viewed"],
                    "finish": finish,
                });
                last.insert(session.clone(), record.clone());
                sessions.insert(session.clone(), record);
            }
            let mut record = json!({"step": i, "poll": "ok", "sessions": sessions});
            if let Some(omit) = step["poll"].get("omit") {
                record["omit"] = omit.clone();
            }
            records.push(record);
        } else if let Some(calls) = step["dismiss"].as_array() {
            // a view or a sync: the exact dismissals the webview sends
            if step["transport"] == "fail" {
                continue;
            }
            for call in calls {
                let session = call[0].as_str().unwrap();
                // an explicit episode label, or the one this session showed
                // at its last poll (what the webview rendered)
                let episode = call[1]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| last.get(session)?["episode"].as_str().map(str::to_string))
                    .unwrap_or_else(|| panic!("{at}: no episode to dismiss"));
                crate::notify::notify_dismiss(World::name(session), episodes.token(&episode))
                    .unwrap();
            }
        } else if let Some(expect) = step.get("expect") {
            check(&at, expect, &world, &last);
        } else if step.get("recreate").is_none() && step.get("view").is_none() {
            panic!("{at}: unknown step");
        }
    }
    json!({"name": name, "records": records})
}

/// The backend half of one `expect` step.
fn check(at: &str, expect: &Value, world: &World, last: &HashMap<String, Value>) {
    let (states, unread, dock) = crate::notify::snapshot_for_tests();
    let announced = crate::notify::announced_for_tests();
    let table = world.table();
    let observed = crate::scheduler::observe_with(world.rows(), |pane| {
        crate::agent_status::foreground_generation(&table, pane)
    });
    for (session, want) in expect["sessions"].as_object().into_iter().flatten() {
        let name = World::name(session);
        let polled = last.get(session).cloned().unwrap_or(Value::Null);
        if let Some(agent) = want.get("agent") {
            assert_eq!(&polled["agent"], agent, "{at}: {session} agent");
        }
        if let Some(flag) = want["unread"].as_bool() {
            assert_eq!(
                unread.contains(&name),
                flag,
                "{at}: {session} unread (notify)"
            );
        }
        if let Some(flag) = want["hold"].as_bool() {
            let owner: crate::scheduler::QueueItem = serde_json::from_value(json!({
                "id": "o", "session": name, "card_id": "c", "dir": "", "cmd": "",
                "text": "x", "mode": "chain", "added": 0
            }))
            .unwrap();
            assert_eq!(
                crate::scheduler::agent_holds(&owner, observed.get(&name)),
                flag,
                "{at}: {session} scheduler hold"
            );
        }
        if let Some(flag) = want["finish"].as_bool() {
            let eligible = polled["agent"].is_null() && polled["finish"] == "shell";
            assert_eq!(eligible, flag, "{at}: {session} retirement evidence");
        }
        if let Some(count) = want["announced"].as_u64() {
            assert_eq!(
                u64::from(announced.get(&name).copied().unwrap_or(0)),
                count,
                "{at}: {session} notifications due"
            );
        }
    }
    if let Some(count) = expect["dock"].as_u64() {
        assert_eq!(dock as u64, count, "{at}: Dock count");
    }
    if let Some(pending) = expect["pending"].as_array() {
        let mut got: Vec<&str> = world
            .sessions
            .iter()
            .filter(|s| {
                let name = World::name(s);
                states.get(&name) == Some(&"needs-input") || unread.contains(&name)
            })
            .map(String::as_str)
            .collect();
        got.sort_unstable();
        let mut want: Vec<&str> = pending.iter().map(|v| v.as_str().unwrap()).collect();
        want.sort_unstable();
        assert_eq!(got, want, "{at}: pending (backend)");
    }
}

#[test]
fn signal_traces_hold_across_the_backend_and_match_the_golden_stream() {
    let _store = STORE_TEST_LOCK.lock_or_recover();
    let _tracker = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
    let traces: Value = serde_json::from_str(TRACES).unwrap();
    let produced = json!({
        "note": "GENERATED by src/signal_trace.rs from signal-traces.json — Signal-only, normalized; do not edit by hand",
        "traces": traces["traces"].as_array().unwrap().iter().map(run).collect::<Vec<_>>(),
    });
    crate::agent_status::reset_for_tests();
    crate::notify::reset_for_tests();
    let text = serde_json::to_string_pretty(&produced).unwrap() + "\n";
    if std::env::var("DECK_SIGNAL_GOLDEN").as_deref() == Ok("update") {
        std::fs::write(GOLDEN_PATH, &text).unwrap();
        return;
    }
    let golden: Value = serde_json::from_str(GOLDEN).unwrap();
    assert!(
        golden == produced,
        "the Signal trace golden is stale; review, then regenerate with \
         DECK_SIGNAL_GOLDEN=update cargo test --bin deck-app signal_trace"
    );
}
