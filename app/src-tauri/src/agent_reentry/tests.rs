//! Re-entry tests: the pure decision, and the production restart transaction
//! run against a throwaway bundled-tmux server with a harmless fake agent
//! (`examples/fake_reentry_agent.rs`) that runs the REAL bundled lifecycle
//! helper. No real agent, credential, user configuration or Deck server is
//! touched: every tmux command carries a private socket, a private HOME and
//! a private status socket.
//!
//! The `#[ignore]` entries named `native_*` are not tests of their own. A
//! native certification driver runs them against a server it made, under
//! the home it names: read-only probes, a passive recorder of lifecycle
//! lines, and one controller that runs the production transaction there.
//!
//! Side effects are asserted on what actually happened: the bytes handed to
//! the paste transport, the number of transport calls, and the argv each
//! launched client recorded for itself.

use super::*;
use crate::procinfo::identity_fixture as fixture;
use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ---------- the override decision ------------------------------------------------

fn scope(policy: Policy, preview: LaunchProfile, frozen: LaunchProfile) -> Scope {
    Scope {
        policy,
        selected: true,
        preview,
        frozen,
        identity_unchanged: true,
        context_proven: true,
    }
}

fn bypass_of(decision: Decision) -> Option<bool> {
    match decision {
        Decision::Reenter { bypass, .. } => Some(bypass),
        _ => None,
    }
}

use LaunchProfile::{Bypass, Ordinary, Unknown, Unsupported};
use Policy::{Ordinary as PolicyA, ReapplyBypass as PolicyB};

/// The mixed three cards: A started with bypass, B ordinary, C ordinary
/// (running in plan). The final permission of all three is unknown, and no
/// field of `Scope` could even carry it.
#[test]
fn policy_a_never_adds_the_override_and_policy_b_adds_it_only_to_the_bypass_launch() {
    let cards = [Bypass, Ordinary, Ordinary];
    let under = |policy| -> Vec<Option<bool>> {
        cards
            .iter()
            .map(|profile| bypass_of(decide(scope(policy, *profile, *profile))))
            .collect()
    };
    assert_eq!(under(PolicyA), [Some(false), Some(false), Some(false)]);
    assert_eq!(under(PolicyB), [Some(true), Some(false), Some(false)]);
}

/// The client was started with bypass and lowered to plan while running.
/// Its launch profile is still bypass, the final mode is not observable, and
/// the user picked B after the stated warning: bypass is applied AGAIN. This
/// is a reauthorization for this restart, not a preserved final state.
#[test]
fn the_override_is_decided_by_the_launch_profile_and_no_final_mode() {
    assert_eq!(
        decide(scope(PolicyB, Bypass, Bypass)),
        Decision::Reenter {
            bypass: true,
            warnings: vec![]
        }
    );
    // the same client under the default policy is left to native resume
    assert_eq!(
        decide(scope(PolicyA, Bypass, Bypass)),
        Decision::Reenter {
            bypass: false,
            warnings: vec![]
        }
    );
}

#[test]
fn the_reauthorized_set_never_grows_past_what_was_previewed_selected_and_frozen() {
    // previewed bypass, read as unknown at freeze: no override; re-entered
    // only because the context is proven independently, otherwise skipped
    assert_eq!(
        decide(scope(PolicyB, Bypass, Unknown)),
        Decision::Reenter {
            bypass: false,
            warnings: vec![Warning::LaunchProfileUnconfirmed]
        }
    );
    let mut unproven = scope(PolicyB, Bypass, Unknown);
    unproven.context_proven = false;
    assert_eq!(
        decide(unproven),
        Decision::Skip(Reason::IdentityUnavailable)
    );
    // previewed bypass, ordinary at freeze: no override, and it is said
    assert_eq!(
        decide(scope(PolicyB, Bypass, Ordinary)),
        Decision::Reenter {
            bypass: false,
            warnings: vec![Warning::BypassNotReapplied]
        }
    );
    // ordinary or unknown in the preview, bypass at freeze: the user was
    // never shown this bypass, so it is not added without a new preview
    for shown in [Ordinary, Unknown] {
        assert_eq!(
            decide(scope(PolicyB, shown, Bypass)),
            Decision::Reenter {
                bypass: false,
                warnings: vec![Warning::BypassNotReapplied]
            }
        );
    }
    // an unsupported launch is never re-entered, under either policy
    for policy in [PolicyA, PolicyB] {
        assert_eq!(
            decide(scope(policy, Bypass, Unsupported)),
            Decision::Skip(Reason::IdentityUnavailable)
        );
    }
    // a changed identity voids the approval instead of rebinding it
    let mut moved = scope(PolicyB, Bypass, Bypass);
    moved.identity_unchanged = false;
    assert_eq!(decide(moved), Decision::Repreview);
    // a card that was not ticked is no part of the operation
    let mut unticked = scope(PolicyB, Bypass, Bypass);
    unticked.selected = false;
    assert_eq!(decide(unticked), Decision::NotApproved);
    // exhaustively: the override exists only in that one cell
    for policy in [PolicyA, PolicyB] {
        for preview in [Bypass, Ordinary, Unknown, Unsupported] {
            for frozen in [Bypass, Ordinary, Unknown, Unsupported] {
                let granted = bypass_of(decide(scope(policy, preview, frozen))) == Some(true);
                assert_eq!(
                    granted,
                    policy == PolicyB && preview == Bypass && frozen == Bypass,
                    "{policy:?} {preview:?} {frozen:?}"
                );
            }
        }
    }
}

/// Nothing in this feature can read a saved card command, and nothing in it
/// reaches the Signal store, the scheduler, readiness or authority.
#[test]
fn the_sources_read_no_saved_command_and_reach_no_signal_or_scheduler_state() {
    for (name, source) in [
        ("mod.rs", include_str!("mod.rs")),
        ("claude.rs", include_str!("claude.rs")),
        ("lifecycle.rs", include_str!("lifecycle.rs")),
        ("launch.rs", include_str!("launch.rs")),
    ] {
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "[\"cmd\"]",
            ".cmd",
            "crate::scheduler",
            "crate::notify",
            "prove_claude_interaction",
            "with_agents",
            "generation_evidence",
            "crate::resume",
            "shell_state",
            "Command::new",
            "run-shell",
            "pipe-pane",
            "send-keys",
            "std::fs::write",
            "atomic_write",
        ] {
            assert!(!code.contains(banned), "{name} must not contain {banned}");
        }
    }
}

#[test]
fn views_carry_closed_codes_and_no_native_identity() {
    let view = EntryView {
        card_id: "c1".into(),
        session: "s1".into(),
        outcome: Outcome::Confirmed,
        reason: None,
        warnings: vec![Warning::BypassReauthorized],
        shell: true,
        client_observed: true,
        settled: true,
    };
    let json = serde_json::to_value(&view).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "cardId",
            "clientObserved",
            "outcome",
            "reason",
            "session",
            "settled",
            "shell",
            "warnings"
        ]
    );
    assert_eq!(json["outcome"], "confirmed");
    assert_eq!(json["warnings"][0], "bypass-reauthorized");
    let approval: Result<Approval, _> = serde_json::from_str(
        r#"{"token":"t","cards":["c1"],"policy":"reapply-bypass","nativeSessionId":"x"}"#,
    );
    assert!(
        approval.is_err(),
        "an approval cannot name a native session"
    );
    let approval: Approval =
        serde_json::from_str(r#"{"token":"t","cards":["c1"],"policy":"ordinary"}"#).unwrap();
    assert_eq!(approval.policy, Policy::Ordinary);
}

/// A final receipt is not a process exit. With the receipt recorded and one
/// old exact client generation still alive, nothing is sealed and the whole
/// restart is refused before the pause and the kill.
#[test]
fn a_final_receipt_seals_only_when_the_old_client_generation_is_gone() {
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let me = std::process::id();
    let birth = crate::procinfo::process_start(me).expect("own birth");
    let own = ExecutableRef::of_this_test();
    let entry = |card: &str, client_birth: (u64, u32)| Entry {
        view: EntryView {
            card_id: card.into(),
            session: card.into(),
            outcome: Outcome::NotAttempted,
            reason: None,
            warnings: Vec::new(),
            shell: false,
            client_observed: false,
            settled: false,
        },
        old: PaneRow::default(),
        shell_birth: (1, 1),
        client: Client {
            pid: me,
            birth: client_birth,
            executable: own.clone(),
            profile: Ordinary,
            additional_dirs: false,
            cwd: DirRef::for_tests(&std::env::temp_dir()),
        },
        bypass: false,
        final_id: None,
        request: Vec::new(),
        slot: Slot::Armed,
        new: None,
        awaiting: false,
    };
    let mut plan = Plan {
        shared: Arc::new(Mutex::new(ReentryResult {
            request_id: "req-seal".into(),
            state: OperationState::Running,
            server_replaced: false,
            entries: Vec::new(),
        })),
        // the first entry's client is this live process; the second names a
        // generation of the same pid that does not exist
        entries: vec![entry("alive", birth), entry("gone", (birth.0 + 1, birth.1))],
        reread: Vec::new(),
    };
    lifecycle::begin("deck-smoke-seal", Path::new("/nonexistent/helper"));
    for (index, id) in [(0, ID_A), (1, ID_B)] {
        lifecycle::arm(
            index,
            lifecycle::Lease::exit(2, "%0", (3, (1, 1)), (me, birth), own.clone()),
        );
        lifecycle::record_for_tests(
            index,
            lifecycle::Fact {
                event: claude::ended(claude::EndReason::PromptInputExit),
                native_id: id.to_string(),
                client: (me, birth),
            },
        );
    }
    // one old client generation is still alive: the whole restart stops
    // here, before the pause and before any kill, with the existing failure
    let error = bare_env(&[("alive", "alive"), ("gone", "gone")], |env| {
        plan.seal_exits(env)
    })
    .unwrap_err();
    assert_eq!(error.message(), "tmux-restart-agent-timeout");
    assert!(plan.entries.iter().all(|entry| entry.final_id.is_none()));
    // sealed: a later event has no lease to land on
    assert!(!lifecycle::armed());
    drop(plan);
    let result = known("req-seal").expect("the dropped plan is the final result");
    assert_eq!(result.state, OperationState::Complete);
    assert!(result
        .entries
        .iter()
        .all(|entry| entry.outcome == Outcome::NotAttempted));
    reset_for_tests();
}

/// Two approved entries that sealed the same native session are both left to
/// the user: one operation never launches a session twice and picks no winner.
#[test]
fn two_entries_with_the_same_final_session_are_both_left_to_the_user() {
    assert_eq!(
        claude::final_session(&[]),
        None,
        "no receipt, no final session"
    );
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let me = std::process::id();
    let birth = crate::procinfo::process_start(me).expect("own birth");
    let gone = (birth.0 + 1, birth.1);
    let own = ExecutableRef::of_this_test();
    let entry = |card: &str| Entry {
        view: EntryView {
            card_id: card.into(),
            session: card.into(),
            outcome: Outcome::NotAttempted,
            reason: None,
            warnings: Vec::new(),
            shell: false,
            client_observed: false,
            settled: false,
        },
        old: PaneRow::default(),
        shell_birth: (1, 1),
        client: Client {
            pid: me,
            birth: gone,
            executable: own.clone(),
            profile: Bypass,
            additional_dirs: false,
            cwd: DirRef::for_tests(&std::env::temp_dir()),
        },
        bypass: true,
        final_id: None,
        request: Vec::new(),
        slot: Slot::Armed,
        new: None,
        awaiting: false,
    };
    let mut plan = Plan {
        shared: Arc::new(Mutex::new(ReentryResult {
            request_id: "req-same".into(),
            state: OperationState::Running,
            server_replaced: false,
            entries: Vec::new(),
        })),
        entries: vec![entry("one"), entry("two"), entry("three")],
        reread: Vec::new(),
    };
    lifecycle::begin("deck-smoke-same", Path::new("/nonexistent/helper"));
    for (index, id) in [(0, ID_A), (1, ID_A), (2, ID_C)] {
        lifecycle::arm(
            index,
            lifecycle::Lease::exit(2, "%0", (3, (1, 1)), (me, gone), own.clone()),
        );
        lifecycle::record_for_tests(
            index,
            lifecycle::Fact {
                event: claude::ended(claude::EndReason::PromptInputExit),
                native_id: id.to_string(),
                client: (me, gone),
            },
        );
    }
    bare_env(
        &[("one", "one"), ("two", "two"), ("three", "three")],
        |env| plan.seal_exits(env),
    )
    .expect("every old client is gone");
    for same in [0, 1] {
        assert_eq!(plan.entries[same].final_id, None);
        assert_eq!(
            plan.entries[same].view.reason,
            Some(Reason::IdentityUnavailable)
        );
    }
    assert_eq!(plan.entries[2].final_id.as_deref(), Some(ID_C));
    drop(plan);
    reset_for_tests();
}

/// An environment for coordinator steps that reach nothing but the Board.
fn bare_env<R>(cards: &[(&str, &str)], work: impl FnOnce(&Env) -> R) -> R {
    let refuse = |_: &[&str]| Err(DeckError::new(ErrorKind::Tmux, "no server in this test"));
    let refuse_stdin =
        |_: &[&str], _: &[u8]| Err(DeckError::new(ErrorKind::Tmux, "no server in this test"));
    let cards = || {
        Some(
            cards
                .iter()
                .map(|(card, session)| (card.to_string(), session.to_string()))
                .collect(),
        )
    };
    let create = |_: &str, _: &Path, _: bool| -> Result<(), DeckError> { Ok(()) };
    let fenced = |work: &mut dyn FnMut()| work();
    work(&Env {
        tmux: &refuse,
        tmux_stdin: &refuse_stdin,
        socket: "deck-smoke-bare",
        cards: &cards,
        capability: &claude::CAPABILITY,
        helper: Path::new("/nonexistent/helper"),
        home: Path::new("/nonexistent/home"),
        login_shell: Path::new("/bin/zsh"),
        observation: true,
        baseline: true,
        fenced: &fenced,
        create_shell: &create,
    })
}

// ---------- the world: a private tmux server, HOME, status socket and fake agent ----

static SEQ: AtomicU64 = AtomicU64::new(0);

/// One recorded tmux call: its arguments and, with stdin, the bytes.
type Call = (Vec<String>, Option<Vec<u8>>);
type Intercept = Box<dyn Fn(&[&str], Option<&[u8]>) -> Option<Result<String, DeckError>>>;

struct World {
    base: PathBuf,
    /// The home the clients run under and the server hands to its panes: the
    /// base directory, except in a native world, where it lies outside it.
    home: PathBuf,
    /// The pane shells' own startup directory (`ZDOTDIR`), where one is set.
    zdot: Option<PathBuf>,
    /// More names every server of this world hands to its panes (a native
    /// world only: what the driver's own server was given beyond the six).
    extra: Vec<(String, String)>,
    socket: String,
    agent: PathBuf,
    helper: PathBuf,
    capability: &'static ReentryCapability,
    /// The cards of the Board this world committed last (card id, session).
    /// Every change goes through `commit`, the Board door's real save.
    cards: RefCell<Vec<(String, String)>>,
    /// Every tmux call made through the environment: its arguments and, for
    /// a call with stdin, the bytes.
    calls: RefCell<Vec<Call>>,
    intercept: RefCell<Option<Intercept>>,
    /// What the production shell creator's `new-session` meets on this
    /// server: "ok", "fail" (tmux refuses) or "sh" (another shell program).
    create: RefCell<&'static str>,
    /// This process created the server and directory and removes them.
    owner: bool,
    observation: std::cell::Cell<bool>,
    baseline: std::cell::Cell<bool>,
    restore: std::cell::Cell<bool>,
    /// Run once, at the last read of a new shell's readiness check: after
    /// the launch's own checks and before the fenced section and the input.
    at_ready: RefCell<Option<Box<dyn Fn()>>>,
    stop: std::sync::Arc<AtomicBool>,
    serving: Option<std::thread::JoinHandle<()>>,
    /// The process's committed Board is this world's for its lifetime.
    _board: crate::documents::test_door::Held,
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// A valid Board document holding `cards`. Each card saves a command that
/// must never reach a pane: re-entry does not read it.
fn board_json(cards: &[(String, String)]) -> String {
    let cards: Vec<serde_json::Value> = cards
        .iter()
        .map(|(id, session)| {
            serde_json::json!({
                "id": id, "projectId": "P1", "columnId": "C1", "title": id,
                "cmd": SAVED_COMMAND, "dir": "/tmp", "session": session,
            })
        })
        .collect();
    serde_json::json!({
        "projects": [{"id": "P1", "name": "main", "columns": [{"id": "C1", "name": "Working"}]}],
        "cards": cards,
    })
    .to_string()
}

const SAVED_COMMAND: &str = "SENTINEL-SAVED-COMMAND --dangerously-skip-permissions";

/// The stand-in client as a program: `examples/fake_reentry_agent.rs`
/// compiled by itself — one call of the `rustc` beside the cargo that runs
/// these tests, and nothing but the standard library.
///
/// It is not cargo's own build of that example. Cargo builds an example as
/// a program only when no target is named: under `--all-targets` (the
/// gate's form) every example becomes a test harness and no program is
/// left, and under `--bin` examples are not built at all. Asking cargo for
/// the example from inside a test is no way out either: that build is this
/// package's, so it runs the package's build script (sidecars, native
/// bridges) beside the running tests.
///
/// Kept in the profile directory under the digest of the source: an edited
/// example is another program, an unedited one is compiled once.
fn standin_program() -> PathBuf {
    static PROGRAM: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            use sha2::{Digest, Sha256};
            let source =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fake_reentry_agent.rs");
            let text = std::fs::read(&source).expect("the stand-in's source");
            let digest: String = Sha256::digest(&text)[..8]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let exe = std::env::current_exe().expect("test executable");
            let profile = exe
                .parent()
                .and_then(Path::parent)
                .expect("target profile dir");
            let dir = profile.join(format!("reentry-standin-{digest}"));
            let program = dir.join("fake_reentry_agent");
            if !program.is_file() {
                std::fs::create_dir_all(&dir).expect("the stand-in's directory");
                // complete or absent: another test process may be compiling it too
                let staged = dir.join(format!("fake_reentry_agent.{}.tmp", std::process::id()));
                let rustc = std::env::var_os("CARGO")
                    .map(|cargo| PathBuf::from(cargo).with_file_name("rustc"))
                    .filter(|rustc| rustc.is_file())
                    .unwrap_or_else(|| PathBuf::from("rustc"));
                let output = Command::new(&rustc)
                    .args(["--edition", "2021", "--crate-type", "bin"])
                    .args(["--crate-name", "fake_reentry_agent"])
                    .args(["-C", "debuginfo=0", "-o"])
                    .arg(&staged)
                    .arg(&source)
                    .output()
                    .unwrap_or_else(|error| panic!("{}: {error}", rustc.display()));
                assert!(
                    output.status.success(),
                    "examples/fake_reentry_agent.rs compiles by itself: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                std::fs::rename(&staged, &program).expect("the compiled stand-in");
            }
            std::fs::canonicalize(&program).expect("the compiled stand-in")
        })
        .clone()
}

/// The signing identifier every stand-in client of these tests carries:
/// "the test product". An ad-hoc signature with no certificate — it stands
/// in for a product's identity and proves nothing about Anthropic's.
const AGENT_IDENTIFIER: &str = "io.c9r.deck.test.fake-reentry-agent";
/// What precedes the build tag inside the stand-in client's file.
const BUILD_MARKER: &[u8] = b"DECK-FAKE-REENTRY-AGENT-BUILD:";

/// A build of the stand-in client at `dest`: the compiled example, with
/// `tag` as its build tag — so two tags are two contents, two digests
/// and two code directories — signed ad hoc under `identifier`.
fn agent_build(dest: &Path, tag: &str, identifier: &str) {
    std::fs::create_dir_all(dest.parent().expect("a build has a directory")).unwrap();
    std::fs::copy(standin_program(), dest).expect("the built stand-in");
    fixture::retag(dest, BUILD_MARKER, tag);
    fixture::sign(dest, identifier);
}

const ID_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const ID_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const ID_C: &str = "cccccccc-0000-4000-8000-000000000003";
const ID_X: &str = "dddddddd-0000-4000-8000-000000000004";

impl World {
    fn new() -> Self {
        // before anything is created: a stand-in that does not compile fails
        // here and leaves no directory behind
        standin_program();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        // short on purpose: unix socket paths are limited to ~100 bytes
        let base = PathBuf::from(format!("/tmp/deck-rt-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(base.join("cards")).expect("world dir");
        let socket = format!("deck-smoke-reentry-{}-{seq}", std::process::id());
        Self::open(&base, &socket, true)
    }

    /// The same world from another process (the crash controller): it takes
    /// over the status socket and never removes anything it did not create.
    fn attach(base: &Path, socket: &str) -> Self {
        Self::open(base, socket, false)
    }

    fn open(base: &Path, socket: &str, owner: bool) -> Self {
        let lock = TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let held = crate::documents::test_door::hold();
        let base = std::fs::canonicalize(base).expect("canonical world dir");
        // this world's own signed build of the stand-in client
        let agent = base.join("bin").join("fake_reentry_agent");
        if owner {
            agent_build(&agent, "build-0", AGENT_IDENTIFIER);
        }
        let helper = std::fs::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("binaries/deck-lifecycle-helper-aarch64-apple-darwin"),
        )
        .expect("build.rs built the lifecycle helper");
        std::fs::write(base.join("helper-path"), helper.to_str().unwrap()).unwrap();
        let listener =
            crate::agent_status::listen_at(&base.join("status.sock")).expect("status socket");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let serving = std::thread::spawn({
            let stop = stop.clone();
            move || crate::agent_status::serve_for_tests(listener, stop)
        });
        let world = Self {
            socket: socket.to_string(),
            owner,
            capability: Self::capability(),
            agent,
            helper,
            home: base.clone(),
            zdot: None,
            extra: Vec::new(),
            base,
            cards: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
            intercept: RefCell::new(None),
            create: RefCell::new("ok"),
            observation: std::cell::Cell::new(true),
            baseline: std::cell::Cell::new(true),
            restore: std::cell::Cell::new(false),
            at_ready: RefCell::new(None),
            stop,
            serving: Some(serving),
            _board: held,
            _lock: lock,
        };
        assert_eq!(
            crate::procinfo::login_shell().as_deref(),
            Some(Path::new("/bin/zsh")),
            "these tests run the production assembly, which reads the account's login shell"
        );
        if owner {
            world
                .raw(&["start-server", ";", "set-option", "-g", "exit-empty", "off"])
                .expect("isolated server");
            world.commit();
        } else {
            // another process's world: its Board is the file that one saved
            let loaded = crate::documents::test_door::load(&world.board_file())
                .expect("the owner's Board loads")
                .expect("the owner saved a Board");
            let board: serde_json::Value = serde_json::from_str(&loaded.0).unwrap();
            *world.cards.borrow_mut() = board["cards"]
                .as_array()
                .unwrap()
                .iter()
                .map(|card| {
                    (
                        card["id"].as_str().unwrap().to_string(),
                        card["session"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
        }
        world
    }

    /// A world over a server, directories and clients that another process
    /// made (the native certification driver). Nothing of theirs is created
    /// or removed here. The clients' home is NOT this world's base directory;
    /// the status socket is in the base directory and is taken over; the
    /// capability is production's unless a stand-in binary is named.
    #[allow(clippy::too_many_arguments)]
    fn native(
        base: &Path,
        socket: &str,
        home: &Path,
        zdot: &Path,
        helper: &Path,
        client: &Path,
        capability: &'static ReentryCapability,
        extra: Vec<(String, String)>,
    ) -> Self {
        let lock = TEST_LOCK.lock_or_recover();
        reset_for_tests();
        let held = crate::documents::test_door::hold();
        let base = std::fs::canonicalize(base).expect("canonical base directory");
        let listener =
            crate::agent_status::listen_at(&base.join("status.sock")).expect("status socket");
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let serving = std::thread::spawn({
            let stop = stop.clone();
            move || crate::agent_status::serve_for_tests(listener, stop)
        });
        Self {
            socket: socket.to_string(),
            owner: false,
            capability,
            agent: client.to_path_buf(),
            helper: helper.to_path_buf(),
            home: home.to_path_buf(),
            zdot: Some(zdot.to_path_buf()),
            extra,
            base,
            cards: RefCell::new(Vec::new()),
            calls: RefCell::new(Vec::new()),
            intercept: RefCell::new(None),
            create: RefCell::new("ok"),
            observation: std::cell::Cell::new(true),
            baseline: std::cell::Cell::new(true),
            restore: std::cell::Cell::new(false),
            at_ready: RefCell::new(None),
            stop,
            serving: Some(serving),
            _board: held,
            _lock: lock,
        }
    }

    fn board_file(&self) -> PathBuf {
        self.base.join("deck.json")
    }

    /// Save this world's cards through the Board door: the real save under
    /// the real fence, committed as the owner's current Board.
    fn commit(&self) {
        crate::documents::test_door::save(&self.board_file(), &board_json(&self.cards.borrow()))
            .expect("the Board saves");
    }

    /// Change the cards and commit the Board.
    fn edit_cards(&self, change: impl FnOnce(&mut Vec<(String, String)>)) {
        change(&mut self.cards.borrow_mut());
        self.commit();
    }

    /// A tmux runner for this server that borrows nothing from the world,
    /// for hooks that must be `'static`.
    fn remote(&self) -> impl Fn(&[&str]) -> Result<String, DeckError> + 'static {
        let (base, home, zdot, socket, extra) = (
            self.base.clone(),
            self.home.clone(),
            self.zdot.clone(),
            self.socket.clone(),
            self.extra.clone(),
        );
        move |args| {
            let mut command = Command::new(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin"),
            );
            command
                .env_clear()
                .env("HOME", &home)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("TERM", "xterm-256color")
                .env("SHELL", "/bin/zsh")
                .env("DECK_STATUS_SOCK", base.join("status.sock"))
                .envs(extra.iter().map(|(name, value)| (name, value)))
                .args(["-f", "/dev/null", "-L", &socket]);
            if let Some(zdot) = &zdot {
                command.env("ZDOTDIR", zdot);
            }
            crate::tmux::captured_output(&mut command, args)
        }
    }

    /// The foreground process of `session`'s pane is not the pane's shell.
    fn foreground_is_child(&self, session: &str) -> bool {
        self.raw(&["display-message", "-p", "-t", session, "#{pane_pid}"])
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .and_then(|pid| Some((pid, crate::agent_status::live_generation(pid)?)))
            .is_some_and(|(pid, foreground)| foreground.pid() != pid)
    }

    /// The stand-in capability: the TEST product's requirement (every
    /// build `agent_build` signs under `AGENT_IDENTIFIER`), the stand-in's
    /// launcher name, and production's own grammar, login shell and request
    /// builder. It names no build: which file a client runs, what that file
    /// is called and what it contains are not part of it, exactly as in
    /// production's. Production code never sees it unless a test hands it in.
    fn capability() -> &'static ReentryCapability {
        static STANDIN: std::sync::OnceLock<ReentryCapability> = std::sync::OnceLock::new();
        STANDIN.get_or_init(|| ReentryCapability {
            product: fixture::product(AGENT_IDENTIFIER),
            launcher: "fake_reentry_agent",
            grammar: claude::CAPABILITY.grammar,
            login_shell: "zsh",
            request: claude::CAPABILITY.request,
            receipts: claude::CAPABILITY.receipts,
        })
    }

    /// A stand-in capability for a binary a native driver names: the product
    /// "exactly that file's own signing identifier". Refused (`None`) for a
    /// file that is production's product — a real client is never judged by
    /// a stand-in's looser requirement.
    fn standin_capability(binary: &Path) -> Option<&'static ReentryCapability> {
        if ExecutableRef::read(binary, claude::CAPABILITY.product).is_ok() {
            return None;
        }
        Some(Box::leak(Box::new(ReentryCapability {
            product: fixture::product_of(binary),
            launcher: claude::CAPABILITY.launcher,
            grammar: claude::CAPABILITY.grammar,
            login_shell: claude::CAPABILITY.login_shell,
            request: claude::CAPABILITY.request,
            receipts: claude::CAPABILITY.receipts,
        })))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin"),
        );
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("TERM", "xterm-256color")
            .env("SHELL", "/bin/zsh")
            .env("DECK_STATUS_SOCK", self.base.join("status.sock"))
            .envs(self.extra.iter().map(|(name, value)| (name, value)))
            .args(["-f", "/dev/null", "-L", &self.socket]);
        if let Some(zdot) = &self.zdot {
            command.env("ZDOTDIR", zdot);
        }
        command
    }

    /// One tmux call that is not part of the code under test.
    fn raw(&self, args: &[&str]) -> Result<String, DeckError> {
        crate::tmux::captured_output(&mut self.command(), args)
    }

    fn tmux(&self, args: &[&str]) -> Result<String, DeckError> {
        self.calls
            .borrow_mut()
            .push((args.iter().map(|s| s.to_string()).collect(), None));
        if let Some(answer) = self.intercept.borrow().as_ref().and_then(|f| f(args, None)) {
            return answer;
        }
        if args.first() == Some(&"new-session") {
            match *self.create.borrow() {
                "fail" => return Err(DeckError::new(ErrorKind::Tmux, "injected create failure")),
                "sh" => {
                    let mut other = args.to_vec();
                    other.push("/bin/sh");
                    return self.raw(&other);
                }
                _ => {}
            }
        }
        // the global-scope home is the last read of a passing readiness check
        if args.first() == Some(&"show-environment") && args.contains(&"-g") {
            if let Some(hook) = self.at_ready.borrow_mut().take() {
                hook();
            }
        }
        self.raw(args)
    }

    fn tmux_stdin(&self, args: &[&str], input: &[u8]) -> Result<String, DeckError> {
        self.calls.borrow_mut().push((
            args.iter().map(|s| s.to_string()).collect(),
            Some(input.to_vec()),
        ));
        if let Some(answer) = self
            .intercept
            .borrow()
            .as_ref()
            .and_then(|f| f(args, Some(input)))
        {
            return answer;
        }
        crate::tmux::command_with_stdin(&mut self.command(), args, input)
    }

    /// The PRODUCTION assembly of the re-entry environment
    /// (`tmux_lifecycle::assemble_reentry_env`) on this world's server: the
    /// real committed-Board reader, the real Board fence, the real shell
    /// creator and the account's login shell. Only the server, the stand-in
    /// capability, the helper path, the home and the two opt-in facts are
    /// this world's.
    fn with_env<R>(&self, work: impl FnOnce(&Env) -> R) -> R {
        let tmux = |args: &[&str]| self.tmux(args);
        let tmux_stdin = |args: &[&str], input: &[u8]| self.tmux_stdin(args, input);
        crate::tmux_lifecycle::assemble_reentry_env(
            &crate::tmux_lifecycle::ReentryParts {
                tmux: &tmux,
                tmux_stdin: &tmux_stdin,
                socket: &self.socket,
                capability: self.capability,
                helper: Some(self.helper.clone()),
                hooks_installed: self.observation.get(),
                home: &self.home,
                baseline: self.baseline.get(),
            },
            work,
        )
    }

    fn wait(&self, what: &str, done: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn lines(&self, file: &str) -> Vec<String> {
        // the fake client writes its own records into its HOME
        std::fs::read_to_string(self.home.join(file))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The argv every fake client recorded at its own start.
    fn launches(&self) -> Vec<String> {
        self.lines("launches.log")
    }

    /// A card with a live shell in its own directory, and — unless `launch`
    /// is `None` — a client started in it by typing `launch` (which may
    /// name `{agent}`), exactly as a user would.
    fn card(&self, name: &str, plan: &str, launch: Option<&str>) {
        let dir = self.base.join("cards").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fake-agent.plan"), plan).unwrap();
        self.raw(&["new-session", "-d", "-s", name, "-c", dir.to_str().unwrap()])
            .expect("card shell");
        self.edit_cards(|cards| cards.push((format!("card-{name}"), name.to_string())));
        self.wait("the card's shell prompt", || {
            self.raw(&["display-message", "-p", "-t", name, "#{bracket_paste_flag}"])
                .is_ok_and(|flag| flag.trim() == "1")
        });
        let Some(launch) = launch else { return };
        let ready = self.lines("ready.log").len();
        let line = launch.replace("{agent}", &format!("'{}'", self.agent.display()));
        self.raw(&["send-keys", "-t", name, "-l", &line]).unwrap();
        self.raw(&["send-keys", "-t", name, "Enter"]).unwrap();
        self.wait("the client to reach its input", || {
            self.lines("ready.log").len() > ready
        });
    }

    fn preview(&self) -> (String, serde_json::Value) {
        let view = self.with_env(preview).expect("preview");
        let json = serde_json::to_value(&view).unwrap();
        (json["token"].as_str().unwrap().to_string(), json)
    }

    fn approval(&self, token: &str, cards: &[&str], policy: Policy) -> Approval {
        Approval {
            token: token.to_string(),
            cards: cards.iter().map(|card| format!("card-{card}")).collect(),
            policy,
        }
    }

    fn restart_with(
        &self,
        request: &str,
        approval: Option<&Approval>,
        post: Duration,
        pause: &dyn Fn() -> Result<(), DeckError>,
        progress: &dyn Fn(&str, usize, usize),
    ) -> Result<(u32, u32, Option<ReentryResult>), DeckError> {
        self.restart_full(
            request,
            approval,
            post,
            false,
            &|| {},
            &|_| pause().map(|()| 0),
            progress,
        )
    }

    /// The production restart (`restart_tmux_server_inner`) on this server.
    #[allow(clippy::too_many_arguments)]
    fn restart_full(
        &self,
        request: &str,
        approval: Option<&Approval>,
        post: Duration,
        real_guards: bool,
        detach: &dyn Fn(),
        pause: &dyn Fn(&[String]) -> Result<usize, DeckError>,
        progress: &dyn Fn(&str, usize, usize),
    ) -> Result<(u32, u32, Option<ReentryResult>), DeckError> {
        self.with_env(|env| {
            crate::tmux_lifecycle::restart_for_tests(crate::tmux_lifecycle::TestRestart {
                run: &|args| self.tmux(args),
                socket_name: &self.socket,
                lifecycle_file: &self.base.join("tmux-lifecycle.json"),
                restore_shells: self.restore.get(),
                env,
                request_id: request,
                approval,
                post_budget: post,
                real_guards,
                detach,
                pause,
                progress,
            })
        })
        .map(|(old, reply)| {
            let (new, _replaced, result) = reply.parts();
            (old, new.unwrap_or_default(), result)
        })
    }

    fn restart(
        &self,
        request: &str,
        approval: &Approval,
    ) -> Result<(u32, u32, Option<ReentryResult>), DeckError> {
        self.restart_with(
            request,
            Some(approval),
            Duration::from_secs(8),
            &|| Ok(()),
            &|_, _, _| {},
        )
    }

    /// The command lines handed to the paste transport, in order.
    fn pasted(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            // a launch paste loads its private `deck-send-…` buffer; a
            // transcript restore loads another buffer and is not a launch
            .filter(|(args, _)| args.iter().any(|arg| arg.starts_with("deck-send-")))
            .filter_map(|(_, input)| input.as_ref())
            .map(|bytes| String::from_utf8(bytes.clone()).unwrap())
            .collect()
    }

    fn enters(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(args, _)| {
                args.iter()
                    .any(|arg| arg.contains("send-keys") && arg.ends_with("Enter"))
            })
            .count()
    }

    /// The pid of the process in the foreground of `session`'s pane.
    fn foreground_pid(&self, session: &str) -> u32 {
        let pane = self
            .raw(&["display-message", "-p", "-t", session, "#{pane_pid}"])
            .expect("the session's pane")
            .trim()
            .parse::<u32>()
            .expect("a pane pid");
        crate::agent_status::live_generation(pane)
            .expect("a foreground generation")
            .pid()
    }

    fn server_pid(&self) -> u32 {
        self.raw(&["display-message", "-p", "#{pid}"])
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    fn has_session(&self, name: &str) -> bool {
        self.raw(&["has-session", "-t", &format!("={name}")])
            .is_ok()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        if !self.owner {
            return;
        }
        let _ = self.raw(&["kill-server"]);
        let socket = Path::new("/tmp")
            .join(format!("tmux-{}", unsafe { libc::getuid() }))
            .join(&self.socket);
        let _ = std::fs::remove_file(socket);
        // the killed server's processes may still be writing for a moment
        for _ in 0..40 {
            if std::fs::remove_dir_all(&self.base).is_ok() || !self.base.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        reset_for_tests();
    }
}

fn entry<'a>(result: &'a ReentryResult, card: &str) -> &'a EntryView {
    result
        .entries
        .iter()
        .find(|entry| entry.card_id == format!("card-{card}"))
        .unwrap_or_else(|| panic!("entry for {card}"))
}

fn line(world: &World, extra: &str, id: &str) -> String {
    format!("'{}' {extra}--resume {id}", world.agent.display())
}

const BYPASS: &str = "--dangerously-skip-permissions";

/// The mixed three cards through the whole transaction, once per policy:
/// A started with bypass, B ordinary, C ordinary in plan mode.
fn mixed_three(policy: Policy) -> (Vec<String>, Vec<String>, ReentryResult) {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}"),
        Some(&format!("{{agent}} {BYPASS}")),
    );
    world.card("b", &format!("session={ID_B}"), Some("{agent}"));
    world.card(
        "c",
        &format!("session={ID_C}"),
        Some("{agent} --permission-mode plan"),
    );
    let (token, view) = world.preview();
    let profiles: Vec<&str> = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["launchProfile"].as_str().unwrap())
        .collect();
    assert_eq!(profiles, ["bypass", "ordinary", "ordinary"]);
    let text = view.to_string();
    for private in [ID_A, ID_B, ID_C, "fake_reentry_agent", "/tmp", "sha", "pid"] {
        assert!(!text.contains(private), "the preview leaks {private}");
    }
    let old = world.server_pid();
    let approval = world.approval(&token, &["a", "b", "c"], policy);
    let (was, now, result) = world.restart("req-mixed", &approval).expect("restart");
    assert_eq!(was, old);
    assert_ne!(now, old, "the server was replaced");
    let result = result.expect("a re-entry result");
    assert!(result.server_replaced);
    assert_eq!(result.state, OperationState::Complete);
    for card in ["a", "b", "c"] {
        let entry = entry(&result, card);
        assert_eq!(entry.outcome, Outcome::Confirmed, "{card}");
        assert!(entry.shell && entry.client_observed, "{card}");
    }
    // each new client recorded its own argv after the three original starts
    let launched = world.launches()[3..].to_vec();
    (world.pasted(), launched, result)
}

#[test]
fn mixed_three_cards_under_policy_a_are_all_resumed_without_any_override() {
    let (pasted, launched, result) = mixed_three(Policy::Ordinary);
    assert_eq!(pasted.len(), 3, "exactly one transport per entry");
    for (text, id) in pasted.iter().zip([ID_A, ID_B, ID_C]) {
        assert!(text.ends_with(&format!(" --resume {id}")), "{text}");
        assert!(!text.contains(BYPASS) && !text.contains("permission-mode"));
    }
    assert_eq!(
        launched,
        [ID_A, ID_B, ID_C].map(|id| format!("--resume {id}"))
    );
    assert!(result.entries.iter().all(|entry| entry.warnings.is_empty()));
}

#[test]
fn mixed_three_cards_under_policy_b_reapply_bypass_only_to_the_bypass_launch() {
    let (pasted, launched, result) = mixed_three(Policy::ReapplyBypass);
    assert_eq!(pasted.len(), 3);
    assert_eq!(
        launched,
        [
            format!("{BYPASS} --resume {ID_A}"),
            format!("--resume {ID_B}"),
            format!("--resume {ID_C}"),
        ]
    );
    assert!(pasted[0].contains(BYPASS));
    assert!(!pasted[1].contains(BYPASS) && !pasted[2].contains(BYPASS));
    assert_eq!(entry(&result, "a").warnings, [Warning::BypassReauthorized]);
    assert!(entry(&result, "b").warnings.is_empty());
    assert!(entry(&result, "c").warnings.is_empty());
}

/// One client switches A → B → C in place and exits at C. The request names
/// C: the id of the exit receipt, not the first id and not the last start.
#[test]
fn the_request_names_the_session_the_client_exited_in() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nswitch={ID_B},{ID_C}"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, new_server, result) = world.restart("req-final", &approval).expect("restart");
    let result = result.unwrap();
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted(), [line(&world, "", ID_C)]);
    assert_eq!(world.launches()[1..], [format!("--resume {ID_C}")]);
    // the receipt confirmed an entry and nothing else: no Signal observation
    // exists for the new pane, and nothing was sent after the one request
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].server_pid, new_server);
    assert!(crate::agent_status::projected(&rows[0]).is_none());
    assert_eq!(world.enters(), 1, "one Enter, none after the receipt");
    // the lifecycle file names the server only
    let disk = std::fs::read_to_string(world.base.join("tmux-lifecycle.json")).unwrap();
    for private in [
        ID_A,
        ID_B,
        ID_C,
        "resume",
        "card-a",
        "bypass",
        "fake_reentry_agent",
    ] {
        assert!(
            !disk.contains(private),
            "the lifecycle file holds {private}"
        );
    }
    // a repeated invoke with the same request id reads the stored result
    assert_eq!(known("req-final"), Some(result));
    assert!(!operation("req-other").known);
}

/// Two confirmed, one that never reported its exit: the restart succeeded,
/// the third card is simply left for the user with no shell and no launch.
#[test]
fn a_restart_succeeds_while_one_entry_is_skipped_for_a_missing_exit_receipt() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card(
        "b",
        &format!("session={ID_B}\nexit=no-end"),
        Some("{agent}"),
    );
    world.card("c", &format!("session={ID_C}\nexit=other"), Some("{agent}"));
    world.card("d", &format!("session={ID_X}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b", "c", "d"], Policy::Ordinary);
    let (old, new, result) = world.restart("req-partial", &approval).expect("restart");
    assert_ne!(old, new);
    let result = result.unwrap();
    assert!(result.server_replaced);
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert_eq!(entry(&result, "d").outcome, Outcome::Confirmed);
    for card in ["b", "c"] {
        let skipped = entry(&result, card);
        assert_eq!(skipped.outcome, Outcome::NotAttempted, "{card}");
        assert_eq!(skipped.reason, Some(Reason::FinalSessionUnconfirmed));
        assert!(
            !skipped.shell && !world.has_session(card),
            "no shell for {card}"
        );
    }
    assert_eq!(
        world.pasted(),
        [line(&world, "", ID_A), line(&world, "", ID_X)]
    );
}

/// The client reported its final session but did not go away. A receipt is
/// not a process exit: the restart keeps its existing failure before any
/// kill, and nothing is launched.
#[test]
fn a_reported_exit_with_a_live_client_is_the_existing_timeout_and_kills_nothing() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nexit=linger"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let old = world.server_pid();
    let error = world.restart("req-linger", &approval).unwrap_err();
    // the shared exit budget ends it; on a heavily loaded machine the
    // preparation budget may end it first — both are the existing pre-kill
    // failures and neither replaces the server
    assert!(
        ["tmux-restart-agent-timeout", "tmux-restart-timeout"].contains(&error.message()),
        "{}",
        error.message()
    );
    assert_eq!(world.server_pid(), old, "the server was not replaced");
    assert!(world.pasted().is_empty());
    let result = known("req-linger").expect("the operation is still readable");
    assert!(!result.server_replaced);
    assert_eq!(result.state, OperationState::Complete);
    assert_eq!(entry(&result, "a").outcome, Outcome::NotAttempted);
    assert_eq!(world.launches().len(), 1);
}

/// The queue pause is persisted before the kill; when it fails nothing is
/// killed and nothing is launched.
#[test]
fn a_failed_queue_pause_keeps_the_server_and_launches_nothing() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let old = world.server_pid();
    let error = world
        .restart_with(
            "req-pause",
            Some(&approval),
            Duration::from_secs(8),
            &|| Err(DeckError::new(ErrorKind::Other, "queue-save-failed")),
            &|_, _, _| {},
        )
        .unwrap_err();
    assert_eq!(error.message(), "queue-save-failed");
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
    assert!(!world
        .calls
        .borrow()
        .iter()
        .any(|(args, _)| args.first().is_some_and(|verb| verb == "kill-server")));
}

/// The pause comes before the kill, and a confirmed re-entry calls it once:
/// nothing in the post phase touches the queues again.
#[test]
fn the_pause_precedes_the_kill_and_a_confirmed_entry_never_touches_it_again() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let paused_at = RefCell::new(Vec::new());
    let (_, _, result) = world
        .restart_with(
            "req-order",
            Some(&approval),
            Duration::from_secs(8),
            &|| {
                paused_at.borrow_mut().push(world.calls.borrow().len());
                Ok(())
            },
            &|_, _, _| {},
        )
        .expect("restart");
    assert_eq!(entry(&result.unwrap(), "a").outcome, Outcome::Confirmed);
    let kill = world
        .calls
        .borrow()
        .iter()
        .position(|(args, _)| args[0] == "kill-server")
        .expect("the server was killed");
    assert_eq!(paused_at.borrow().len(), 1);
    assert!(paused_at.borrow()[0] <= kill);
}

/// No receipt within the share: the entry is unconfirmed, a client is seen
/// in the shell, and the request was sent exactly once.
#[test]
fn a_missing_receipt_is_unconfirmed_and_is_never_sent_again() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nresume=silent"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-silent",
            Some(&approval),
            Duration::from_secs(4),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    let silent = entry(&result, "a");
    assert_eq!(silent.outcome, Outcome::Unconfirmed);
    assert_eq!(silent.reason, Some(Reason::LaunchUnconfirmed));
    assert!(silent.shell && silent.client_observed);
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 2, "one original start, one resume");
    // the token is spent and the request id is taken: another invoke with
    // the same confirmation cannot start anything
    let again = world.restart("req-silent-2", &approval).unwrap_err();
    assert_eq!(again.message(), "reentry-preview-expired");
    assert_eq!(known("req-silent"), Some(result));
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.launches().len(), 2);
}

/// A resume receipt for another id, a start that is not a resume, and a
/// client that entered and left again: none of them confirms.
#[test]
fn a_receipt_for_another_id_or_source_or_a_client_that_left_does_not_confirm() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nresume=wrong:{ID_X}"),
        Some("{agent}"),
    );
    world.card(
        "b",
        &format!("session={ID_B}\nresume=startup"),
        Some("{agent}"),
    );
    world.card(
        "c",
        &format!("session={ID_C}\nresume=leave"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b", "c"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-wrong", &approval).expect("restart");
    let result = result.unwrap();
    assert!(result.server_replaced);
    for card in ["a", "b", "c"] {
        let unconfirmed = entry(&result, card);
        assert_eq!(unconfirmed.outcome, Outcome::Unconfirmed, "{card}");
        assert_eq!(unconfirmed.reason, Some(Reason::LaunchUnconfirmed));
    }
    assert_eq!(world.pasted().len(), 3);
    assert_eq!(world.enters(), 3);
}

/// The transport's three ambiguous or refusing answers, one per entry: an
/// atomic refusal (nothing pasted), a lost paste reply, a refused Enter.
/// None is retried; only the atomic refusal is known not to have launched.
#[test]
fn a_refused_lost_or_half_done_transport_is_never_retried() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card("b", &format!("session={ID_B}"), Some("{agent}"));
    world.card("c", &format!("session={ID_C}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b", "c"], Policy::Ordinary);
    let pastes = std::cell::Cell::new(0);
    *world.intercept.borrow_mut() = Some(Box::new(move |args, input| {
        if input.is_some() {
            pastes.set(pastes.get() + 1);
            return match pastes.get() {
                // the guard refuses: tmux pastes nothing
                1 => Some(Ok("deck-context-refused\n".into())),
                // the reply is lost: the paste may or may not have happened
                2 => Some(Err(DeckError::new(ErrorKind::Other, "reply lost"))),
                _ => None,
            };
        }
        let enter = args
            .iter()
            .any(|arg| arg.contains("send-keys") && arg.ends_with("Enter"));
        enter.then(|| Ok("deck-context-refused\n".into()))
    }));
    let (_, _, result) = world.restart("req-transport", &approval).expect("restart");
    let result = result.unwrap();
    assert!(result.server_replaced);
    let refused = entry(&result, "a");
    assert_eq!(refused.outcome, Outcome::KnownNoLaunchFailure);
    assert_eq!(refused.reason, Some(Reason::LaunchRefused));
    for card in ["b", "c"] {
        let unknown = entry(&result, card);
        assert_eq!(unknown.outcome, Outcome::Unconfirmed, "{card}");
        assert_eq!(unknown.reason, Some(Reason::LaunchUnconfirmed));
    }
    assert_eq!(world.pasted().len(), 3, "one paste attempt per entry");
    assert_eq!(
        world.enters(),
        1,
        "only the third entry reached Enter, once"
    );
    assert_eq!(world.launches().len(), 3, "no client was started");
    // every shell is kept for the user
    for card in ["a", "b", "c"] {
        assert!(world.has_session(card));
        assert!(entry(&result, card).shell);
    }
}

/// Approvals are bound to the previewed targets: a replaced client, a card
/// that was not shown, a repeated card, a spent or foreign token all refuse
/// before any exit key, and nothing is ever rebound.
#[test]
fn an_approval_that_no_longer_names_the_previewed_target_refuses_the_restart() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card("b", &format!("session={ID_B}"), None);
    let old = world.server_pid();
    let untouched = |world: &World| {
        assert_eq!(world.server_pid(), old);
        assert!(world.pasted().is_empty());
        assert_eq!(world.launches().len(), 1);
    };
    // a card that is a plain shell was never a candidate
    let (token, view) = world.preview();
    assert_eq!(view["entries"][1]["kind"], "shell");
    let error = world
        .restart(
            "r1",
            &world.approval(&token, &["a", "b"], Policy::ReapplyBypass),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    // that failure consumed the token
    let error = world
        .restart("r2", &world.approval(&token, &["a"], Policy::Ordinary))
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    for (request, cards) in [("r3", vec!["a", "a"]), ("r4", vec![]), ("r5", vec!["zz"])] {
        let (token, _) = world.preview();
        let error = world
            .restart(request, &world.approval(&token, &cards, Policy::Ordinary))
            .unwrap_err();
        assert_eq!(error.message(), "reentry-preview-expired", "{request}");
    }
    let (_, _) = world.preview();
    let error = world
        .restart(
            "r6",
            &world.approval("0123456789abcdef", &["a"], Policy::Ordinary),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    untouched(&world);
    // The previewed ordinary client is replaced by a bypass one in the same
    // pane. Policy B must not reach the new client: the approval is void.
    let (token, view) = world.preview();
    assert_eq!(view["entries"][0]["launchProfile"], "ordinary");
    world.raw(&["send-keys", "-t", "a", "C-d"]).unwrap();
    world.raw(&["send-keys", "-t", "a", "C-d"]).unwrap();
    world.wait("the first client to exit", || {
        world
            .raw(&["display-message", "-p", "-t", "a", "#{bracket_paste_flag}"])
            .is_ok_and(|flag| flag.trim() == "1")
    });
    let ready = world.lines("ready.log").len();
    let relaunch = format!("'{}' {BYPASS}", world.agent.display());
    world
        .raw(&["send-keys", "-t", "a", "-l", &relaunch])
        .unwrap();
    world.raw(&["send-keys", "-t", "a", "Enter"]).unwrap();
    world.wait("the second client", || {
        world.lines("ready.log").len() > ready
    });
    let error = world
        .restart("r7", &world.approval(&token, &["a"], Policy::ReapplyBypass))
        .unwrap_err();
    assert_eq!(error.message(), "tmux-server-impact-changed");
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
    assert_eq!(world.launches().len(), 2, "only the user's own two starts");
    // the card is rebound to another session: the approval is void as well
    let (token, _) = world.preview();
    world.edit_cards(|cards| {
        cards[0].1 = "b".into();
        cards[1].1 = "a".into();
    });
    let error = world
        .restart("r8", &world.approval(&token, &["a"], Policy::Ordinary))
        .unwrap_err();
    assert_eq!(error.message(), "tmux-server-impact-changed");
    assert_eq!(world.server_pid(), old);
}

/// What qualifies and what does not is read from the live client only: its
/// running code decides whose it is — under any file name and for any
/// content — and its launch image decides whether the shape is supported.
#[test]
fn only_a_direct_client_of_the_product_in_a_supported_launch_shape_is_a_candidate() {
    let world = World::new();
    // another build of the SAME product under the launcher's name: other
    // content, another digest, the same signer and product
    let other = world.base.join("other").join("fake_reentry_agent");
    agent_build(&other, "build-1", AGENT_IDENTIFIER);
    // the same program signed as ANOTHER product, under the launcher's name
    let foreign = world.base.join("foreign").join("fake_reentry_agent");
    agent_build(&foreign, "build-0", "io.c9r.deck.test.another-product");
    // the product's own build with bytes after its signature: no longer
    // intact signed code, though it still runs under that name
    let appended = world.base.join("appended").join("fake_reentry_agent");
    agent_build(&appended, "build-0", AGENT_IDENTIFIER);
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&appended)
            .unwrap()
            .write_all(b"another build")
            .unwrap();
    }
    world.card(
        "ok",
        "",
        Some(&format!("{{agent}} --model opus --resume {ID_A}")),
    );
    world.card("flag", "", Some("{agent} --unknown-flag"));
    world.card(
        "allow",
        "",
        Some("{agent} --allow-dangerously-skip-permissions"),
    );
    world.card(
        "mode",
        "",
        Some("{agent} --permission-mode bypassPermissions"),
    );
    world.card(
        "both",
        "",
        Some(&format!("{{agent}} {BYPASS} --permission-mode plan")),
    );
    world.card("prompt", "", Some("{agent} do-something"));
    world.card("env", "", Some("ANTHROPIC_BASE_URL=x {agent}"));
    world.card("config", "", Some("CLAUDE_CONFIG_DIR=/x {agent}"));
    world.card("wrapper", "", Some("/usr/bin/time {agent}"));
    world.card("build", "", Some(&format!("'{}'", other.display())));
    world.card("foreign", "", Some(&format!("'{}'", foreign.display())));
    world.card("appended", "", Some(&format!("'{}'", appended.display())));
    world.card(
        "dirs",
        "",
        Some(&format!("{{agent}} {BYPASS} --add-dir /a /b")),
    );
    world.card("shell", "", None);
    world.raw(&["split-window", "-d", "-t", "shell"]).unwrap();
    let (_, view) = world.preview();
    let read = |card: &str| -> (String, String, String) {
        let entry = view["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["cardId"] == format!("card-{card}"))
            .unwrap_or_else(|| panic!("{card} is previewed"));
        let text = |key: &str| entry[key].as_str().unwrap_or("-").to_string();
        (text("kind"), text("reason"), text("launchProfile"))
    };
    let cell = |kind: &str, reason: &str, profile: &str| {
        (kind.to_string(), reason.to_string(), profile.to_string())
    };
    // a supported exact resume is classified again; its id is dropped
    assert_eq!(read("ok"), cell("candidate", "-", "ordinary"));
    assert_eq!(read("dirs"), cell("candidate", "-", "bypass"));
    // another build of the product is the product: no list has to name it
    assert_eq!(read("build"), cell("candidate", "-", "ordinary"));
    // a wrapper is the foreground leader: the client is not a direct launch
    assert_eq!(
        read("wrapper"),
        cell("unsupported", "unsupported-agent", "-")
    );
    for card in ["flag", "allow", "mode", "both", "prompt", "env", "config"] {
        assert_eq!(
            read(card),
            cell("unsupported", "identity-unavailable", "-"),
            "{card}"
        );
    }
    // the launcher's name and a working lifecycle hook make nothing the
    // product: another signer's product, and a file that is no longer
    // intact signed code
    for card in ["foreign", "appended"] {
        assert_eq!(
            read(card),
            cell("unsupported", "identity-unavailable", "-"),
            "{card}"
        );
    }
    assert_eq!(read("shell"), cell("unsupported", "multiple-panes", "-"));
    // Without the observation opt-in the same sessions are still read as
    // what they are — the dialog names the setting that is off, not a session
    // it could not verify — and nothing can be confirmed: the view says the
    // observation is off and no token was kept for it.
    world.observation.set(false);
    let (token, view) = world.preview();
    assert_eq!(view["lifecycleObservation"], false);
    let kinds = |view: &serde_json::Value, card: &str| -> (String, String) {
        let entry = view["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["cardId"] == format!("card-{card}"))
            .unwrap_or_else(|| panic!("no entry for {card}"));
        (
            entry["kind"].as_str().unwrap_or_default().to_owned(),
            entry["reason"].as_str().unwrap_or("-").to_owned(),
        )
    };
    assert_eq!(kinds(&view, "ok"), ("candidate".to_owned(), "-".to_owned()));
    for card in ["foreign", "appended"] {
        assert_eq!(
            kinds(&view, card),
            ("unsupported".to_owned(), "identity-unavailable".to_owned()),
            "{card}: turning observation off makes nothing a candidate"
        );
    }
    let error = world
        .restart("r-off", &world.approval(&token, &["ok"], Policy::Ordinary))
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
}

/// A client that runs under its release's file name: tmux shows a version
/// and argv[0] is a path, so the name-based exit path does not know it, and
/// no name this feature holds matches it. What was frozen for that exact
/// pane — a client qualified by its running code — does, and the request
/// names that exact file, not anything `PATH` would find.
#[test]
fn a_client_under_its_version_name_is_exited_and_reentered_by_its_exact_file() {
    let world = World::new();
    let versions = world.base.join("versions");
    std::fs::create_dir_all(&versions).unwrap();
    let release = versions.join("2.1.294");
    std::fs::copy(&world.agent, &release).unwrap();
    let quoted = format!("'{}'", release.display());
    world.card(
        "a",
        &format!("session={ID_A}"),
        Some(&format!("{quoted} {BYPASS}")),
    );
    assert_eq!(
        world
            .raw(&[
                "display-message",
                "-p",
                "-t",
                "a",
                "#{pane_current_command}"
            ])
            .unwrap()
            .trim(),
        "2.1.294"
    );
    let (token, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
    let (_, _, result) = world.restart("req-version", &approval).expect("restart");
    let result = result.unwrap();
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert_eq!(
        world.pasted(),
        [format!("{quoted} {BYPASS} --resume {ID_A}")]
    );
    // the next restart starts from the default policy with no grant left:
    // the same client, now running the generated request, gets no override
    let (token, view) = world.preview();
    assert_eq!(view["entries"][0]["launchProfile"], "bypass");
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world
        .restart("req-version-2", &approval)
        .expect("second restart");
    assert_eq!(entry(&result.unwrap(), "a").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted()[1], format!("{quoted} --resume {ID_A}"));
    assert_eq!(
        world.launches(),
        [
            BYPASS.to_string(),
            format!("{BYPASS} --resume {ID_A}"),
            format!("--resume {ID_A}")
        ]
    );
}

/// Between the exit and the launch the frozen executable is replaced, or the
/// working directory disappears: nothing is started, and no other file or
/// directory is used instead.
#[test]
fn a_changed_executable_or_a_lost_directory_after_the_exit_is_never_launched() {
    let world = World::new();
    let release = world.base.join("versions").join("2.1.294");
    std::fs::create_dir_all(release.parent().unwrap()).unwrap();
    std::fs::copy(&world.agent, &release).unwrap();
    let quoted = format!("'{}'", release.display());
    world.card("exe", &format!("session={ID_A}"), Some(&quoted));
    world.card("dir", &format!("session={ID_B}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["exe", "dir"], Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-drift",
            Some(&approval),
            Duration::from_secs(8),
            &|| {
                // an "update" puts another, validly signed build of the same
                // product at the same path
                let staged = release.with_extension("new");
                agent_build(&staged, "build-updated", AGENT_IDENTIFIER);
                std::fs::rename(&staged, &release).unwrap();
                std::fs::remove_dir_all(world.base.join("cards").join("dir")).unwrap();
                Ok(())
            },
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    let exe = entry(&result, "exe");
    assert_eq!(exe.outcome, Outcome::NotAttempted);
    assert_eq!(exe.reason, Some(Reason::ExecutableUnavailable));
    let dir = entry(&result, "dir");
    assert_eq!(dir.outcome, Outcome::NotAttempted);
    assert_eq!(dir.reason, Some(Reason::UserActionRequired));
    assert!(world.pasted().is_empty());
    assert!(!world.has_session("exe") && !world.has_session("dir"));
    assert_eq!(world.launches().len(), 2);
}

/// A shell somebody else created under the card's name is not borrowed; a
/// shell that is not the certified login shell is never typed into; a shell
/// that cannot be created leaves nothing behind.
#[test]
fn a_foreign_unverified_or_missing_shell_is_never_typed_into() {
    for (mode, outcome, shell) in [
        ("race", Outcome::NotAttempted, true),
        ("sh", Outcome::RefusedBeforeLaunch, true),
        ("fail", Outcome::NotAttempted, false),
    ] {
        let world = World::new();
        world.card("a", &format!("session={ID_A}"), Some("{agent}"));
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::Ordinary);
        if mode != "race" {
            *world.create.borrow_mut() = mode;
        }
        let raced = std::cell::Cell::new(false);
        let (_, _, result) = world
            .restart_with(
                "req-shell",
                Some(&approval),
                Duration::from_secs(4),
                &|| Ok(()),
                &|phase, _, _| {
                    if mode == "race" && phase == "reopening" && !raced.replace(true) {
                        world.raw(&["new-session", "-d", "-s", "a"]).unwrap();
                    }
                },
            )
            .expect("the restart itself succeeded");
        let result = result.unwrap();
        assert!(result.server_replaced, "{mode}");
        let refused = entry(&result, "a");
        assert_eq!(refused.outcome, outcome, "{mode}");
        assert_eq!(refused.reason, Some(Reason::LaunchRefused), "{mode}");
        assert_eq!(refused.shell, shell, "{mode}");
        assert_eq!(world.has_session("a"), shell, "{mode}: the shell is kept");
        assert!(world.pasted().is_empty(), "{mode}");
        assert_eq!(world.enters(), 0, "{mode}");
        assert_eq!(world.launches().len(), 1, "{mode}");
    }
}

/// The replacement fails after the kill: there is no launch, and what stays
/// on disk for the next boot is the server intent alone. Forgetting the
/// process memory (a crash) leaves nothing that could launch.
#[test]
fn a_failed_replacement_launches_nothing_and_leaves_no_agent_data_for_the_next_boot() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
    *world.intercept.borrow_mut() = Some(Box::new(|args, _| {
        (args[0] == "start-server").then(|| {
            Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux start-server failed: injected",
            ))
        })
    }));
    let error = world.restart("req-crash", &approval).unwrap_err();
    assert_eq!(error.message(), "tmux start-server failed: injected");
    assert!(world.pasted().is_empty());
    let result = known("req-crash").unwrap();
    assert!(!result.server_replaced);
    assert_eq!(entry(&result, "a").outcome, Outcome::NotAttempted);
    let disk = std::fs::read_to_string(world.base.join("tmux-lifecycle.json")).unwrap();
    assert!(disk.contains("\"operation\""), "the server intent is kept");
    for private in [
        ID_A,
        "resume",
        "card-a",
        "bypass",
        "policy",
        "fake_reentry_agent",
    ] {
        assert!(!disk.contains(private), "the intent holds {private}");
    }
    // the process forgets everything: no plan, no token, no result to replay
    reset_for_tests();
    assert!(!operation("req-crash").known);
    assert!(!lifecycle::armed());
    *world.intercept.borrow_mut() = None;
    world
        .raw(&["start-server", ";", "set-option", "-g", "exit-empty", "off"])
        .unwrap();
    let error = world.restart("req-crash", &approval).unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    assert!(world.pasted().is_empty());
    assert_eq!(world.launches().len(), 1);
}

/// A real helper connection that does not come from the lease's client is a
/// claim without kernel support, whatever it says about pane and server.
#[test]
fn a_real_helper_that_is_not_the_leased_clients_child_is_refused() {
    use std::io::{Read, Write};
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    let row = &rows[0];
    let shell_birth = crate::procinfo::process_start(row.pane_pid).unwrap();
    let client = crate::agent_status::live_generation(row.pane_pid).unwrap();
    // our own listener, so the verdict of `receive` can be read directly
    // (the helper only accepts a socket file named exactly `status.sock`)
    std::fs::create_dir_all(world.base.join("probe")).unwrap();
    let path = world.base.join("probe").join("status.sock");
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let report = |lease: lifecycle::Lease| -> Result<(), &'static str> {
        lifecycle::begin(&world.socket, &world.helper);
        lifecycle::arm(0, lease);
        let mut child = Command::new(&world.helper)
            .args(["claude-code", "session-end"])
            .env_clear()
            // where an instrumented helper's counters belong (`cargo llvm-cov`)
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|file| ("LLVM_PROFILE_FILE", file)))
            .env("HOME", &world.base)
            .env("DECK_STATUS_SOCK", &path)
            .env(
                "TMUX",
                format!("/tmp/tmux-0/{},{},0", world.socket, row.server_pid),
            )
            .env("TMUX_PANE", &row.pane_id)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let payload = format!(r#"{{"session_id":"{ID_X}","reason":"prompt_input_exit"}}"#);
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
                Err(error) => panic!("the helper never connected: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        let wire = lifecycle::parse(text.trim_end(), |source| {
            (source == "claude-code").then_some(&claude::RECEIPTS)
        })
        .expect("the real helper writes a valid line");
        // as the receipt worker does: the lease the line names, then the
        // verification under the receipt's own deadline
        let deadline = Instant::now() + lifecycle::RECEIPT_BUDGET;
        let verdict = lifecycle::claim(wire).and_then(|claim| {
            let _budget = crate::session_runtime::Deadline::until(deadline);
            lifecycle::receive(
                claim,
                crate::procinfo::peer_pid(&stream),
                deadline,
                &lifecycle::Kernel::live(),
            )
        });
        drop(stream);
        let _ = child.wait();
        assert!(lifecycle::seal(0).is_empty(), "nothing was recorded");
        verdict
    };
    // the client's own executable: what an exit lease of it is bound to
    let executable = ExecutableRef::for_tests(&world.agent);
    // the exact leased pane and client, reported by a helper this test
    // process started: the pane's shell is not in its ancestry
    let exact = lifecycle::Lease::exit(
        row.server_pid,
        &row.pane_id,
        (row.pane_pid, shell_birth),
        (client.pid(), client.birth()),
        executable.clone(),
    );
    assert_eq!(report(exact).unwrap_err(), "lifecycle-foreign-pane");
    // a lease whose shell generation is not the live one (a reused pid)
    let reused = lifecycle::Lease::exit(
        row.server_pid,
        &row.pane_id,
        (row.pane_pid, (shell_birth.0 + 1, shell_birth.1)),
        (client.pid(), client.birth()),
        executable.clone(),
    );
    assert_eq!(report(reused).unwrap_err(), "lifecycle-foreign-pane");
    // a lease for another server generation that has the same pane id
    let other = lifecycle::Lease::exit(
        row.server_pid + 1,
        &row.pane_id,
        (row.pane_pid, shell_birth),
        (client.pid(), client.birth()),
        executable,
    );
    assert_eq!(report(other).unwrap_err(), "lifecycle-no-lease");
}

// ---------- follow-up 1: counterexamples for the independent review ---------------

/// A baseline that is KNOWN to be unsupported before the restart — what Deck
/// hands its server, or no lifecycle observation — is shown in the preview,
/// keeps no confirmable token and types nothing. The ordinary configuration
/// is not among them: the mixed-three tests above reach `Confirmed` through
/// this same production assembly.
#[test]
fn a_known_unsupported_baseline_is_shown_before_the_restart_and_types_nothing() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}"),
        Some(&format!("{{agent}} {BYPASS}")),
    );
    // the ordinary configuration offers the candidate
    let (_, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    assert_eq!(view["boardKnown"], true);
    assert_eq!(view["entries"][0]["launchProfile"], "bypass");
    world.baseline.set(false);
    let (token, view) = world.preview();
    assert_eq!(view["lifecycleObservation"], true);
    assert_eq!(view["entries"][0]["kind"], "unsupported");
    assert_eq!(view["entries"][0]["reason"], "identity-unavailable");
    let old = world.server_pid();
    let error = world
        .restart(
            "req-baseline",
            &world.approval(&token, &["a"], Policy::ReapplyBypass),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
    assert_eq!(world.launches().len(), 1);
    // a token made while the baseline was supported does not outlive it
    world.baseline.set(true);
    let (token, _) = world.preview();
    world.baseline.set(false);
    let error = world
        .restart(
            "req-baseline-2",
            &world.approval(&token, &["a"], Policy::Ordinary),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
}

/// The accepted window, and what happens after it. The new shell's startup
/// files change the home, the config root or a provider variable between
/// the two shells. That cannot be read before the request, so the request
/// IS typed — exactly once — and the started client is then found to be in
/// another launch environment: the entry is never confirmed, nothing is
/// typed again, and the client is left running for the user.
#[test]
fn a_startup_file_that_changes_the_launch_context_is_typed_once_and_never_confirmed() {
    for export in ["HOME", "CLAUDE_CONFIG_DIR", "ANTHROPIC_BASE_URL"] {
        let world = World::new();
        world.card("a", &format!("session={ID_A}"), Some("{agent}"));
        // another home in which the stand-in client still finds its helper,
        // so that it reports and the check itself is what decides
        let other_home = world.base.join("other-home");
        std::fs::create_dir_all(&other_home).unwrap();
        std::fs::copy(
            world.base.join("helper-path"),
            other_home.join("helper-path"),
        )
        .unwrap();
        let value = if export == "HOME" {
            other_home.to_str().unwrap().to_string()
        } else {
            "/elsewhere".to_string()
        };
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::Ordinary);
        let (_, _, result) = world
            .restart_with(
                "req-rc",
                Some(&approval),
                Duration::from_secs(8),
                &|| {
                    // the user's shell startup changes between the two shells
                    std::fs::write(
                        world.base.join(".zshenv"),
                        format!("export {export}={value}\n"),
                    )
                    .unwrap();
                    Ok(())
                },
                &|_, _, _| {},
            )
            .expect("the restart itself succeeded");
        let result = result.unwrap();
        assert!(result.server_replaced, "{export}: the replacement stands");
        let changed = entry(&result, "a");
        assert_eq!(changed.outcome, Outcome::Unconfirmed, "{export}");
        assert_eq!(changed.reason, Some(Reason::ContextChanged), "{export}");
        assert!(changed.shell && changed.client_observed && changed.settled);
        assert_eq!(world.pasted().len(), 1, "{export}: typed once, never again");
        assert_eq!(world.enters(), 1, "{export}: one Enter");
        // the started client is still there for the user: not killed
        assert!(world.foreground_is_child("a"), "{export}");
    }
}

/// The launch image of the started client cannot be read: no proof either
/// way, so no confirmation — and still one request only.
#[test]
fn an_unreadable_launch_image_after_the_start_is_unconfirmed() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    crate::procinfo::LAUNCH_IMAGE_UNREADABLE.with(|fault| fault.set(true));
    let settled = world.restart("req-unreadable", &approval);
    crate::procinfo::LAUNCH_IMAGE_UNREADABLE.with(|fault| fault.set(false));
    let (_, _, result) = settled.expect("the restart itself succeeded");
    let result = result.unwrap();
    let unread = entry(&result, "a");
    assert_eq!(unread.outcome, Outcome::Unconfirmed);
    assert_eq!(unread.reason, Some(Reason::ContextUnverified));
    assert!(unread.client_observed);
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.launches().len(), 2, "the client was started once");
    assert!(world.foreground_is_child("a"), "and is left running");
}

/// The right receipt from a client that was not started by this operation's
/// request. The client replaces its own image: once with another session id,
/// once with the SAME id and launch profile but one more argument. Both
/// report the wanted session; neither is confirmed.
#[test]
fn a_client_started_by_another_request_shape_is_not_confirmed_by_its_receipt() {
    for other in [
        format!("--resume {ID_X}"),
        format!("--resume {ID_A} --permission-mode plan"),
        format!("{BYPASS} --resume {ID_A}"),
    ] {
        let world = World::new();
        world.card(
            "a",
            &format!("session={ID_A}\nresume=reexec:{other}"),
            Some("{agent}"),
        );
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::Ordinary);
        let (_, _, result) = world.restart("req-shape", &approval).expect("restart");
        let result = result.unwrap();
        let reshaped = entry(&result, "a");
        assert_eq!(reshaped.outcome, Outcome::Unconfirmed, "{other}");
        assert_eq!(reshaped.reason, Some(Reason::ContextChanged), "{other}");
        assert!(
            reshaped.warnings.is_empty(),
            "{other}: no bypass was confirmed"
        );
        assert_eq!(world.pasted(), [line(&world, "", ID_A)], "{other}");
        // what Deck typed, then the image the client replaced itself with
        assert_eq!(
            world.launches()[1..],
            [format!("--resume {ID_A}"), other.clone()],
            "{other}"
        );
    }
}

/// The started client is not in the frozen working directory when looked at.
#[test]
fn a_client_in_another_directory_is_not_confirmed() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nresume=chdir"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-cwd", &approval).expect("restart");
    let result = result.unwrap();
    let moved = entry(&result, "a");
    assert_eq!(moved.outcome, Outcome::Unconfirmed);
    assert_eq!(moved.reason, Some(Reason::ContextChanged));
    assert_eq!(world.pasted().len(), 1);
}

/// A tmux runner that answers from a script and records what it was asked.
fn scripted_env<R>(
    home: &Path,
    answer: &dyn Fn(&[&str]) -> Result<String, DeckError>,
    work: impl FnOnce(&Env) -> R,
) -> R {
    scripted_env_for(&claude::CAPABILITY, home, answer, work)
}

/// `scripted_env` for a given capability.
fn scripted_env_for<R>(
    capability: &ReentryCapability,
    home: &Path,
    answer: &dyn Fn(&[&str]) -> Result<String, DeckError>,
    work: impl FnOnce(&Env) -> R,
) -> R {
    let refuse_stdin =
        |_: &[&str], _: &[u8]| Err(DeckError::new(ErrorKind::Tmux, "no stdin in this test"));
    let cards = || Some(Vec::new());
    let create = |_: &str, _: &Path, _: bool| -> Result<(), DeckError> { Ok(()) };
    let fenced = |work: &mut dyn FnMut()| work();
    work(&Env {
        tmux: answer,
        tmux_stdin: &refuse_stdin,
        socket: "deck-smoke-scripted",
        cards: &cards,
        capability,
        helper: Path::new("/nonexistent/helper"),
        home,
        login_shell: Path::new("/bin/zsh"),
        observation: true,
        baseline: true,
        fenced: &fenced,
        create_shell: &create,
    })
}

/// The pane's home by tmux's own inheritance rule, one row per answer: the
/// global scope is asked ONLY when the session scope positively does not
/// mention the variable; a removal, a failure, a timeout, a malformed or
/// ambiguous answer never falls back, and never passes.
#[test]
fn the_baseline_home_falls_back_to_the_global_scope_only_on_a_positive_absence() {
    let home = Path::new("/Users/u");
    let set = |value: &str| Ok(format!("HOME={value}\n"));
    let absent = || {
        Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux show-environment failed: unknown variable: HOME",
        ))
    };
    let failed = |text: &'static str| Err(DeckError::new(ErrorKind::Tmux, text));
    type Answer = Result<String, DeckError>;
    // (session scope, global scope, supported, global scope was asked)
    let rows: Vec<(Answer, Answer, bool, bool)> = vec![
        // Set
        (set("/Users/u"), set("/Users/other"), true, false),
        (set("/Users/other"), set("/Users/u"), false, false),
        (set(""), set("/Users/u"), false, false),
        // Removed: no fallback, although the global value would pass
        (Ok("-HOME\n".into()), set("/Users/u"), false, false),
        // Absent: the one case that asks the global scope
        (absent(), set("/Users/u"), true, true),
        (absent(), set("/Users/other"), false, true),
        (absent(), Ok("-HOME\n".into()), false, true),
        (absent(), absent(), false, true),
        (
            absent(),
            failed("tmux not runnable: timed out"),
            false,
            true,
        ),
        (
            absent(),
            Ok("HOME=/Users/u\nHOME=/Users/u\n".into()),
            false,
            true,
        ),
        // Unavailable: no fallback, although the global value would pass
        (
            failed("tmux show-environment failed: no such session: =a"),
            set("/Users/u"),
            false,
            false,
        ),
        (
            failed("tmux show-environment failed: no server running on /tmp/x"),
            set("/Users/u"),
            false,
            false,
        ),
        (
            Err(DeckError::restart(RestartFailure::Deadline)),
            set("/Users/u"),
            false,
            false,
        ),
        (Ok(String::new()), set("/Users/u"), false, false),
        (Ok("PATH=/usr/bin\n".into()), set("/Users/u"), false, false),
        (
            Ok("HOME=/Users/u\n-HOME\n".into()),
            set("/Users/u"),
            false,
            false,
        ),
    ];
    for (index, (session, global, supported, asked_global)) in rows.into_iter().enumerate() {
        let asked = RefCell::new(Vec::new());
        let (session, global) = (RefCell::new(Some(session)), RefCell::new(Some(global)));
        let answer = |args: &[&str]| -> Result<String, DeckError> {
            asked.borrow_mut().push(args.join(" "));
            let scope = if args.contains(&"-g") {
                &global
            } else {
                &session
            };
            scope
                .borrow_mut()
                .take()
                .expect("each scope is asked at most once")
        };
        let got = scripted_env(home, &answer, |env| launch::baseline_home(env, "a"));
        assert_eq!(got, supported, "row {index}");
        let asked = asked.borrow();
        assert_eq!(asked[0], "show-environment -t =a HOME", "row {index}");
        assert_eq!(
            asked.get(1).map(String::as_str),
            asked_global.then_some("show-environment -g HOME"),
            "row {index}"
        );
        assert!(
            asked.len() <= 2,
            "row {index}: one variable, by name, twice at most"
        );
    }
}

/// The same rule through the whole transaction on a real server: a session
/// that marks HOME removed, a session answer that fails while the global
/// scope would pass, an ambiguous answer and another home — each refuses
/// the launch before anything is typed, and only HOME was ever asked for.
#[test]
fn a_removed_unavailable_or_foreign_home_in_the_tmux_environment_is_not_the_baseline() {
    for (session_answer, global_answer) in [
        (Some(Ok("-HOME\n")), None),
        (Some(Err("tmux not runnable: timed out")), None),
        (Some(Ok("HOME=/somewhere\nHOME=/else\n")), None),
        (Some(Ok("HOME=/somewhere/else\n")), None),
        (None, Some(Ok("HOME=/somewhere/else\n"))),
        (
            None,
            Some(Err("tmux show-environment failed: no server running")),
        ),
    ] {
        let world = World::new();
        world.card("a", &format!("session={ID_A}"), Some("{agent}"));
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::Ordinary);
        let queried = std::rc::Rc::new(RefCell::new(Vec::new()));
        *world.intercept.borrow_mut() = Some(Box::new({
            let queried = queried.clone();
            move |args, _| {
                if args[0] != "show-environment" {
                    return None;
                }
                queried.borrow_mut().push(args.join(" "));
                let scripted: Option<Result<&str, &str>> = if args.contains(&"-g") {
                    global_answer
                } else {
                    session_answer
                };
                scripted.map(|answer| {
                    answer
                        .map(str::to_string)
                        .map_err(|text| DeckError::new(ErrorKind::Tmux, text))
                })
            }
        }));
        let (_, _, result) = world
            .restart_with(
                "req-home",
                Some(&approval),
                Duration::from_secs(4),
                &|| Ok(()),
                &|_, _, _| {},
            )
            .expect("the restart itself succeeded");
        let refused = entry(&result.unwrap(), "a").clone();
        assert_eq!(refused.outcome, Outcome::RefusedBeforeLaunch);
        assert!(world.pasted().is_empty());
        // one variable by name, never the whole environment
        assert!(!queried.borrow().is_empty());
        assert!(queried.borrow().iter().all(|call| call.ends_with(" HOME")));
        // a doubt in the session scope never asks the global scope
        if session_answer.is_some() {
            assert!(queried.borrow().iter().all(|call| !call.contains(" -g ")));
        }
    }
}

/// The shell is back in the foreground but the old exact client is still
/// alive (it stopped itself). That is not an exit: the restart fails before
/// the pause and the kill with the existing failure, and nothing launches.
#[test]
fn a_live_old_client_behind_a_foreground_shell_aborts_the_restart_before_any_kill() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nexit=suspend"),
        Some("{agent}"),
    );
    world.card("b", &format!("session={ID_B}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    let old = world.server_pid();
    let paused = std::cell::Cell::new(0);
    let error = world
        .restart_with(
            "req-suspended",
            Some(&approval),
            Duration::from_secs(8),
            &|| {
                paused.set(paused.get() + 1);
                Ok(())
            },
            &|_, _, _| {},
        )
        .unwrap_err();
    assert_eq!(error.message(), "tmux-restart-agent-timeout");
    assert_eq!(
        paused.get(),
        0,
        "the queues were not paused for a refused restart"
    );
    assert_eq!(world.server_pid(), old, "no replacement");
    assert!(!world
        .calls
        .borrow()
        .iter()
        .any(|(args, _)| args[0] == "kill-server" || args[0] == "start-server"));
    assert!(world.pasted().is_empty());
    assert_eq!(world.launches().len(), 2, "only the two original starts");
    let result = known("req-suspended").unwrap();
    assert!(!result.server_replaced);
    assert!(result
        .entries
        .iter()
        .all(|entry| entry.outcome == Outcome::NotAttempted && entry.settled));
}

/// The card is deleted after the freeze: no shell is created for it and
/// nothing is typed. The other approved card is unaffected.
#[test]
fn a_card_deleted_after_the_freeze_is_never_launched() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card("b", &format!("session={ID_B}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-deleted",
            Some(&approval),
            Duration::from_secs(8),
            &|| {
                world.edit_cards(|cards| cards.retain(|(card, _)| card != "card-a"));
                Ok(())
            },
            &|_, _, _| {},
        )
        .expect("restart");
    let result = result.unwrap();
    let deleted = entry(&result, "a");
    assert_eq!(deleted.outcome, Outcome::NotAttempted);
    assert_eq!(deleted.reason, Some(Reason::IdentityUnavailable));
    assert!(!deleted.shell && !world.has_session("a"));
    assert_eq!(entry(&result, "b").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted(), [line(&world, "", ID_B)]);
}

/// The card is rebound — by a real Board save — between the launch's own
/// checks and the input. The binding is read again under the Board writer's
/// real fence, immediately before the transport: the committed rebind is
/// seen and nothing is typed.
#[test]
fn a_rebind_between_the_launch_checks_and_the_input_types_nothing() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (file, rebound) = (
        world.board_file(),
        board_json(&[("card-a".into(), "elsewhere".into())]),
    );
    *world.at_ready.borrow_mut() = Some(Box::new(move || {
        crate::documents::test_door::save(&file, &rebound).expect("the rebind commits")
    }));
    let (_, _, result) = world.restart("req-rebound", &approval).expect("restart");
    let result = result.unwrap();
    let rebound = entry(&result, "a");
    assert_eq!(rebound.outcome, Outcome::RefusedBeforeLaunch);
    assert_eq!(rebound.reason, Some(Reason::IdentityUnavailable));
    assert!(rebound.shell, "the shell that was created is kept");
    assert!(world.pasted().is_empty());
    assert_eq!(world.enters(), 0);
    assert_eq!(world.launches().len(), 1);
}

/// A real Board save that arrives while the input is being made waits for
/// the real fence (`documents::board_fence`): it commits after the paste and
/// the Enter, never between the binding check and them.
#[test]
fn a_board_commit_during_the_input_is_serialized_after_it() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    // when the save began and committed, and when the paste began and the
    // Enter — the input's last transport call — had returned
    let began = std::sync::Arc::new(Mutex::new(None::<Instant>));
    let committed = std::sync::Arc::new(Mutex::new(None::<Instant>));
    let writer = std::rc::Rc::new(RefCell::new(None::<std::thread::JoinHandle<()>>));
    let (pasting, entered) = (
        std::rc::Rc::new(std::cell::Cell::new(None::<Instant>)),
        std::rc::Rc::new(std::cell::Cell::new(None::<Instant>)),
    );
    let fence_held_at_paste = std::rc::Rc::new(std::cell::Cell::new(false));
    *world.intercept.borrow_mut() = Some(Box::new({
        let (began, committed, writer) = (began.clone(), committed.clone(), writer.clone());
        let (pasting, entered) = (pasting.clone(), entered.clone());
        let fence_held_at_paste = fence_held_at_paste.clone();
        let run = world.remote();
        let (file, rebound) = (
            world.board_file(),
            board_json(&[("card-a".into(), "elsewhere".into())]),
        );
        move |args, input| {
            let send = args.iter().any(|arg| arg.starts_with("deck-send-"));
            if send && input.is_some() {
                pasting.set(Some(Instant::now()));
                fence_held_at_paste.set(crate::documents::board_fence_busy());
                // a Board writer starts its save now, while the fence is held
                let (began, committed) = (began.clone(), committed.clone());
                let (file, rebound) = (file.clone(), rebound.clone());
                *writer.borrow_mut() = Some(std::thread::spawn(move || {
                    *began.lock_or_recover() = Some(Instant::now());
                    crate::documents::test_door::save(&file, &rebound).expect("the save commits");
                    *committed.lock_or_recover() = Some(Instant::now());
                }));
                // give it every chance to get in before the input
                std::thread::sleep(Duration::from_millis(150));
            } else if args
                .iter()
                .any(|arg| arg.contains("send-keys") && arg.ends_with("Enter"))
            {
                // the real Enter, and the instant it had returned
                let sent = run(args);
                entered.set(Some(Instant::now()));
                return Some(sent);
            }
            None
        }
    }));
    let (_, _, result) = world.restart("req-commit", &approval).expect("restart");
    writer.borrow_mut().take().unwrap().join().unwrap();
    let result = result.unwrap();
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted().len(), 1);
    assert!(
        fence_held_at_paste.get(),
        "the input is made under the real fence"
    );
    let (began, committed) = (
        began.lock_or_recover().expect("the save began"),
        committed.lock_or_recover().expect("the save committed"),
    );
    let (pasting, entered) = (
        pasting.get().unwrap(),
        entered.get().expect("the Enter was sent"),
    );
    assert!(
        pasting <= began && began < entered,
        "the save began during the input"
    );
    assert!(
        committed >= entered,
        "the save committed only after the input finished"
    );
    // and it did commit: the Board now binds the card elsewhere
    let board = crate::documents::board_authority().expect("a current Board");
    assert_eq!(board["cards"][0]["session"], "elsewhere");
}

/// After the shell was verified ready, a child with the SAME name takes the
/// foreground. tmux's name guard still matches; the kernel's exact
/// generation does not: nothing is pasted.
#[test]
fn a_same_named_foreground_started_after_the_ready_check_gets_no_input() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    *world.at_ready.borrow_mut() = Some(Box::new({
        let run = world.remote();
        move || {
            run(&["send-keys", "-t", "a", "-l", "zsh -f"]).unwrap();
            run(&["send-keys", "-t", "a", "Enter"]).unwrap();
            // until the child shell shows its own prompt: same name, paste mode on
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                let state = run(&[
                    "display-message",
                    "-p",
                    "-t",
                    "a",
                    "#{pane_pid} #{bracket_paste_flag} #{pane_current_command}",
                ])
                .unwrap();
                let mut words = state.split_whitespace();
                let pane: u32 = words.next().unwrap().parse().unwrap();
                let child = crate::agent_status::live_generation(pane)
                    .is_some_and(|foreground| foreground.pid() != pane);
                if child && words.next() == Some("1") && words.next() == Some("zsh") {
                    break;
                }
                assert!(Instant::now() < until, "the child shell came up");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }));
    let (_, _, result) = world.restart("req-same-name", &approval).expect("restart");
    let result = result.unwrap();
    let refused = entry(&result, "a");
    assert_eq!(refused.outcome, Outcome::KnownNoLaunchFailure);
    assert_eq!(refused.reason, Some(Reason::LaunchRefused));
    assert!(world.pasted().is_empty(), "no command reached the pane");
    assert_eq!(world.enters(), 0);
    assert_eq!(world.launches().len(), 1);
    assert!(world.foreground_is_child("a"));
}

/// The request was pasted; before the Enter a same-named child takes the
/// foreground. No Enter is added, the entry stays unknown, nothing is retried.
#[test]
fn a_same_named_child_between_paste_and_enter_gets_no_enter() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    *world.intercept.borrow_mut() = Some(Box::new({
        let run = world.remote();
        let (base, socket) = (world.base.clone(), world.socket.clone());
        move |args, input| {
            let input = input?;
            if !args.iter().any(|arg| arg.starts_with("deck-send-")) {
                return None;
            }
            // the real paste first
            let mut command = Command::new(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin"),
            );
            command
                .env_clear()
                .env("HOME", &base)
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .args(["-f", "/dev/null", "-L", &socket]);
            let pasted = crate::tmux::command_with_stdin(&mut command, args, input);
            // then the line is replaced and a same-named child is started
            run(&["send-keys", "-t", "a", "C-u"]).unwrap();
            run(&["send-keys", "-t", "a", "-l", "zsh -f"]).unwrap();
            run(&["send-keys", "-t", "a", "Enter"]).unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                let pane: u32 = run(&["display-message", "-p", "-t", "a", "#{pane_pid}"])
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if crate::agent_status::live_generation(pane)
                    .is_some_and(|foreground| foreground.pid() != pane)
                {
                    break;
                }
                assert!(Instant::now() < until, "the child shell came up");
                std::thread::sleep(Duration::from_millis(20));
            }
            Some(pasted)
        }
    }));
    let (_, _, result) = world.restart("req-mid", &approval).expect("restart");
    let result = result.unwrap();
    let unknown = entry(&result, "a");
    assert_eq!(unknown.outcome, Outcome::Unconfirmed);
    assert_eq!(unknown.reason, Some(Reason::LaunchUnconfirmed));
    assert_eq!(world.pasted().len(), 1, "one paste, no retry");
    assert_eq!(world.enters(), 0, "no Enter was added");
    assert_eq!(world.launches().len(), 1, "no client was started");
}

/// The client entered the session and was gone at once, its end receipt
/// never sent. The resume receipt is history: at completion that client is
/// no longer the pane's foreground, so no unambiguous success is shown.
#[test]
fn a_client_that_entered_and_vanished_without_an_end_is_not_confirmed() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nresume=vanish"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-vanish", &approval).expect("restart");
    let result = result.unwrap();
    let vanished = entry(&result, "a");
    assert_eq!(vanished.outcome, Outcome::Unconfirmed);
    assert_eq!(vanished.reason, Some(Reason::LaunchUnconfirmed));
    assert!(!vanished.client_observed);
    assert_eq!(world.pasted().len(), 1);
}

/// The client entered the session and then left the foreground while still
/// alive (it stopped itself): its launch facts would still match, but it is
/// not the pane's foreground generation any more, so it is not shown as an
/// entry — the receipt is history, not presence.
#[test]
fn a_client_that_entered_and_left_the_foreground_alive_is_not_confirmed() {
    let world = World::new();
    world.card(
        "a",
        &format!("session={ID_A}\nresume=stop"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-stopped", &approval).expect("restart");
    let result = result.unwrap();
    let stopped = entry(&result, "a");
    assert_eq!(stopped.outcome, Outcome::Unconfirmed);
    assert_eq!(stopped.reason, Some(Reason::LaunchUnconfirmed));
    assert!(!stopped.client_observed, "the shell holds the foreground");
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.launches().len(), 2, "the client was started once");
}

/// The production restart with the REAL gates, PTY state and queue pause on
/// a private server: a delivery in flight refuses it as busy, and without
/// one it replaces the server.
#[test]
fn the_production_restart_takes_the_real_gates_detaches_and_pauses() {
    let world = World::new();
    world.card("a", "", None);
    let pty = crate::pty::PtyState::default();
    let queues = crate::scheduler::Queues::new(Default::default());
    let _scope = crate::session_runtime::test_activity_scope();
    let run = |world: &World| {
        world.restart_full(
            "req-real",
            None,
            Duration::from_secs(1),
            true,
            &|| pty.detach_all(),
            &|sessions| crate::scheduler::pause_for_server_restart(&queues, sessions),
            &|_, _, _| {},
        )
    };
    let old = world.server_pid();
    {
        // a delivery holds the shared activity gate
        let _delivery = crate::session_runtime::activity_guard().expect("shared activity");
        assert_eq!(run(&world).unwrap_err().message(), "tmux-restart-busy");
        assert_eq!(world.server_pid(), old);
    }
    let (was, now, result) = run(&world).expect("restart with the real gates");
    assert_eq!(was, old);
    assert_ne!(now, old);
    assert!(
        result.is_none(),
        "a shell-only restart has no re-entry result"
    );
    // both gates were released
    assert!(crate::session_runtime::activity_guard().is_ok());
}

// ---------- follow-up 2: the production pieces, not stand-ins ---------------------

/// The production assembly itself: its Board reader is the committed Board
/// as AUTHORITY, and its fenced section holds the real Board fence. A Board
/// that is not the user's current version — none loaded, or one answered
/// from the backup — names no card, so nothing can be previewed or bound.
#[test]
fn the_production_assembly_reads_the_committed_board_under_the_real_fence() {
    use crate::documents::test_door;
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.with_env(|env| {
        assert_eq!((env.cards)(), Some(vec![("card-a".into(), "a".into())]));
        assert!(!crate::documents::board_fence_busy());
        let mut inside = None;
        (env.fenced)(&mut || {
            inside = Some((crate::documents::board_fence_busy(), (env.cards)()));
        });
        let (held, cards) = inside.expect("the fenced work ran");
        assert!(held, "the fenced section holds documents::board_fence");
        assert_eq!(cards, Some(vec![("card-a".into(), "a".into())]));
        assert!(!crate::documents::board_fence_busy(), "and releases it");
        assert!(env.observation && env.baseline);
        assert_eq!(env.login_shell, Path::new("/bin/zsh"));
    });
    // a real save is what the reader sees next
    world.edit_cards(|cards| cards[0].1 = "elsewhere".into());
    world.with_env(|env| {
        assert_eq!(
            (env.cards)(),
            Some(vec![("card-a".into(), "elsewhere".into())])
        );
    });
    world.edit_cards(|cards| cards[0].1 = "a".into());
    let (_, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    // a new process that has not loaded its Board yet: no authority
    test_door::restart();
    world.with_env(|env| assert_eq!((env.cards)(), None));
    let (token, view) = world.preview();
    assert_eq!(view["entries"].as_array().unwrap().len(), 0);
    assert_eq!(
        view["boardKnown"], false,
        "unknown, never \"nothing to reopen\""
    );
    let old = world.server_pid();
    let error = world
        .restart(
            "req-no-board",
            &world.approval(&token, &["a"], Policy::Ordinary),
        )
        .unwrap_err();
    // no Board to bind the approval to: refused before any exit key
    assert_eq!(error.message(), "tmux-server-impact-changed");
    // a Board answered from the backup is recovery material, not authority
    std::fs::write(world.board_file(), "{ damaged").unwrap();
    let (_, source, _) = test_door::load(&world.board_file()).unwrap().unwrap();
    assert_eq!(source, "backup");
    world.with_env(|env| assert_eq!((env.cards)(), None));
    let (_, recovered) = world.preview();
    assert_eq!(recovered["entries"].as_array().unwrap().len(), 0);
    assert_eq!(recovered["boardKnown"], false);
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
    // without the opt-in there is no observation, whatever helper exists
    world.observation.set(false);
    world.with_env(|env| assert!(!env.observation));
}

/// (session, paused) of every row of the queue document at `path`, through
/// the typed loader a boot uses. Empty when there is no such document.
fn queue_rows_at(path: &Path) -> Vec<(String, bool)> {
    crate::storage::read_typed::<crate::scheduler::QueueState>(path)
        .ok()
        .flatten()
        .and_then(|loaded| {
            serde_json::from_str::<crate::scheduler::QueueState>(&loaded.payload).ok()
        })
        .map(|state| crate::scheduler::paused_for_tests(&state))
        .unwrap_or_default()
}

/// The real queue pause in the combined transaction, with rows waiting: the
/// row of a session on the restarted server is paused ON DISK before the
/// kill, a row of a session elsewhere is untouched, and a confirmed
/// re-entry leaves the pause exactly as it is — nothing is released and
/// nothing but the one request is typed.
#[test]
fn a_waiting_queue_row_is_paused_on_disk_before_the_kill_and_stays_paused_after_reentry() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let queues =
        crate::scheduler::Queues::new(crate::scheduler::waiting_for_tests(&["a", "elsewhere"]));
    let on_disk = || queue_rows_at(&crate::scheduler::queue_path());
    let paused_a = [("a".to_string(), true), ("elsewhere".to_string(), false)];
    let at_kill = std::rc::Rc::new(RefCell::new(None));
    *world.intercept.borrow_mut() = Some(Box::new({
        let at_kill = at_kill.clone();
        move |args, _| {
            if args.first() == Some(&"kill-server") {
                *at_kill.borrow_mut() = Some(on_disk());
            }
            None
        }
    }));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let (_, _, result) = world
        .restart_full(
            "req-queue",
            Some(&approval),
            Duration::from_secs(8),
            false,
            &|| {},
            &|sessions| crate::scheduler::pause_for_server_restart(&queues, sessions),
            &|_, _, _| {},
        )
        .expect("restart");
    assert_eq!(
        at_kill.borrow().as_deref(),
        Some(&paused_a[..]),
        "the pause was durable before the old server was stopped"
    );
    let result = result.unwrap();
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    // after a confirmed entry: still paused, in memory and on disk
    assert_eq!(
        crate::scheduler::paused_for_tests(&queues.q.lock_or_recover()),
        paused_a
    );
    assert_eq!(on_disk(), paused_a);
    assert_eq!(world.pasted(), [line(&world, "", ID_A)], "one request only");
    assert_eq!(world.enters(), 1);
}

/// The ordinary transcript restore fails part-way while the shell for an
/// approved entry is created. Whatever the failure point, the entry ends
/// with exactly ONE shell (the one that exists is kept, a missing one is
/// created clean), the private restore buffer is not left behind, and the
/// launch still happens exactly once.
#[test]
fn a_transcript_restore_that_fails_part_way_keeps_one_shell_and_one_request() {
    for failure in ["emit", "after-create", "before-create"] {
        let world = World::new();
        world.restore.set(true);
        world.card("tr", &format!("session={ID_A}"), Some("{agent}"));
        let restore_calls = std::rc::Rc::new(std::cell::Cell::new(0));
        *world.intercept.borrow_mut() = Some(Box::new({
            let run = world.remote();
            let (base, socket) = (world.base.clone(), world.socket.clone());
            let restore_calls = restore_calls.clone();
            move |args, input| {
                let restoring = args.iter().any(|arg| arg.starts_with("deck-restore-"));
                let fail = || {
                    Some(Err(DeckError::new(
                        ErrorKind::Tmux,
                        "injected restore failure",
                    )))
                };
                match (failure, input) {
                    // the transcript could not be written to the new pane
                    ("emit", None) if restoring && args.first() == Some(&"save-buffer") => {
                        restore_calls.set(restore_calls.get() + 1);
                        fail()
                    }
                    // the batch that loads the transcript and creates the
                    // pane reports a failure after the pane exists …
                    ("after-create", Some(input)) if restoring => {
                        restore_calls.set(restore_calls.get() + 1);
                        let mut command = Command::new(
                            Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("binaries/tmux-aarch64-apple-darwin"),
                        );
                        command
                            .env_clear()
                            .env("HOME", &base)
                            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                            .env("TERM", "xterm-256color")
                            .env("SHELL", "/bin/zsh")
                            .args(["-f", "/dev/null", "-L", &socket]);
                        let _ = crate::tmux::command_with_stdin(&mut command, args, input);
                        fail()
                    }
                    // … or before any pane exists
                    ("before-create", Some(_)) if restoring => {
                        restore_calls.set(restore_calls.get() + 1);
                        fail()
                    }
                    _ => {
                        let _ = &run;
                        None
                    }
                }
            }
        }));
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["tr"], Policy::Ordinary);
        let (_, _, result) = world.restart("req-restore", &approval).expect("restart");
        let result = result.unwrap();
        assert_eq!(
            restore_calls.get(),
            1,
            "{failure}: the restore was attempted and failed where intended"
        );
        let restored = entry(&result, "tr");
        assert_eq!(restored.outcome, Outcome::Confirmed, "{failure}");
        assert!(restored.shell, "{failure}: the shell is kept");
        let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
        assert_eq!(rows.len(), 1, "{failure}: exactly one shell, one pane");
        assert_eq!(rows[0].session_name, "tr");
        assert_eq!(world.pasted(), [line(&world, "", ID_A)], "{failure}");
        assert_eq!(world.enters(), 1, "{failure}: one Enter");
        assert_eq!(world.launches().len(), 2, "{failure}: one new client");
        let buffers = world.raw(&["list-buffers"]).unwrap_or_default();
        assert!(
            !buffers.contains("deck-restore-") && !buffers.contains("deck-send-"),
            "{failure}: no private buffer is left behind"
        );
        let _ = crate::shell_state::clear_snapshot("tr");
    }
}

/// With the restore working, the transcript is written before the request
/// and the request is still the only command typed.
#[test]
fn a_restored_transcript_precedes_the_one_request() {
    let world = World::new();
    world.restore.set(true);
    world.card("tr", &format!("session={ID_A}"), Some("{agent}"));
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["tr"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-restored", &approval).expect("restart");
    assert_eq!(entry(&result.unwrap(), "tr").outcome, Outcome::Confirmed);
    let calls = world.calls.borrow();
    let position = |what: &dyn Fn(&Call) -> bool| calls.iter().position(what);
    let restored = position(&|(args, _)| args.first().is_some_and(|verb| verb == "save-buffer"))
        .expect("the transcript was written to the new pane");
    let requested = position(&|(args, input)| {
        input.is_some() && args.iter().any(|arg| arg.starts_with("deck-send-"))
    })
    .expect("the request was pasted");
    assert!(restored < requested);
    drop(calls);
    assert_eq!(world.pasted(), [line(&world, "", ID_A)]);
    assert_eq!(world.enters(), 1);
    let _ = crate::shell_state::clear_snapshot("tr");
}

// ---------- forward compatibility: no release is named ---------------------------
//
// The stand-in builds below differ in the label of their file, in their
// content and so in their digest and code directory. "Known" is only the
// name of a label that was once on a list (2.1.296): nothing at runtime
// knows it, none of these files is that release, and a passing run is no
// native evidence about any release.

/// One build of the stand-in under a release-like file name of `world`.
fn release(world: &World, label: &str, tag: &str) -> PathBuf {
    let path = world.base.join("versions").join(label);
    agent_build(&path, tag, AGENT_IDENTIFIER);
    path
}

/// A label that was once listed, and a build tag for it.
const KNOWN: (&str, &str) = ("2.1.296", "build-known");
/// A label, a file name and a content that no list anywhere names.
const NEW: (&str, &str) = ("9.9.999", "build-new");

/// The queue rows of a file: (session, paused).
type QueueRows = Vec<(String, bool)>;

/// What one restart over a bypass launch (card `a`) and an ordinary launch
/// (card `b`) of ONE build did, as far as a test may know it.
struct Run {
    digest: String,
    /// The launch profile the preview showed for each card.
    profiles: Vec<String>,
    /// The lines handed to the paste transport, the executable as `<exe>`.
    requests: Vec<String>,
    /// The arguments each NEW client recorded for itself.
    launched: Vec<String>,
    enters: usize,
    result: ReentryResult,
    /// Every card's pane is on the replaced server with a client in front.
    new_generation: bool,
    /// The queue rows on disk when the old server was killed, and at the end.
    queue: (QueueRows, QueueRows),
    /// The result as it crosses IPC.
    json: String,
}

fn bypass_and_ordinary((label, tag): (&str, &str), policy: Policy) -> Run {
    let world = World::new();
    let binary = release(&world, label, tag);
    let quoted = format!("'{}'", binary.display());
    world.card(
        "a",
        &format!("session={ID_A}"),
        Some(&format!("{quoted} {BYPASS}")),
    );
    world.card("b", &format!("session={ID_B}"), Some(&quoted));
    let queues = crate::scheduler::Queues::new(crate::scheduler::waiting_for_tests(&["a", "b"]));
    let on_disk = || queue_rows_at(&crate::scheduler::queue_path());
    let at_kill = std::rc::Rc::new(RefCell::new(Vec::new()));
    *world.intercept.borrow_mut() = Some(Box::new({
        let at_kill = at_kill.clone();
        move |args, _| {
            if args.first() == Some(&"kill-server") {
                *at_kill.borrow_mut() = on_disk();
            }
            None
        }
    }));
    let (token, view) = world.preview();
    let profiles = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            assert_eq!(entry["kind"], "candidate", "{label}");
            entry["launchProfile"].as_str().unwrap().to_string()
        })
        .collect();
    let old = world.server_pid();
    let approval = world.approval(&token, &["a", "b"], policy);
    let (was, now, result) = world
        .restart_full(
            "req-run",
            Some(&approval),
            Duration::from_secs(8),
            false,
            &|| {},
            &|sessions| crate::scheduler::pause_for_server_restart(&queues, sessions),
            &|_, _, _| {},
        )
        .expect("restart");
    assert_eq!(was, old);
    assert_ne!(now, old, "the server was replaced");
    let result = result.expect("a re-entry result");
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    let run = Run {
        digest: ExecutableRef::for_tests(&binary).digest_hex(),
        profiles,
        requests: world
            .pasted()
            .iter()
            .map(|line| line.replace(&quoted, "<exe>"))
            .collect(),
        launched: world.launches()[2..].to_vec(),
        enters: world.enters(),
        new_generation: rows.len() == 2
            && rows
                .iter()
                .all(|row| row.server_pid == now && world.foreground_is_child(&row.session_name)),
        queue: (at_kill.borrow().clone(), on_disk()),
        json: serde_json::to_string(&result).unwrap(),
        result,
    };
    assert_eq!(
        crate::scheduler::paused_for_tests(&queues.q.lock_or_recover()),
        run.queue.1,
        "memory and disk agree"
    );
    run
}

/// The one expectation every build is held to, under either policy.
fn assert_reentered(run: &Run, policy: Policy) {
    let bypass = policy == Policy::ReapplyBypass;
    let flag = if bypass {
        format!("{BYPASS} ")
    } else {
        String::new()
    };
    assert_eq!(run.profiles, ["bypass", "ordinary"]);
    // the exact words, in order, once per entry — and what each client got
    assert_eq!(
        run.requests,
        [
            format!("<exe> {flag}--resume {ID_A}"),
            format!("<exe> --resume {ID_B}")
        ]
    );
    assert_eq!(
        run.launched,
        [format!("{flag}--resume {ID_A}"), format!("--resume {ID_B}")]
    );
    assert_eq!(run.enters, 2, "one Enter per entry, none after a receipt");
    for text in run.requests.iter().chain(&run.launched) {
        for never in [
            "--permission-mode",
            "default",
            "--model",
            "--add-dir",
            "--continue",
            "SENTINEL",
        ] {
            assert!(!text.contains(never), "{text}: {never}");
        }
        assert!(bypass || !text.contains(BYPASS), "{text}");
    }
    assert!(run.result.server_replaced);
    assert_eq!(run.result.state, OperationState::Complete);
    for card in ["a", "b"] {
        let entry = entry(&run.result, card);
        assert_eq!(entry.outcome, Outcome::Confirmed, "{card}");
        assert!(
            entry.shell && entry.client_observed && entry.settled,
            "{card}"
        );
    }
    let expected: &[Warning] = if bypass {
        &[Warning::BypassReauthorized]
    } else {
        &[]
    };
    assert_eq!(entry(&run.result, "a").warnings, expected);
    assert!(entry(&run.result, "b").warnings.is_empty());
    assert!(run.new_generation, "the clients are the new generation's");
    // paused before the kill and still paused: a confirmed entry releases nothing
    let paused = [("a".to_string(), true), ("b".to_string(), true)];
    assert_eq!(run.queue.0, paused);
    assert_eq!(run.queue.1, paused);
    // Deck says that a flag was sent — never that a permission is in effect —
    // and names no id, digest or release
    for never in [
        "permission",
        "effective",
        "verified",
        "restored",
        "digest",
        "sha",
        "version",
        ID_A,
        ID_B,
        run.digest.as_str(),
    ] {
        assert!(!run.json.contains(never), "the result says {never}");
    }
}

#[test]
fn known_client_reenters_under_policy_a() {
    assert_reentered(
        &bypass_and_ordinary(KNOWN, Policy::Ordinary),
        Policy::Ordinary,
    );
}

/// The flag is sent again, before `--resume`. What permission is then in
/// effect is not something any result of Deck's says.
#[test]
fn known_bypass_launch_is_reauthorized_under_policy_b() {
    assert_reentered(
        &bypass_and_ordinary(KNOWN, Policy::ReapplyBypass),
        Policy::ReapplyBypass,
    );
}

/// A build no list names — another label, another file name, another
/// digest — is the product by its signature, and re-enters with nothing
/// added to any table: the capability is the one every other test uses.
#[test]
fn new_digest_with_matching_contract_reenters_under_policy_a() {
    let run = bypass_and_ordinary(NEW, Policy::Ordinary);
    assert_reentered(&run, Policy::Ordinary);
    assert_eq!(run.digest.len(), 64);
}

/// The same build under Policy B: the override needs a bypass launch that
/// was previewed and frozen, and no list of its own.
#[test]
fn new_digest_with_matching_contract_reenters_under_policy_b() {
    assert_reentered(
        &bypass_and_ordinary(NEW, Policy::ReapplyBypass),
        Policy::ReapplyBypass,
    );
}

/// Two releases of the product side by side in ONE restart. Their label,
/// file name and content differ — and so do their digests; nothing else
/// about what is previewed, asked and confirmed does.
#[test]
fn a_release_that_differs_in_label_name_and_digest_keeps_both_policies() {
    let world = World::new();
    let builds = [KNOWN, NEW].map(|(label, tag)| release(&world, label, tag));
    let quoted = builds
        .each_ref()
        .map(|path| format!("'{}'", path.display()));
    let ids = [[ID_A, ID_B], [ID_C, ID_X]];
    for (build, quoted) in quoted.iter().enumerate() {
        world.card(
            &format!("bypass-{build}"),
            &format!("session={}", ids[build][0]),
            Some(&format!("{quoted} {BYPASS}")),
        );
        world.card(
            &format!("plain-{build}"),
            &format!("session={}", ids[build][1]),
            Some(quoted.as_str()),
        );
    }
    let digests = builds
        .each_ref()
        .map(|path| ExecutableRef::for_tests(path).digest_hex());
    assert_ne!(digests[0], digests[1], "two contents");
    assert_ne!(builds[0].file_name(), builds[1].file_name());
    let (token, view) = world.preview();
    let shown: Vec<(String, String)> = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap().to_string(),
                entry["launchProfile"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        shown[..2],
        shown[2..],
        "the same preview for either release"
    );
    assert_eq!(
        shown[..2],
        [("candidate", "bypass"), ("candidate", "ordinary")]
            .map(|(kind, profile)| (kind.to_string(), profile.to_string()))
    );
    let cards = ["bypass-0", "plain-0", "bypass-1", "plain-1"];
    let approval = world.approval(&token, &cards, Policy::ReapplyBypass);
    let (_, _, result) = world
        .restart_with(
            "req-two",
            Some(&approval),
            Duration::from_secs(10),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("restart");
    let result = result.unwrap();
    let pasted = world.pasted();
    for build in 0..2 {
        let (bypass, plain) = (format!("bypass-{build}"), format!("plain-{build}"));
        assert_eq!(entry(&result, &bypass).outcome, Outcome::Confirmed);
        assert_eq!(entry(&result, &plain).outcome, Outcome::Confirmed);
        assert_eq!(
            entry(&result, &bypass).warnings,
            [Warning::BypassReauthorized]
        );
        assert!(entry(&result, &plain).warnings.is_empty());
        // each request names ITS release's exact file
        assert_eq!(
            pasted[build * 2..build * 2 + 2],
            [
                format!("{} {BYPASS} --resume {}", quoted[build], ids[build][0]),
                format!("{} --resume {}", quoted[build], ids[build][1]),
            ]
        );
    }
    assert_eq!(pasted.len(), 4);
    assert_eq!(world.enters(), 4);
}

/// The launcher's name, a release-like path, a working lifecycle hook and
/// receipts of the right shape make nothing the product. A client signed
/// as another product, one whose file is no longer intact signed code, and
/// one that still runs an old image while its path already holds a valid
/// build are never candidates under either policy; an approval that names
/// them is void, and they are never started.
#[test]
fn untrusted_client_never_gains_reentry_from_receipts() {
    let world = World::new();
    let good = release(&world, NEW.0, NEW.1);
    let foreign = world.base.join("foreign").join("fake_reentry_agent");
    agent_build(&foreign, NEW.1, "io.c9r.deck.test.another-product");
    let appended = world.base.join("appended").join("fake_reentry_agent");
    agent_build(&appended, NEW.1, AGENT_IDENTIFIER);
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&appended)
            .unwrap()
            .write_all(b"tail")
            .unwrap();
    }
    let swapped = world.base.join("swapped").join("fake_reentry_agent");
    agent_build(&swapped, "build-old", AGENT_IDENTIFIER);
    let launch = |path: &Path| format!("'{}' {BYPASS}", path.display());
    world.card("good", &format!("session={ID_A}"), Some(&launch(&good)));
    world.card(
        "foreign",
        &format!("session={ID_B}"),
        Some(&launch(&foreign)),
    );
    world.card(
        "appended",
        &format!("session={ID_C}"),
        Some(&launch(&appended)),
    );
    world.card(
        "swapped",
        &format!("session={ID_X}"),
        Some(&launch(&swapped)),
    );
    // the old image keeps running; its path now holds a valid new build
    let staged = swapped.with_extension("new");
    agent_build(&staged, NEW.1, AGENT_IDENTIFIER);
    std::fs::rename(&staged, &swapped).unwrap();
    // every one of them reported its start through the real helper
    assert_eq!(world.launches().len(), 4);
    let (_, view) = world.preview();
    let read = |card: &str| {
        let entry = view["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["cardId"] == format!("card-{card}"))
            .unwrap_or_else(|| panic!("{card} is previewed"));
        (
            entry["kind"].as_str().unwrap().to_string(),
            entry["reason"].as_str().unwrap_or("-").to_string(),
            entry["launchProfile"].as_str().unwrap_or("-").to_string(),
        )
    };
    let cell = |kind: &str, reason: &str, profile: &str| {
        (kind.to_string(), reason.to_string(), profile.to_string())
    };
    assert_eq!(read("good"), cell("candidate", "-", "bypass"));
    assert_eq!(
        read("foreign"),
        cell("unsupported", "identity-unavailable", "-")
    );
    assert_eq!(
        read("appended"),
        cell("unsupported", "identity-unavailable", "-")
    );
    assert_eq!(
        read("swapped"),
        cell("unsupported", "executable-unavailable", "-")
    );
    // an approval that names any of them is void as a whole, under A and B
    let old = world.server_pid();
    for (request, cards, policy) in [
        ("r-foreign", vec!["good", "foreign"], Policy::ReapplyBypass),
        ("r-appended", vec!["appended"], Policy::Ordinary),
        ("r-swapped", vec!["swapped"], Policy::ReapplyBypass),
    ] {
        let (token, _) = world.preview();
        let error = world
            .restart(request, &world.approval(&token, &cards, policy))
            .unwrap_err();
        assert_eq!(error.message(), "reentry-preview-expired", "{request}");
        assert_eq!(world.server_pid(), old, "{request}");
        assert!(world.pasted().is_empty(), "{request}");
    }
    // the one trusted client alone: it is re-entered, and nobody else is
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["good"], Policy::ReapplyBypass);
    let (_, _, result) = world.restart("r-good", &approval).expect("restart");
    let result = result.unwrap();
    assert_eq!(result.entries.len(), 1, "one entry: the approved card");
    assert_eq!(entry(&result, "good").outcome, Outcome::Confirmed);
    assert_eq!(
        world.pasted(),
        [format!("'{}' {BYPASS} --resume {ID_A}", good.display())]
    );
    assert_eq!(world.enters(), 1);
    // four starts of the user's own and the one resume: nothing else ran
    assert_eq!(world.launches().len(), 5);
    for card in ["foreign", "appended", "swapped"] {
        assert!(!world.has_session(card), "no shell was created for {card}");
    }
}

/// A build no list names whose hooks no longer run as the client's direct
/// child. Where the old client's end comes through one more process, no
/// final session exists and nothing is started; where only the new
/// client's start does, the one request was typed and is never confirmed.
#[test]
fn changed_hook_topology_fails_at_its_observation_boundary() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    world.card(
        "end",
        &format!("session={ID_A}\nhooks=wrap-end"),
        Some(&quoted),
    );
    world.card(
        "start",
        &format!("session={ID_B}\nhooks=wrap-start"),
        Some(&quoted),
    );
    world.card("direct", &format!("session={ID_C}"), Some(&quoted));
    let (token, view) = world.preview();
    assert!(view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["kind"] == "candidate"));
    let approval = world.approval(&token, &["end", "start", "direct"], Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-topology",
            Some(&approval),
            Duration::from_secs(5),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    let end = entry(&result, "end");
    assert_eq!(end.outcome, Outcome::NotAttempted);
    assert_eq!(end.reason, Some(Reason::FinalSessionUnconfirmed));
    assert!(!end.shell && !world.has_session("end"));
    let start = entry(&result, "start");
    assert_eq!(start.outcome, Outcome::Unconfirmed);
    assert_eq!(start.reason, Some(Reason::LaunchUnconfirmed));
    assert!(
        start.shell && start.client_observed,
        "started, left as it is"
    );
    assert_eq!(entry(&result, "direct").outcome, Outcome::Confirmed);
    assert_eq!(
        world.pasted(),
        [
            format!("{quoted} --resume {ID_B}"),
            format!("{quoted} --resume {ID_C}")
        ],
        "one request each for the two that had a final session"
    );
    assert_eq!(world.enters(), 2, "nothing was typed again");
    assert_eq!(world.launches().len(), 5, "three starts and two resumes");
}

/// A build no list names whose lifecycle payload lost, renamed or retyped
/// the session id, or uses a word this build does not know. At the old
/// client's end that leaves no final session and nothing is started; at the
/// new client's start the one request is not confirmed. Fields this feature
/// does not read — new ones included — change nothing.
#[test]
fn changed_required_lifecycle_fields_fail_closed() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    let variants = ["no-id", "renamed", "typed", "word"];
    let ends: Vec<String> = variants.iter().map(|v| format!("end-{v}")).collect();
    let starts: Vec<String> = variants.iter().map(|v| format!("start-{v}")).collect();
    let id = |n: usize| format!("eeeeeeee-0000-4000-8000-00000000000{n}");
    for (n, (card, variant)) in ends.iter().zip(variants).enumerate() {
        world.card(
            card,
            &format!("session={}\nend-payload={variant}", id(n)),
            Some(&quoted),
        );
    }
    for (n, (card, variant)) in starts.iter().zip(variants).enumerate() {
        world.card(
            card,
            &format!("session={}\nstart-payload={variant}", id(n + 4)),
            Some(&quoted),
        );
    }
    // unrelated fields, one of them with a value no release has today
    world.card(
        "extra",
        &format!("session={ID_A}\nfinal-mode=a_mode_of_a_later_release"),
        Some(&quoted),
    );
    let (token, _) = world.preview();
    let mut cards: Vec<&str> = ends.iter().chain(&starts).map(String::as_str).collect();
    cards.push("extra");
    let approval = world.approval(&token, &cards, Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-fields",
            Some(&approval),
            Duration::from_secs(7),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    for card in &ends {
        let skipped = entry(&result, card);
        assert_eq!(skipped.outcome, Outcome::NotAttempted, "{card}");
        assert_eq!(skipped.reason, Some(Reason::FinalSessionUnconfirmed));
        assert!(!skipped.shell && !world.has_session(card), "{card}");
    }
    for card in &starts {
        let asked = entry(&result, card);
        assert_eq!(asked.outcome, Outcome::Unconfirmed, "{card}");
        assert_eq!(asked.reason, Some(Reason::LaunchUnconfirmed), "{card}");
        assert!(asked.shell && asked.client_observed, "{card}");
    }
    assert_eq!(entry(&result, "extra").outcome, Outcome::Confirmed);
    let pasted = world.pasted();
    assert_eq!(
        pasted.len(),
        5,
        "four unconfirmed requests and one confirmed"
    );
    for (n, line) in pasted[..4].iter().enumerate() {
        assert_eq!(*line, format!("{quoted} --resume {}", id(n + 4)));
    }
    assert_eq!(world.enters(), 5);
}

/// Policy B over clients that were NOT started with bypass: an ordinary
/// launch stays an ordinary request, whatever its permission mode, its
/// saved card command or its native payload says, and a launch shape that
/// is not supported is not a candidate at all. Nothing gets the flag.
#[test]
fn policy_b_does_not_upgrade_a_non_bypass_launch() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    world.card("plain", &format!("session={ID_A}"), Some(&quoted));
    world.card(
        "plan",
        &format!("session={ID_B}"),
        Some(&format!("{quoted} --permission-mode plan")),
    );
    world.card(
        "accept",
        &format!("session={ID_C}"),
        Some(&format!("{quoted} --permission-mode acceptEdits")),
    );
    world.card(
        "mode",
        &format!("session={ID_X}"),
        Some(&format!("{quoted} --permission-mode bypassPermissions")),
    );
    assert!(
        SAVED_COMMAND.contains(BYPASS),
        "every saved command says bypass"
    );
    let (token, view) = world.preview();
    let shown: Vec<(&str, &str)> = view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap(),
                entry["launchProfile"].as_str().unwrap_or("-"),
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("candidate", "ordinary"),
            ("candidate", "ordinary"),
            ("candidate", "ordinary"),
            ("unsupported", "-")
        ]
    );
    let approval = world.approval(&token, &["plain", "plan", "accept"], Policy::ReapplyBypass);
    let (_, _, result) = world.restart("req-ordinary-b", &approval).expect("restart");
    let result = result.unwrap();
    for card in ["plain", "plan", "accept"] {
        let entry = entry(&result, card);
        assert_eq!(entry.outcome, Outcome::Confirmed, "{card}");
        assert!(
            entry.warnings.is_empty(),
            "{card}: nothing was reauthorized"
        );
    }
    assert_eq!(
        world.pasted(),
        [ID_A, ID_B, ID_C].map(|id| format!("{quoted} --resume {id}"))
    );
    assert_eq!(
        world.launches()[4..],
        [ID_A, ID_B, ID_C].map(|id| format!("--resume {id}"))
    );
    assert!(
        !world.has_session("mode"),
        "the unsupported launch is not reopened"
    );
    // an approval that names the unsupported launch is void, under B as under A
    let (token, _) = world.preview();
    let error = world
        .restart(
            "req-unsupported-b",
            &world.approval(&token, &["mode"], Policy::ReapplyBypass),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
}

/// The approval names ONE executable. When another validly signed build
/// takes its place — before the freeze, between the exit and the input, or
/// only as what the started client turns out to run — the approval is never
/// carried over to it: the restart is refused, the entry is not launched,
/// or the one launch is not confirmed. Nothing is frozen again and nothing
/// is tried twice.
#[test]
fn executable_changes_never_rebind_an_approval() {
    // before the freeze: the whole restart is refused before any exit key
    {
        let world = World::new();
        let binary = release(&world, NEW.0, NEW.1);
        world.card(
            "a",
            &format!("session={ID_A}"),
            Some(&format!("'{}' {BYPASS}", binary.display())),
        );
        let (token, _) = world.preview();
        let staged = binary.with_extension("new");
        agent_build(&staged, "build-newer", AGENT_IDENTIFIER);
        std::fs::rename(&staged, &binary).unwrap();
        let old = world.server_pid();
        let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
        let error = world.restart("req-early", &approval).unwrap_err();
        assert_eq!(error.message(), "tmux-server-impact-changed");
        assert!(
            !crate::session_runtime::deadline_active(),
            "a refused restart leaves none of its deadlines on the thread"
        );
        assert_eq!(world.server_pid(), old, "nothing was replaced");
        assert!(world.pasted().is_empty());
        assert_eq!(world.launches().len(), 1);
        // the approval is spent, and the client that still runs the old
        // image is not offered again as if it were the new build. (Under a
        // release's file name nothing hints at what it is: it is simply a
        // foreground this feature does not take for the product.)
        let again = world.restart("req-early-2", &approval).unwrap_err();
        assert_eq!(again.message(), "reentry-preview-expired");
        let (_, view) = world.preview();
        assert_eq!(view["entries"][0]["kind"], "unsupported");
        assert_eq!(view["entries"][0]["reason"], "unsupported-agent");
    }
    // between the exit and the input: nothing is started, no shell is made
    {
        let world = World::new();
        let binary = release(&world, NEW.0, NEW.1);
        world.card(
            "a",
            &format!("session={ID_A}"),
            Some(&format!("'{}' {BYPASS}", binary.display())),
        );
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
        let (_, _, result) = world
            .restart_with(
                "req-middle",
                Some(&approval),
                Duration::from_secs(8),
                &|| {
                    let staged = binary.with_extension("new");
                    agent_build(&staged, "build-newer", AGENT_IDENTIFIER);
                    std::fs::rename(&staged, &binary).unwrap();
                    Ok(())
                },
                &|_, _, _| {},
            )
            .expect("the restart itself succeeded");
        let result = result.unwrap();
        assert!(result.server_replaced);
        let changed = entry(&result, "a");
        assert_eq!(changed.outcome, Outcome::NotAttempted);
        assert_eq!(changed.reason, Some(Reason::ExecutableUnavailable));
        assert!(!changed.shell && !world.has_session("a"));
        assert!(world.pasted().is_empty(), "the new build was never asked");
        assert_eq!(world.launches().len(), 1);
    }
    // only found after the launch: the started client is another build
    {
        let world = World::new();
        let binary = release(&world, NEW.0, NEW.1);
        let other = release(&world, "9.9.1000", "build-newer");
        world.card(
            "a",
            &format!("session={ID_A}\nresume=exec:{}", other.display()),
            Some(&format!("'{}' {BYPASS}", binary.display())),
        );
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
        let (_, _, result) = world
            .restart_with(
                "req-late-change",
                Some(&approval),
                Duration::from_secs(4),
                &|| Ok(()),
                &|_, _, _| {},
            )
            .expect("the restart itself succeeded");
        let result = result.unwrap();
        let started = entry(&result, "a");
        assert_eq!(started.outcome, Outcome::Unconfirmed);
        assert_eq!(started.reason, Some(Reason::LaunchUnconfirmed));
        assert!(started.shell && started.client_observed, "left running");
        assert!(
            started.warnings.is_empty(),
            "no flag is said to have been sent"
        );
        assert_eq!(
            world.pasted(),
            [format!("'{}' {BYPASS} --resume {ID_A}", binary.display())],
            "the one request named the frozen file"
        );
        assert_eq!(world.enters(), 1);
        // its own start, the asked resume, and the other build that took over
        assert_eq!(world.launches().len(), 3);
    }
}

/// The leased old client replaces its image with another validly signed
/// build of the product, under the same pid, and THAT program reports the
/// final session. Pid and birth instant are the frozen ones; the running
/// code is not, so its report names nothing: no final session, no shell and
/// no request for that card, while the card beside it is re-entered.
#[test]
fn an_exit_reported_by_another_image_under_the_frozen_pid_names_no_final_session() {
    let world = World::new();
    let binary = release(&world, NEW.0, NEW.1);
    let other = release(&world, "9.9.1000", "build-newer");
    let quoted = format!("'{}'", binary.display());
    world.card(
        "swapped",
        &format!("session={ID_A}\nexit=exec:{}", other.display()),
        Some(&quoted),
    );
    world.card("kept", &format!("session={ID_B}"), Some(&quoted));
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    let old = rows
        .iter()
        .find(|row| row.session_name == "swapped")
        .and_then(|row| crate::agent_status::live_generation(row.pane_pid))
        .expect("the old client");
    let (token, view) = world.preview();
    assert!(view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["kind"] == "candidate"));
    let approval = world.approval(&token, &["swapped", "kept"], Policy::Ordinary);
    let (_, _, result) = world
        .restart("req-exit-image", &approval)
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    // the frozen generation did exit: what is missing is its final session
    assert_eq!(
        crate::procinfo::generation_liveness(old.pid(), old.birth()),
        crate::procinfo::Liveness::Gone
    );
    let swapped = entry(&result, "swapped");
    assert_eq!(swapped.outcome, Outcome::NotAttempted);
    assert_eq!(swapped.reason, Some(Reason::FinalSessionUnconfirmed));
    assert!(!swapped.shell && !world.has_session("swapped"));
    assert_eq!(entry(&result, "kept").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted(), [format!("{quoted} --resume {ID_B}")]);
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 3, "two starts and the one resume");
}

/// A resume receipt for another id, with another source, two answers of
/// which the later one is right, and a client that entered and left: none
/// confirms, no answer is picked as the winner, and each request was typed
/// once.
#[test]
fn mismatched_resume_receipts_never_confirm() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    for (card, id, resume) in [
        ("wrong", ID_A, format!("wrong:{ID_X}")),
        ("source", ID_B, "startup".to_string()),
        ("both", ID_C, format!("both:{ID_X}")),
        (
            "left",
            "ffffffff-0000-4000-8000-000000000005",
            "leave".to_string(),
        ),
    ] {
        world.card(
            card,
            &format!("session={id}\nresume={resume}"),
            Some(&quoted),
        );
    }
    let (token, _) = world.preview();
    let cards = ["wrong", "source", "both", "left"];
    let approval = world.approval(&token, &cards, Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-receipts",
            Some(&approval),
            Duration::from_secs(6),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.unwrap();
    assert!(result.server_replaced);
    for card in cards {
        let unconfirmed = entry(&result, card);
        assert_eq!(unconfirmed.outcome, Outcome::Unconfirmed, "{card}");
        assert_eq!(
            unconfirmed.reason,
            Some(Reason::LaunchUnconfirmed),
            "{card}"
        );
    }
    assert_eq!(world.pasted().len(), 4);
    assert_eq!(world.enters(), 4);
    assert_eq!(
        world.launches().len(),
        8,
        "four starts, four resumes, no more"
    );
}

/// Started with bypass and lowered while running: whatever the client's
/// last word about its permission mode is — plan, nothing at all, bypass —
/// its launch profile is still bypass, and under Policy B this restart
/// sends the flag again. The final mode is read by nothing and gates
/// nothing.
#[test]
fn bypass_launch_profile_is_reauthorized_after_runtime_downgrade() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    let launch = format!("{quoted} {BYPASS}");
    world.card(
        "lowered",
        &format!("session={ID_A}\nfinal-mode=plan"),
        Some(&launch),
    );
    world.card(
        "silent",
        &format!("session={ID_B}\nfinal-mode=absent"),
        Some(&launch),
    );
    world.card("kept", &format!("session={ID_C}"), Some(&launch));
    let (token, view) = world.preview();
    assert!(view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["launchProfile"] == "bypass"));
    let cards = ["lowered", "silent", "kept"];
    let approval = world.approval(&token, &cards, Policy::ReapplyBypass);
    let (_, _, result) = world.restart("req-lowered", &approval).expect("restart");
    let result = result.unwrap();
    for card in cards {
        let entry = entry(&result, card);
        assert_eq!(entry.outcome, Outcome::Confirmed, "{card}");
        assert_eq!(entry.warnings, [Warning::BypassReauthorized], "{card}");
    }
    assert_eq!(
        world.pasted(),
        [ID_A, ID_B, ID_C].map(|id| format!("{quoted} {BYPASS} --resume {id}")),
        "the flag, then the resume, for each"
    );
    assert_eq!(
        world.launches()[3..],
        [ID_A, ID_B, ID_C].map(|id| format!("{BYPASS} --resume {id}"))
    );
}

/// Policy A over every launch shape that qualifies and every last word
/// about a permission mode: each request is the executable, `--resume` and
/// the id. No bypass, no permission mode, no model, nothing of the old
/// client's own arguments.
#[test]
fn policy_a_never_adds_a_permission_override() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    let id = |n: usize| format!("abababab-0000-4000-8000-00000000000{n}");
    let shapes = [
        (format!("{quoted} {BYPASS}"), "final-mode=plan"),
        (format!("{quoted} {BYPASS}"), "final-mode=absent"),
        (format!("{quoted} {BYPASS}"), "final-mode=bypassPermissions"),
        (quoted.clone(), "final-mode=default"),
        (
            format!("{quoted} --permission-mode plan"),
            "final-mode=plan",
        ),
        (format!("{quoted} --permission-mode acceptEdits"), ""),
        (format!("{quoted} --permission-mode default"), ""),
        (format!("{quoted} --model opus {BYPASS}"), ""),
    ];
    let cards: Vec<String> = (0..shapes.len()).map(|n| format!("shape-{n}")).collect();
    for (n, (launch, mode)) in shapes.iter().enumerate() {
        world.card(
            &cards[n],
            &format!("session={}\n{mode}", id(n)),
            Some(launch),
        );
    }
    let (token, _) = world.preview();
    let names: Vec<&str> = cards.iter().map(String::as_str).collect();
    let approval = world.approval(&token, &names, Policy::Ordinary);
    let (_, _, result) = world
        .restart_with(
            "req-a",
            Some(&approval),
            Duration::from_secs(14),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("restart");
    let result = result.unwrap();
    let expected: Vec<String> = (0..shapes.len())
        .map(|n| format!("{quoted} --resume {}", id(n)))
        .collect();
    assert_eq!(world.pasted(), expected);
    assert_eq!(
        world.launches()[shapes.len()..],
        (0..shapes.len())
            .map(|n| format!("--resume {}", id(n)))
            .collect::<Vec<_>>()[..]
    );
    for card in &cards {
        let entry = entry(&result, card);
        assert_eq!(entry.outcome, Outcome::Confirmed, "{card}");
        assert!(entry.warnings.is_empty(), "{card}");
    }
    assert_eq!(world.enters(), shapes.len());
}

/// Policy B reaches the ticked cards of THIS preview and no other: a bypass
/// launch that was not ticked gets no shell and no request, a card created
/// after the preview cannot be named, and neither an earlier preview's
/// token nor a second use of this approval starts anything.
#[test]
fn unapproved_cards_never_receive_bypass() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    let launch = format!("{quoted} {BYPASS}");
    world.card("ticked", &format!("session={ID_A}"), Some(&launch));
    world.card("unticked", &format!("session={ID_B}"), Some(&launch));
    let old = world.server_pid();
    let (stale, _) = world.preview();
    let (token, view) = world.preview();
    assert!(view["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["launchProfile"] == "bypass"));
    // an earlier preview's token is not this approval
    let error = world
        .restart(
            "req-stale",
            &world.approval(&stale, &["ticked", "unticked"], Policy::ReapplyBypass),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    // that refusal spent the current preview as well: preview again
    drop(token);
    let (token, _) = world.preview();
    // a bypass card that appears after the preview is not part of it
    world.card("later", &format!("session={ID_C}"), Some(&launch));
    let error = world
        .restart(
            "req-later",
            &world.approval(&token, &["ticked", "later"], Policy::ReapplyBypass),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    assert_eq!(world.server_pid(), old);
    assert!(world.pasted().is_empty());
    // the one ticked card
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["ticked"], Policy::ReapplyBypass);
    let (_, _, result) = world.restart("req-ticked", &approval).expect("restart");
    let result = result.unwrap();
    assert_eq!(result.entries.len(), 1, "one entry: the ticked card");
    assert_eq!(entry(&result, "ticked").outcome, Outcome::Confirmed);
    assert_eq!(world.pasted(), [format!("{launch} --resume {ID_A}")]);
    assert_eq!(world.enters(), 1);
    for card in ["unticked", "later"] {
        assert!(!world.has_session(card), "no shell for {card}");
    }
    assert_eq!(world.launches().len(), 4, "three starts and the one resume");
    // the approval is spent: another operation gets nothing from it
    let again = world.restart("req-ticked-2", &approval).unwrap_err();
    assert_eq!(again.message(), "reentry-preview-expired");
    assert_eq!(world.pasted().len(), 1);
}

/// A launch profile that could not be read is never taken for bypass, and
/// nothing else is read in its place: not the card's saved command, not a
/// permission mode the native payload names.
#[test]
fn unknown_launch_profiles_never_authorize_bypass() {
    for (preview, frozen) in [
        (Unknown, Unknown),
        (Unknown, Bypass),
        (Bypass, Unknown),
        (Ordinary, Unknown),
    ] {
        for proven in [true, false] {
            let mut read = scope(PolicyB, preview, frozen);
            read.context_proven = proven;
            assert_ne!(
                bypass_of(decide(read)),
                Some(true),
                "{preview:?} {frozen:?} {proven}"
            );
        }
        // without an independently proven context it is not re-entered at all
        let mut unproven = scope(PolicyB, preview, Unknown);
        unproven.context_proven = false;
        assert_eq!(
            decide(unproven),
            Decision::Skip(Reason::IdentityUnavailable)
        );
    }
    assert_eq!(profile_word(Unknown), "unknown");
    assert_eq!(profile_word(Unsupported), "unknown");
    // an ordinary launch whose saved command and whose payloads all say
    // bypass: under Policy B it is asked without the flag
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    world.card(
        "a",
        &format!("session={ID_A}\nfinal-mode=bypassPermissions"),
        Some(&quoted),
    );
    assert!(SAVED_COMMAND.contains(BYPASS));
    let (token, view) = world.preview();
    assert_eq!(view["entries"][0]["launchProfile"], "ordinary");
    let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
    let (_, _, result) = world.restart("req-unknown", &approval).expect("restart");
    let result = result.unwrap();
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert!(entry(&result, "a").warnings.is_empty());
    assert_eq!(world.pasted(), [format!("{quoted} --resume {ID_A}")]);
}

/// Every way an outcome can be unknown — a lost paste reply, a refused
/// Enter, no receipt, a receipt that arrives after the operation closed —
/// uses the entry's one slot and is never followed by a second request:
/// not by the operation, not by reading its result, not by using the
/// approval again, and not after the process forgot everything.
///
/// The times belong together. Four entries are launched one after another,
/// and an entry is attempted only while the post share still has
/// `ATTEMPT_FLOOR` (1.5 s) left: with a 4 s share all four had to be started
/// within 2.5 s, which a hosted CI runner did not always manage (the last
/// entry then is, correctly, refused before its launch, and this test is not
/// about that). With 8 s the four have 6.5 s. The late receipt comes 8.5 s
/// after its client started, so always after the post share ended, and at
/// the latest 15 s after that share began: the wait below ends after that.
#[test]
fn unknown_outcomes_are_never_retried() {
    let world = World::new();
    let quoted = format!("'{}'", release(&world, NEW.0, NEW.1).display());
    world.card("lost", &format!("session={ID_A}"), Some(&quoted));
    world.card("enter", &format!("session={ID_B}"), Some(&quoted));
    world.card(
        "silent",
        &format!("session={ID_C}\nresume=silent"),
        Some(&quoted),
    );
    world.card(
        "late",
        &format!("session={ID_X}\nresume=late:8500"),
        Some(&quoted),
    );
    let (token, _) = world.preview();
    let cards = ["lost", "enter", "silent", "late"];
    let approval = world.approval(&token, &cards, Policy::Ordinary);
    let pastes = std::cell::Cell::new(0);
    let enters = std::cell::Cell::new(0);
    *world.intercept.borrow_mut() = Some(Box::new(move |args, input| {
        if input.is_some() {
            pastes.set(pastes.get() + 1);
            // the first entry's paste reply is lost
            return (pastes.get() == 1)
                .then(|| Err(DeckError::new(ErrorKind::Other, "reply lost")));
        }
        let enter = args
            .iter()
            .any(|arg| arg.contains("send-keys") && arg.ends_with("Enter"));
        if enter {
            enters.set(enters.get() + 1);
        }
        // the second entry's Enter is refused
        (enter && enters.get() == 1).then(|| Ok("deck-context-refused\n".into()))
    }));
    let (_, _, result) = world
        .restart_with(
            "req-unknown-outcomes",
            Some(&approval),
            Duration::from_secs(8),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    *world.intercept.borrow_mut() = None;
    let result = result.unwrap();
    assert!(result.server_replaced);
    assert_eq!(result.state, OperationState::Complete);
    for card in cards {
        let unknown = entry(&result, card);
        assert_eq!(unknown.outcome, Outcome::Unconfirmed, "{card}");
        assert_eq!(unknown.reason, Some(Reason::LaunchUnconfirmed), "{card}");
        assert!(unknown.shell, "{card}: its shell is kept");
    }
    let (pasted, entered, launched) =
        (world.pasted().len(), world.enters(), world.launches().len());
    assert_eq!(pasted, 4, "one paste per entry");
    assert_eq!(entered, 3, "the lost paste was not followed by an Enter");
    assert_eq!(
        launched, 6,
        "four starts and the two clients that did start"
    );
    // reading the operation, again and again, executes nothing
    for _ in 0..3 {
        assert_eq!(known("req-unknown-outcomes"), Some(result.clone()));
        assert!(operation("req-unknown-outcomes").known);
    }
    // the late answer arrives after the operation closed: nobody takes it
    std::thread::sleep(Duration::from_millis(7500));
    assert_eq!(known("req-unknown-outcomes"), Some(result.clone()));
    assert!(!lifecycle::armed(), "no lease is left to land on");
    // the approval again, and the same request id again: nothing starts
    for request in ["req-unknown-outcomes", "req-unknown-outcomes-2"] {
        let again = world.restart(request, &approval).unwrap_err();
        assert_eq!(again.message(), "reentry-preview-expired", "{request}");
    }
    // a process that forgot everything (a crash) replays nothing either
    reset_for_tests();
    assert!(!operation("req-unknown-outcomes").known);
    assert_eq!(world.pasted().len(), pasted);
    assert_eq!(world.enters(), entered);
}

// ---------- native certification, step 0: a read-only probe ------------------------

/// Not a test of its own: the read-only probe of native certification step
/// 0. A driver outside this process starts one real client in a pane of a
/// private tmux server under a throwaway HOME; this entry then asks the
/// PRODUCTION qualification (`inspect`, `procinfo::qualify_client`) and the
/// PRODUCTION post-launch check (`procinfo::check_launch`) about that exact
/// process, with the production capability (`claude::CAPABILITY`: its
/// product requirement, grammar and request builder) and nothing looser.
/// It types nothing, starts nothing and prints one line of closed words,
/// pids, births and counts — never an argument, an environment entry, a
/// path or a native id.
///
/// To test the DRIVER without a real client, the driver may name a
/// harmless stand-in binary. The same production functions then run with a
/// stand-in capability (the product "that binary's own signing identifier",
/// the production grammar and request builder), and every line says
/// `capability=stand-in`: such a line is about the driver and is never
/// evidence about a real client. A binary that IS production's product is
/// refused as a stand-in.
#[test]
#[ignore = "run only by the native certification driver, against a process it started"]
fn native_step0_probe() {
    let var = |name: &str| std::env::var(name).ok();
    let (Some(socket), Some(session), Some(home), Some(cwd), Some(stage)) = (
        var("DECK_N0_SOCKET"),
        var("DECK_N0_SESSION"),
        var("DECK_N0_HOME"),
        var("DECK_N0_CWD"),
        var("DECK_N0_STAGE"),
    ) else {
        return;
    };
    let (home, cwd) = (PathBuf::from(home), PathBuf::from(cwd));
    // the checks below ask for a deadline owner; the preview's share is one
    let _phase = crate::session_runtime::Deadline::until(Instant::now() + PREVIEW_BUDGET);
    let standin = var("DECK_N0_STANDIN").map(PathBuf::from);
    let capability: &ReentryCapability = match &standin {
        Some(binary) => match World::standin_capability(binary) {
            Some(capability) => capability,
            None => return println!("N0 stage={stage} capability=refused"),
        },
        None => &claude::CAPABILITY,
    };
    let which = if standin.is_some() {
        "stand-in"
    } else {
        "production"
    };
    // The expected arguments, from structured parameters only: an optional
    // native id and whether the bypass flag is part of the launch. With an
    // id the words are the production request builder's; a start without an
    // id carries nothing but, when asked for, the grammar's own bypass flag.
    let bypass = var("DECK_N0_BYPASS").is_some_and(|flag| flag == "1");
    let words = match var("DECK_N0_RESUME") {
        Some(id) => (capability.request)(&id, bypass).expect("a native-shaped id"),
        None if bypass => vec![capability.grammar.bypass.to_string()],
        None => Vec::new(),
    };
    let run = |args: &[&str]| -> Result<String, DeckError> {
        let mut command = Command::new(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin"),
        );
        command
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .args(["-f", "/dev/null", "-L", &socket]);
        crate::tmux::captured_output(&mut command, args)
    };
    let report = |facts: String| {
        println!(
            "N0 stage={stage} capability={which} words={} {facts}",
            words.len()
        )
    };
    let rows: Vec<PaneRow> = match crate::tmux::list_panes_with(&run) {
        Ok(rows) => rows
            .into_iter()
            .filter(|row| row.session_name == session)
            .collect(),
        Err(_) => return report("pane=unlisted".into()),
    };
    let [row] = rows.as_slice() else {
        return report(format!("pane=count-{}", rows.len()));
    };
    let table = crate::procinfo::processes();
    let Some(foreground) = crate::agent_status::foreground_generation(&table, row.pane_pid) else {
        return report("foreground=unavailable".into());
    };
    if foreground.pid() == row.pane_pid {
        // no client in the foreground: it has not started or has exited
        return report("foreground=shell".into());
    }
    let (pid, birth) = (foreground.pid(), foreground.birth());
    let direct = table
        .get(&pid)
        .is_some_and(|client| client.ppid == row.pane_pid);
    let shape = match crate::procinfo::launch_image_shape_for_tests(pid) {
        Some((argc, envc)) => format!("argc={argc} envc={envc}"),
        None => "image=unparsed".into(),
    };
    let read = crate::procinfo::qualify_client(
        pid,
        birth,
        &ClientSpec {
            product: capability.product,
            launcher: capability.launcher,
            grammar: capability.grammar,
            home: &home,
        },
        &[],
    );
    let (qualified, executable) = match read {
        ClientRead::Gone => ("qualify=gone".to_string(), None),
        ClientRead::NotProduct { named, why } => {
            (format!("qualify=not-product/{why:?} named={named}"), None)
        }
        ClientRead::Product(read) => (
            format!(
                "qualify=product executable={} profile={:?} dirs={} cwd={}",
                match &read.executable {
                    Ok(_) => "bound".to_string(),
                    Err(why) => format!("{why:?}"),
                },
                read.profile,
                read.additional_dirs,
                match &read.cwd {
                    None => "unread",
                    Some(dir) if dir.path() == cwd => "expected",
                    Some(_) => "other",
                },
            ),
            read.executable.ok(),
        ),
    };
    // the production preview classification of this pane
    let inspected = scripted_env_for(
        capability,
        &home,
        &|_| Err(DeckError::new(ErrorKind::Tmux, "no tmux in this probe")),
        |env| inspect(env, row, 1, &[]),
    );
    let checked = executable.as_ref().map(|executable| {
        crate::procinfo::check_launch(
            pid,
            birth,
            &crate::procinfo::LaunchExpectation {
                executable,
                words: &words,
                home: &home,
                grammar: capability.grammar,
                cwd: &DirRef::for_tests(&cwd),
            },
        )
    });
    report(format!(
        "pid={pid} birth={}.{:06} direct={direct} {shape} {qualified} inspect={:?}/{} check={}",
        birth.0,
        birth.1,
        inspected.kind,
        inspected.reason.map_or("none", Reason::code),
        checked.map_or("not-run".to_string(), |check| format!("{check:?}")),
    ));
}

/// Not a test of its own: the identity tool of the native certification
/// driver. Every operation is about exact process generations — a pid
/// together with the kernel's full birth instant (seconds and microseconds)
/// — and none looks a process up by name:
///
/// - `birth`: the full birth instant of a pid, given out only while that
///   pid is a child of the named parent (how the driver learns the
///   generation of a process it has just created);
/// - `tree`: every process under one exact parent generation, with its own
///   full birth instant, from a single process-table snapshot taken while
///   the parent was — before and after — that generation;
/// - `alive`: whether one recorded generation still exists. Never signals;
/// - `foreground`: the generation that is the foreground of one pane shell's
///   terminal right now — its pid and full birth instant, and whether it is
///   that shell's direct child — or `shell` when the shell itself is. Never
///   signals;
/// - `check` / `term` / `kill`: one generation that must also still be a
///   child of the named parent. All three facts are re-read immediately
///   before a signal; `check` signals nothing.
#[test]
#[ignore = "run only by the native certification driver, against a process it started"]
fn native_step0_generation() {
    let var = |name: &str| std::env::var(name).ok();
    let number = |name: &str| var(name).and_then(|value| value.parse::<u32>().ok());
    let instant = |name: &str| {
        var(name).and_then(|birth| {
            let (seconds, micros) = birth.split_once('.')?;
            Some((seconds.parse::<u64>().ok()?, micros.parse::<u32>().ok()?))
        })
    };
    let Some(op) = var("DECK_N0_OP") else {
        return;
    };
    let started = crate::procinfo::process_start;
    let parent_of = |pid: u32| crate::procinfo::process(pid).map(|info| info.ppid);
    match op.as_str() {
        "birth" => {
            let (Some(pid), Some(parent)) = (number("DECK_N0_PID"), number("DECK_N0_PARENT"))
            else {
                return println!("N0-birth state=bad-arguments");
            };
            let before = started(pid);
            let child = parent_of(pid) == Some(parent);
            let after = started(pid);
            match (before, after) {
                (Some(a), Some(b)) if a == b && child => {
                    println!("N0-birth pid={pid} state=exact birth={}.{:06}", a.0, a.1)
                }
                (Some(a), Some(b)) if a == b => println!("N0-birth pid={pid} state=other-parent"),
                _ => println!("N0-birth pid={pid} state=gone"),
            }
        }
        "tree" => {
            let (Some(parent), Some(birth)) = (number("DECK_N0_PID"), instant("DECK_N0_BIRTH"))
            else {
                return println!("N0-tree parent=bad-arguments count=0");
            };
            let generation = || match started(parent) {
                None => "gone",
                Some(now) if now != birth => "other-generation",
                Some(_) => "exact",
            };
            let before = generation();
            if before != "exact" {
                return println!("N0-tree parent={before} count=0");
            }
            let table = crate::procinfo::processes();
            let mut members: Vec<(u32, u64, u32)> = table
                .iter()
                .filter(|(pid, _)| {
                    crate::procinfo::ancestry_in(&table, **pid, 64)
                        .iter()
                        .skip(1)
                        .any(|ancestor| *ancestor == parent)
                })
                .map(|(pid, info)| (*pid, info.start_seconds, info.start_micros))
                .collect();
            members.sort_unstable();
            // a listing of THIS parent generation only if it still is that one
            if generation() != "exact" {
                return println!("N0-tree parent=changed count=0");
            }
            for (pid, seconds, micros) in &members {
                println!("N0-member {pid}@{seconds}.{micros:06}");
            }
            println!("N0-tree parent=exact count={}", members.len());
        }
        "foreground" => {
            let Some(pane) = number("DECK_N0_PID") else {
                return println!("N0-foreground state=bad-arguments");
            };
            let table = crate::procinfo::processes();
            match crate::agent_status::foreground_generation(&table, pane) {
                None => println!("N0-foreground pane={pane} state=unavailable"),
                Some(leader) if leader.pid() == pane => {
                    println!("N0-foreground pane={pane} state=shell")
                }
                Some(leader) => println!(
                    "N0-foreground pane={pane} state=client pid={} birth={}.{:06} direct={}",
                    leader.pid(),
                    leader.birth().0,
                    leader.birth().1,
                    table
                        .get(&leader.pid())
                        .is_some_and(|client| client.ppid == pane)
                ),
            }
        }
        "alive" => {
            let (Some(pid), Some(birth)) = (number("DECK_N0_PID"), instant("DECK_N0_BIRTH")) else {
                return println!("N0-generation op=alive state=bad-arguments");
            };
            let state = match started(pid) {
                None => "gone",
                Some(now) if now != birth => "other-generation",
                Some(_) => "alive",
            };
            println!("N0-generation op=alive pid={pid} state={state} signal=none");
        }
        "check" | "term" | "kill" => {
            let (Some(pid), Some(birth), Some(parent)) = (
                number("DECK_N0_PID"),
                instant("DECK_N0_BIRTH"),
                number("DECK_N0_PARENT"),
            ) else {
                return println!("N0-generation op={op} state=bad-arguments");
            };
            let signal = match op.as_str() {
                "term" => Some(libc::SIGTERM),
                "kill" => Some(libc::SIGKILL),
                _ => None,
            };
            // the generation, read now: this pid, born at this instant, under this parent
            let state = match started(pid) {
                None => "gone",
                Some(now) if now != birth => "other-generation",
                Some(_) if parent_of(pid) != Some(parent) => "other-parent",
                Some(_) => "exact",
            };
            let sent = match (state, signal) {
                ("exact", Some(signal)) => {
                    // SAFETY: signals the one process whose pid, birth
                    // instant and parent were read in the lines above.
                    let rc = unsafe { libc::kill(pid as libc::pid_t, signal) };
                    if rc == 0 {
                        "sent"
                    } else {
                        "failed"
                    }
                }
                _ => "none",
            };
            println!("N0-generation op={op} pid={pid} state={state} signal={sent}");
        }
        _ => println!("N0-generation op=unknown"),
    }
}

// ---------- native certification, stage M1: a recorder and the transaction ----------

/// A salted digest of a native id: what the native certification writes
/// instead of the id itself.
fn id_digest(salt: &str, id: &str) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b":");
    hasher.update(id.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `name=value,name=value` as pairs.
fn pairs(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// Not a test of its own: the passive recorder of the native certification's
/// stage M1. Before the transaction — while the old clients start and take
/// their fixed inputs — it owns the experiment's status socket and writes
/// one line of closed facts for every line a helper sends: the event and its
/// closed word (read with the production parser), a salted digest of the
/// native id (never the id), which registered pane the reporting helper
/// sits under, and what the kernel says about that helper while it is still
/// connected. It then closes the connection (the helper's ACK). It arms no
/// lease, feeds no store and touches no pane.
///
/// Like the production listener it first asks which protocol family a line
/// belongs to (`lifecycle::is_family`). A line of another family — a Signal
/// hook of the user's own settings, routed here by the same environment —
/// is written as `family=signal` or `family=unknown` with where it came
/// from, and is never handed to the lifecycle parser.
#[test]
#[ignore = "run only by the native certification driver, on its own status socket"]
fn native_m1_tap() {
    use std::io::{Read, Write};
    let var = |name: &str| std::env::var(name).ok();
    let (Some(base), Some(socket), Some(salt), Some(panes), Some(helper), Some(log)) = (
        var("DECK_M1_BASE"),
        var("DECK_M1_SOCKET"),
        var("DECK_M1_SALT"),
        var("DECK_M1_PANES"),
        var("DECK_M1_HELPER"),
        var("DECK_M1_TAPLOG"),
    ) else {
        return;
    };
    let base = PathBuf::from(base);
    let panes: Vec<(String, u32)> = pairs(&panes)
        .into_iter()
        .filter_map(|(card, pid)| Some((card, pid.parse().ok()?)))
        .collect();
    let seconds = var("DECK_M1_TAP_SECONDS")
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .unwrap_or(3600);
    let listener = crate::agent_status::listen_at(&base.join("status.sock"))
        .expect("the experiment's status socket");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .expect("the recorder's log");
    let _ = writeln!(out, "M1-tap state=listening pid={}", std::process::id());
    let stop = base.join("tap-stop");
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut seen = 0usize;
    while !stop.exists() && Instant::now() < until {
        let Ok((mut stream, _)) = listener.accept() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        seen += 1;
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut bytes = Vec::new();
        let _ = (&mut stream).take(2048).read_to_end(&mut bytes);
        // who sent it, read while the helper is still connected
        let peer = crate::procinfo::peer_pid(&stream);
        let table = crate::procinfo::processes();
        let text = String::from_utf8_lossy(&bytes);
        let line = text.trim_end_matches('\n');
        // The production listener dispatches by protocol family before any
        // parser runs (`agent_status::handle_stream`): a line that is not of
        // the lifecycle family goes to the Signal parser and never reaches
        // the lifecycle one. A client whose user also has Signal hooks
        // installed sends such lines to this same socket, so the recorder
        // keeps them apart as their own kind of line: which registered pane
        // shell is among the sender's ancestors, if any, and whether that
        // pane's foreground generation is among them too. Nothing of the
        // line is kept but whether it has the shape of a Signal line.
        if !lifecycle::is_family(line) {
            let chain = peer
                .map(|pid| {
                    crate::procinfo::ancestry_in(&table, pid, crate::agent_origin::ORIGIN_HOPS)
                })
                .unwrap_or_default();
            let under = panes.iter().find(|(_, pane)| chain.contains(pane));
            let under_foreground = under.is_some_and(|(_, pane)| {
                crate::agent_status::foreground_generation(&table, *pane)
                    .is_some_and(|leader| leader.pid() != *pane && chain.contains(&leader.pid()))
            });
            let signal_shaped = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .is_some_and(|value| {
                    value
                        .get("source")
                        .is_some_and(serde_json::Value::is_string)
                        && value.get("state").is_some_and(serde_json::Value::is_string)
                });
            let _ = writeln!(
                out,
                "M1-tap n={seen} card={} family={} by-helper={} under-foreground-client={under_foreground}",
                under.map_or("unknown", |(card, _)| card.as_str()),
                if signal_shaped { "signal" } else { "unknown" },
                peer.is_some_and(|pid| crate::procinfo::runs_file(pid, Path::new(&helper))),
            );
            // closing the stream is that helper's ACK as well
            drop(stream);
            continue;
        }
        // a hook helper is its client's direct child; a direct client is the
        // pane shell's child
        let client = peer.and_then(|pid| table.get(&pid)).map(|info| info.ppid);
        let pane = client.and_then(|pid| table.get(&pid)).map(|info| info.ppid);
        let card = pane
            .and_then(|pane| panes.iter().find(|(_, pid)| *pid == pane))
            .map_or("unknown", |(card, _)| card.as_str());
        let by_helper = peer.is_some_and(|pid| crate::procinfo::runs_file(pid, Path::new(&helper)));
        let (foreground, continuous) = match (peer, client, pane) {
            (Some(peer), Some(client), Some(pane)) => {
                let leader =
                    crate::agent_status::foreground_generation(&table, pane).map(|g| g.pid());
                let chain =
                    crate::procinfo::ancestry_in(&table, peer, crate::agent_origin::ORIGIN_HOPS);
                (
                    leader == Some(client),
                    leader.is_some_and(|leader| {
                        crate::agent_origin::terminal_continuous(&table, &chain, leader, pane)
                    }),
                )
            }
            _ => (false, false),
        };
        let generation =
            client
                .and_then(|pid| table.get(&pid))
                .map_or("unknown".to_string(), |info| {
                    format!(
                        "{}@{}.{:06}",
                        info.pid, info.start_seconds, info.start_micros
                    )
                });
        let fact = match lifecycle::parse(line, |source| {
            (source == "claude-code").then_some(&claude::RECEIPTS)
        }) {
            Ok(wire) => {
                let (event, word) = match wire.event {
                    lifecycle::Event::Claude(claude::Told::Start(source)) => (
                        "session-start",
                        match source {
                            claude::StartSource::Startup => "startup",
                            claude::StartSource::Resume => "resume",
                            claude::StartSource::Clear => "clear",
                            claude::StartSource::Compact => "compact",
                            claude::StartSource::Other => "other",
                        },
                    ),
                    lifecycle::Event::Claude(claude::Told::End(reason)) => (
                        "session-end",
                        match reason {
                            claude::EndReason::Clear => "clear",
                            claude::EndReason::Resume => "resume",
                            claude::EndReason::Logout => "logout",
                            claude::EndReason::PromptInputExit => "prompt-input-exit",
                            claude::EndReason::Other => "other",
                        },
                    ),
                    lifecycle::Event::Overflow => ("none", "overflow"),
                };
                format!(
                    "event={event} word={word} idh={} claims-this-socket={}",
                    id_digest(&salt, &wire.native_id),
                    wire.socket == socket
                )
            }
            Err(reason) => format!("refused={}", reason.replace(' ', "-")),
        };
        let _ = writeln!(
            out,
            "M1-tap n={seen} card={card} {fact} by-helper={by_helper} client-is-foreground={foreground} terminal-continuous={continuous} client={generation}"
        );
        // closing the stream is the helper's ACK
        drop(stream);
    }
    let _ = writeln!(out, "M1-tap state=stopped lines={seen}");
    let _ = std::fs::remove_file(base.join("status.sock"));
}

/// Not a test of its own: the controller of the native certification's stage
/// M1. A driver outside this process made a private tmux server with one
/// session per card and started one client in each. This entry is the Deck
/// of that experiment for ONE restart: it commits a Board naming those
/// cards, keeps a real queue row waiting, takes over the experiment's status
/// socket with the production listener, and runs the PRODUCTION transaction
/// (`tmux_lifecycle::restart_for_tests` over `assemble_reentry_env`, with
/// the real operation and activity gates) for every card: under Policy A,
/// or — when the driver names it (stage M2) — under Policy B, this
/// restart's explicit bypass reauthorization. The capability is
/// production's and the client Anthropic's product unless the driver names
/// a stand-in; a binary that does not fit the mode is refused.
///
/// It prints closed facts only — check names, outcomes, reason codes, pids,
/// births and counts — and never a native id: ids are compared in memory
/// and leave this process only as salted digests. It removes nothing; the
/// server it leaves behind is the new generation, recorded for the driver
/// to end.
#[test]
#[ignore = "run only by the native certification driver, against a server it made"]
fn native_m1_transact() {
    let var = |name: &str| std::env::var(name).ok();
    let (
        Some(base),
        Some(socket),
        Some(home),
        Some(zdot),
        Some(helper),
        Some(client),
        Some(cards),
        Some(expect),
        Some(salt),
    ) = (
        var("DECK_M1_BASE"),
        var("DECK_M1_SOCKET"),
        var("DECK_M1_HOME"),
        var("DECK_M1_ZDOT"),
        var("DECK_M1_HELPER"),
        var("DECK_M1_CLIENT"),
        var("DECK_M1_CARDS"),
        var("DECK_M1_EXPECT"),
        var("DECK_M1_SALT"),
    )
    else {
        return;
    };
    let standin = var("DECK_M1_STANDIN").is_some_and(|flag| flag == "1");
    let named = |name: &str| match var(name).as_deref() {
        Some("ordinary") => Some(Policy::Ordinary),
        Some("reapply-bypass") => Some(Policy::ReapplyBypass),
        _ => None,
    };
    // Policy A unless the driver names B (stage M2).
    let policy = match (var("DECK_M1_POLICY"), named("DECK_M1_POLICY")) {
        (None, _) => Policy::Ordinary,
        (Some(_), Some(policy)) => policy,
        (Some(_), None) => {
            println!("M1 refused=unknown-policy");
            std::process::exit(2);
        }
    };
    // What the checks below expect is that same policy. A stand-in run may
    // name another one for the checks alone, to show that they fail when the
    // requests are not that policy's.
    let checked = if standin {
        named("DECK_M1_TEST_CHECK_POLICY").unwrap_or(policy)
    } else {
        policy
    };
    let reapply = checked == Policy::ReapplyBypass;
    let (base, home, zdot, helper, client) = (
        PathBuf::from(base),
        PathBuf::from(home),
        PathBuf::from(zdot),
        PathBuf::from(helper),
        PathBuf::from(client),
    );
    let failed = std::cell::Cell::new(0usize);
    let check = |name: &str, ok: bool, detail: &str| {
        if !ok {
            failed.set(failed.get() + 1);
        }
        let verdict = if ok { "ok" } else { "FAIL" };
        if detail.is_empty() {
            println!("M1 check={name} {verdict}");
        } else {
            println!("M1 check={name} {verdict} {detail}");
        }
    };
    // Two modes, each with its own kind of binary: a stand-in (never
    // production's product) under its own stand-in capability, or a real
    // client — any release of the product — under production's capability.
    let product = ExecutableRef::read(&client, claude::CAPABILITY.product).is_ok();
    let capability: Option<&'static ReentryCapability> = match standin {
        true => World::standin_capability(&client),
        false if product => Some(&claude::CAPABILITY),
        false => None,
    };
    let Some(capability) = capability else {
        println!("M1 refused=client-does-not-fit-the-mode");
        std::process::exit(2);
    };
    // what the driver's server was given beyond the six names, to be handed
    // to the server this transaction starts as well
    let extra = pairs(&var("DECK_M1_ENV").unwrap_or_default());
    // (session, how its old client was started) and (session, expected outcome)
    let (cards, expect) = (pairs(&cards), pairs(&expect));
    // (session, digest of the id it exited in) and (session, digest of an id it left behind)
    let finals = pairs(&var("DECK_M1_FINAL").unwrap_or_default());
    let firsts = pairs(&var("DECK_M1_FIRST").unwrap_or_default());
    let world = World::native(
        &base,
        &socket,
        &home,
        &zdot,
        &helper,
        &client,
        capability,
        extra.clone(),
    );
    world.edit_cards(|list| {
        list.extend(
            cards
                .iter()
                .map(|(name, _)| (format!("card-{name}"), name.clone())),
        )
    });
    println!(
        "M1 capability={} cards={} policy={}",
        if standin { "stand-in" } else { "production" },
        cards.len(),
        if policy == Policy::ReapplyBypass {
            "reapply-bypass"
        } else {
            "ordinary"
        }
    );
    // this process's Deck data (Board door, queue) lives in the experiment
    let deck_dir = crate::datadir::deck_dir();
    check(
        "data-directory-inside-the-experiment",
        var("DECK_M1_DATA").is_some_and(|root| deck_dir.starts_with(root))
            && !deck_dir.starts_with(&home),
        "",
    );
    let server_env = |name: &str| -> Option<String> {
        world
            .raw(&["show-environment", "-g", name])
            .ok()
            .and_then(|line| {
                line.trim()
                    .split_once('=')
                    .map(|(_, value)| value.to_string())
            })
    };
    // what a server hands its panes: the experiment's status socket, the
    // clients' home, the pane shells' own startup directory
    let environment_as_made = || {
        server_env("DECK_STATUS_SOCK").as_deref() == world.base.join("status.sock").to_str()
            && server_env("HOME").as_deref() == home.to_str()
            && server_env("ZDOTDIR").as_deref() == zdot.to_str()
            && extra
                .iter()
                .all(|(name, value)| server_env(name).as_deref() == Some(value.as_str()))
    };
    let pane_pid = |rows: &[PaneRow], name: &str| -> Option<u32> {
        let mut found = rows.iter().filter(|row| row.session_name == name);
        let pid = found.next()?.pane_pid;
        found.next().is_none().then_some(pid)
    };

    // ---- the old generation, as the driver left it
    let old_server = world.server_pid();
    check("old-server-environment", environment_as_made(), "");
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap_or_default();
    // (session, pane shell, client pid, client birth)
    let mut old = Vec::new();
    for (name, _) in &cards {
        let pane = pane_pid(&rows, name);
        let client = pane
            .and_then(crate::agent_status::live_generation)
            .filter(|generation| Some(generation.pid()) != pane);
        if let (Some(pane), Some(client)) = (pane, client) {
            println!(
                "M1 old card={name} pane={pane} client={}@{}.{:06}",
                client.pid(),
                client.birth().0,
                client.birth().1
            );
            old.push((name.clone(), pane, client.pid(), client.birth()));
        }
    }
    check(
        "old-clients-in-the-foreground",
        old.len() == cards.len() && rows.len() == cards.len(),
        &format!("panes={}", rows.len()),
    );
    // a real queue with a row waiting for the first card's session and one
    // for a session that is not on this server
    let first = cards
        .first()
        .map(|(name, _)| name.clone())
        .unwrap_or_default();
    let queues =
        crate::scheduler::Queues::new(crate::scheduler::waiting_for_tests(&[&first, "elsewhere"]));
    let waiting = vec![(first, true), ("elsewhere".to_string(), false)];
    let on_disk = || queue_rows_at(&crate::scheduler::queue_path());

    // ---- preview and approval
    let view = match world.with_env(preview) {
        Ok(view) => serde_json::to_value(&view).unwrap_or_default(),
        Err(error) => {
            println!("M1 preview=refused code={}", error.message());
            std::process::exit(1);
        }
    };
    let entries = view["entries"].as_array().cloned().unwrap_or_default();
    check(
        "preview-candidates-and-launch-profiles",
        entries.len() == cards.len()
            && cards.iter().all(|(name, profile)| {
                entries.iter().any(|entry| {
                    entry["session"] == name.as_str()
                        && entry["kind"] == "candidate"
                        && entry["launchProfile"] == profile.as_str()
                })
            }),
        "",
    );
    let client_name = client
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?");
    let shown = view.to_string();
    check(
        "preview-carries-no-native-identity",
        ![
            home.to_str().unwrap_or("?"),
            world.base.to_str().unwrap_or("?"),
            client_name,
            "sha",
            "pid",
        ]
        .iter()
        .any(|private| shown.contains(private)),
        "",
    );
    let token = view["token"].as_str().unwrap_or_default().to_string();
    let names: Vec<&str> = cards.iter().map(|(name, _)| name.as_str()).collect();
    let approval = world.approval(&token, &names, policy);
    let at_kill = std::rc::Rc::new(RefCell::new(None));
    *world.intercept.borrow_mut() = Some(Box::new({
        let at_kill = at_kill.clone();
        move |args, _| {
            if args.first() == Some(&"kill-server") {
                *at_kill.borrow_mut() = Some(on_disk());
            }
            None
        }
    }));

    // ---- the production transaction, once
    let _scope = crate::session_runtime::test_activity_scope();
    let reply = world.restart_full(
        "req-m1",
        Some(&approval),
        POST_BUDGET,
        true,
        &|| {},
        &|sessions| crate::scheduler::pause_for_server_restart(&queues, sessions),
        &|_, _, _| {},
    );
    let (was, now, result) = match reply {
        Ok(reply) => reply,
        Err(error) => {
            println!("M1 restart=refused code={}", error.message());
            std::process::exit(1);
        }
    };
    let Some(result) = result else {
        println!("M1 restart=no-reentry-result");
        std::process::exit(1);
    };
    println!(
        "M1 restart old-server={was} new-server={now} replaced={} state={:?}",
        result.server_replaced, result.state
    );
    // the server replacement is its own conclusion, whatever the entries say
    check(
        "server-replaced-and-settled",
        was == old_server
            && now != 0
            && now != old_server
            && result.server_replaced
            && result.state == OperationState::Complete,
        "",
    );
    let mut as_expected = result.entries.len() == expect.len();
    for entry in &result.entries {
        let word = serde_json::to_value(entry.outcome)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let codes: Vec<String> = entry
            .warnings
            .iter()
            .filter_map(|warning| serde_json::to_value(warning).ok())
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        println!(
            "M1 entry card={} outcome={word} reason={} shell={} client={} settled={} warnings={} warning-codes={}",
            entry.session,
            entry.reason.map_or("none", Reason::code),
            entry.shell,
            entry.client_observed,
            entry.settled,
            entry.warnings.len(),
            if codes.is_empty() {
                "none".to_string()
            } else {
                codes.join("+")
            }
        );
        as_expected &= entry.settled
            && expect
                .iter()
                .any(|(name, wanted)| *name == entry.session && *wanted == word);
    }
    check("every-entry-has-its-own-expected-outcome", as_expected, "");
    // where the driver names the reason an entry must end with
    if let Some(reasons) = var("DECK_M1_REASONS") {
        let reasons = pairs(&reasons);
        check(
            "every-named-entry-has-its-expected-reason",
            !reasons.is_empty()
                && reasons.iter().all(|(name, wanted)| {
                    result.entries.iter().any(|entry| {
                        entry.session == *name
                            && entry.reason.map_or("none", Reason::code) == wanted.as_str()
                    })
                }),
            "",
        );
    }
    // how the driver says each card's old client was started
    let started_with_bypass = |name: &str| {
        cards
            .iter()
            .any(|(card, profile)| card == name && profile == "bypass")
    };
    if reapply {
        // B's own word for what it did — "reauthorized", never "kept" — is on
        // exactly the confirmed entries of the bypass launches
        check(
            "policy-b-notes-the-reauthorization-on-the-bypass-launches-only",
            result.entries.iter().all(|entry| {
                if started_with_bypass(&entry.session) && entry.outcome == Outcome::Confirmed {
                    entry.warnings == [Warning::BypassReauthorized]
                } else {
                    entry.warnings.is_empty()
                }
            }),
            "",
        );
    } else {
        check(
            "policy-a-carries-no-warning",
            result.entries.iter().all(|entry| entry.warnings.is_empty()),
            "",
        );
    }

    // ---- once only, and exactly the request
    let launched: Vec<&str> = expect
        .iter()
        .filter(|(_, wanted)| wanted == "confirmed" || wanted == "unconfirmed")
        .map(|(name, _)| name.as_str())
        .collect();
    let pasted = world.pasted();
    let prefix = format!("'{}' --resume ", client.display());
    let overridden = format!(
        "'{}' --dangerously-skip-permissions --resume ",
        client.display()
    );
    // (carries the override, the id). Nothing but the frozen executable, the
    // resume flag and one native-shaped id makes such a line — and, under
    // Policy B only, the one bypass flag before them: no other word can hide
    // in it, and under Policy A a line with the flag is not a request at all
    let requests: Vec<(bool, &str)> = pasted
        .iter()
        .filter_map(|text| {
            text.strip_prefix(&prefix)
                .map(|id| (false, id))
                .or_else(|| {
                    text.strip_prefix(&overridden)
                        .filter(|_| reapply)
                        .map(|id| (true, id))
                })
        })
        .filter(|(_, id)| claude::native_id_ok(id))
        .collect();
    let ids: Vec<&str> = requests.iter().map(|(_, id)| *id).collect();
    check(
        "one-request-per-launched-entry",
        pasted.len() == launched.len() && world.enters() == launched.len(),
        &format!("requests={} enters={}", pasted.len(), world.enters()),
    );
    check(
        if reapply {
            "every-request-is-the-executable-the-reauthorized-flag-or-none-resume-and-one-id"
        } else {
            "every-request-is-the-executable-resume-and-one-id"
        },
        ids.len() == pasted.len(),
        "",
    );
    // which request was whose: by the digest of the session each card's old
    // client was last seen to start. The flag is on the requests of the
    // bypass launches under Policy B, and on no other request.
    let mut flag_as_decided = true;
    for name in &launched {
        let final_digest = finals
            .iter()
            .find(|(card, _)| card == name)
            .map(|(_, digest)| digest.as_str());
        let request = requests
            .iter()
            .find(|(_, id)| Some(id_digest(&salt, id).as_str()) == final_digest);
        let wanted = reapply && started_with_bypass(name);
        match request {
            Some((flag, _)) => {
                println!("M1 request card={name} bypass-override={flag}");
                flag_as_decided &= *flag == wanted;
            }
            None => {
                println!("M1 request card={name} bypass-override=unknown");
                flag_as_decided = false;
            }
        }
    }
    check(
        "the-bypass-flag-is-on-the-requests-this-policy-decides-and-on-no-other",
        flag_as_decided,
        &format!(
            "with-the-flag={}",
            requests.iter().filter(|(flag, _)| *flag).count()
        ),
    );
    let digests: Vec<String> = ids.iter().map(|id| id_digest(&salt, id)).collect();
    let distinct = digests
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        == digests.len();
    check(
        "requests-name-the-sessions-the-clients-exited-in",
        distinct
            && launched.iter().all(|name| {
                finals
                    .iter()
                    .any(|(card, digest)| card == name && digests.contains(digest))
            }),
        &format!("recorded-final-sessions={}", finals.len()),
    );
    if var("DECK_M1_FIRST").is_some() {
        check(
            "no-request-names-a-session-left-behind",
            !firsts.is_empty() && firsts.iter().all(|(_, digest)| !digests.contains(digest)),
            &format!("recorded-earlier-sessions={}", firsts.len()),
        );
    } else {
        // a matrix in which no client switched session says nothing here
        println!("M1 info no-earlier-session-was-recorded-for-this-matrix");
    }
    check(
        "a-repeated-invoke-reads-the-stored-result",
        known("req-m1").as_ref() == Some(&result)
            && !operation("req-other").known
            && world.pasted().len() == pasted.len(),
        "",
    );

    // ---- the queue stays paused
    check(
        "queue-row-paused-on-disk-before-the-kill",
        at_kill.borrow().as_deref() == Some(&waiting[..]),
        "",
    );
    check(
        "queue-row-still-paused-after-the-reentry",
        crate::scheduler::paused_for_tests(&queues.q.lock_or_recover()) == waiting
            && on_disk() == waiting,
        "",
    );

    // ---- the new generation
    check("new-server-environment", environment_as_made(), "");
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap_or_default();
    let mut record = format!("server {now}\n");
    let mut fresh = rows.iter().all(|row| row.server_pid == now);
    for (name, pane_before, client_before, birth_before) in &old {
        let pane_now = pane_pid(&rows, name);
        if !launched.contains(&name.as_str()) {
            // an entry that was not launched got no shell on the new server
            println!("M1 new card={name} shell={}", pane_now.is_some());
            fresh &= pane_now.is_none();
            continue;
        }
        let client_now = pane_now.and_then(crate::agent_status::live_generation);
        let old_gone = crate::procinfo::generation_liveness(*client_before, *birth_before)
            == crate::procinfo::Liveness::Gone;
        let (Some(pane_now), Some(client_now)) = (pane_now, client_now) else {
            println!("M1 new card={name} unreadable old-client-gone={old_gone}");
            fresh = false;
            continue;
        };
        let shell_birth = crate::procinfo::process_start(pane_now).unwrap_or_default();
        println!(
            "M1 new card={name} pane={pane_now} client={}@{}.{:06} old-client-gone={old_gone}",
            client_now.pid(),
            client_now.birth().0,
            client_now.birth().1
        );
        fresh &= old_gone
            && pane_now != *pane_before
            && client_now.pid() != pane_now
            && (client_now.pid(), client_now.birth()) != (*client_before, *birth_before);
        record += &format!(
            "pane {name} {pane_now}@{}.{:06}\nclient {name} {}@{}.{:06}\n",
            shell_birth.0,
            shell_birth.1,
            client_now.pid(),
            client_now.birth().0,
            client_now.birth().1
        );
    }
    check("a-new-generation-for-every-launched-entry", fresh, "");
    let recorded = std::fs::write(world.base.join("new-generation"), record).is_ok();
    check("new-generation-recorded-for-the-driver", recorded, "");
    let disk = std::fs::read_to_string(world.base.join("tmux-lifecycle.json")).unwrap_or_default();
    check(
        "lifecycle-file-names-the-server-only",
        !disk.is_empty()
            && !["resume", "card-", "bypass", client_name]
                .iter()
                .any(|private| disk.contains(private))
            && !ids.iter().any(|id| disk.contains(id)),
        "",
    );
    let observed = rows
        .iter()
        .filter(|row| crate::agent_status::projected(row).is_some())
        .count();
    println!("M1 info signal-observations-on-the-new-panes={observed}");
    if standin {
        check("a-receipt-created-no-signal-observation", observed == 0, "");
        // the stand-in records its own arguments: what was typed is what ran
        let resumed: Vec<(bool, String)> = world
            .launches()
            .iter()
            .filter_map(|line| {
                line.strip_prefix("--resume ")
                    .map(|id| (false, id))
                    .or_else(|| {
                        line.strip_prefix("--dangerously-skip-permissions --resume ")
                            .map(|id| (true, id))
                    })
            })
            .map(|(flag, id)| (flag, id_digest(&salt, id)))
            .collect();
        let asked: Vec<(bool, String)> = requests
            .iter()
            .map(|(flag, id)| (*flag, id_digest(&salt, id)))
            .collect();
        check(
            "stand-in-clients-ran-with-exactly-those-arguments",
            resumed.len() == launched.len() && resumed.iter().all(|ran| asked.contains(ran)),
            "",
        );
    }
    println!("M1 result failed-checks={}", failed.get());
    assert_eq!(failed.get(), 0, "stage M1: a check failed");
}

// ---------- crash matrix: a real controller process killed at each phase ----------

const CRASH_PHASES: &[&str] = &[
    "exit",
    "intent",
    "kill",
    "new-server",
    "shell-create",
    "launch-transport",
    "receipt",
];

/// Not a test of its own: the controller process of the crash matrix. It
/// attaches to the parent's world, runs the production restart with an
/// approved re-entry and the REAL queue pause over a waiting row, and aborts
/// itself at the named phase. As `reboot` it is the process started after
/// the crash: it runs the production boot reconcile on the same server and
/// lifecycle file, loads the queue as a boot does, and checks that nothing
/// of the dead operation exists to be replayed.
#[test]
#[ignore = "run only as the crash matrix's controller process"]
fn crash_controller() {
    let (Ok(base), Ok(socket), Ok(phase)) = (
        std::env::var("DECK_RT_BASE"),
        std::env::var("DECK_RT_SOCKET"),
        std::env::var("DECK_RT_PHASE"),
    ) else {
        return;
    };
    let world = World::attach(Path::new(&base), &socket);
    if phase == "reboot" {
        // A fresh process after the crash: no plan, no token, no lease, no
        // result.
        assert!(!operation("req-crash-matrix").known);
        assert!(!lifecycle::armed());
        // the boot gate's reconcile, on this server and its lifecycle file
        let lifecycle_file = world.base.join("tmux-lifecycle.json");
        crate::tmux_lifecycle::reconcile_for_tests(
            &|args| world.tmux(args),
            &world.socket,
            &lifecycle_file,
            &|| crate::tmux::init_deck_server_with(&|args| world.tmux(args), &world.base),
        );
        let (intent, _) = crate::tmux_lifecycle::lifecycle_file_for_tests(&lifecycle_file);
        assert_eq!(intent, None, "the reconcile settled the restart intent");
        world
            .raw(&["display-message", "-p", "#{pid}"])
            .expect("a server is running after the reconcile");
        // the reconcile reads and writes the server only: no Agent input
        assert!(world.pasted().is_empty());
        assert_eq!(world.enters(), 0);
        assert!(!world.calls.borrow().iter().any(|(args, _)| args
            .iter()
            .any(|arg| arg.contains("send-keys") || arg.contains("paste-buffer"))));
        // the queue as a boot loads it: the durable pause is still there
        if let Ok(queue) = std::env::var("DECK_RT_QUEUE") {
            std::fs::copy(&queue, crate::scheduler::queue_path()).expect("the crashed queue");
            let loaded = crate::scheduler::load_queue();
            assert_eq!(
                crate::scheduler::paused_for_tests(&loaded),
                [("a".to_string(), true)],
                "the row paused before the kill is paused after the reboot"
            );
        }
        // a preview reads only
        let _ = world.with_env(preview);
        assert!(world.pasted().is_empty());
        let _ = std::fs::remove_dir_all(crate::datadir::deck_dir());
        std::mem::forget(world);
        return;
    }
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::ReapplyBypass);
    let queues = crate::scheduler::Queues::new(crate::scheduler::waiting_for_tests(&["a"]));
    std::fs::write(
        world.base.join("queue-path"),
        crate::scheduler::queue_path().to_str().unwrap(),
    )
    .unwrap();
    let die = |after: bool, run: &dyn Fn() -> Option<Result<String, DeckError>>| {
        if after {
            let _ = run();
        }
        std::process::abort();
    };
    *world.intercept.borrow_mut() = Some(Box::new({
        let run = world.remote();
        let (base, socket, phase) = (world.base.clone(), world.socket.clone(), phase.clone());
        move |args, input| {
            let real = || -> Option<Result<String, DeckError>> {
                match input {
                    None => Some(run(args)),
                    Some(input) => {
                        let mut command = Command::new(
                            Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("binaries/tmux-aarch64-apple-darwin"),
                        );
                        command
                            .env_clear()
                            .env("HOME", &base)
                            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                            .args(["-f", "/dev/null", "-L", &socket]);
                        Some(crate::tmux::command_with_stdin(&mut command, args, input))
                    }
                }
            };
            let exit_key = args.iter().any(|arg| arg.ends_with("C-d"));
            let enter = args
                .iter()
                .any(|arg| arg.starts_with("send-keys") && arg.ends_with("Enter"));
            let paste = input.is_some() && args.iter().any(|arg| arg.starts_with("deck-send-"));
            match phase.as_str() {
                "exit" if exit_key => die(true, &real),
                "intent" if args[0] == "kill-server" => die(false, &real),
                "kill" if args[0] == "kill-server" => die(true, &real),
                "new-server" if args[0] == "start-server" => die(true, &real),
                "shell-create" if args.contains(&"new-session") => die(true, &real),
                "launch-transport" if paste => die(true, &real),
                "receipt" if enter => die(true, &real),
                _ => {}
            }
            None
        }
    }));
    let _ = world.restart_full(
        "req-crash-matrix",
        Some(&approval),
        Duration::from_secs(8),
        false,
        &|| {},
        &|sessions| crate::scheduler::pause_for_server_restart(&queues, sessions),
        &|_, _, _| {},
    );
    // reaching here means the phase was never hit: fail loudly in the parent
    std::process::exit(3);
}

fn controller(world: &World, phase: &str, queue: Option<&Path>) -> std::process::ExitStatus {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "agent_reentry::tests::crash_controller",
            "--ignored",
            "--nocapture",
        ])
        .env("DECK_RT_BASE", &world.base)
        .env("DECK_RT_SOCKET", &world.socket)
        .env("DECK_RT_PHASE", phase)
        .stdout(std::process::Stdio::null())
        .stderr(
            std::fs::File::create(world.base.join(format!("controller-{phase}.err")))
                .map(std::process::Stdio::from)
                .unwrap_or_else(|_| std::process::Stdio::null()),
        );
    if let Some(queue) = queue {
        command.env("DECK_RT_QUEUE", queue);
    }
    command.status().expect("controller process")
}

/// A real controller process is killed at each phase of the transaction.
/// Whatever it had done stays done; nothing more happens. Then a fresh
/// process runs the production boot reconcile on the same server: the
/// restart intent on disk is settled (finished, recovered or dropped), a
/// server is running, the queue row paused before the kill loads as paused,
/// and no Agent is started, typed into or replayed.
#[test]
fn a_controller_killed_at_any_phase_leaves_no_replay_and_no_agent_data() {
    use std::os::unix::process::ExitStatusExt;
    for phase in CRASH_PHASES {
        let world = World::new();
        world.card(
            "a",
            &format!("session={ID_A}"),
            Some(&format!("{{agent}} {BYPASS}")),
        );
        let original = world.server_pid();
        let status = controller(&world, phase, None);
        assert_eq!(
            status.signal(),
            Some(libc::SIGABRT),
            "{phase}: the controller died at its phase ({status:?})"
        );
        // what the dead controller had started may still finish
        std::thread::sleep(Duration::from_millis(1500));
        let launched = world.launches().len();
        let expected = if *phase == "receipt" { 2 } else { 1 };
        assert_eq!(launched, expected, "{phase}: launches after the crash");
        // the REAL queue document the dead controller persisted
        let queue = PathBuf::from(
            std::fs::read_to_string(world.base.join("queue-path")).expect("the queue's path"),
        );
        let paused = queue_rows_at(&queue);
        if *phase == "exit" {
            assert!(
                paused.is_empty(),
                "{phase}: no pause before the exits finished"
            );
        } else {
            assert_eq!(
                paused,
                [("a".to_string(), true)],
                "{phase}: the pause precedes the kill and survives the crash"
            );
        }
        let lifecycle_file = world.base.join("tmux-lifecycle.json");
        let disk = std::fs::read_to_string(&lifecycle_file).unwrap_or_default();
        let intent_kept = matches!(*phase, "intent" | "kill" | "new-server");
        let (intent, _) = crate::tmux_lifecycle::lifecycle_file_for_tests(&lifecycle_file);
        assert_eq!(
            intent.map(|(old_pid, _)| old_pid),
            intent_kept.then_some(original),
            "{phase}: the server intent is on disk exactly while the replacement is open"
        );
        for private in [
            ID_A,
            "resume",
            "card-a",
            "bypass",
            "policy",
            "fake_reentry_agent",
        ] {
            assert!(
                !disk.contains(private),
                "{phase}: the lifecycle file holds {private}"
            );
        }
        // A fresh process runs the boot reconcile on what the crash left.
        let before = world
            .raw(&["display-message", "-p", "#{pid}"])
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok());
        assert_eq!(
            before.is_none(),
            *phase == "kill",
            "{phase}: server after the crash"
        );
        let reboot = controller(
            &world,
            "reboot",
            (*phase != "exit").then_some(queue.as_path()),
        );
        assert!(
            reboot.success(),
            "{phase}: the fresh process reconciled a clean slate: {}",
            std::fs::read_to_string(world.base.join("controller-reboot.err")).unwrap_or_default()
        );
        let after = world.server_pid();
        match *phase {
            // the confirmed restart is finished: the old server is replaced
            "intent" => assert_ne!(Some(after), before, "{phase}"),
            // the server the crash left missing is started
            "kill" => {}
            // nothing to finish: the running server is left as it is
            _ => assert_eq!(Some(after), before, "{phase}"),
        }
        let (intent, notice) = crate::tmux_lifecycle::lifecycle_file_for_tests(&lifecycle_file);
        assert_eq!(intent, None, "{phase}: no intent after the reconcile");
        if matches!(*phase, "intent" | "kill") {
            assert_eq!(notice.as_deref(), Some("restartCompleted"), "{phase}");
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(world.launches().len(), launched, "{phase}: no replay");
        // the dead controller's own data directory, by the path it recorded
        if let Some(dir) = queue.parent().filter(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("deck-test-"))
        }) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Cards that share one binary: the preview reads its content and validates
/// its signature ONCE, the freeze reads and validates nothing, and the post
/// phase reads the content once more — equal content is the same signed
/// code, so there is no second validation. Every client is still bound to
/// that one reference on its own, at every phase.
#[test]
fn cards_that_share_a_binary_read_its_content_once_per_restart() {
    use crate::procinfo::{DYNAMIC_CHECKS, FULL_READS, STATIC_VALIDATIONS};
    let world = World::new();
    for (card, id) in [("a", ID_A), ("b", ID_B), ("c", ID_C)] {
        world.card(card, &format!("session={id}"), Some("{agent}"));
    }
    let counts = || {
        (
            FULL_READS.with(std::cell::Cell::get),
            STATIC_VALIDATIONS.with(std::cell::Cell::get),
            DYNAMIC_CHECKS.with(std::cell::Cell::get),
        )
    };
    let before = counts();
    let (token, _) = world.preview();
    let previewed = counts();
    assert_eq!(previewed.0 - before.0, 1, "one full read for three clients");
    assert_eq!(previewed.1 - before.1, 1, "one full signature validation");
    // the first client: is it the product, then its binding; the others: their binding
    assert_eq!(previewed.2 - before.2, 4, "each client bound on its own");
    let approval = world.approval(&token, &["a", "b", "c"], Policy::Ordinary);
    let (_, _, result) = world.restart("req-shared", &approval).expect("restart");
    let after = counts();
    let result = result.unwrap();
    for card in ["a", "b", "c"] {
        assert_eq!(entry(&result, card).outcome, Outcome::Confirmed, "{card}");
    }
    assert_eq!(
        after.0 - previewed.0,
        1,
        "one full read for three entries of one binary"
    );
    assert_eq!(after.1 - previewed.1, 0, "no second signature validation");
    println!(
        "COUNT restart dynamic_checks_on_the_worker={}",
        after.2 - previewed.2
    );
    assert!(
        after.2 - previewed.2 >= 6,
        "every client is bound again at the freeze and after its launch"
    );
}

/// Clears this thread's read stall when dropped, a failed assertion included.
struct Stall;

impl Stall {
    fn set(time: Duration) -> Self {
        crate::procinfo::READ_STALL.with(|stall| stall.set(Some(time)));
        Self
    }
}

impl Drop for Stall {
    fn drop(&mut self) {
        crate::procinfo::READ_STALL.with(|stall| stall.set(None));
    }
}

/// The post phase's re-read of the frozen program cannot be interrupted. When
/// it outlives the phase, nothing is created and nothing is typed, and the
/// entry says that the check did not finish — not that the program changed.
/// When it returns in time but leaves less than one attempt's share, no shell
/// is created for a request that could no longer be made.
#[test]
fn a_program_check_that_outlives_its_phase_types_nothing_and_is_not_called_a_change() {
    for (stall, reason) in [
        (Duration::from_millis(3600), Reason::CheckIncomplete),
        (Duration::from_millis(2000), Reason::LaunchRefused),
    ] {
        let world = World::new();
        world.card("a", &format!("session={ID_A}"), Some("{agent}"));
        let (token, _) = world.preview();
        let approval = world.approval(&token, &["a"], Policy::Ordinary);
        let stalled = RefCell::new(None);
        let started = Instant::now();
        let (_, _, result) = world
            .restart_with(
                "req-late",
                Some(&approval),
                Duration::from_secs(3),
                // after the exit, before the replacement: every full read
                // on this worker stalls from here on
                &|| {
                    *stalled.borrow_mut() = Some(Stall::set(stall));
                    Ok(())
                },
                &|_, _, _| {},
            )
            .expect("the restart itself succeeded");
        drop(stalled);
        assert!(started.elapsed() >= stall, "the read was waited for");
        let result = result.unwrap();
        assert!(result.server_replaced, "{reason:?}");
        assert_eq!(result.state, OperationState::Complete);
        let late = entry(&result, "a");
        assert_eq!(late.outcome, Outcome::NotAttempted, "{reason:?}");
        assert_eq!(late.reason, Some(reason));
        assert!(
            !late.shell && !world.has_session("a"),
            "no shell: {reason:?}"
        );
        assert!(world.pasted().is_empty(), "{reason:?}");
        assert_eq!(world.enters(), 0, "{reason:?}");
        assert_eq!(world.launches().len(), 1, "only the user's own start");
        // the operation is over: the same approval starts nothing
        let again = world.restart("req-late-2", &approval).unwrap_err();
        assert_eq!(again.message(), "reentry-preview-expired");
    }
}

/// The preview is bounded and runs one at a time. A program whose read does
/// not finish inside the preview's share is shown as not checked — never as
/// a candidate and never as changed — and a preview does not start its reads
/// beside one that is still running.
#[test]
fn a_preview_that_cannot_finish_its_checks_says_so_and_previews_run_one_at_a_time() {
    use crate::session_runtime::Deadline;
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let (token, view) = {
        let _stall = Stall::set(Duration::from_millis(700));
        let _phase = Deadline::until(Instant::now() + Duration::from_millis(350));
        world.preview()
    };
    assert_eq!(view["entries"][0]["kind"], "unsupported");
    assert_eq!(view["entries"][0]["reason"], "check-incomplete");
    assert!(view["entries"][0]["launchProfile"].is_null());
    // what was not checked cannot be approved
    let error = world
        .restart(
            "r-unchecked",
            &world.approval(&token, &["a"], Policy::Ordinary),
        )
        .unwrap_err();
    assert_eq!(error.message(), "reentry-preview-expired");
    assert!(world.pasted().is_empty());
    // unhurried, the same client is a candidate
    let (_, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    // While one preview holds the turn, another reads nothing. It waits for
    // the turn no longer than its OWN limit — the holder's is ten seconds
    // away — and says busy.
    let turn = Turn::take(Instant::now() + PREVIEW_BUDGET).expect("a free turn");
    let reads = crate::procinfo::FULL_READS.with(std::cell::Cell::get);
    let checks = crate::procinfo::DYNAMIC_CHECKS.with(std::cell::Cell::get);
    let own = Duration::from_millis(200);
    let asked = Instant::now();
    let busy = world.with_env(|env| preview_by(env, asked + own));
    let waited = asked.elapsed();
    assert_eq!(busy.unwrap_err().message(), "reentry-preview-busy");
    assert!(
        waited >= own && waited < own + Duration::from_secs(2),
        "waited to its own limit and not to the holder's: {waited:?}"
    );
    assert_eq!(
        crate::procinfo::FULL_READS.with(std::cell::Cell::get),
        reads
    );
    assert_eq!(
        crate::procinfo::DYNAMIC_CHECKS.with(std::cell::Cell::get),
        checks
    );
    drop(turn);
    // a holder that is already past its own deadline is stalled: the next
    // preview is refused at once instead of waiting for it
    let stalled = Turn::take(Instant::now() + Duration::from_millis(30)).expect("a free turn");
    while Instant::now() < stalled.end {
        std::thread::sleep(Duration::from_millis(5));
    }
    let asked = Instant::now();
    // the caller's own limit is far away: it is not what refuses
    let refused = world.with_env(|env| preview_by(env, asked + Duration::from_secs(3)));
    assert_eq!(refused.unwrap_err().message(), "reentry-preview-busy");
    assert!(
        asked.elapsed() < Duration::from_millis(500),
        "not waited for"
    );
    drop(stalled);
    assert_eq!(world.preview().1["entries"][0]["kind"], "candidate");
}

/// The production shell creator on a private server: exactly the requested
/// directory, no command, an existing session is not borrowed, and with the
/// restore asked for but nothing saved it is a clean shell.
#[test]
fn the_production_shell_creator_makes_one_command_less_shell_in_the_exact_directory() {
    let world = World::new();
    let dir = world.base.join("cards").join("exact");
    std::fs::create_dir_all(&dir).unwrap();
    let create = |name: &str, dir: &Path, restore: bool| {
        crate::commands::create_reentry_shell(
            &|args| world.tmux(args),
            &|args, input| world.tmux_stdin(args, input),
            name,
            dir,
            restore,
        )
    };
    create("fresh", &dir, true).expect("a new shell");
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(Path::new(&rows[0].path), dir);
    assert!(crate::context::shell_process(Some(&rows[0].command)));
    // nothing was typed: no key and no paste went through the creator
    assert_eq!(world.enters(), 0);
    assert!(!world
        .calls
        .borrow()
        .iter()
        .any(|(args, _)| args.iter().any(|arg| arg.contains("send-keys"))));
    assert_eq!(
        create("fresh", &dir, false).unwrap_err().message(),
        "session-exists"
    );
    assert!(create("gone", &world.base.join("missing"), false).is_err());
    assert!(!world.has_session("gone"), "no fallback directory");
    assert!(create("bad name", &dir, false).is_err());
}

// ---------- a validation that cannot be interrupted, and answers late ----------
//
// `procinfo::identity_hold` keeps back the truthful answer of chosen dynamic
// validations: the call returns late, or not until the test lets it. What is
// tested below is what each caller does with that — the full-read stall tests
// above are about another call.

use crate::procinfo::identity_hold as hold;

/// This world's re-entry environment, assembled on whichever thread runs the
/// returned closure: what the preview's own worker is handed in production
/// (`tmux_lifecycle::agent_reentry_preview`), with this world's server.
fn remote_read(world: &World) -> impl FnOnce() -> Result<Previewed, DeckError> + Send + 'static {
    let tmux = world.remote();
    let (socket, helper, home) = (
        world.socket.clone(),
        world.helper.clone(),
        world.home.clone(),
    );
    let (capability, observation, baseline) = (
        world.capability,
        world.observation.get(),
        world.baseline.get(),
    );
    move || {
        crate::tmux_lifecycle::assemble_reentry_env(
            &crate::tmux_lifecycle::ReentryParts {
                tmux: &tmux,
                tmux_stdin: &|_, _| Err(DeckError::new(ErrorKind::Tmux, "a preview types nothing")),
                socket: &socket,
                capability,
                helper: Some(helper),
                hooks_installed: observation,
                home: &home,
                baseline,
            },
            read_preview,
        )
    }
}

/// The token of the one preview that can be confirmed right now.
fn kept_token() -> Option<String> {
    with_registry(|registry| {
        registry
            .preview
            .as_ref()
            .map(|preview| preview.token.clone())
    })
}

/// R1, preview. The check of a client's program ends in a validation that
/// cannot be interrupted, and a deadline scope does not answer the caller.
/// Read on the calling thread, a validation that answers "yes" after the
/// preview's share names no candidate. Asked through the preview's worker —
/// as the command asks — a validation that does not return leaves the caller
/// with a timeout AT its deadline and keeps nothing: the earlier preview is
/// neither replaced nor revived, the stalled worker keeps the turn so that no
/// second thread or read starts beside it, and what it finally returns has
/// no receiver.
#[test]
fn a_preview_whose_validation_does_not_return_is_answered_in_time_and_keeps_nothing() {
    use crate::procinfo::{DYNAMIC_CHECKS, FULL_READS};
    use crate::session_runtime::Deadline;
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let client = world.foreground_pid("a");
    let (_, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    {
        // the validation answers yes, after the preview's share
        let me = std::thread::current().id();
        let held = hold::install(
            move |pid| pid == client && std::thread::current().id() == me,
            hold::Until::DeadlinePassed,
        );
        let before = (
            FULL_READS.with(std::cell::Cell::get),
            DYNAMIC_CHECKS.with(std::cell::Cell::get),
        );
        let (token, view) = {
            let _phase = Deadline::until(Instant::now() + Duration::from_millis(300));
            world.preview()
        };
        assert_eq!(held.entered(), 1, "asked once");
        assert_eq!(
            (
                FULL_READS.with(std::cell::Cell::get),
                DYNAMIC_CHECKS.with(std::cell::Cell::get)
            ),
            (before.0, before.1 + 1),
            "nothing is read or asked after a late answer"
        );
        assert_eq!(view["entries"][0]["kind"], "unsupported");
        assert_eq!(view["entries"][0]["reason"], "check-incomplete");
        assert!(view["entries"][0]["launchProfile"].is_null());
        // what was not checked in time cannot be approved
        let error = world
            .restart(
                "r-late-preview",
                &world.approval(&token, &["a"], Policy::Ordinary),
            )
            .unwrap_err();
        assert_eq!(error.message(), "reentry-preview-expired");
        assert!(world.pasted().is_empty());
        assert_eq!(
            world.foreground_pid("a"),
            client,
            "the client was left alone"
        );
    }
    // an earlier preview the dialog may still be showing
    let (earlier, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    assert_eq!(kept_token().as_deref(), Some(earlier.as_str()));
    // through the worker: the validation answers yes and does not return
    let held = hold::install(
        move |pid| pid == client && std::thread::current().name() == Some("deck-reentry-preview"),
        hold::Until::Released,
    );
    let budget = Duration::from_millis(600);
    let asked = Instant::now();
    let error = preview_within(budget, remote_read(&world)).unwrap_err();
    let waited = asked.elapsed();
    assert_eq!(error.message(), "reentry-preview-timeout");
    assert!(
        waited >= budget && waited < budget + Duration::from_secs(2),
        "answered at the deadline, not when the validation returns: {waited:?}"
    );
    assert!(
        held.wait_entered(1, Duration::from_secs(5)),
        "the worker is inside the validation"
    );
    assert_eq!(
        kept_token().as_deref(),
        Some(earlier.as_str()),
        "a timeout keeps nothing and drops nothing"
    );
    // the stalled worker still owns the turn: the next preview is refused at
    // once and starts neither a thread nor a validation beside it
    let asked = Instant::now();
    let busy = preview_within(budget, remote_read(&world)).unwrap_err();
    assert_eq!(busy.message(), "reentry-preview-busy");
    assert!(
        asked.elapsed() < Duration::from_millis(400),
        "not waited for"
    );
    assert_eq!(held.entered(), 1, "no validation beside the stalled one");
    // the validation returns at last: its answer reaches nobody
    held.release();
    world.wait("the stalled preview to end", || {
        PREVIEW_TURN.lock_or_recover().is_none()
    });
    assert_eq!(
        kept_token().as_deref(),
        Some(earlier.as_str()),
        "a late answer is not a preview"
    );
    assert_eq!(held.entered(), 1, "and nothing was asked again");
    drop(held);
    // unhurried, the worker's preview is the one that is kept
    let view = preview_within(PREVIEW_BUDGET, remote_read(&world)).expect("an unhurried preview");
    let view = serde_json::to_value(&view).unwrap();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    assert_eq!(kept_token().as_deref(), view["token"].as_str());
}

/// R1, freeze with the previewed reference reused. Nothing is read again, so
/// the one call that can take time is the binding's validation — and it
/// answers "yes" after the preparation ended. That freezes nothing: the
/// restart ends as its own timeout, not as a target that changed, before a
/// lease is armed and before any exit key; the client is still running, the
/// server is the old one, the approval is used up and nothing is asked twice.
#[test]
fn a_freeze_whose_validation_answers_late_is_the_restarts_timeout_before_any_exit_key() {
    use crate::procinfo::{DYNAMIC_CHECKS, FULL_READS, STATIC_VALIDATIONS};
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let client = world.foreground_pid("a");
    let server = world.server_pid();
    let (token, view) = world.preview();
    assert_eq!(view["entries"][0]["kind"], "candidate");
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let counts = || {
        (
            FULL_READS.with(std::cell::Cell::get),
            STATIC_VALIDATIONS.with(std::cell::Cell::get),
            DYNAMIC_CHECKS.with(std::cell::Cell::get),
        )
    };
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| pid == client && std::thread::current().id() == me,
        hold::Until::DeadlinePassed,
    );
    let before = counts();
    let started = Instant::now();
    let error = world.restart("req-late-freeze", &approval).unwrap_err();
    let took = started.elapsed();
    let after = counts();
    assert_eq!(
        error.message(),
        "tmux-restart-timeout",
        "a check that did not finish is not a target that changed"
    );
    assert!(
        took >= crate::restart::PREPARE_BUDGET
            && took < crate::restart::PREPARE_BUDGET + Duration::from_secs(2),
        "the preparation's own deadline: {took:?}"
    );
    assert_eq!(
        (after.0 - before.0, after.1 - before.1, after.2 - before.2),
        (0, 0, 1),
        "the reference was reused; one validation, never asked again"
    );
    assert_eq!(held.entered(), 1);
    // nothing crossed the side-effect boundary
    assert!(
        !world
            .calls
            .borrow()
            .iter()
            .any(|(args, _)| args.iter().any(|arg| arg.contains("C-d"))),
        "no exit key"
    );
    assert!(world.pasted().is_empty());
    assert_eq!(world.enters(), 0);
    assert_eq!(world.server_pid(), server, "the server was not replaced");
    assert_eq!(world.foreground_pid("a"), client, "the client still runs");
    assert_eq!(world.launches().len(), 1);
    assert!(!lifecycle::armed(), "no lease was armed");
    assert!(known("req-late-freeze").is_none(), "no operation exists");
    // the approval was used: it starts nothing again, and asks nothing
    drop(held);
    let before = counts();
    let again = world.restart("req-late-freeze-2", &approval).unwrap_err();
    assert_eq!(again.message(), "reentry-preview-expired");
    assert_eq!(counts(), before);
    assert_eq!(world.foreground_pid("a"), client);
    // and the same client is the same candidate for a new preview
    assert_eq!(world.preview().1["entries"][0]["kind"], "candidate");
}

/// R1, the started client's final check. The receipt arrived and the client
/// is in the foreground; the check of what it runs answers "matches" after
/// the confirmation's share. That confirms nothing — under either policy,
/// and without a word about a bypass flag — and it is not called a change.
/// While the check is stalled the entry is in progress, never a result. The
/// client that was started is left alone, the one request is never made
/// again, and the settled result does not move afterwards.
#[test]
fn a_final_check_that_answers_late_confirms_nothing_and_is_never_asked_again() {
    for policy in [Policy::Ordinary, Policy::ReapplyBypass] {
        let world = World::new();
        let launch = match policy {
            Policy::Ordinary => "{agent}".to_string(),
            Policy::ReapplyBypass => format!("{{agent}} {BYPASS}"),
        };
        world.card("a", &format!("session={ID_A}"), Some(&launch));
        let old = world.foreground_pid("a");
        let (token, view) = world.preview();
        assert_eq!(view["entries"][0]["kind"], "candidate", "{policy:?}");
        let approval = world.approval(&token, &["a"], policy);
        // The transaction's own thread validates a process that is not the
        // old client exactly once it checks the client it started.
        let me = std::thread::current().id();
        let held = hold::install(
            move |pid| pid != old && std::thread::current().id() == me,
            hold::Until::DeadlinePassed,
        );
        let request = "req-late-final";
        let (seen, stalled) = std::sync::mpsc::channel();
        let settled = std::thread::scope(|scope| {
            // what the operation says of the entry WHILE its check is stalled
            scope.spawn(|| {
                if held.wait_entered(1, Duration::from_secs(40)) {
                    let _ = seen.send(known(request));
                }
            });
            world.restart_with(
                request,
                Some(&approval),
                Duration::from_secs(4),
                &|| Ok(()),
                &|_, _, _| {},
            )
        });
        let (_, _, result) = settled.expect("the restart itself succeeded");
        let result = result.expect("a re-entry result");
        assert!(result.server_replaced, "{policy:?}");
        assert_eq!(result.state, OperationState::Complete);
        let late = entry(&result, "a");
        assert_eq!(late.outcome, Outcome::Unconfirmed, "{policy:?}");
        assert_eq!(
            late.reason,
            Some(Reason::ContextUnverified),
            "{policy:?}: not verified in time — neither confirmed nor changed"
        );
        assert!(late.shell && late.client_observed && late.settled);
        assert!(
            !late.warnings.contains(&Warning::BypassReauthorized),
            "{policy:?}: an unconfirmed entry says nothing about the flag"
        );
        let during = stalled
            .recv_timeout(Duration::from_secs(1))
            .expect("the entry was seen while its check was stalled")
            .expect("the operation is known while it runs");
        assert_eq!(during.state, OperationState::Running);
        let pending = entry(&during, "a");
        assert_eq!(
            (pending.outcome, pending.reason, pending.settled),
            (Outcome::Unconfirmed, Some(Reason::LaunchUnconfirmed), false),
            "{policy:?}: in progress while stalled, never a confirmation"
        );
        // one request, one Enter, one started client — and it was left alone
        let pasted = world.pasted();
        assert_eq!(pasted.len(), 1, "{policy:?}");
        assert_eq!(
            pasted[0].contains(BYPASS),
            policy == Policy::ReapplyBypass,
            "{policy:?}"
        );
        assert_eq!(world.enters(), 1);
        assert_eq!(world.launches().len(), 2);
        let started = world.foreground_pid("a");
        assert!(world.foreground_is_child("a") && started != old);
        assert_eq!(held.entered(), 1, "one validation, never asked again");
        // the result is settled: the late answer moved nothing, then or later
        drop(held);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(known(request).as_ref(), Some(&result));
        assert_eq!(world.enters(), 1);
        assert_eq!(world.launches().len(), 2);
        assert_eq!(
            world.foreground_pid("a"),
            started,
            "not undone, not restarted"
        );
    }
}

/// One more connection on the shared status socket, written and read to the
/// listener's close: how long the accept loop took to serve it.
fn served(socket: &Path, line: &str) -> Duration {
    use std::io::{Read, Write};
    let asked = Instant::now();
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the status socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(line.as_bytes()).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut rest = Vec::new();
    stream
        .read_to_end(&mut rest)
        .expect("closed by the listener");
    asked.elapsed()
}

/// R2, the shared listener. The exit receipts of two clients arrive; the
/// receipt worker's validation of the first answers "yes" and does not
/// return. The accept loop does not wait for it: a Signal line and another
/// lifecycle line are each served at once. The worker is ONE: the second
/// receipt waits behind the first and starts no validation. Neither helper
/// is acknowledged in time, so no final session is named, no entry is
/// attempted and nothing is typed. When the validation returns, its late
/// "yes" writes nothing, the receipt that waited is refused unread, and
/// nothing is asked again.
#[test]
fn a_receipt_whose_validation_does_not_return_holds_one_worker_and_never_the_listener() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card("b", &format!("session={ID_B}"), Some("{agent}"));
    let clients = [world.foreground_pid("a"), world.foreground_pid("b")];
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    // whichever thread validates an old client from here on, except the
    // transaction's own: in production that is the receipt worker alone
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| clients.contains(&pid) && std::thread::current().id() != me,
        hold::Until::Released,
    );
    let socket = world.base.join("status.sock");
    let lifecycle_line = format!(
        r#"{{"protocol":"agent-lifecycle","v":1,"source":"claude-code","event":"session-start","nativeSessionId":"{ID_X}","startSource":"resume","socket":"{}","server_pid":1,"pane":"%0"}}"#,
        world.socket
    );
    let probed = std::cell::Cell::new(false);
    let request = "req-stalled-receipt";
    let (_, _, result) = world
        .restart_with(
            request,
            Some(&approval),
            Duration::from_secs(3),
            // after the exits were sealed: the worker is still inside its
            // first validation, and both helpers have stopped waiting
            &|| {
                assert!(
                    held.wait_entered(1, Duration::from_secs(5)),
                    "the receipt worker is validating"
                );
                for line in ["{\"v\":1,\"source\":\"nobody\"}\n", &lifecycle_line] {
                    let took = served(&socket, line);
                    assert!(
                        took < Duration::from_secs(1),
                        "the accept loop waited for the validation: {took:?}"
                    );
                }
                assert_eq!(held.entered(), 1, "the second receipt started nothing");
                probed.set(true);
                Ok(())
            },
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    assert!(probed.get());
    let result = result.expect("a re-entry result");
    assert!(result.server_replaced);
    for card in ["a", "b"] {
        let skipped = entry(&result, card);
        assert_eq!(skipped.outcome, Outcome::NotAttempted, "{card}");
        assert_eq!(
            skipped.reason,
            Some(Reason::FinalSessionUnconfirmed),
            "{card}"
        );
        assert!(!skipped.shell, "{card}");
    }
    assert!(world.pasted().is_empty());
    assert_eq!(world.enters(), 0);
    assert_eq!(world.launches().len(), 2, "only the user's own two starts");
    // the validation returns, long after its receipt's budget
    held.release();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        held.entered(),
        1,
        "the receipt that waited is refused unread; nothing is asked again"
    );
    assert_eq!(known(request).as_ref(), Some(&result));
    assert!(world.pasted().is_empty());
    assert_eq!(world.launches().len(), 2);
    // the listener is still serving
    assert!(served(&socket, "{\"v\":1}\n") < Duration::from_secs(1));
}

/// R2, the launch receipt. The started client reports its resume; the
/// receipt worker's validation of that client answers "yes" and does not
/// return. No fact is written while the lease is armed, the entry ends as
/// "attempted, outcome unknown", and the one request is never made again —
/// not while the worker is stalled and not when its late answer finds the
/// lease sealed.
#[test]
fn a_launch_receipt_whose_validation_does_not_return_confirms_nothing_and_sends_nothing_twice() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let old = world.foreground_pid("a");
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    // whichever thread validates the started client, except the
    // transaction's own: in production that is the receipt worker alone
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| pid != old && std::thread::current().id() != me,
        hold::Until::Released,
    );
    let request = "req-stalled-launch-receipt";
    let (_, _, result) = world
        .restart_with(
            request,
            Some(&approval),
            Duration::from_secs(3),
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    let result = result.expect("a re-entry result");
    assert!(result.server_replaced);
    let unknown = entry(&result, "a");
    assert_eq!(unknown.outcome, Outcome::Unconfirmed);
    assert_eq!(unknown.reason, Some(Reason::LaunchUnconfirmed));
    assert!(unknown.shell && unknown.client_observed);
    assert_eq!(held.entered(), 1, "the worker validated the started client");
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 2);
    let started = world.foreground_pid("a");
    // the late answer: the lease is sealed, the budget is long over
    held.release();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(held.entered(), 1, "nothing is asked again");
    assert_eq!(
        known(request).as_ref(),
        Some(&result),
        "nothing was rewritten"
    );
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 2);
    assert_eq!(
        world.foreground_pid("a"),
        started,
        "the started client is left alone"
    );
}

/// R1, what is read before a call of unknown length is not evidence of
/// after it. The started client reports, is in the foreground, and its
/// final check begins; WHILE that check runs the client is stopped and the
/// shell takes the terminal back. The check itself answers "matches" in
/// time — the process is alive and runs the frozen file — and the entry is
/// still not confirmed: the client is not the pane's foreground at the
/// instant it would be.
#[test]
fn a_client_that_leaves_the_foreground_during_its_final_check_is_not_confirmed() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let old = world.foreground_pid("a");
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a"], Policy::Ordinary);
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| pid != old && std::thread::current().id() == me,
        hold::Until::Released,
    );
    let left = AtomicBool::new(false);
    let started = std::sync::Mutex::new(None);
    let settled = std::thread::scope(|scope| {
        scope.spawn(|| {
            if held.wait_entered(1, Duration::from_secs(40)) {
                let client = held.pids()[0];
                let birth = crate::procinfo::process_start(client);
                let shell = crate::procinfo::process(client).map_or(0, |info| info.ppid);
                *started.lock_or_recover() = Some((client, birth));
                // SAFETY: a stop signal to the stand-in this test started.
                unsafe { libc::kill(client as libc::pid_t, libc::SIGSTOP) };
                let until = Instant::now() + Duration::from_secs(5);
                while Instant::now() < until && !left.load(Ordering::Acquire) {
                    let shell_in_front = crate::agent_status::live_generation(shell)
                        .is_some_and(|foreground| foreground.pid() == shell);
                    left.store(shell_in_front, Ordering::Release);
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            held.release();
        });
        world.restart_with(
            "req-left-during-final",
            Some(&approval),
            Duration::from_secs(8),
            &|| Ok(()),
            &|_, _, _| {},
        )
    });
    assert!(
        left.load(Ordering::Acquire),
        "the shell took the terminal back while the check was running"
    );
    let (_, _, result) = settled.expect("the restart itself succeeded");
    let result = result.expect("a re-entry result");
    let entered = entry(&result, "a");
    assert_eq!(entered.outcome, Outcome::Unconfirmed);
    assert_eq!(
        entered.reason,
        Some(Reason::LaunchUnconfirmed),
        "alive and matching, and not the foreground when it would be confirmed"
    );
    assert!(entered.shell);
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 2);
    // the stopped stand-in is this test's own: end that exact generation
    let stopped = *started.lock_or_recover();
    if let Some((client, Some(birth))) = stopped {
        if crate::procinfo::process_start(client) == Some(birth) {
            // SAFETY: the generation was read again just above.
            unsafe { libc::kill(client as libc::pid_t, libc::SIGKILL) };
        }
    }
}

/// R1, a client that runs under its release's file name. Its first question
/// — whose code is this running image — answers after the preview's share.
/// Nothing is then known about it: that is a check that did not finish, and
/// not "an agent this feature does not support", which would be a statement
/// about a program nobody identified.
#[test]
fn a_late_answer_about_a_client_under_a_release_file_name_is_a_check_that_did_not_finish() {
    use crate::session_runtime::Deadline;
    let world = World::new();
    let binary = release(&world, NEW.0, NEW.1);
    world.card(
        "a",
        &format!("session={ID_A}"),
        Some(&format!("'{}'", binary.display())),
    );
    let client = world.foreground_pid("a");
    // unhurried: a candidate, by its code and whatever its file is called
    assert_eq!(world.preview().1["entries"][0]["kind"], "candidate");
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| pid == client && std::thread::current().id() == me,
        hold::Until::DeadlinePassed,
    );
    let (_, view) = {
        let _phase = Deadline::until(Instant::now() + Duration::from_millis(300));
        world.preview()
    };
    assert_eq!(held.entered(), 1, "asked once");
    assert_eq!(view["entries"][0]["kind"], "unsupported");
    assert_eq!(view["entries"][0]["reason"], "check-incomplete");
    assert!(view["entries"][0]["launchProfile"].is_null());
}

/// One entry never reports, so the receipts are waited for until the post
/// share ends. The other entry reported long before that: its confirmation
/// has a share of its own after the wait and is not lost to the entry that
/// stayed silent.
#[test]
fn an_entry_that_never_reports_does_not_cost_another_its_confirmation() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    world.card(
        "b",
        &format!("session={ID_B}\nresume=silent"),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    let post = Duration::from_secs(6);
    let started = Instant::now();
    let (_, _, result) = world
        .restart_with(
            "req-one-silent",
            Some(&approval),
            post,
            &|| Ok(()),
            &|_, _, _| {},
        )
        .expect("the restart itself succeeded");
    assert!(
        started.elapsed() >= post,
        "the receipts were waited for until the post share ended"
    );
    let result = result.expect("a re-entry result");
    let confirmed = entry(&result, "a");
    assert_eq!(
        (confirmed.outcome, confirmed.reason),
        (Outcome::Confirmed, None),
        "confirmed in the confirmation's own share"
    );
    let silent = entry(&result, "b");
    assert_eq!(
        (silent.outcome, silent.reason),
        (Outcome::Unconfirmed, Some(Reason::LaunchUnconfirmed))
    );
    assert_eq!(world.pasted().len(), 2);
    assert_eq!(world.enters(), 2);
    assert_eq!(world.launches().len(), 4);
}

// ---------- the receipts end at the post deadline; the confirmation is its own share ----------
//
// Each of these decides the order of its threads itself — by a kept-back
// validation, a pause before the seal, or a file the stand-in client waits
// for — and none waits for a race to happen.

/// What a test wants done on the restart's own thread after the receipts'
/// wait ended and before they are sealed: the window a slow thread leaves
/// there. Run once.
static BEFORE_SEAL: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

/// Called by `Plan::reopen` at that point (tests only).
pub(super) fn before_seal() {
    // taken out first: the lock is not held while a test's pause runs
    let pause = BEFORE_SEAL.lock_or_recover().take();
    if let Some(pause) = pause {
        pause();
    }
}

/// A pause before the seal, for as long as this lives; removed when dropped,
/// run or not.
struct BeforeSeal;

impl BeforeSeal {
    fn install(pause: impl Fn() + Send + 'static) -> Self {
        *BEFORE_SEAL.lock_or_recover() = Some(Box::new(pause));
        Self
    }
}

impl Drop for BeforeSeal {
    fn drop(&mut self) {
        BEFORE_SEAL.lock_or_recover().take();
    }
}

/// Wait until the stand-in clients have made `count` of the reports a test
/// asked them for (`resume=on:` / `then=`): their hooks ran and returned.
/// `false` when that did not happen in a generous time.
fn reports_made(log: &Path, count: usize) -> bool {
    let until = Instant::now() + Duration::from_secs(30);
    loop {
        let made = std::fs::read_to_string(log).map_or(0, |text| text.lines().count());
        if made >= count {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The first card reported in time and its final check is running — kept
/// back. The second card had not reported when the post share ended, and
/// reports NOW: after the post deadline, with all of its own receipt budget.
/// It is not counted. The second card ends as "no receipt" — not as a check
/// that ran late, and not as a confirmation — its client is left alone,
/// nothing is sent again, and the stored result does not move. The first
/// card keeps its own result.
#[test]
fn a_receipt_after_the_post_deadline_is_not_counted_while_another_entrys_check_runs() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let gate = world.base.join("cards").join("b").join("report-now");
    world.card(
        "b",
        &format!("session={ID_B}\nresume=on:{}", gate.display()),
        Some("{agent}"),
    );
    let old = [world.foreground_pid("a"), world.foreground_pid("b")];
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    // The transaction's own thread validates a process that is not an old
    // client exactly when it checks a client it started; card a's is first.
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| !old.contains(&pid) && std::thread::current().id() == me,
        hold::Until::Released,
    );
    let reported = world.home.join("reported.log");
    let post = Duration::from_secs(6);
    let request = "req-receipt-after-post";
    let reopening = std::cell::Cell::new(None);
    let let_go = std::sync::Mutex::new(None);
    let settled = std::thread::scope(|scope| {
        scope.spawn(|| {
            // Card a's check began: the wait for the receipts is over, and
            // it ended at the post deadline, because card b was silent.
            if held.wait_entered(1, Duration::from_secs(60)) {
                std::fs::write(&gate, b"").expect("the gate");
                if reports_made(&reported, 1) {
                    *let_go.lock_or_recover() = Some(Instant::now());
                }
            }
            held.release();
        });
        world.restart_with(
            request,
            Some(&approval),
            post,
            &|| Ok(()),
            &|phase, _, _| {
                if phase == "reopening" && reopening.get().is_none() {
                    reopening.set(Some(Instant::now()));
                }
            },
        )
    });
    let (_, _, result) = settled.expect("the restart itself succeeded");
    let result = result.expect("a re-entry result");
    let let_go = let_go
        .lock_or_recover()
        .expect("card b reported while card a's check was running");
    let late = entry(&result, "b");
    assert_eq!(
        (late.outcome, late.reason),
        (Outcome::Unconfirmed, Some(Reason::LaunchUnconfirmed)),
        "no receipt counts after the post deadline"
    );
    assert!(late.shell && late.client_observed && late.settled);
    // Card a: confirmed by its own check — unless this test's own pause used
    // up the confirmation's share, which is then what the entry says.
    let confirm_end = reopening.get().expect("the post phase began") + post + CONFIRM_SHARE;
    let first = entry(&result, "a");
    match (first.outcome, first.reason) {
        (Outcome::Confirmed, None) => {}
        (Outcome::Unconfirmed, Some(Reason::ContextUnverified)) => assert!(
            let_go + Duration::from_millis(300) >= confirm_end,
            "let go {:?} before the share ended, and not confirmed",
            confirm_end - let_go
        ),
        other => panic!("card a: {other:?}"),
    }
    // one request each, and nothing after it
    assert_eq!(world.pasted().len(), 2);
    assert_eq!(world.enters(), 2);
    assert_eq!(world.launches().len(), 4);
    assert!(
        world.foreground_is_child("b"),
        "the late client is left alone"
    );
    assert_eq!(known(request).as_ref(), Some(&result));
}

/// The window between the post deadline and the seal. The restart's thread
/// is past the wait — the post share is over — and has not sealed yet: the
/// lease is still armed. The client reports in that window, with all of its
/// own receipt budget. Nothing is recorded: what closes the receipts is the
/// deadline, not how soon the thread gets to seal.
#[test]
fn a_receipt_between_the_post_deadline_and_the_seal_is_not_counted() {
    let world = World::new();
    let gate = world.base.join("cards").join("b").join("report-now");
    world.card(
        "b",
        &format!("session={ID_B}\nresume=on:{}", gate.display()),
        Some("{agent}"),
    );
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["b"], Policy::Ordinary);
    let reported = world.home.join("reported.log");
    let (paused, at_the_seal) = std::sync::mpsc::channel();
    let (go, gone) = std::sync::mpsc::channel::<()>();
    let _pause = BeforeSeal::install(move || {
        let _ = paused.send(Instant::now());
        let _ = gone.recv_timeout(Duration::from_secs(60));
    });
    let post = Duration::from_secs(4);
    let request = "req-receipt-before-seal";
    let made = AtomicBool::new(false);
    let asked = Instant::now();
    let (settled, paused_at) = std::thread::scope(|scope| {
        let (gate, reported, made) = (&gate, &reported, &made);
        let pause = scope.spawn(move || {
            let at = at_the_seal.recv_timeout(Duration::from_secs(60)).ok();
            if at.is_some() {
                assert!(lifecycle::armed(), "not sealed yet: the lease is armed");
                std::fs::write(gate, b"").expect("the gate");
                made.store(reports_made(reported, 1), Ordering::Release);
            }
            let _ = go.send(());
            at
        });
        let settled = world.restart_with(request, Some(&approval), post, &|| Ok(()), &|_, _, _| {});
        (settled, pause.join().expect("the pause ended"))
    });
    let (_, _, result) = settled.expect("the restart itself succeeded");
    let result = result.expect("a re-entry result");
    let paused_at = paused_at.expect("the restart reached the seal");
    assert!(
        paused_at >= asked + post,
        "the wait ended at the post deadline, not before"
    );
    assert!(
        made.load(Ordering::Acquire),
        "the client reported in the window"
    );
    let late = entry(&result, "b");
    assert_eq!(
        (late.outcome, late.reason),
        (Outcome::Unconfirmed, Some(Reason::LaunchUnconfirmed)),
        "a receipt after the post deadline is not counted, sealed or not"
    );
    assert!(late.shell && late.client_observed);
    assert_eq!(world.pasted().len(), 1);
    assert_eq!(world.enters(), 1);
    assert_eq!(world.launches().len(), 2);
    assert_eq!(known(request).as_ref(), Some(&result));
}

/// The receipts were complete long before the post deadline, so they were
/// sealed then — for every entry at once. While the first card's check is
/// kept back, the second card's client reports again: a resume of ANOTHER
/// session, which would have made its record a conflict. It arrives after
/// the seal and changes nothing: the second card is confirmed from what was
/// sealed, nothing is sent again, and the stored result does not move.
#[test]
fn a_receipt_after_the_seal_changes_nothing_that_was_sealed() {
    let world = World::new();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    let gate = world.base.join("cards").join("b").join("report-again");
    world.card(
        "b",
        &format!("session={ID_B}\nthen={ID_X}:{}", gate.display()),
        Some("{agent}"),
    );
    let old = [world.foreground_pid("a"), world.foreground_pid("b")];
    let (token, _) = world.preview();
    let approval = world.approval(&token, &["a", "b"], Policy::Ordinary);
    let me = std::thread::current().id();
    let held = hold::install(
        move |pid| !old.contains(&pid) && std::thread::current().id() == me,
        hold::Until::Released,
    );
    let reported = world.home.join("reported.log");
    // far away: the receipts end by being complete, not by this deadline
    let post = Duration::from_secs(40);
    let request = "req-receipt-after-seal";
    let made = AtomicBool::new(false);
    let asked = Instant::now();
    let settled = std::thread::scope(|scope| {
        scope.spawn(|| {
            // card a's check began: every entry was sealed just before it
            if held.wait_entered(1, Duration::from_secs(60)) {
                std::fs::write(&gate, b"").expect("the gate");
                made.store(reports_made(&reported, 1), Ordering::Release);
            }
            held.release();
        });
        world.restart_with(request, Some(&approval), post, &|| Ok(()), &|_, _, _| {})
    });
    let (_, _, result) = settled.expect("the restart itself succeeded");
    assert!(
        asked.elapsed() < post,
        "the receipts were complete before the post deadline"
    );
    assert!(
        made.load(Ordering::Acquire),
        "card b reported again while card a's check was running"
    );
    let result = result.expect("a re-entry result");
    for card in ["a", "b"] {
        let confirmed = entry(&result, card);
        assert_eq!(
            (confirmed.outcome, confirmed.reason),
            (Outcome::Confirmed, None),
            "{card}: confirmed from the sealed set"
        );
    }
    assert_eq!(world.pasted().len(), 2);
    assert_eq!(world.enters(), 2);
    assert_eq!(world.launches().len(), 4);
    assert_eq!(known(request).as_ref(), Some(&result));
}

// ---------- a preview is published under its turn ---------------------------------

/// What one preview's read returns, without a world: nothing to inspect,
/// and a token a test can name.
fn canned(token: &str) -> Previewed {
    Previewed {
        view: PreviewView {
            token: token.into(),
            lifecycle_observation: true,
            board_known: true,
            entries: Vec::new(),
        },
        kept: Some(Preview {
            token: token.into(),
            made: Instant::now(),
            entries: Vec::new(),
        }),
    }
}

/// Wait until `count` previews are parked waiting for the turn. `otherwise`
/// is asked meanwhile, and what it answers fails the test: what a preview
/// that did NOT wait did instead.
fn waiting_for_the_turn(count: usize, otherwise: impl Fn() -> Option<&'static str>) {
    let until = Instant::now() + Duration::from_secs(30);
    while TURN_WAITERS.load(Ordering::SeqCst) < count {
        if let Some(instead) = otherwise() {
            panic!("{instead}");
        }
        assert!(
            Instant::now() < until,
            "no preview came to wait for the turn"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// As if the turn had passed — with no instant in which it was free — to a
/// preview that asked later: what a waiter sees when somebody else took the
/// turn before it could. The same slot, held, with a later deadline.
fn succeeded_by(turn: &mut Turn, end: Instant) {
    *PREVIEW_TURN.lock_or_recover() = Some(end);
    turn.end = end;
    PREVIEW_FREED.notify_all();
}

const UNHURRIED: Duration = Duration::from_secs(60);

/// The first preview's worker has finished reading and its caller has the
/// answer and has not published it; a second preview asks now. It cannot
/// read — let alone publish — before the first is published: it waits for
/// the turn. So the two are published in the order they were read, and the
/// preview that can be confirmed in the end is the one that was read last;
/// the older result never replaces it.
#[test]
fn a_preview_is_published_before_the_next_one_reads_and_never_over_a_newer_one() {
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let (first, turn) =
        read_in_turn(Instant::now() + UNHURRIED, || Ok(canned("first"))).expect("answered in time");
    assert_eq!(kept_token(), None, "read, and not published yet");
    std::thread::scope(|scope| {
        let (entered, reading) = std::sync::mpsc::channel();
        let (proceed, kept_back) = std::sync::mpsc::channel::<()>();
        let second = scope.spawn(move || {
            preview_within(UNHURRIED, move || {
                let _ = entered.send(());
                let _ = kept_back.recv();
                Ok(canned("second"))
            })
        });
        waiting_for_the_turn(1, || {
            reading
                .try_recv()
                .is_ok()
                .then_some("the second preview read while the first was not published")
        });
        assert_eq!(kept_token(), None);
        // the first is published; only now can the second read
        let view = turn.publish(first).expect("published inside its limit");
        assert_eq!(view.token, "first");
        reading
            .recv_timeout(Duration::from_secs(30))
            .expect("the second preview reads once the first was published");
        assert_eq!(
            kept_token().as_deref(),
            Some("first"),
            "published first: the one that can be confirmed while the second reads"
        );
        proceed.send(()).expect("the second preview is reading");
        let view = second
            .join()
            .expect("the second preview ended")
            .expect("published");
        assert_eq!(view.token, "second");
    });
    assert_eq!(
        kept_token().as_deref(),
        Some("second"),
        "the preview read last is the one that can be confirmed"
    );
    assert!(
        PREVIEW_TURN.lock_or_recover().is_none(),
        "every turn was given back"
    );
    reset_for_tests();
}

/// What a deadline does to each party. A preview that waits for the turn
/// leaves at its OWN limit and reads nothing. A preview whose answer is in
/// hand and whose limit passes before it is published is not published: the
/// earlier preview is neither replaced nor revived, and a request that came
/// meanwhile was refused at once, not kept waiting for a holder that could
/// no longer answer in time.
#[test]
fn a_preview_past_its_limit_is_not_published_and_a_waiter_leaves_at_its_own_limit() {
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let read = Arc::new(AtomicBool::new(false));
    let reading = |token: &'static str| {
        let read = read.clone();
        move || {
            read.store(true, Ordering::Release);
            Ok(canned(token))
        }
    };
    // an unpublished preview holds the turn, with a minute to go
    let (first, turn) =
        read_in_turn(Instant::now() + UNHURRIED, || Ok(canned("first"))).expect("answered in time");
    let own = Duration::from_millis(300);
    let asked = Instant::now();
    let waited_out = preview_within(own, reading("waiter"));
    let waited = asked.elapsed();
    assert_eq!(waited_out.unwrap_err().message(), "reentry-preview-busy");
    assert!(
        waited >= own && waited < own + Duration::from_secs(2),
        "it left at its own limit: {waited:?}"
    );
    assert!(!read.load(Ordering::Acquire), "and read nothing");
    assert_eq!(kept_token(), None);
    // the holder was not disturbed
    assert_eq!(turn.publish(first).expect("published").token, "first");
    assert_eq!(kept_token().as_deref(), Some("first"));
    // answered in time, published too late
    let limit = Instant::now() + Duration::from_secs(1);
    let (late, turn) = read_in_turn(limit, || Ok(canned("late"))).expect("answered in time");
    while Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(5));
    }
    let asked = Instant::now();
    let refused = preview_within(UNHURRIED, reading("beside"));
    assert_eq!(refused.unwrap_err().message(), "reentry-preview-busy");
    assert!(
        asked.elapsed() < Duration::from_secs(2),
        "a holder past its deadline is not waited for"
    );
    assert!(!read.load(Ordering::Acquire));
    let error = turn.publish(late).unwrap_err();
    assert_eq!(error.message(), "reentry-preview-timeout");
    assert_eq!(
        kept_token().as_deref(),
        Some("first"),
        "neither replaced nor revived"
    );
    assert!(PREVIEW_TURN.lock_or_recover().is_none(), "the turn is free");
    // unhurried, the next preview is the one that can be confirmed
    let view = preview_within(UNHURRIED, reading("next")).expect("published");
    assert_eq!(view.token, "next");
    assert!(read.load(Ordering::Acquire));
    assert_eq!(kept_token().as_deref(), Some("next"));
    reset_for_tests();
}

/// A worker that outlived its caller. The caller is answered at its limit;
/// the worker goes on reading and still has the turn, so no second read and
/// no second thread starts beside it. When its read returns at last, nothing
/// is published — it has no caller to publish — and the one turn it gives
/// back is its own: the next preview takes a turn of its own and is the one
/// that can be confirmed.
#[test]
fn a_worker_that_outlived_its_caller_publishes_nothing_and_gives_back_its_own_turn() {
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let (earlier, turn) = read_in_turn(Instant::now() + UNHURRIED, || Ok(canned("earlier")))
        .expect("answered in time");
    turn.publish(earlier).expect("published");
    let (entered, reading) = std::sync::mpsc::channel();
    let (proceed, kept_back) = std::sync::mpsc::channel::<()>();
    let budget = Duration::from_millis(300);
    let asked = Instant::now();
    let timed_out = preview_within(budget, move || {
        let _ = entered.send(());
        let _ = kept_back.recv();
        Ok(canned("stalled"))
    });
    let waited = asked.elapsed();
    assert_eq!(timed_out.unwrap_err().message(), "reentry-preview-timeout");
    assert!(
        waited >= budget && waited < budget + Duration::from_secs(2),
        "answered at its limit: {waited:?}"
    );
    reading
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker is inside its read");
    assert_eq!(kept_token().as_deref(), Some("earlier"));
    // the stalled worker has the turn: nothing reads beside it
    let beside = Arc::new(AtomicBool::new(false));
    let asked = Instant::now();
    let busy = preview_within(UNHURRIED, {
        let beside = beside.clone();
        move || {
            beside.store(true, Ordering::Release);
            Ok(canned("beside"))
        }
    });
    assert_eq!(busy.unwrap_err().message(), "reentry-preview-busy");
    assert!(asked.elapsed() < Duration::from_secs(2), "not waited for");
    assert!(!beside.load(Ordering::Acquire), "no second read");
    assert!(
        PREVIEW_TURN.lock_or_recover().is_some(),
        "the turn is the stalled worker's"
    );
    // its read returns at last
    proceed.send(()).expect("the worker is still reading");
    let until = Instant::now() + Duration::from_secs(30);
    while PREVIEW_TURN.lock_or_recover().is_some() {
        assert!(Instant::now() < until, "the late worker gave its turn back");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        kept_token().as_deref(),
        Some("earlier"),
        "a late result is not published"
    );
    // the next preview: a turn of its own, published under it
    let (next, turn) =
        read_in_turn(Instant::now() + UNHURRIED, || Ok(canned("next"))).expect("answered in time");
    assert!(PREVIEW_TURN.lock_or_recover().is_some());
    assert_eq!(turn.publish(next).expect("published").token, "next");
    assert_eq!(kept_token().as_deref(), Some("next"));
    assert!(PREVIEW_TURN.lock_or_recover().is_none());
    reset_for_tests();
}

/// Previews that keep coming do not keep a waiter waiting. The turn is held
/// and — as far as the waiter can see — passes three times to a preview that
/// asked later, each with a later deadline, without ever being free. The
/// waiter still leaves at its own limit, having read nothing.
#[test]
fn the_wait_for_the_turn_ends_at_the_callers_own_limit_whoever_takes_the_turn_next() {
    let _lock = TEST_LOCK.lock_or_recover();
    reset_for_tests();
    let mut turn = Turn::take(Instant::now() + Duration::from_secs(5)).expect("a free turn");
    let own = Duration::from_millis(800);
    let read = Arc::new(AtomicBool::new(false));
    let (result, waited) = std::thread::scope(|scope| {
        let waiter = scope.spawn({
            let read = read.clone();
            move || {
                let asked = Instant::now();
                let result = preview_within(own, move || {
                    read.store(true, Ordering::Release);
                    Ok(canned("waiter"))
                });
                (result, asked.elapsed())
            }
        });
        for later in [6, 7, 8] {
            waiting_for_the_turn(1, || None);
            succeeded_by(&mut turn, Instant::now() + Duration::from_secs(later));
        }
        waiter.join().expect("the waiter ended")
    });
    assert_eq!(result.unwrap_err().message(), "reentry-preview-busy");
    assert!(
        waited >= own && waited < own + Duration::from_secs(2),
        "its own limit, and no holder's: {waited:?}"
    );
    assert!(!read.load(Ordering::Acquire), "it read nothing");
    drop(turn);
    assert!(PREVIEW_TURN.lock_or_recover().is_none());
    reset_for_tests();
}

/// Pi is never re-entered. Its adapter has no re-entry capability, so a card
/// whose foreground is a Pi that Deck verified is shown with its own reason
/// and label and is no candidate — whatever is observed of its session, and
/// even when the process would otherwise pass for a candidate. An approval
/// that names it (a forged request: the dialog offers no such box) is refused
/// whole: no client is asked to exit, no shell is replaced, nothing is typed.
/// The Claude card beside it is a candidate as before and is reopened by an
/// approval that names it alone.
#[test]
fn a_verified_pi_is_named_refused_and_never_started_whatever_is_approved() {
    use crate::agent_pi::{SessionEvent, StartReason};
    use crate::agent_status::{PaneKey, PiCoverage};
    assert!(
        crate::agent_pi::PI.reentry.is_none(),
        "Pi has no re-entry capability"
    );
    let world = World::new();
    let _store = crate::agent_status::STORE_TEST_LOCK.lock_or_recover();
    crate::agent_status::reset_for_tests();
    lifecycle::reset_pi_for_tests();
    world.card("a", &format!("session={ID_A}"), Some("{agent}"));
    // the "Pi": a second client of the very product a candidate must be
    world.card("p", &format!("session={ID_B}"), Some("{agent}"));
    let rows = crate::tmux::list_panes_with(&|args| world.raw(args)).unwrap();
    let row = rows.iter().find(|row| row.session_name == "p").unwrap();
    let generation = crate::agent_status::live_generation(row.pane_pid).unwrap();
    // without a verified observer both are what they were before: candidates
    let (_, plain) = world.preview();
    assert_eq!(plain["entries"][1]["kind"], "candidate");
    // a Pi observer verified for exactly that foreground generation
    let (instance, became) = crate::procinfo::InstanceWatch::stand_in();
    crate::agent_status::pi_observed(row, generation, PiCoverage::Connected, &instance);
    let pi = world.foreground_pid("p");
    let shown = |view: &serde_json::Value| {
        let entry = |name: &str| {
            view["entries"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["session"] == name)
                .unwrap()
                .clone()
        };
        (entry("a"), entry("p"))
    };
    let (token, view) = world.preview();
    let (a, p) = shown(&view);
    assert_eq!(
        (&a["kind"], &a["source"]),
        (&"candidate".into(), &"claude-code".into())
    );
    assert_eq!(
        (&p["kind"], &p["reason"], &p["source"], &p["launchProfile"]),
        (
            &"unsupported".into(),
            &"client-identity-unverifiable".into(),
            &"pi".into(),
            &serde_json::Value::Null
        ),
        "named as Pi, with the reason that is Pi's own"
    );
    // a session observed for that Pi changes nothing about it
    let id = match crate::agent_pi::parse_message(
        r#"{"protocol":"pi-observer","v":2,"source":"pi","lease":77,"nonce":"00000000000000000000000000000000","seq":1,"event":"snapshot","nativeSessionId":"session-one"}"#,
    ) {
        Ok(crate::agent_pi::Message::Frame {
            frame: Ok(crate::agent_pi::Frame::Lifecycle { native, .. }),
            ..
        }) => native,
        other => panic!("{other:?}"),
    };
    lifecycle::observe_pi(
        77,
        &PaneKey::of(row),
        generation,
        SessionEvent::Start(StartReason::Resume),
        &id,
    )
    .unwrap();
    let (token_again, observed) = world.preview();
    assert_eq!(
        shown(&observed).1,
        p,
        "an observed session makes Pi no candidate"
    );
    // a forged approval — Pi alone, Pi beside the Claude card, under either
    // policy — is refused before anything is touched
    let (server, launches) = (world.server_pid(), world.launches().len());
    for (request, cards, policy) in [
        ("req-pi", &["p"][..], PolicyB),
        ("req-pi-a", &["a", "p"][..], PolicyB),
        ("req-pi-ordinary", &["p", "a"][..], PolicyA),
    ] {
        let error = world
            .restart(request, &world.approval(&token_again, cards, policy))
            .unwrap_err();
        assert_eq!(error.message(), "reentry-preview-expired", "{request}");
        assert_eq!(
            world.server_pid(),
            server,
            "{request}: nothing was replaced"
        );
        assert_eq!(
            world.launches().len(),
            launches,
            "{request}: nothing was started"
        );
        assert!(world.pasted().is_empty(), "{request}: nothing was typed");
        assert_eq!(
            world.foreground_pid("p"),
            pi,
            "{request}: the Pi was not asked to exit"
        );
    }
    let _ = token;
    // The same pid and birth instant, ANOTHER PROGRAM (an exec): what the
    // observer proved names nothing that runs there now. The card is judged
    // as what it is — here, the product a candidate must be — and is not
    // called Pi on the word of a program that is gone.
    became.executed();
    let (_, after) = world.preview();
    assert_eq!(
        (&shown(&after).1["kind"], &shown(&after).1["source"]),
        (&"candidate".into(), &"claude-code".into()),
        "nothing of the old observer names this client"
    );
    let (instance, _same) = crate::procinfo::InstanceWatch::stand_in();
    crate::agent_status::pi_observed(row, generation, PiCoverage::Connected, &instance);
    // the Claude card alone is reopened as before; the Pi is left as it is
    let (token, _) = world.preview();
    let (old, new, result) = world
        .restart("req-claude", &world.approval(&token, &["a"], PolicyA))
        .expect("restart");
    assert_ne!(old, new);
    let result = result.unwrap();
    assert_eq!(
        result.entries.len(),
        1,
        "the operation holds the approved card only"
    );
    assert_eq!(entry(&result, "a").outcome, Outcome::Confirmed);
    assert_eq!(
        world.pasted(),
        [line(&world, "", ID_A)],
        "one request, for the Claude card"
    );
    assert_eq!(world.launches().len(), launches + 1);
    crate::agent_status::reset_for_tests();
    lifecycle::reset_pi_for_tests();
}
