//! The adapter's tests: the closed wire, and (`native`) the backend half of
//! the native Pi run.

use super::*;

fn hello(change: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut line = serde_json::json!({
        "protocol": "pi-observer", "v": 1, "source": "pi", "socket": "deck",
        "server_pid": 4242, "pane": "%7", "observerVersion": 1, "piVersion": "1.1.0",
        "mode": "tui", "capabilities": { "signal": true, "lifecycle": false }
    });
    change(&mut line);
    line.to_string()
}

fn frame(body: serde_json::Value) -> String {
    let mut line = serde_json::json!({ "protocol": "pi-observer", "v": 1, "source": "pi" });
    line.as_object_mut()
        .unwrap()
        .extend(body.as_object().unwrap().clone());
    line.to_string()
}

#[test]
fn pi_is_registered_without_a_hook_family_or_a_reentry_capability() {
    assert_eq!(PI.source, "pi");
    assert_eq!(PI.class, SignalClass::Pi);
    assert!(matches!(PI.install, Install::PiExtension));
    assert!(PI.reentry.is_none(), "Pi is never re-entered automatically");
    // the registered entry is this one, exactly once
    let registered = crate::extensions::agent(SOURCE).expect("registered");
    assert_eq!(registered.source, PI.source);
    assert!(registered.reentry.is_none());
    // an executable name is not a source word, and Pi is no remote agent
    assert_eq!(crate::admission::interactive_agent(PROCESS), None);
    assert_eq!(crate::admission::channel_agent_command("pi"), None);
    assert_eq!(
        crate::admission::channel_agent_command("pi --no-tools"),
        None
    );
    assert_eq!(crate::admission::interactive_agent("node"), None);
}

#[test]
fn the_hello_is_one_closed_shape() {
    let parsed = parse_hello(&hello(|_| {})).expect("the canonical hello");
    assert_eq!(
        parsed,
        Hello {
            socket: "deck".into(),
            server_pid: 4242,
            pane: "%7".into(),
            version: PiVersion::Supported,
            capabilities: Capabilities {
                signal: true,
                lifecycle: false
            },
        }
    );
    assert!(is_family(&hello(|_| {})));
    // every malformed variation is refused with a closed code
    let refused = |change: &dyn Fn(&mut serde_json::Value), code: &str| {
        assert_eq!(parse_hello(&hello(change)), Err(code), "{code}");
    };
    refused(&|l| l["v"] = 2.into(), "bad-version");
    refused(&|l| l["v"] = serde_json::json!(1.5), "bad-frame");
    refused(&|l| l["v"] = serde_json::json!("1"), "bad-frame");
    refused(&|l| l["observerVersion"] = 2.into(), "bad-version");
    refused(&|l| l["source"] = "claude-code".into(), "bad-frame");
    refused(&|l| l["protocol"] = "agent-lifecycle".into(), "bad-frame");
    refused(&|l| l["mode"] = "rpc".into(), "bad-mode");
    refused(&|l| l["mode"] = "print".into(), "bad-mode");
    refused(&|l| l["mode"] = "json".into(), "bad-mode");
    refused(&|l| l["mode"] = "gui".into(), "bad-frame");
    refused(&|l| l["server_pid"] = 1.into(), "bad-frame");
    refused(&|l| l["server_pid"] = (-4).into(), "bad-frame");
    refused(
        &|l| l["server_pid"] = serde_json::json!(4242.5),
        "bad-frame",
    );
    refused(
        &|l| l["server_pid"] = serde_json::json!(4294967296u64),
        "bad-frame",
    );
    refused(&|l| l["pane"] = "7".into(), "bad-frame");
    refused(&|l| l["pane"] = "%".into(), "bad-frame");
    refused(&|l| l["pane"] = "%12345678901".into(), "bad-frame");
    refused(&|l| l["pane"] = "%7;x".into(), "bad-frame");
    refused(&|l| l["socket"] = "../deck".into(), "bad-frame");
    refused(&|l| l["socket"] = "".into(), "bad-frame");
    refused(&|l| l["socket"] = "d".repeat(65).into(), "bad-frame");
    refused(&|l| l["piVersion"] = "".into(), "bad-frame");
    refused(&|l| l["piVersion"] = "1".repeat(33).into(), "bad-frame");
    refused(&|l| l["piVersion"] = "1.1.0 beta".into(), "bad-frame");
    refused(&|l| l["piVersion"] = 1.into(), "bad-frame");
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": false, "lifecycle": false}),
        "bad-frame",
    );
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": true}),
        "bad-frame",
    );
    refused(
        &|l| l["capabilities"] = serde_json::json!({"signal": true, "lifecycle": 1}),
        "bad-frame",
    );
    refused(
        &|l| {
            l["capabilities"] =
                serde_json::json!({"signal": true, "lifecycle": true, "tools": true})
        },
        "bad-frame",
    );
    refused(&|l| l["capabilities"] = true.into(), "bad-frame");
    // an unknown key, a key of another shape, a missing key
    refused(&|l| l["title"] = "x".into(), "bad-frame");
    refused(&|l| l["prompt"] = "x".into(), "bad-frame");
    refused(&|l| l["seq"] = 1.into(), "bad-frame");
    refused(&|l| l["state"] = "working".into(), "bad-frame");
    refused(&|l| l["nativeSessionId"] = "abc".into(), "bad-frame");
    for key in [
        "socket",
        "server_pid",
        "pane",
        "observerVersion",
        "piVersion",
        "mode",
        "capabilities",
    ] {
        refused(
            &|l| {
                l.as_object_mut().unwrap().remove(key);
            },
            if key == "observerVersion" {
                "bad-version"
            } else {
                "bad-frame"
            },
        );
    }
    // a repeated key is not "the last one wins"
    let twice = hello(|_| {}).replacen("\"pane\":\"%7\"", "\"pane\":\"%7\",\"pane\":\"%8\"", 1);
    assert_eq!(parse_hello(&twice), Err("bad-frame"));
    let twice = hello(|_| {}).replacen("\"signal\":true", "\"signal\":true,\"signal\":true", 1);
    assert_eq!(parse_hello(&twice), Err("bad-frame"));
    // not an object, trailing bytes, an oversized line
    assert_eq!(parse_hello("[]"), Err("bad-frame"));
    assert_eq!(
        parse_hello(&format!("{} x", hello(|_| {}))),
        Err("bad-frame")
    );
    let long = hello(|l| l["socket"] = "d".repeat(60).into());
    assert!(long.len() < MAX_LINE);
    let padded = format!("{}{}", " ".repeat(MAX_LINE), long);
    assert_eq!(parse_hello(&padded), Err("bad-frame"));
}

#[test]
fn the_version_is_compatibility_and_nothing_else() {
    let version =
        |text: &str| parse_hello(&hello(|l| l["piVersion"] = text.into())).map(|h| h.version);
    assert_eq!(version("1.1.0"), Ok(PiVersion::Supported));
    assert_eq!(version("1.1.7"), Ok(PiVersion::Supported));
    assert_eq!(version("1.1.12345"), Ok(PiVersion::Supported));
    for other in [
        "1.2.0",
        "1.0.9",
        "2.1.0",
        "0.87.0",
        "1.1",
        "1.1.0.1",
        "1.1.0-rc.1",
        "1.1.x",
        "1.1.123456",
    ] {
        assert_eq!(version(other), Ok(PiVersion::Unsupported), "{other}");
    }
}

#[test]
fn frames_are_four_closed_shapes() {
    assert_eq!(
        parse_frame(&frame(serde_json::json!({"seq": 1}))),
        Ok(Frame::Ping { seq: 1 })
    );
    assert_eq!(
        parse_frame(&frame(
            serde_json::json!({"seq": 2, "run": 1, "state": "working"})
        )),
        Ok(Frame::Signal {
            seq: 2,
            run: 1,
            state: RunState::Working
        })
    );
    assert_eq!(
        parse_frame(&frame(
            serde_json::json!({"seq": 3, "run": 1, "state": "turn-done"})
        )),
        Ok(Frame::Signal {
            seq: 3,
            run: 1,
            state: RunState::TurnDone
        })
    );
    for (word, reason) in [
        ("new", ResetReason::New),
        ("resume", ResetReason::Resume),
        ("fork", ResetReason::Fork),
        ("reload", ResetReason::Reload),
        ("quit", ResetReason::Quit),
        ("fault", ResetReason::Fault),
    ] {
        assert_eq!(
            parse_frame(&frame(serde_json::json!({"seq": 4, "reason": word}))),
            Ok(Frame::Reset { seq: 4, reason })
        );
    }
    let lifecycle = |body: serde_json::Value| parse_frame(&frame(body));
    assert_eq!(
        lifecycle(
            serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc", "reason": "startup"})
        ),
        Ok(Frame::Lifecycle {
            seq: 5,
            event: SessionEvent::Start(StartReason::Startup),
            native: NativeId::Kept(PiNativeId("abc".into()))
        })
    );
    assert_eq!(
        lifecycle(serde_json::json!({"seq": 5, "event": "shutdown", "nativeSessionId": "abc", "reason": "quit"}))
            .map(|f| f.seq()),
        Ok(5)
    );
    assert!(matches!(
        lifecycle(serde_json::json!({"seq": 5, "event": "snapshot", "nativeSessionId": "abc"})),
        Ok(Frame::Lifecycle {
            event: SessionEvent::Snapshot,
            ..
        })
    ));
    // a word of the other event, a reason on a snapshot, no reason on a start
    for body in [
        serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc", "reason": "quit"}),
        serde_json::json!({"seq": 5, "event": "shutdown", "nativeSessionId": "abc", "reason": "startup"}),
        serde_json::json!({"seq": 5, "event": "snapshot", "nativeSessionId": "abc", "reason": "startup"}),
        serde_json::json!({"seq": 5, "event": "start", "nativeSessionId": "abc"}),
        serde_json::json!({"seq": 5, "event": "end", "nativeSessionId": "abc", "reason": "quit"}),
        serde_json::json!({"seq": 5, "event": "start", "reason": "startup"}),
        serde_json::json!({"seq": 5, "nativeSessionId": "abc"}),
    ] {
        assert_eq!(lifecycle(body.clone()), Err("bad-frame"), "{body}");
    }
}

#[test]
fn a_pi_run_can_never_say_that_input_is_needed() {
    for state in ["needs-input", "idle", "done", "ready", "", "Working"] {
        assert_eq!(
            parse_frame(&frame(
                serde_json::json!({"seq": 1, "run": 1, "state": state})
            )),
            Err("bad-frame"),
            "{state}"
        );
    }
    // the closed type has two words and the store maps exactly those
    let _exhaustive = |state: RunState| match state {
        RunState::Working | RunState::TurnDone => (),
    };
}

#[test]
fn numbers_are_whole_positive_and_exact() {
    let seq = |value: serde_json::Value| parse_frame(&frame(serde_json::json!({ "seq": value })));
    assert_eq!(seq(1.into()), Ok(Frame::Ping { seq: 1 }));
    assert_eq!(
        seq(serde_json::json!(9007199254740991u64)),
        Ok(Frame::Ping {
            seq: MAX_SAFE_INTEGER
        })
    );
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(1e3),
        serde_json::json!(9007199254740992u64),
        serde_json::json!(18446744073709551615u64),
        serde_json::json!("1"),
        serde_json::json!(null),
        serde_json::json!(true),
        serde_json::json!([1]),
    ] {
        assert_eq!(seq(bad.clone()), Err("bad-frame"), "{bad}");
    }
    // JSON has no NaN or Infinity: such a line is not JSON at all
    for text in ["NaN", "Infinity", "-Infinity", "1e400"] {
        let line =
            format!("{{\"protocol\":\"pi-observer\",\"v\":1,\"source\":\"pi\",\"seq\":{text}}}");
        assert_eq!(parse_frame(&line), Err("bad-frame"), "{text}");
    }
    let run = |value: serde_json::Value| {
        parse_frame(&frame(
            serde_json::json!({"seq": 1, "run": value, "state": "working"}),
        ))
    };
    assert!(run(1.into()).is_ok());
    for bad in [
        serde_json::json!(0),
        serde_json::json!(-2),
        serde_json::json!(2.5),
    ] {
        assert_eq!(run(bad.clone()), Err("bad-frame"), "{bad}");
    }
}

#[test]
fn a_frame_carries_no_field_of_another_shape_and_no_content() {
    for body in [
        serde_json::json!({"seq": 1, "run": 1}),
        serde_json::json!({"seq": 1, "state": "working"}),
        serde_json::json!({"seq": 1, "run": 1, "state": "working", "reason": "quit"}),
        serde_json::json!({"seq": 1, "reason": "stopped"}),
        serde_json::json!({"seq": 1, "reason": "quit", "run": 1}),
        serde_json::json!({"seq": 1, "pane": "%7"}),
        serde_json::json!({"seq": 1, "socket": "deck"}),
        serde_json::json!({"seq": 1, "mode": "tui"}),
        serde_json::json!({"seq": 1, "capabilities": {"signal": true, "lifecycle": true}}),
        serde_json::json!({"seq": 1, "title": "Allow tool?"}),
        serde_json::json!({"seq": 1, "prompt": "do it"}),
        serde_json::json!({"seq": 1, "messages": []}),
        serde_json::json!({"seq": 1, "path": "/tmp/x"}),
        serde_json::json!({"seq": 1, "aborted": true}),
        serde_json::json!({"run": 1, "state": "working"}),
        serde_json::json!({}),
    ] {
        assert_eq!(
            parse_frame(&frame(body.clone())),
            Err("bad-frame"),
            "{body}"
        );
    }
    // the hello is not a frame and a frame is not a hello
    assert_eq!(parse_frame(&hello(|_| {})), Err("bad-frame"));
    assert_eq!(
        parse_hello(&frame(serde_json::json!({"seq": 1}))),
        Err("bad-frame")
    );
    let twice =
        frame(serde_json::json!({"seq": 1})).replacen("\"seq\":1", "\"seq\":1,\"seq\":2", 1);
    assert_eq!(parse_frame(&twice), Err("bad-frame"));
    assert_eq!(
        parse_frame(&frame(serde_json::json!({"seq": 1})).replace("\"v\":1", "\"v\":3")),
        Err("bad-version")
    );
}

#[test]
fn a_native_id_is_pi_s_own_shape_and_never_cut_short() {
    let named = |id: &str| {
        parse_frame(&frame(
            serde_json::json!({"seq": 1, "event": "snapshot", "nativeSessionId": id}),
        ))
    };
    let kept = |id: &str| {
        Ok(Frame::Lifecycle {
            seq: 1,
            event: SessionEvent::Snapshot,
            native: NativeId::Kept(PiNativeId(id.to_string())),
        })
    };
    // Pi's default (a UUIDv7) and what `--session-id` accepts
    for id in [
        "0199e3a4-7c2b-7f31-9a50-3b2c1d0e4f5a",
        "probe.custom-ID_123",
        "a",
        "A9",
    ] {
        assert_eq!(named(id), kept(id), "{id}");
        // Claude's validator would refuse most of them: the two never mix
        if id != "0199e3a4-7c2b-7f31-9a50-3b2c1d0e4f5a" {
            assert!(!crate::agent_reentry::claude::native_id_ok(id), "{id}");
        }
    }
    for id in [
        "", "-a", "a-", ".a", "a.", "a b", "a/b", "../x", "a\u{e9}", "a\n", "a;b",
    ] {
        assert_eq!(named(id), Err("bad-native-id"), "{id:?}");
    }
    let longest = "a".repeat(MAX_NATIVE_ID);
    assert_eq!(named(&longest), kept(&longest));
    let longer = "a".repeat(MAX_NATIVE_ID + 1);
    assert_eq!(
        named(&longer),
        Ok(Frame::Lifecycle {
            seq: 1,
            event: SessionEvent::Snapshot,
            native: NativeId::Unsupported
        }),
        "reported as unsupported, not shortened"
    );
}

#[test]
fn replies_are_three_closed_lines() {
    assert_eq!(
        accepted_line(),
        "{\"protocol\":\"pi-observer\",\"v\":1,\"accepted\":true}\n"
    );
    assert_eq!(
        ack_line(7),
        "{\"protocol\":\"pi-observer\",\"v\":1,\"ack\":7}\n"
    );
    assert_eq!(
        refused_line("duplicate-observer"),
        "{\"protocol\":\"pi-observer\",\"v\":1,\"refused\":\"duplicate-observer\"}\n"
    );
    // each is one JSON object with nothing but those keys
    for (line, key) in [
        (accepted_line(), "accepted"),
        (ack_line(7), "ack"),
        (refused_line("busy"), "refused"),
    ] {
        let value: serde_json::Value = serde_json::from_str(&line).expect("JSON");
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3, "{line}");
        assert!(value.get(key).is_some() && line.ends_with('\n'));
    }
    // a refusal code is a plain word: no quote or escape can appear
    for code in REFUSALS {
        assert!(code.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'));
    }
    // every refusal has its own drop-diagnostics name
    for code in REFUSALS {
        assert_eq!(drop_reason(code), format!("pi-{code}"));
    }
    assert_eq!(drop_reason("anything-else"), "other");
}

#[test]
fn capability_sets_compose_without_a_third_value() {
    let both = Capabilities {
        signal: true,
        lifecycle: true,
    };
    let signal = Capabilities::NONE.with(Capability::Signal, true);
    assert!(signal.within(both) && !both.within(signal));
    assert!(signal.signal && !signal.lifecycle);
    assert!(!Capabilities::NONE.any() && Capabilities::NONE.within(signal));
    assert_eq!(both.with(Capability::Lifecycle, false), signal);
}

mod native {
    //! The backend half of the native Pi run (`scripts/pi_native.py`). Test-only:
    //! one ignored test that is a long-running harness, not an assertion.
    //!
    //! # What runs here
    //! Production's own code, not a model of it:
    //! - the status listener's accept step (`agent_status::
    //!   serve_counting_for_tests` → `handle_stream`: the first line, the family
    //!   dispatch, the Pi transport started the way production starts it, with
    //!   `World::live` — the kernel, the bundled tmux beside this executable,
    //!   Pi's settings file);
    //! - the Board poll's body (`commands::poll_from_listing`: reconcile, the
    //!   projection, the coverage word) on the one-shot form of the same
    //!   snapshot, at the webview's interval;
    //! - the Settings commands (`agent_hooks_set("pi", …)`,
    //!   `agent_pi_lifecycle_set`, `agent_hooks_status`), the boot migration
    //!   and the dismiss command (`notify_dismiss`);
    //! - the session observations a lifecycle stream wrote
    //!   (`agent_reentry::lifecycle`). Their native ids appear in this
    //!   harness's trace so a run can compare them with what Pi itself
    //!   logged; that trace stays in the experiment root and is never
    //!   committed;
    //! - what the scheduler asks before it sends (`generation_evidence` by point
    //!   reads, `pi_foreground`).
    //!
    //! The harness adds a command line on standard input and, on standard
    //! output, what those returned (`@tap {json}` lines). app.log is not written
    //! in a test build, so the drop diagnostics are read where the listener
    //! counts them. Nothing here types into a pane, starts a program or a tmux
    //! server, or reads a pane's content.
    //!
    //! # Where it may run
    //! Only as an executable inside ONE experiment root that also holds its
    //! home, its temporary directory, its tmux socket directory, Pi's agent
    //! directory and the bundled tmux (`root`). Anything else is refused before
    //! a socket is bound or a file is read. The driver places this binary at
    //! `<root>/home/Applications/deck.app/Contents/MacOS/deck-app`: under that
    //! relocated home it is a stable install by the ordinary rule
    //! (`tmux_lifecycle::stable_installed_bundle`), so the Settings switch writes
    //! Pi's settings file there, and only there. No production flag, variable
    //! or path makes that possible anywhere else.
    //!
    //! The driver starts the tmux server with the server defaults this harness
    //! writes (`tmux_conf_text` for its fixed data directory, so the panes carry
    //! the status socket it listens on), registers every process with
    //! `scripts/edr_runtime.py experiment`, and stops them through it.

    use std::collections::BTreeMap;
    use std::io::{BufRead, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use serde_json::{json, Value};

    use crate::agent_pi::transport::probe;
    use crate::agent_status::{self, DropCounts};
    use crate::sync::LockRecover;
    use crate::tmux::PaneRow;

    /// The webview's poll interval (`ui/js/state.js`, `POLL_MS`).
    const POLL: Duration = Duration::from_millis(2500);
    /// How often the store is looked at between polls. Every accepted change
    /// allocates the next episode number, so a change this misses still shows
    /// as a number that was skipped.
    const SAMPLE: Duration = Duration::from_millis(2);

    /// The experiment root, or `None` without the opt-in. Everything this
    /// process reads or writes outside itself must be inside it.
    fn root() -> Option<PathBuf> {
        let root = PathBuf::from(std::env::var_os("DECK_PI_NATIVE_ROOT")?);
        assert!(
            root.starts_with("/private/tmp")
                && std::fs::canonicalize(&root).is_ok_and(|real| real == root),
            "the experiment root is not a real directory under /private/tmp"
        );
        let inside = |what: &str, path: PathBuf| {
            assert!(
                path.is_absolute() && path != root && path.starts_with(&root),
                "{what} is outside the experiment root"
            );
        };
        inside(
            "the executable",
            std::env::current_exe().expect("an executable path"),
        );
        for variable in ["HOME", "TMPDIR", "TMUX_TMPDIR", "PI_CODING_AGENT_DIR"] {
            inside(
                variable,
                PathBuf::from(std::env::var_os(variable).unwrap_or_default()),
            );
        }
        inside("the home directory", dirs::home_dir().unwrap_or_default());
        inside("the data directory", crate::datadir::deck_dir());
        inside("the bundled tmux", PathBuf::from(crate::tmux::tmux_bin()));
        Some(root)
    }

    fn wall() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    }

    /// One line to the driver.
    fn say(mut event: Value, started: Instant) {
        if let Some(object) = event.as_object_mut() {
            object.insert("t".into(), json!(started.elapsed().as_millis() as u64));
            object.insert("wall".into(), json!(wall()));
        }
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "@tap {event}");
        let _ = out.flush();
    }

    /// What the Board asked for and what the last listing held.
    #[derive(Default)]
    struct Board {
        names: Mutex<Vec<String>>,
        rows: Mutex<Vec<PaneRow>>,
        exits: Mutex<crate::shell_exit::ExitEvidence>,
    }

    /// The Signal part of one poll's payload, as the webview receives it.
    const POLL_KEYS: [&str; 11] = [
        "name",
        "alive",
        "fg",
        "agent",
        "pi_signal",
        "pi_session",
        "codex_signal",
        "episode",
        "episode_viewed",
        "finish_fg",
        "exited_normally",
    ];

    /// One Board poll: production's body over a listing read now.
    fn poll(board: &Board) -> Value {
        let mut exits = board.exits.lock_or_recover();
        let names = board.names.lock_or_recover().clone();
        let listing = crate::tmux::snapshot_or_empty_with(&crate::tmux::tmux);
        let rows = listing.as_ref().map(|snapshot| snapshot.rows.clone()).ok();
        let polled = crate::commands::poll_from_listing(
            names,
            Vec::new(),
            false,
            listing,
            crate::procinfo::processes,
            &mut exits,
            &|_| false,
        );
        let (sessions, rows) = match (polled, rows) {
            (Ok(sessions), Some(rows)) => (sessions, rows),
            (Err(error), _) => return json!({ "error": error.code() }),
            (Ok(_), None) => return json!({ "error": "no-listing" }),
        };
        let sessions: Vec<Value> = serde_json::to_value(&sessions)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .map(|session| {
                let kept: serde_json::Map<String, Value> = POLL_KEYS
                    .iter()
                    .filter_map(|key| Some(((*key).to_string(), session.get(*key)?.clone())))
                    .collect();
                Value::Object(kept)
            })
            .collect();
        let asked = scheduler(&rows);
        *board.rows.lock_or_recover() = rows;
        json!({ "sessions": sessions, "scheduler": asked })
    }

    /// What the scheduler would know about each session's Signal target right
    /// now: the tick's evidence and the pre-fire question, both by point reads.
    fn scheduler(rows: &[PaneRow]) -> Value {
        let evidence = agent_status::generation_evidence(
            rows,
            agent_status::live_generation,
            agent_status::foreground_name,
        );
        let asked: BTreeMap<String, Value> = agent_status::signal_targets(rows)
            .into_iter()
            .map(|(session, row)| {
                let seen = evidence.get(&session).copied().unwrap_or_default();
                let pre_fire = agent_status::pi_foreground(
                    row.server_pid,
                    &row.pane_id,
                    row.pane_pid,
                    agent_status::live_generation,
                    agent_status::foreground_name,
                );
                (
                    session,
                    json!({ "pi": seen.pi, "pi_signal": seen.pi_signal, "pre_fire_pi": pre_fire }),
                )
            })
            .collect();
        json!(asked)
    }

    /// What the store, the transport and the diagnostics hold right now.
    fn sample(board: &Board, drops: &DropCounts) -> Value {
        let rows = board.rows.lock_or_recover().clone();
        let store: BTreeMap<String, Value> = agent_status::projections(&rows)
            .into_iter()
            .map(|(session, seen)| {
                (
                    session,
                    json!({ "state": seen.state, "episode": seen.episode, "viewed": seen.viewed }),
                )
            })
            .collect();
        let announced: BTreeMap<String, u32> =
            crate::notify::announced_for_tests().into_iter().collect();
        let leases: Vec<Value> = probe::leases()
            .into_iter()
            .map(|(lease, session, run, open, next)| {
                json!({ "lease": lease, "session": session, "run": run, "open": open, "next_seq": next })
            })
            .collect();
        let ended: Vec<Value> = probe::ended()
            .into_iter()
            .map(|(session, code)| json!({ "session": session, "code": code }))
            .collect();
        let sessions: Vec<Value> = crate::agent_reentry::lifecycle::pi_sessions_for_tests()
            .into_iter()
            .map(|(lease, id, last)| json!({ "lease": lease, "id": id, "last": said(last) }))
            .collect();
        json!({
            "store": store,
            "streams": probe::streams(),
            "leases": leases,
            "ended": ended,
            "sessions": sessions,
            "pending": probe::pending(),
            "drops": drops.counts(),
            "announced": announced,
        })
    }

    /// Pi's part of the Settings status command's reply.
    fn settings() -> Value {
        let status = serde_json::to_value(agent_status::agent_hooks_status()).unwrap_or_default();
        json!({
            "pi": status.get("pi"),
            "pi_lifecycle": status.get("pi_lifecycle"),
            "pi_installation": status.get("pi_installation"),
        })
    }

    /// The last thing a runtime said of its session, as one closed word.
    fn said(event: crate::agent_pi::SessionEvent) -> String {
        use crate::agent_pi::SessionEvent;
        match event {
            SessionEvent::Start(reason) => format!("start-{reason:?}").to_lowercase(),
            SessionEvent::Shutdown(reason) => format!("shutdown-{reason:?}").to_lowercase(),
            SessionEvent::Snapshot => "snapshot".into(),
        }
    }

    /// One command of the driver; the reply's body.
    fn answer(command: &Value, board: &Board, drops: &DropCounts) -> Value {
        let text = |key: &str| command.get(key).and_then(Value::as_str).unwrap_or_default();
        match text("op") {
            "names" => {
                *board.names.lock_or_recover() = command
                    .get("names")
                    .and_then(Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| name.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({})
            }
            "poll" => json!({ "poll": poll(board), "now": sample(board, drops) }),
            "status" => settings(),
            "signal" => {
                let enable = command.get("enable").and_then(Value::as_bool) == Some(true);
                let result = agent_status::agent_hooks_set(crate::agent_pi::SOURCE.into(), enable);
                json!({
                    "ok": result.is_ok(),
                    "error": result.err().map(|error| error.message().to_owned()),
                    "status": settings(),
                })
            }
            "lifecycle" => {
                let enable = command.get("enable").and_then(Value::as_bool) == Some(true);
                let result = crate::agent_pi::config::agent_pi_lifecycle_set(enable);
                json!({
                    "ok": result.is_ok(),
                    "error": result.err().map(|error| error.message().to_owned()),
                    "status": settings(),
                })
            }
            "migrate" => {
                crate::agent_pi::config::migrate_on_boot();
                settings()
            }
            "viewed" => {
                let episode = command.get("episode").and_then(Value::as_u64).unwrap_or(0);
                let result = crate::notify::notify_dismiss(text("session").into(), episode);
                json!({ "ok": result.is_ok() })
            }
            _ => json!({ "error": "unknown-op" }),
        }
    }

    /// Run until the driver says `quit` or closes standard input.
    #[test]
    #[ignore = "the native Pi run's backend: started by scripts/pi_native.py inside an experiment root"]
    fn tap() {
        let Some(root) = root() else {
            eprintln!("agent_pi::tests::native::tap: no DECK_PI_NATIVE_ROOT, nothing to do");
            return;
        };
        let started = Instant::now();
        let data = root.join("deck");
        crate::datadir::create_private_dir(&data).expect("the data directory");
        let conf = data.join("tmux.conf");
        crate::datadir::write_private(&conf, crate::tmux::tmux_conf_text(&data).as_bytes())
            .expect("the server defaults");
        let socket = data.join("status.sock");
        let listener = agent_status::listen_at(&socket).expect("the status socket");
        let stop = Arc::new(AtomicBool::new(false));
        let drops = DropCounts::new();
        let board = Arc::new(Board::default());

        let listening = std::thread::spawn({
            let (stop, drops) = (stop.clone(), drops.clone());
            move || agent_status::serve_counting_for_tests(listener, stop, &drops)
        });
        let polling = std::thread::spawn({
            let (stop, board) = (stop.clone(), board.clone());
            move || {
                let mut last = Value::Null;
                let mut next = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    if Instant::now() >= next {
                        next += POLL;
                        let now = poll(&board);
                        if now != last {
                            say(json!({ "poll": now }), started);
                            last = now;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        });
        let sampling = std::thread::spawn({
            let (stop, board, drops) = (stop.clone(), board.clone(), drops.clone());
            move || {
                let mut last = Value::Null;
                while !stop.load(Ordering::Acquire) {
                    let now = sample(&board, &drops);
                    if now != last {
                        say(json!({ "now": now }), started);
                        last = now;
                    }
                    std::thread::sleep(SAMPLE);
                }
            }
        });

        say(
            json!({
                "ready": {
                    "status_sock": path_text(&socket),
                    "conf": path_text(&conf),
                    "socket": crate::tmux::socket(),
                    "tmux": crate::tmux::tmux_bin(),
                    "settings": settings(),
                }
            }),
            started,
        );
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(command) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if command.get("op").and_then(Value::as_str) == Some("quit") {
                break;
            }
            let mut reply = answer(&command, &board, &drops);
            if let Some(object) = reply.as_object_mut() {
                object.insert("re".into(), command.get("id").cloned().unwrap_or_default());
            }
            say(reply, started);
        }
        stop.store(true, Ordering::Release);
        for thread in [listening, polling, sampling] {
            let _ = thread.join();
        }
        say(json!({ "bye": true }), started);
    }

    fn path_text(path: &Path) -> String {
        path.display().to_string()
    }
}
