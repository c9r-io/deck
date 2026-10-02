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

/// What the typed-documents door may name, as a closed list: the
/// infrastructure underneath it, two closed vocabularies it validates
/// against, the two delegations it makes to feature modules — and one
/// registered debt. A module that is not listed fails. A count is pinned only
/// where the number is the rule: one lookup per vocabulary, one call per
/// delegation, and a debt that may only shrink. Counted over production code
/// (no comments, no strings, not the trailing test module).
const DOCUMENTS_MAY_NAME: &[(&str, Option<usize>, &str)] = &[
    ("error", None, "the one error type"),
    (
        "storage",
        None,
        "the typed envelope and atomic writes underneath",
    ),
    ("datadir", None, "where deck.json and settings.json live"),
    (
        "smoke_faults",
        None,
        "debug-only save faults of the isolated smoke",
    ),
    (
        "tmux",
        Some(1),
        "the session-name rule the runtime enforces on start and attach",
    ),
    ("voice", Some(1), "the closed list of dictation languages"),
    (
        "inbound",
        Some(1),
        "delegation: the `inbound` settings section to its owner's validator",
    ),
    (
        "admission",
        Some(1),
        "delegation: task-preset commands to the channel admission table",
    ),
    // Registered debt, not an endorsement. Since the Card Reminder work the
    // Board door takes the reminder type and its validator (a Board-domain
    // delegation like the two above), and `load_board` / `save_board` also
    // read and feed the reminder module's in-memory mirror of the committed
    // Board. Moving that mirror behind the persistence side is governance
    // item 12-C3; until then this count may only go down.
    (
        "reminder",
        Some(8),
        "DEBT (12-C3): the reminder type, its validator and the committed-Board mirror",
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
/// queue's review barrier, which writes the document back — and every other
/// one is `read_typed`. The quarantining `load_typed` is in none of them.
#[test]
fn only_the_settings_owner_uses_a_door_that_moves_the_file() {
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
    ];
    let (mut doors, mut paths) = (Vec::new(), Vec::new());
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
                if argument.rsplit("::").next() == Some("SettingsDoc") {
                    doors.push(format!(
                        "{door} in {file} {}",
                        source_scan::enclosing_function(&code, at)
                    ));
                }
            }
        }
        for (at, _) in code.match_indices("settings_path()") {
            let before = &code[..at];
            if before.ends_with("fn ")
                || before
                    .chars()
                    .next_back()
                    .is_some_and(source_scan::is_ident)
            {
                continue; // the declaration, or another module's own path
            }
            paths.push(format!(
                "{file} {}",
                source_scan::enclosing_function(&code, at)
            ));
        }
    }
    doors.sort();
    paths.sort();
    assert_eq!(
        doors,
        [
            "ensure_review_schema in scheduler/mod.rs save_queue",
            "load_as_owner in documents.rs load_settings_at",
            "read_typed in documents.rs settings_value_at",
            "read_typed in inbound.rs read_config_strict_at",
            "read_typed in inbound_channel.rs read_config_at",
            "save_typed_as_owner in documents.rs save_settings_at",
        ],
        "a settings read outside the webview's load goes through storage::read_typed, which \
         never moves a file; only the owner's load and save and the review barrier may set \
         one aside"
    );
    assert_eq!(
        paths,
        [
            "documents.rs load_settings",
            "documents.rs save_settings",
            "documents.rs settings_value",
            "inbound.rs read_config_strict",
            "inbound_channel.rs read_config",
            "scheduler/mod.rs save_queue",
        ],
        "a new reader of settings.json: route it through documents::settings_value, \
         inbound::read_config_strict or inbound_channel::read_config"
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
    // the command is the path-bound form of the one function that writes
    let command = source_scan::function_body(&documents, "save_settings").unwrap();
    assert!(command.contains("save_settings_at(&settings_path(), &data)"));
    assert!(
        !command.contains("storage::"),
        "the command writes nothing itself"
    );
    let save = source_scan::function_body(&documents, "save_settings_at").unwrap();
    let fence = save
        .find("storage::settings_fence()")
        .expect("save_settings takes the fence");
    assert!(
        fence
            < save
                .find("storage::save_typed_as_owner::<SettingsDoc>")
                .unwrap()
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
    assert!(held.contains("match fence(&sel, config.as_ref().and_then(Option::as_ref))"));
    assert!(
        held.contains("match first_send::fence(&sel, config.as_ref().and_then(Option::as_ref))")
    );
}
