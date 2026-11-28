//! IPC-thread census: every synchronous Tauri command, classified.
//!
//! An ordinary (non-async) `#[tauri::command]` runs on the window's main
//! thread, inside WebKit's URL-scheme callback: while it runs nothing else
//! of the window does (2026-11-26 and 2026-11-27 freezes,
//! `docs/evidence/deck-freeze-2026112*`). A command that may wait on a
//! process, tmux, another process's service, a lock a worker may hold, the
//! Keychain or a path outside `~/.deck` belongs off that thread
//! (`session_runtime::off_ipc_thread`, `pty.rs` `attach_within`,
//! `commands.rs` `session_op`). `#[tauri::command(async)] fn` and `async fn`
//! are not on it and are not listed here.
//!
//! This test finds every synchronous command in production source and
//! requires it to be registered in `SYNC_COMMANDS` with a class, and the
//! risky tokens its OWN body contains (spawn, tmux, waits and locks,
//! Keychain, user paths, sockets) to be exactly the registered ones. A new
//! synchronous command, a removed one, or a body that gains or loses one of
//! those tokens fails here until someone reviews it and updates the row.
//! It is a tripwire over each command's body, not over what it calls: the
//! class records what was reviewed beyond the body. The classes and their
//! rationale are in `docs/ipc-thread.md`.

mod source_scan;
use source_scan::{body_span, code_only, production_sources};

/// (needle, kind) searched in each synchronous command's body (comments and
/// string contents removed, whitespace collapsed).
const TOKENS: &[&str] = &[
    "Command::new",
    ".output()",
    ".status()",
    "tmux(",
    "tmux_owned(",
    "tmux_with_stdin(",
    "tmux_batch(",
    "query_snapshot(",
    "raw_probe(",
    "probe_server(",
    ".lock_or_recover()",
    ".lock()",
    ".recv(",
    ".wait(",
    "sleep(",
    "wait_or_recover",
    "keychain::",
    "canonicalize",
    "metadata(",
    "is_dir()",
    "exists()",
    "read_dir",
    "std::fs::",
    "home_dir",
    "UnixStream",
    "send_runner",
    ".delivery",
];

/// Why a synchronous command may stay on the IPC thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// In-memory state only (and events); its locks are held by short
    /// in-memory sections, never across a wait.
    Memory,
    /// Reads or writes Deck's own files under `~/.deck` (internal disk),
    /// bounded, plus in-memory state.
    DeckFiles,
    /// Must run on the main thread (AppKit, TIS, the notification bridge
    /// enqueue) and waits for no other process on it; or its native bridge
    /// hops to the main thread itself (`onMain`), so leaving the IPC thread
    /// would not take the work off it.
    MainThread,
    /// Reads the Keychain (securityd) on the IPC thread: a follow-up to move
    /// off (`docs/ipc-thread.md`, "Still on the IPC thread").
    Keychain,
    /// Stats or reads fixed paths of the system volume (`/Applications`,
    /// the home directory's own shell history), never a card's directory.
    InternalPaths,
    /// Debug smoke instrumentation: refused outside an isolated smoke launch.
    Smoke,
}

use Class::*;

/// Every synchronous command: (file, fn, class, risky tokens in its body).
const SYNC_COMMANDS: &[(&str, &str, Class, &[&str])] = &[
    ("commands.rs", "default_dir", Memory, &["home_dir"]),
    (
        "commands.rs",
        "detect_editors",
        InternalPaths,
        &["exists()", "home_dir"],
    ),
    ("commands.rs", "ping_event", Memory, &[]),
    ("connector/commands.rs", "connector_addresses", Memory, &[]),
    (
        "connector/commands.rs",
        "connector_claim",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "connector/commands.rs",
        "connector_complete",
        DeckFiles,
        &[],
    ),
    (
        "connector/commands.rs",
        "connector_disable",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "connector/commands.rs",
        "connector_pairing",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    ("connector/commands.rs", "connector_pending", DeckFiles, &[]),
    (
        "connector/commands.rs",
        "connector_reset_identity",
        Keychain,
        &[".lock_or_recover()", "keychain::"],
    ),
    ("connector/commands.rs", "connector_revoke", DeckFiles, &[]),
    (
        "connector/commands.rs",
        "connector_set_phone_replies",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "connector/commands.rs",
        "connector_set_scope",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "connector/commands.rs",
        "connector_set_trusted",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    ("connector/commands.rs", "connector_status", DeckFiles, &[]),
    (
        "connector/commands.rs",
        "connector_validate",
        DeckFiles,
        &[],
    ),
    (
        "connector/commands.rs",
        "connector_validate_admission",
        DeckFiles,
        &[],
    ),
    ("connector/smoke.rs", "connector_smoke_seed", Smoke, &[]),
    (
        "connector/smoke.rs",
        "connector_smoke_transport",
        Smoke,
        &["tmux(", ".lock_or_recover()", "keychain::", "std::fs::"],
    ),
    ("connector/smoke.rs", "connector_smoke_window", Smoke, &[]),
    ("diagnostics.rs", "debug_logging_enabled", Memory, &[]),
    ("diagnostics.rs", "log_size", DeckFiles, &[]),
    ("diagnostics.rs", "reset_logs", DeckFiles, &[]),
    ("diagnostics.rs", "ui_event", DeckFiles, &[]),
    ("documents.rs", "board_lost_exit", DeckFiles, &[]),
    ("documents.rs", "board_recovery_state", DeckFiles, &[]),
    ("documents.rs", "load_board", DeckFiles, &[]),
    ("documents.rs", "load_settings", DeckFiles, &[]),
    ("documents.rs", "save_board", DeckFiles, &[]),
    ("documents.rs", "save_settings", DeckFiles, &[]),
    ("documents.rs", "storage_warnings", Memory, &[]),
    ("drops.rs", "save_dropped_file", DeckFiles, &[]),
    ("history.rs", "history_clear", DeckFiles, &[]),
    (
        "history.rs",
        "recent_commands",
        InternalPaths,
        &["std::fs::", "home_dir"],
    ),
    ("history.rs", "record_command", DeckFiles, &[]),
    ("inbound.rs", "inbound_ack", DeckFiles, &[]),
    (
        "inbound.rs",
        "inbound_check_now",
        Memory,
        &[".lock_or_recover()"],
    ),
    ("inbound.rs", "inbound_pending", DeckFiles, &[]),
    ("inbound.rs", "inbound_run_ended", DeckFiles, &[]),
    ("inbound.rs", "inbound_runs", DeckFiles, &[]),
    ("inbound.rs", "inbound_status", Keychain, &["keychain::"]),
    ("inbound_channel.rs", "channel_ack", DeckFiles, &[]),
    ("inbound_channel.rs", "channel_check_pending", Memory, &[]),
    ("inbound_channel.rs", "channel_pending", DeckFiles, &[]),
    ("inbound_channel.rs", "channel_smoke_identity", Smoke, &[]),
    ("inbound_channel.rs", "channel_smoke_seed", Smoke, &[]),
    (
        "inbound_channel.rs",
        "channel_status",
        Keychain,
        &[".lock_or_recover()", "keychain::"],
    ),
    (
        "input_source.rs",
        "input_source_snapshot",
        MainThread,
        &[".lock_or_recover()"],
    ),
    (
        "intelligence/pasteboard.rs",
        "translation_clipboard_arm",
        MainThread,
        &[],
    ),
    (
        "intelligence/pasteboard.rs",
        "translation_clipboard_disarm",
        Memory,
        &[],
    ),
    (
        "intelligence/pasteboard.rs",
        "translation_clipboard_poll",
        MainThread,
        &[],
    ),
    (
        "intelligence/pasteboard.rs",
        "translation_clipboard_write",
        MainThread,
        &[],
    ),
    (
        "intelligence/translation.rs",
        "translation_cancel",
        Memory,
        &[".lock_or_recover()"],
    ),
    (
        "intelligence/translation.rs",
        "translation_capability",
        DeckFiles,
        &[],
    ),
    (
        "intelligence/translation.rs",
        "translation_pack_status",
        DeckFiles,
        &[],
    ),
    ("main.rs", "set_native_locale", MainThread, &[]),
    ("mcp/commands.rs", "mcp_adapter_path", Memory, &[]),
    (
        "mcp/commands.rs",
        "mcp_card_closed",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    ("mcp/commands.rs", "mcp_claim", DeckFiles, &[]),
    (
        "mcp/commands.rs",
        "mcp_client_add",
        Keychain,
        &["keychain::"],
    ),
    (
        "mcp/commands.rs",
        "mcp_enable",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    ("mcp/commands.rs", "mcp_pending", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_proposal_decide", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_proposals", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_status", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_worker_capacity", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_worker_hand_back", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_worker_take_over", DeckFiles, &[]),
    ("mcp/commands.rs", "mcp_workers_ui", DeckFiles, &[]),
    ("mcp/mod.rs", "mcp_card_mark_clear", DeckFiles, &[]),
    ("model_api.rs", "model_api_status", Keychain, &[]),
    ("notify.rs", "notify_cards", Memory, &[".lock_or_recover()"]),
    (
        "notify.rs",
        "notify_configure",
        MainThread,
        &[".lock_or_recover()"],
    ),
    (
        "notify.rs",
        "notify_dismiss",
        MainThread,
        &[".lock_or_recover()"],
    ),
    // `.status()` here is the notification bridge's authorization word
    ("notify.rs", "notify_status", MainThread, &[".status()"]),
    ("pty.rs", "detach_session", Memory, &[]),
    ("pty.rs", "pty_ack", Memory, &[]),
    ("pty.rs", "pty_write", Memory, &[".lock_or_recover()"]),
    (
        "reminder.rs",
        "reminder_ack",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "reminder.rs",
        "reminder_actions",
        Memory,
        &[".lock_or_recover()"],
    ),
    ("reminder.rs", "reminder_launch_visible", Memory, &[]),
    (
        "reminder.rs",
        "reminder_request_permission",
        MainThread,
        &[],
    ),
    ("reminder.rs", "reminder_show", MainThread, &[]),
    (
        "reminder.rs",
        "reminder_status",
        Memory,
        &[".lock_or_recover()"],
    ),
    (
        "scheduler/ops.rs",
        "channel_queue_add",
        DeckFiles,
        &[".lock_or_recover()"],
    ),
    (
        "scheduler/ops.rs",
        "channel_queue_add_reviewed_list",
        DeckFiles,
        &[],
    ),
    ("scheduler/ops.rs", "queue_acknowledge", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_add", DeckFiles, &[]),
    (
        "scheduler/ops.rs",
        "queue_add_reviewed_list",
        DeckFiles,
        &[],
    ),
    ("scheduler/ops.rs", "queue_clear_sessions", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_pause", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_remove", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_retry", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_skip", DeckFiles, &[]),
    ("scheduler/ops.rs", "queue_update", DeckFiles, &[]),
    ("scheduler/ops.rs", "smoke_flush_queue", Smoke, &[]),
    (
        "scheduler/ops.rs",
        "smoke_queue_state",
        Smoke,
        &[".lock_or_recover()"],
    ),
    (
        "scheduler/ops.rs",
        "smoke_seed_ambiguous",
        Smoke,
        &[".lock_or_recover()", ".delivery"],
    ),
    ("scheduler/review.rs", "queue_cancel_list", DeckFiles, &[]),
    ("scheduler/review.rs", "queue_review_mode", DeckFiles, &[]),
    ("shell_state.rs", "shell_snapshots_clear", DeckFiles, &[]),
    ("slack_api.rs", "slack_manifest", Memory, &[]),
    (
        "slack_mentions.rs",
        "slack_mention_clear_mark",
        DeckFiles,
        &[],
    ),
    ("slack_mentions.rs", "slack_mention_dismiss", DeckFiles, &[]),
    ("slack_mentions.rs", "slack_mention_route", DeckFiles, &[]),
    (
        "slack_mentions.rs",
        "slack_mention_routes_publish",
        Memory,
        &[".lock_or_recover()"],
    ),
    ("slack_mentions.rs", "slack_mentions", DeckFiles, &[]),
    ("slack_mentions.rs", "slack_mentions_refresh", Memory, &[]),
    ("slack_mentions.rs", "smoke_mentions_seed", Smoke, &[]),
    (
        "slack_transport.rs",
        "slack_channel_prepare_cancel",
        Memory,
        &[],
    ),
    ("smoke_faults.rs", "smoke_channel_fixture", Smoke, &[]),
    (
        "smoke_faults.rs",
        "smoke_clipboard_metrics",
        Smoke,
        &[".output()"],
    ),
    ("smoke_faults.rs", "smoke_attach_state", Smoke, &["tmux("]),
    (
        "smoke_faults.rs",
        "smoke_fault_set",
        Smoke,
        &[".lock_or_recover()"],
    ),
    ("smoke_faults.rs", "smoke_pi_fixture", Smoke, &[]),
    ("smoke_faults.rs", "smoke_query_channel", Smoke, &[]),
    ("smoke_faults.rs", "smoke_signal_fixture", Smoke, &[]),
    (
        "smoke_faults.rs",
        "smoke_tmux_server_signal",
        Smoke,
        &["tmux(", ".lock_or_recover()"],
    ),
    (
        "smoke_native.rs",
        "smoke_native_account_root",
        Smoke,
        &["is_dir()", "std::fs::"],
    ),
    ("smoke_native.rs", "smoke_native_fixture", Smoke, &[]),
    (
        "smoke_native.rs",
        "smoke_native_fixture_receipt",
        Smoke,
        &["std::fs::"],
    ),
    (
        "smoke_native.rs",
        "smoke_native_scenario",
        Smoke,
        &["std::fs::"],
    ),
    (
        "tmux_lifecycle.rs",
        "acknowledge_tmux_lifecycle_notice",
        DeckFiles,
        &[],
    ),
    ("tmux_lifecycle.rs", "agent_reentry_operation", Memory, &[]),
    ("updater.rs", "build_identity", Memory, &[]),
    ("voice.rs", "voice_cancel", MainThread, &[]),
    ("voice.rs", "voice_snapshot", Memory, &[]),
    ("voice.rs", "voice_start", MainThread, &[]),
    ("voice.rs", "voice_stop", MainThread, &[]),
];

/// (file, fn, risky tokens) of every synchronous command found.
fn found() -> Vec<(String, String, Vec<&'static str>)> {
    let mut out = Vec::new();
    for (file, source) in production_sources() {
        let code = code_only(&source);
        for (at, _) in code.match_indices("#[tauri::command") {
            let rest = &code[at + "#[tauri::command".len()..];
            if rest.starts_with("(async)") {
                continue;
            }
            // the item after the attribute(s): `[pub(crate)] [async] fn name(`
            let Some(fn_at) = rest.find("fn ") else {
                continue;
            };
            let head = &rest[..fn_at];
            if head.contains(" async ") || head.trim_end().ends_with("async") {
                continue;
            }
            let name: String = rest[fn_at + 3..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let start = at + "#[tauri::command".len() + fn_at;
            let (open, close) = body_span(&code, start).expect("command body");
            let body = &code[open..=close];
            let tokens = TOKENS
                .iter()
                .copied()
                .filter(|t| body.contains(t))
                .collect();
            out.push((file.clone(), name, tokens));
        }
    }
    out.sort();
    out
}

#[test]
fn every_synchronous_command_is_classified_and_its_risky_tokens_reviewed() {
    let found = found();
    let mut problems = Vec::new();
    for (file, name, tokens) in &found {
        match SYNC_COMMANDS
            .iter()
            .find(|(f, n, _, _)| f == file && n == name)
        {
            None => problems.push(format!(
                "unregistered synchronous command: (\"{file}\", \"{name}\", ?, &{tokens:?}),"
            )),
            Some((_, _, _, reviewed)) if reviewed != &tokens.as_slice() => problems.push(format!(
                "{file} {name}: risky tokens changed from {reviewed:?} to {tokens:?}: review it"
            )),
            Some(_) => {}
        }
    }
    for (file, name, _, _) in SYNC_COMMANDS {
        if !found.iter().any(|(f, n, _)| f == file && n == name) {
            problems.push(format!(
                "{file} {name}: registered but no longer a synchronous command"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// No synchronous command may start a process or call tmux on the IPC
/// thread outside debug smoke instrumentation: those are exactly what
/// froze the window.
#[test]
fn no_synchronous_command_spawns_or_waits_on_tmux() {
    let blocking = [
        "Command::new",
        "tmux(",
        "tmux_owned(",
        "tmux_with_stdin(",
        "tmux_batch(",
        "query_snapshot(",
        "raw_probe(",
        "probe_server(",
        "send_runner",
        "UnixStream",
    ];
    for (file, name, class, tokens) in SYNC_COMMANDS {
        if *class == Smoke {
            continue;
        }
        let bad: Vec<_> = tokens.iter().filter(|t| blocking.contains(t)).collect();
        assert!(
            bad.is_empty(),
            "{file} {name} waits on {bad:?} on the IPC thread"
        );
    }
}

/// The commands the 2026-11-27 freeze investigation named (and the ones
/// found beside them) are no longer synchronous.
#[test]
fn the_commands_that_froze_or_could_freeze_the_window_are_off_the_ipc_thread() {
    let found = found();
    for name in [
        "attach_session",
        "start_session",
        "kill_session",
        "mcp_start_session",
        "tmux_available",
        "write_clipboard",
        "export_logs",
        "resolve_parent_dir",
        "open_target",
        "link_remote",
        "open_remote_target",
        "voice_open_settings",
        "mcp_scope_preview",
        "agent_hooks_status",
        "agent_hooks_set",
        "project_env_names",
        "project_env_set",
        "project_env_clear",
        "queue_send_now",
        "queue_list",
        "queue_probe_context",
        "queue_review_preview",
        "queue_review_confirm",
        "tmux_server_status",
        "defer_tmux_restart",
        "mcp_close_admit",
        "mcp_complete",
        "mcp_client_delete",
        "mcp_output_retention",
        "inbound_setup",
        "relaunch_after_update",
    ] {
        assert!(
            !found.iter().any(|(_, n, _)| n == name),
            "{name} is synchronous again"
        );
    }
}
