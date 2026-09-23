//! Layering tripwires complement the behavioral delivery/restart tests.
use std::path::Path;

fn source(name: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap()
}

#[test]
fn session_primitives_do_not_depend_on_business_policy() {
    let runtime = source("session_runtime.rs");
    for module in ["tmux", "restart", "shell_state", "scheduler", "voice"] {
        assert!(
            !runtime.contains(&format!("crate::{module}::")),
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
            !source(module).contains("crate::restart::"),
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
            !lifecycle.contains(&format!("crate::{module}::")),
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
            !ledger.contains(&format!("crate::{module}::")),
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
            !documents.contains(&format!("crate::{module}::")),
            "documents depends on {module}"
        );
    }
    assert_eq!(documents.matches("crate::inbound::").count(), 1);
    assert!(documents.contains("crate::inbound::validate_settings("));
    assert_eq!(documents.matches("crate::inbound_channel::").count(), 1);
    assert!(documents.contains("crate::inbound_channel::channel_agent_command("));
}
