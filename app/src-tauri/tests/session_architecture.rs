//! Layering tripwires complement the behavioral delivery/restart tests.
//!
//! The real boundary for a layer is a crate split (a module that cannot name
//! another does not compile against it); until deck splits crates these
//! source scans stand in. `references` counts every way a file can reach a
//! module — a full path, `use crate::m;` followed by `m::x`, a grouped
//! `use crate::{a, m::x}`, and the same through `super::` — so a plain
//! import no longer slips past a check for `crate::m::`.
use std::path::Path;

fn source(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap()
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// How many times `source` names `module` through `crate::` or `super::`
/// (as a path, a `use`, or an item of a `{…}` group).
fn references(source: &str, module: &str) -> usize {
    let mut count = 0;
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
            count += heads
                .iter()
                .filter(|item| {
                    let item = item.trim_start();
                    let len = item.bytes().take_while(|&b| is_ident(b)).count();
                    &item[..len] == module
                })
                .count();
        }
    }
    count
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

/// The typed-documents door delegates exactly two things to feature modules:
/// the `inbound` settings section to its owner's validator and task-preset
/// commands to the channel admission table. Every other feature stays out.
#[test]
fn documents_delegate_only_settings_validation_and_preset_admission() {
    let documents = source("documents.rs");
    for module in [
        "mcp",
        "connector",
        "scheduler",
        "inbound_slack",
        "inbound_clock",
    ] {
        assert!(
            references(&documents, module) == 0,
            "documents depends on {module}"
        );
    }
    assert_eq!(references(&documents, "inbound"), 1);
    assert!(documents.contains("crate::inbound::validate_settings("));
    assert_eq!(references(&documents, "inbound_channel"), 0);
    assert_eq!(references(&documents, "admission"), 1);
    assert!(documents.contains("crate::admission::channel_agent_command("));
}
