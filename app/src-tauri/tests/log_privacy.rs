//! Privacy tripwires for app.log. Structure of the guarantee:
//!
//! 1. The ONLY frontend→log channel is the `ui_event` command, whose backend
//!    formatter admits nothing but a whitelisted code, a short slug (no
//!    spaces/slashes → no prose, prompts, paths or URLs) and two integers —
//!    that formatter is unit-tested in commands.rs
//!    (`ui_events_admit_no_free_form_content`).
//! 2. These tests pin the structure: no module may resurrect a free-form
//!    log channel (`ui_log`/`ulog`/`dlog`), every event call site must use a
//!    whitelisted code, no event call may build content-bearing arguments,
//!    and backend `applog` lines must not interpolate user content.
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

/// Every frontend event call must use a code the backend whitelists —
/// anything else is silently dropped as "unknown-event", i.e. dead code.
#[test]
fn every_event_call_site_uses_a_whitelisted_code() {
    let commands = std::fs::read_to_string(manifest("src/commands.rs")).unwrap();
    let list = commands
        .split("UI_EVENT_CODES: &[&str] = &[")
        .nth(1)
        .expect("whitelist present")
        .split("];")
        .next()
        .unwrap();
    let codes: Vec<&str> = list.split('"').skip(1).step_by(2).collect();
    assert!(codes.len() > 10, "whitelist parsed: {codes:?}");

    let mut sites = 0;
    for (name, src) in frontend_sources() {
        for caller in ["uev('", "duev('", "uevRaw('"] {
            for part in src.split(caller).skip(1) {
                let code = part.split('\'').next().unwrap_or("");
                sites += 1;
                assert!(
                    codes.contains(&code),
                    "{name}: event code {code:?} is not in the backend whitelist"
                );
            }
        }
    }
    assert!(sites > 20, "event call sites found: {sites}");
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
/// command lines, or a raw frontend string.
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

/// The verbose diagnostics gate must stay default-off, and even debug mode
/// must go through the structured channel (duev), not free-form strings.
#[test]
fn debug_logging_defaults_off_and_stays_structured() {
    let all: String = frontend_sources().into_iter().map(|(_, s)| s).collect();
    assert!(
        all.contains("window.__DECK_DEBUG = !!settings.debug"),
        "debug flag must derive from settings.debug"
    );
    assert!(
        all.contains("debug: false"),
        "settings.debug must default to false"
    );
    assert!(
        all.contains("if (window.__DECK_DEBUG) uev("),
        "debug logging must route through the structured event channel"
    );
}

// File-permission guarantees are BEHAVIORAL tests now, not source scans:
// storage.rs (every_saved_artifact_is_user_only, harden_migrates_legacy_modes_
// idempotently, quarantined_corrupt_file_is_user_only, concurrent_saves_stay_
// user_only) and history.rs (history_files_are_user_only_and_clear_removes_
// backup) verify real filesystem metadata in temp dirs.
