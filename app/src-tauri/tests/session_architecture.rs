//! Layering tripwires complement the behavioral delivery/restart tests.
//!
//! The real boundary for a layer is a crate split (a module that cannot name
//! another does not compile against it); until deck splits crates these
//! source scans stand in. `references` counts every way a file can reach a
//! module — a full path, `use crate::m;` followed by `m::x`, a grouped
//! `use crate::{a, m::x}`, and the same through `super::` — so a plain
//! import no longer slips past a check for `crate::m::`. `referenced` lists
//! every module a file names that way, so a rule can be a closed allow-list
//! (the documents door) instead of a list of names to keep out: a deny-list
//! only knows the features that existed when it was written.
mod source_scan;
use source_scan::{code_only, production_region};
use std::collections::BTreeMap;
use std::path::Path;

fn source(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap()
}

#[test]
fn managed_creation_and_final_restart_check_share_the_lifecycle_gate() {
    let managed = source("mcp/commands.rs");
    let start = managed.find("fn mcp_start_session(").unwrap();
    let creation = &managed[start
        ..managed
            .find("fn runner_launch_args(")
            .unwrap_or(managed.len())];
    assert!(creation.contains("tmux_lifecycle::session_creation_guard()?"));
    let lifecycle = source("tmux_lifecycle.rs");
    let restart = &lifecycle[lifecycle.find("fn restart_tmux_server_inner(").unwrap()..];
    let gate = restart.find("let _guard = try_operation()?").unwrap();
    let blockers = restart
        .find("require_no_restart_blockers(&restart_constraints()?)?")
        .unwrap();
    let destructive = restart.find("tmux::stop_query_channel()").unwrap();
    assert!(
        gate < blockers && blockers < destructive,
        "the final blocker check must run inside the shared gate before tmux impact"
    );
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Every module `source` names through `crate::` or `super::` (as a path, a
/// `use`, or an item of a `{…}` group), with how many times.
fn referenced(source: &str) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for prefix in ["crate::", "super::"] {
        for (at, _) in source.match_indices(prefix) {
            if at > 0 && is_ident(source.as_bytes()[at - 1]) {
                continue;
            }
            let rest = &source[at + prefix.len()..];
            let heads: Vec<&str> = if let Some(group) = rest.strip_prefix('{') {
                let mut depth = 1;
                let end = group
                    .char_indices()
                    .find(|&(_, c)| {
                        depth += match c {
                            '{' => 1,
                            '}' => -1,
                            _ => 0,
                        };
                        depth == 0
                    })
                    .map_or(group.len(), |(i, _)| i);
                // top-level items only: a nested group belongs to its head
                let mut items = Vec::new();
                let (mut nested, mut start) = (0, 0);
                for (i, c) in group[..end].char_indices() {
                    match c {
                        '{' => nested += 1,
                        '}' => nested -= 1,
                        ',' if nested == 0 => {
                            items.push(&group[start..i]);
                            start = i + 1;
                        }
                        _ => {}
                    }
                }
                items.push(&group[start..end]);
                items
            } else {
                vec![rest]
            };
            for item in heads {
                let item = item.trim_start();
                let len = item.bytes().take_while(|&b| is_ident(b)).count();
                if len > 0 {
                    *found.entry(item[..len].to_owned()).or_default() += 1;
                }
            }
        }
    }
    found
}

/// How many times `source` names `module` (`referenced`).
fn references(source: &str, module: &str) -> usize {
    referenced(source).get(module).copied().unwrap_or(0)
}

#[test]
fn references_sees_every_import_form() {
    let forms = [
        "crate::restart::x();",
        "use crate::restart;",
        "use crate::restart as policy;",
        "use crate::{tmux, restart::exit};",
        "use crate::{restart, tmux};",
        "use crate::{tmux::{a, b}, restart};",
        "use super::restart;",
        "super::restart::x();",
    ];
    for form in forms {
        assert_eq!(references(form, "restart"), 1, "{form}");
    }
    for other in [
        "crate::restart_state::x();",
        "use crate::{tmux, restarted};",
        "my_crate::restart::x();",
        "use crate::tmux::{restart};",
    ] {
        assert_eq!(references(other, "restart"), 0, "{other}");
    }
}

#[test]
fn session_primitives_do_not_depend_on_business_policy() {
    let runtime = source("session_runtime.rs");
    for module in ["tmux", "restart", "shell_state", "scheduler", "voice"] {
        assert!(
            references(&runtime, module) == 0,
            "runtime depends on {module}"
        );
    }
    for module in [
        "tmux.rs",
        "shell_state.rs",
        "prompt_delivery.rs",
        "voice.rs",
        "pty.rs",
    ] {
        assert!(
            references(&source(module), "restart") == 0,
            "{module} depends on restart policy"
        );
    }
}

/// The lifecycle layer consults features only through the guard they
/// register (`tmux_lifecycle::set_restart_guard`, set from `mcp::spawn`), and
/// the shared durable-document mechanism knows none of its owners.
#[test]
fn lifecycle_and_ledger_do_not_name_feature_modules() {
    let lifecycle = source("tmux_lifecycle.rs");
    for module in ["mcp", "connector", "inbound", "inbound_channel", "voice"] {
        assert!(
            references(&lifecycle, module) == 0,
            "tmux_lifecycle depends on {module}"
        );
    }
    let ledger = source("ledger.rs");
    for module in [
        "mcp",
        "connector",
        "inbound",
        "inbound_channel",
        "scheduler",
    ] {
        assert!(
            references(&ledger, module) == 0,
            "ledger depends on its owner {module}"
        );
    }
}

/// What the scheduler names of the automation rule model (`inbound`), as a
/// register that may only shrink. At 0.7.16 no scheduler file named
/// `inbound`. Content authority (`authority.rs`) and the first-send readiness
/// override (`first_send.rs`) each brought the rule, its settings reader and
/// the pending event into the scheduler, a few references at a time, and the
/// two modules now name each other. The two policies are independent and are
/// not folded into one mechanism here (a third policy would be the moment).
/// Until then: a file that is not listed may not name `inbound`, a listed
/// file names it exactly as often as registered, and the number is lowered
/// when a reference goes. A higher number, or a new row, needs its reason
/// written here.
const SCHEDULER_NAMES_INBOUND: &[(&str, usize, &str)] = &[
    (
        "scheduler/authority.rs",
        3,
        "the rule with its grant and the config; the pending event of a bounded step",
    ),
    (
        "scheduler/first_send.rs",
        1,
        "the rule, the config and the pending event of a head row's claim",
    ),
    (
        "scheduler/delivery.rs",
        1,
        "the type of the settings read handed to the pre-fire fences",
    ),
    (
        "scheduler/ops.rs",
        6,
        "admission: the settings read, the pending event, the clock slot",
    ),
    (
        "scheduler/thread.rs",
        2,
        "the tick's settings read for the revocation sweep and the fences",
    ),
    ("scheduler/review.rs", 1, "the queue view's settings read"),
];

#[test]
fn the_scheduler_names_the_rule_model_only_where_registered() {
    let sources = source_scan::production_sources();
    let mut problems = Vec::new();
    for (file, text) in sources
        .iter()
        .filter(|(file, _)| file.starts_with("scheduler/"))
    {
        let count = references(&code_only(text), "inbound");
        match SCHEDULER_NAMES_INBOUND
            .iter()
            .find(|(name, _, _)| name == file)
        {
            None if count > 0 => problems.push(format!(
                "{file} names `inbound` ({count}): what the scheduler takes from the automation \
                 rule model is a closed register (SCHEDULER_NAMES_INBOUND). Reach it through a \
                 file that is registered, or add a row that says why this file needs it"
            )),
            Some((_, registered, why)) if *registered != count => problems.push(format!(
                "{file} names `inbound` {count} time(s), registered {registered} ({why}): lower \
                 the number when a reference goes; a higher one needs its own reason"
            )),
            _ => {}
        }
    }
    for (file, _, why) in SCHEDULER_NAMES_INBOUND {
        if !sources.iter().any(|(name, _)| name == file) {
            problems.push(format!(
                "stale entry `{file}` ({why}): no such production file"
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    // the other direction: the rule model wakes the scheduler once a run's
    // rows are queued, and names nothing else of it
    let back: Vec<(String, usize)> = sources
        .iter()
        .filter(|(file, _)| file.starts_with("inbound"))
        .map(|(file, text)| (file.clone(), references(&code_only(text), "scheduler")))
        .filter(|(_, count)| *count > 0)
        .collect();
    assert_eq!(
        back,
        [("inbound.rs".to_string(), 1), ("inbound_channel.rs".to_string(), 7)],
        "badge wakes the scheduler; channel reuses only normalization, supported-command and placeholder classification"
    );
    assert!(code_only(&source("inbound.rs")).contains("crate::scheduler::wake_scheduler();"));
    let channel = code_only(production_region(&source("inbound_channel.rs")));
    for (helper, count) in [
        ("normalize_prompt", 1),
        ("first_send::supported_command", 3),
        ("authority::has_placeholder", 3),
    ] {
        assert_eq!(
            channel
                .matches(&format!("crate::scheduler::{helper}("))
                .count(),
            count
        );
    }
    assert!(!channel.contains("first_send::verify"));
    assert!(!channel.contains("StepAuthority"));
}

/// What the typed-documents door may name, as a closed list: the
/// infrastructure underneath it, two closed vocabularies it validates
/// against, and the delegations it makes to feature modules. A module that is
/// not listed fails. A count is pinned only where the number is the rule: one
/// lookup per vocabulary, one call per delegation, and for the reminder the
/// two types and two validators the Board door takes. Counted over production
/// code (no comments, no strings, not the trailing test module).
const DOCUMENTS_MAY_NAME: &[(&str, Option<usize>, &str)] = &[
    ("error", None, "the one error type"),
    (
        "storage",
        None,
        "the typed envelope and atomic writes underneath",
    ),
    ("datadir", None, "where deck.json and settings.json live"),
    (
        "sync",
        None,
        "poison-tolerant locking for the door's own committed-Board copy",
    ),
    (
        "smoke_faults",
        None,
        "debug-only save faults of the isolated smoke",
    ),
    (
        "tmux",
        Some(2),
        "validate both card and frozen channel launch session names",
    ),
    ("voice", Some(1), "the closed list of dictation languages"),
    ("inbound_channel", Some(24), "native channel grant issue/retirement/activation recovery and durable Board-event binding/consumption at the two document save boundaries"),
    ("scheduler", Some(1), "normalize effective channel template heads with the same queue text contract"),

    (
        "inbound",
        Some(1),
        "delegation: the `inbound` settings section to its owner's validator",
    ),
    (
        "admission",
        Some(2),
        "validate task-preset commands and frozen channel targets with the existing admission table",
    ),
    // A Board-domain delegation like the two above: a card's reminder and a
    // save's claims are the reminder module's types (four uses: the card, the
    // save command, the save door at a path, the save itself), checked by
    // its two validators. The door reads no reminder state and calls no
    // reminder policy: it owns the committed-Board copy, and the projection
    // is the observer the reminder module registers.
    (
        "reminder",
        Some(5),
        "delegation: the reminder and claim types and their two validators",
    ),
];

#[test]
fn the_documents_door_names_only_reviewed_modules() {
    let documents = source("documents.rs");
    let named = referenced(&code_only(production_region(&documents)));
    let mut problems = Vec::new();
    for (module, count) in &named {
        match DOCUMENTS_MAY_NAME
            .iter()
            .find(|(name, _, _)| name == module)
        {
            None => problems.push(format!(
                "documents.rs names `{module}` ({count}): the document door does not depend on \
                 features. Point the dependency the other way (a guard or callback the feature \
                 registers, like `tmux_lifecycle::set_restart_guard`), or add a reviewed row to \
                 DOCUMENTS_MAY_NAME that says why"
            )),
            Some((_, Some(reviewed), why)) if reviewed != count => problems.push(format!(
                "documents.rs names `{module}` {count} time(s), reviewed {reviewed} ({why}): \
                 lower the count when the dependency shrinks; a higher one needs its own reason"
            )),
            Some(_) => {}
        }
    }
    for (module, _, why) in DOCUMENTS_MAY_NAME {
        if !named.contains_key(*module) {
            problems.push(format!(
                "stale entry `{module}` ({why}): documents.rs no longer names it"
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    // the two vocabularies and the two delegations, by the exact use they are
    assert!(documents.contains("crate::tmux::validate_session_name("));
    assert!(documents.contains("crate::voice::SUPPORTED_LANGUAGES"));
    assert!(documents.contains("crate::inbound::validate_settings("));
    assert!(documents.contains("crate::admission::channel_agent_command("));
}

/// Every call of a storage door that names `document` as its type argument,
/// as "door in file function", over production code.
fn storage_doors(document: &str) -> Vec<String> {
    const DOORS: &[&str] = &[
        "load_typed",
        "peek_typed",
        "read_typed",
        "load_as_owner",
        "save_typed",
        "save_typed_version",
        "save_typed_ephemeral",
        "save_typed_as_owner",
        "save_validated",
        "ensure_review_schema",
        "newest_valid_copy",
    ];
    let mut doors = Vec::new();
    for (file, text) in source_scan::production_sources() {
        let code = code_only(&text);
        for door in DOORS {
            let call = format!("{door}::<");
            for (at, _) in code.match_indices(&call) {
                if code[..at]
                    .chars()
                    .next_back()
                    .is_some_and(source_scan::is_ident)
                {
                    continue; // a longer name ending in this one
                }
                let argument = &code[at + call.len()..];
                let argument = &argument[..argument.find('>').expect("a closed type argument")];
                if argument.rsplit("::").next() == Some(document) {
                    doors.push(format!(
                        "{door} in {file} {}",
                        source_scan::enclosing_function(&code, at)
                    ));
                }
            }
        }
    }
    doors.sort();
    doors
}

/// Every production call of `path_fn()` (a document's path), as "file
/// function"; the declaration and longer names ending in it are not calls.
fn path_callers(path_fn: &str) -> Vec<String> {
    callers_of(&format!("{path_fn}()"))
}

/// Every production occurrence of `call` (a function name with its opening
/// parenthesis), as "file function"; the declaration and longer names ending
/// in it are not calls.
fn callers_of(call: &str) -> Vec<String> {
    let mut callers = Vec::new();
    for (file, text) in source_scan::production_sources() {
        let code = code_only(&text);
        for (at, _) in code.match_indices(call) {
            let before = &code[..at];
            if before.ends_with("fn ")
                || before
                    .chars()
                    .next_back()
                    .is_some_and(source_scan::is_ident)
            {
                continue;
            }
            callers.push(format!(
                "{file} {}",
                source_scan::enclosing_function(&code, at)
            ));
        }
    }
    callers.sort();
    callers
}

/// settings.json has one owner — the webview, through `load_settings` and
/// `save_settings` — and readers all over the backend: the scheduler's
/// authority check, the pollers, the notification, locale and translation
/// switches. A reader that goes through the owner's door moves a damaged file
/// aside before the webview has seen it: the recovery warning is dropped, the
/// next read finds no file, and the scheduler takes "no file" for "no rules"
/// and strips every approval. So the doors are a closed list: each storage
/// door that names `SettingsDoc`, and each function that asks for the
/// settings path, with the one function it is allowed in. Three of them may
/// set a damaged file aside — the owner's load, the owner's save, and the
/// queue's review barrier, which raises the envelope and never makes a
/// recovered document the main file — and every other one is `read_typed`. The quarantining `load_typed` is in none of them.
#[test]
fn only_the_settings_owner_uses_a_door_that_moves_the_file() {
    assert_eq!(
        storage_doors("SettingsDoc"),
        [
            "ensure_review_schema in scheduler/mod.rs save_queue",
            "load_as_owner in documents.rs load_settings_at",
            "read_typed in documents.rs current_settings_value",
            "read_typed in documents.rs settings_value_at",
            "read_typed in inbound.rs read_config_strict_at",
            "read_typed in inbound_channel.rs read_config_at",
            "read_typed in inbound_channel.rs read_config_strict_result",
            "save_typed_as_owner in documents.rs save_settings_locked_at",
        ],
        "a settings read outside the webview's load goes through storage::read_typed, which \
         never moves a file; only the owner's load and save and the review barrier may set \
         one aside"
    );
    assert_eq!(
        path_callers("settings_path"),
        [
            "documents.rs load_settings",
            "documents.rs recover_channel_grant_activations",
            "documents.rs retire_channel_grants_locked",
            "documents.rs save_settings",
            "documents.rs settings_value",
            "inbound.rs read_config_strict",
            "inbound_channel.rs read_config",
            "inbound_channel.rs read_config_strict_result",
            "scheduler/mod.rs save_queue",
        ],
        "a new reader of settings.json: route it through documents::settings_value, \
         inbound::read_config_strict or inbound_channel::read_config"
    );
}

/// deck.json the same way: the webview loads it (`load_board_at`) and saves
/// it (`save_board_at`), the backend reads it for the Connector
/// (`connector_board_payload_at`), and the way out of a
/// lost Board reads the backup and the copies a recovery set aside
/// (`board_recovery_at`, `lost_exit_at`). Only the owner's two doors may set
/// a damaged Board aside — the load, which then tells the user once, and the
/// save, when the file was damaged while deck ran — and every other read is
/// `read_typed`, `newest_valid_copy` or the non-moving `peek_typed`.
#[test]
fn only_the_board_owner_uses_a_door_that_moves_the_file() {
    assert_eq!(
        storage_doors("BoardDoc"),
        [
            "load_as_owner in documents.rs load_board_at",
            "newest_valid_copy in documents.rs lost_exit_at",
            "peek_typed in documents.rs save_board_at",
            "read_typed in documents.rs board_recovery_at",
            "read_typed in documents.rs connector_board_payload_at",
            "read_typed in documents.rs lost_exit_at",
            "save_typed_as_owner in documents.rs save_board_at",
        ],
        "a Board read outside the webview's load goes through storage::read_typed, which \
         never moves a file; only the owner's load and save may set one aside"
    );
    assert_eq!(
        path_callers("board_path"),
        [
            "documents.rs board_lost_exit",
            "documents.rs board_recovery_state",
            "documents.rs connector_board_payload",
            "documents.rs load_board",
            "documents.rs save_board",
        ],
        "a new reader of deck.json: route it through documents::connector_board_payload"
    );
}

/// The Board this process committed is what lifts the save fence over a lost
/// Board, so who commits one is a closed list: the webview's load, its save,
/// and the user's explicit way out. A fourth place would let something other
/// than the user's choice turn "nothing loadable" into a Board that saves.
#[test]
fn only_a_load_a_save_and_the_users_exit_commit_a_board() {
    assert_eq!(
        callers_of("commit_board("),
        [
            "documents.rs board_lost_exit_door",
            "documents.rs load_board_door",
            "documents.rs save_board",
        ],
        "a Board becomes committed by loading it, saving it, or the user's way out of a lost \
         Board (documents::board_lost_exit) — nowhere else"
    );
    // each door is its command at the one Board path, and nothing else
    for (door, command) in [
        ("load_board_door(", "documents.rs load_board"),
        ("board_lost_exit_door(", "documents.rs board_lost_exit"),
    ] {
        assert_eq!(callers_of(door), [command], "{door}");
    }
    assert!(
        callers_of("observe_committed(").is_empty(),
        "nothing calls the reminder projection's observer by name: the Board door tells its \
         registered observer from documents::commit_board"
    );
}

/// Who owns the committed Board. The save fence over a lost Board and the
/// base of the reminder checks are persistence decisions, so the copy they
/// read belongs to the Board door: one static, read by `committed_board`,
/// written by `commit_board`. `board_authority` is its second reader, for a
/// caller that treats what the Board says as authority; it sees the Board
/// only while it is the user's current version, never a recovered one.
/// `board_preset_choices` is the third, for the sweep and the fence that take
/// a preset's choice back: it also sees a recovered Board, which holds no
/// choice because `commit_board` withdraws them. The reminder module keeps its own copy to
/// project from (a wake or a tick projects without a commit) and offers it to
/// nobody. The door reaches the projection through one observer, registered
/// by the reminder module itself when its bridge starts; a second registrant,
/// or none, would change which Board is projected or stop the projection.
#[test]
fn the_board_door_owns_the_committed_board_and_tells_one_observer() {
    // every function that names `name`, its `static` declaration aside
    let users = |text: &str, name: &str| {
        let mut functions: Vec<String> = text
            .match_indices(name)
            .filter(|&(at, _)| {
                !text[..at].ends_with("static ")
                    && !text[..at]
                        .chars()
                        .next_back()
                        .is_some_and(source_scan::is_ident)
                    && !text[at + name.len()..]
                        .chars()
                        .next()
                        .is_some_and(source_scan::is_ident)
            })
            .map(|(at, _)| source_scan::enclosing_function(text, at))
            .collect();
        functions.sort();
        functions
    };
    let documents = code_only(production_region(&source("documents.rs")));
    assert_eq!(
        users(&documents, "COMMITTED_BOARD"),
        [
            "board_authority",
            "board_preset_choices",
            "commit_board",
            "committed_board"
        ],
        "the door's committed-Board copy has one writer and three readers: `committed_board` \
         for the door's own persistence decisions, `board_authority`, which answers only for a \
         current Board, for a reader that grants something on what the Board says, and \
         `board_preset_choices` for the readers that take a preset's choice back"
    );
    // who may read which: an admission grants only on the current Board, and
    // the one reader of a recovered Board's (empty) choices is the source the
    // sweep and the fence share
    assert_eq!(
        callers_of("board_preset_choices("),
        ["scheduler/first_send.rs board_side"]
    );
    assert_eq!(
        callers_of("set_commit_observer("),
        ["reminder.rs init"],
        "the Board door's commit observer is registered once, by the reminder bridge's init"
    );
    assert!(
        code_only(&source("notify.rs")).contains("crate::reminder::init();")
            && code_only(&source("main.rs")).contains("notify::init("),
        "the reminder bridge starts in setup, before the webview can load a Board"
    );
    // the door never names the projection, and the reminder module hands its
    // copy to no other module
    for name in ["observe_committed", "reconcile"] {
        assert!(
            users(&documents, name).is_empty(),
            "documents.rs names `{name}`: the projection is reached through the observer only"
        );
    }
    let reminder = code_only(production_region(&source("reminder.rs")));
    assert_eq!(
        users(&reminder, "COMMITTED"),
        ["observe_committed", "reconcile"],
        "the reminder module's copy of the Board is written by its observer and read by its \
         projection, and offered to no other module"
    );
}

/// The data directory has one resolver, `datadir::deck_dir()`, and in a
/// unit-test build it answers with a directory of the test process's own —
/// so a test that reaches a real save, a quarantine or the tmux config can
/// never touch the data of the person running the tests. A second place
/// that builds `~/.deck` from the home directory would get around that, so
/// those places are a closed list: the resolver, the boot-time removal of
/// the pre-0.5.12 helper copy (called from `main` only), and the one default
/// socket path each sidecar binary must spell for itself.
#[test]
fn the_data_directory_has_one_resolver() {
    const JOIN: &str = ".join(\".deck";
    let sites = |file: &str, text: &str| {
        text.match_indices(JOIN)
            .map(|(at, _)| format!("{file} {}", source_scan::enclosing_function(text, at)))
            .collect::<Vec<_>>()
    };
    let mut app = Vec::new();
    for (file, text) in source_scan::production_sources() {
        app.extend(sites(&file, &text));
    }
    app.sort();
    assert_eq!(
        app,
        [
            "agent_status.rs retire_legacy_helper_copy",
            "datadir.rs deck_dir",
        ],
        "a path under the data directory starts from datadir::deck_dir(), which unit tests \
         redirect; building it from the home directory reaches the user's real data"
    );
    for (file, function) in [
        ("status-helper/src/main.rs", "socket_path"),
        ("mcp-adapter/src/main.rs", "parse_args"),
    ] {
        let text = std::fs::read_to_string(source_scan::manifest(file)).unwrap();
        assert_eq!(
            sites(file, production_region(&text)),
            [format!("{file} {function}")],
            "{file}: one default socket path, where it was reviewed"
        );
    }
    // the resolver itself: the user's directory only outside a test build
    let datadir = source("datadir.rs");
    for (gate, body) in [
        ("#[cfg(not(test))]", ".join(\".deck\")"),
        ("#[cfg(test)]", "std::env::temp_dir()"),
    ] {
        let declared = format!("{gate}\npub(crate) fn deck_dir() -> PathBuf");
        assert_eq!(datadir.matches(&declared).count(), 1, "{declared}");
        let at = datadir.find(&declared).unwrap();
        let (open, close) = source_scan::body_span(&datadir, at).unwrap();
        assert!(datadir[open..close].contains(body), "{gate}: {body}");
    }
}

/// Automation authority revocation fence (`scheduler/authority.rs`): a
/// settings write and the automatic send's pre-fire authority decision
/// serialize on `storage::settings_fence`, taken before the queue lock and
/// released only after the firing intent transaction, before injection.
#[test]
fn settings_writes_and_the_pre_fire_authority_check_share_one_fence() {
    let documents = source("documents.rs");
    let command = source_scan::function_body(&documents, "save_settings").unwrap();
    let settings = command.find("storage::settings_fence()").unwrap();
    let board = command.find("board_fence()").unwrap();
    let save = command
        .find("save_settings_locked_at(&path, &canonical)")
        .unwrap();
    assert!(settings < board && board < save);
    for name in [
        "save_settings_at",
        "save_board",
        "recover_channel_grant_activations",
    ] {
        let body = source_scan::function_body(&documents, name).unwrap();
        assert!(body.contains("storage::settings_fence()"), "{name}");
    }
    let locked = source_scan::function_body(&documents, "save_settings_locked_at").unwrap();
    assert!(locked.contains("storage::save_typed_as_owner::<SettingsDoc>"));
    let board_save = source_scan::function_body(&documents, "save_board").unwrap();
    assert!(
        board_save.find("retire_channel_grants_locked").unwrap()
            < board_save.find("let saved = save_board_at").unwrap()
    );
    let delivery = source("scheduler/delivery.rs");
    let guarded = &delivery[delivery.find("fn send_one_guarded(").unwrap()..];
    let taken = guarded
        .find(".then(crate::storage::settings_fence)")
        .expect("fence taken");
    let intent = guarded.find("with_queue_opt(qm, persist,").unwrap();
    let released = guarded.find("drop(fence_guard);").expect("fence released");
    let fire = guarded.find("match (h.fire)(&item)").unwrap();
    assert!(taken < intent && intent < released && released < fire);
    // one settings read under the fence decides both the approval and the
    // first-send readiness override (`scheduler/first_send.rs`)
    let held = &guarded[..released];
    let read = held
        .find(".then(|| (h.authority)())")
        .expect("settings read");
    assert!(taken < read);
    assert!(held.contains("match fence(&sel, sources)"));
    // ...except a phone task's override and approval, which the Board side
    // backs (each decision reads the one source that backs it): that read
    // happens in the same held region, under the Board fence taken after
    // the settings fence and before the queue lock
    let board_taken = guarded
        .find(".then(crate::documents::board_fence);")
        .expect("Board fence taken");
    let board_released = guarded
        .find("drop(board_guard);")
        .expect("Board fence released");
    let board_read = held.find(".then(|| (h.board)())").expect("Board read");
    assert!(taken < board_taken && board_taken < board_read);
    assert!(intent < board_released && board_released < released);
    assert!(held.contains("settings: config.as_ref().and_then(Option::as_ref),"));
    assert!(held.contains("board: board.as_ref().and_then(Option::as_ref),"));
    assert!(held.contains("match first_send::fence(&sel, sources)"));
    assert!(held.contains("if overridden && sel.readiness_override.is_some()"));
    assert!(held.contains("channel_first_send::standing"));
    assert!(held.contains("inbound_channel::read_config_strict_result"));
    assert!(held.contains("documents::board_authority"));
}

/// The server outlives every exit but one. A launch that ends without ever
/// having been an ordinary one (`reminder::background_launch`: it only
/// delivered a notification answer) gives back the server its own boot gate
/// started when none existed (`tmux_lifecycle::retire_boot_server`). The
/// gate remembers such a start at both of its start sites and nowhere else:
/// a server that session creation starts is in use and is never remembered,
/// and no other exit or command retires anything.
#[test]
fn only_a_response_only_exit_gives_back_the_server_its_boot_gate_started() {
    assert_eq!(
        callers_of("retire_boot_server("),
        ["main.rs main"],
        "one caller: the exit event in main.rs"
    );
    let main = code_only(&source("main.rs"));
    let exit = &main[main.find("tauri::RunEvent::Exit").unwrap()..];
    let (open, close) = source_scan::body_span(exit, 0).unwrap();
    let handler = &exit[open..=close];
    let guard = handler
        .find("if reminder::background_launch() {")
        .expect("the retirement is asked for a response-only launch only");
    let (inner_open, inner_close) = source_scan::body_span(handler, guard).unwrap();
    assert_eq!(
        handler[inner_open..=inner_close]
            .split_whitespace()
            .collect::<Vec<_>>(),
        ["{", "tmux_lifecycle::retire_boot_server();", "}"],
        "the guarded block retires and does nothing else"
    );
    assert_eq!(handler.matches("retire_boot_server").count(), 1);

    assert_eq!(
        callers_of("start_missing_server_at_boot("),
        [
            "tmux_lifecycle.rs reconcile_on_boot",
            "tmux_lifecycle.rs reconcile_on_boot"
        ],
        "the boot gate remembers both of its starts from nothing"
    );
    let lifecycle = code_only(production_region(&source("tmux_lifecycle.rs")));
    let gate = source_scan::function_body(&lifecycle, "reconcile_on_boot").unwrap();
    assert!(
        !gate.contains("start_current_server("),
        "a start in the boot gate that is not remembered could never be given back"
    );
    let creation = source_scan::function_body(&lifecycle, "session_creation_guard").unwrap();
    assert!(
        creation.contains("start_current_server(&build)?")
            && !creation.contains("start_missing_server_at_boot"),
        "a server started for a session is in use: it is not remembered"
    );
    let mut users: Vec<String> = lifecycle
        .match_indices("BOOT_STARTED")
        .filter(|&(at, _)| !lifecycle[..at].ends_with("static "))
        .map(|(at, _)| source_scan::enclosing_function(&lifecycle, at))
        .collect();
    users.sort();
    assert_eq!(
        users,
        ["retire_boot_server", "start_missing_server_at_boot"],
        "the remembered server has one writer and one reader, which takes it"
    );
    // the retirement takes the record, then the lifecycle gate, then a bound
    // on how long a leaving process waits for tmux, and only then asks tmux
    let retire = source_scan::function_body(&lifecycle, "retire_boot_server").unwrap();
    let step = |text: &str| {
        retire
            .find(text)
            .unwrap_or_else(|| panic!("missing: {text}"))
    };
    let taken = step("BOOT_STARTED.lock_or_recover().take()");
    let gated = step("try_operation()");
    let bounded = step("Deadline::until(");
    let asked = step("retire_started_server_on(&deck_server(), &started)");
    assert!(taken < gated && gated < bounded && bounded < asked);
}
