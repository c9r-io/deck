//! Privacy tripwires for app.log. Structure of the guarantee:
//!
//! 1. The ONLY frontend→log channel is the `ui_event` command, whose backend
//!    formatter admits nothing but a whitelisted code, a closed detail and
//!    two integers. The formatter is unit-tested in diagnostics.rs
//!    (`ui_events_admit_no_free_form_content`), every (code, detail) pair the
//!    frontend can emit is pushed through it there
//!    (`every_frontend_event_label_survives_the_formatter`), and the writer
//!    and export sanitizers are exercised on real files in applog.rs and
//!    diagnostics.rs.
//! 2. These tests pin the remaining structure: no module may resurrect a
//!    free-form log channel (`ui_log`/`ulog`/`dlog`), no event call may build
//!    content-bearing arguments, backend `applog` lines must not interpolate
//!    user content, and the scheduler's context probe reads metadata only.
//!
//! All Rust modules and all frontend sources (js + index.html) are scanned.

use std::path::PathBuf;

fn manifest(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// (filename, contents) of every frontend source: all ES modules + the page.
fn frontend_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let dir = manifest("../ui/js");
    for e in std::fs::read_dir(&dir).expect("ui/js") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "js" || x == "mjs") {
            out.push((
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
    }
    out.push((
        "index.html".into(),
        std::fs::read_to_string(manifest("../ui/index.html")).unwrap(),
    ));
    assert!(out.len() > 3, "frontend sources found");
    out
}

/// (filename, contents) of every backend module.
fn backend_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(manifest("src")).expect("src") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            out.push((
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&p).unwrap(),
            ));
        }
    }
    assert!(out.len() >= 6, "all backend modules found");
    out
}

#[test]
fn free_form_log_channel_stays_dead() {
    for (name, src) in frontend_sources() {
        for pat in ["'ui_log'", "\"ui_log\"", "ulog(", "dlog("] {
            assert!(
                !src.contains(pat),
                "{name}: free-form log channel resurrected ({pat}) — use uev/duev event codes"
            );
        }
    }
    for (name, src) in backend_sources() {
        assert!(
            !src.contains("fn ui_log"),
            "{name}: ui_log command resurrected — ui_event is the only frontend log entry"
        );
    }
}

/// Event calls must not smuggle content through their arguments: no string
/// building, no error/stack objects, no chunk contents.
#[test]
fn event_calls_carry_no_content_bearing_arguments() {
    let forbidden = [
        "JSON.stringify", // serialized chunk/object contents
        "${",             // template-literal string building
        " + e",           // concatenating an error/exception into the slug
        ".stack",         // stack traces embed paths and source lines
        ".message +",     // building prose from error messages
    ];
    for (name, src) in frontend_sources() {
        for line in src.lines() {
            if line.contains("uev(") || line.contains("uevRaw(") || line.contains("duev(") {
                for pat in forbidden {
                    assert!(
                        !line.contains(pat),
                        "{name}: event call may leak content ({pat}):\n  {line}"
                    );
                }
            }
        }
    }
}

/// Backend log lines must never interpolate user content — prompt text,
/// command lines, a raw frontend string, or a raw tmux session name (which
/// is derived from the card title): those go through crate::applog::session_tag.
#[test]
fn backend_logs_carry_no_user_content() {
    let forbidden = [
        "&item.text",
        "item.text)",
        "item.text,",
        "{msg}", // the old ui_log passthrough
        "{text}",
        "{cmd}",
        "{prompt}",
        ".stack",
        // raw error Display / paths must go through crate::applog::err_code or a
        // file_name() — never interpolated into a log line verbatim
        ": {e}",
        ": {err}",
        ": {pe}",
        ".display()",
        // session names: log the per-run tag, never the name itself
        "{name}",
        "{session}",
        "{reader_name}",
        "{thread_name}",
        "item.session,",
        "self.name,",
    ];
    for (name, src) in backend_sources() {
        for line in src.lines() {
            if line.contains("applog") {
                for pat in forbidden {
                    assert!(
                        !line.contains(pat),
                        "{name}: backend log includes user content ({pat}):\n  {line}"
                    );
                }
            }
        }
    }
}

/// Verbose diagnostics are maintainer-only: they must stay default-off, derive
/// from the launch flag rather than user settings, and use the structured
/// channel (duev), not free-form strings.
#[test]
fn debug_logging_is_command_line_only_and_stays_structured() {
    let all: String = frontend_sources().into_iter().map(|(_, s)| s).collect();
    assert!(
        all.contains("window.__DECK_DEBUG = false"),
        "verbose diagnostics must default off before boot"
    );
    assert!(
        all.contains("inv('debug_logging_enabled')"),
        "verbose diagnostics must derive from the backend launch flag"
    );
    assert!(
        !all.contains("set-debug"),
        "Settings must not expose a debug control"
    );
    assert!(
        !all.contains("settings.debug"),
        "settings.json must not enable diagnostics"
    );
    assert!(
        all.contains("if (globalThis.window?.__DECK_DEBUG) uev("),
        "debug logging must route through the structured event channel"
    );
}

#[test]
fn scheduler_context_probe_is_metadata_only_and_content_free() {
    let context = std::fs::read_to_string(manifest("src/context.rs")).unwrap();
    assert!(context.contains("#{pane_current_command}"));
    assert!(
        !context.contains("@deck_agent_"),
        "production probe must not read user hook options"
    );
    assert!(
        !context.contains("\"capture-pane\""),
        "context probe must not invoke terminal capture"
    );
    assert!(
        !context.contains("applog("),
        "raw probe metadata must never be logged"
    );
}

// File-permission guarantees are BEHAVIORAL tests now, not source scans:
// storage.rs / datadir.rs (every_saved_artifact_is_user_only, harden_migrates_legacy_modes_
// idempotently, quarantined_corrupt_file_is_user_only, concurrent_saves_stay_
// user_only) and history.rs (history_files_are_user_only_and_clear_removes_
// backup) verify real filesystem metadata in temp dirs.
