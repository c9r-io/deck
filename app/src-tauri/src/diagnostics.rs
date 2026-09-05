//! Structured, content-free diagnostics: the closed `ui_event` whitelist
//! (code + per-code detail policy + two ints), log size/reset, and sanitized
//! exports. Nothing free-form from the webview ever reaches `app.log`.
//!
//! # Contract
//! Run: `app/run.sh` — builds, wraps the binary in a minimal .app, launches via
//! `open`. NEVER run the bare binary from a background shell: outside the GUI
//! login session the process can't reach macOS text-input services (TSM/IMK) —
//! window and mouse work, keyboard is silently dead. `~/.deck/app.log` (0600)
//! collects backend + frontend diagnostics. Maintainer-only verbose frontend
//! events are enabled at launch with `app/run.sh --debug-logging`; there is no
//! user setting for them. Frontend logging is STRUCTURED
//! ONLY: the `ui_event` command takes a whitelisted code + a detail vetted by
//! that code's OWN closed policy (enum values / version pattern — no generic
//! slug rule) + two ints, and redacts everything else — never add a free-form
//! frontend log channel (log_privacy tests enforce this). Backend log lines
//! never interpolate raw error Display text or a raw session NAME:
//! `storage::err_code()` maps errors to stable path-free categories (the full
//! error goes only to the operation's caller) and `storage::session_tag()`
//! gives a per-RUN, non-reversible tag. Every line is redacted again by
//! `sanitize_log` on its way to disk (absolute paths, `~/`, any `scheme://`,
//! credential prefixes, long opaque tokens, session-name shapes →
//! `<redacted>`); exports sanitize their own header AND body instead of
//! trusting app.log; `sanitize_existing_logs` migrates logs/exports an older
//! deck wrote, in place, at boot (atomic, 0600, no raw copy kept). The
//! runtime privacy tests write REAL files through `applog_to` into temp dirs
//! — never stub the writer and call it proven. The whole `~/.deck` tree is private by construction
//! (dir 0700, every file created 0600 — atomic-write temps, `.bak`,
//! `.corrupt-*`, log, exports); `harden_data_dir()` re-migrates legacy modes
//! at every boot.
//!
//! Clipboard diagnostics are always structured and content-free. Copy records
//! terminal key capture, Deck/native/no-selection routing, snapshot loss and
//! the `pbcopy`/Web Clipboard writer result. `pbcopy` is spawned with
//! `LANG=en_US.UTF-8` (`pbcopy_command`): a GUI-launched deck has no locale,
//! and under the C locale pbcopy writes an EMPTY pasteboard item for any
//! non-ASCII input while exiting 0 — every copy from an agent pane (Chinese,
//! box-drawing, `⏺`) "succeeded" and pasted nothing. Text paste records the closed chain
//! key capture → xterm key handler → native paste event → xterm `onData` → PTY
//! write, with bounded missing-stage timers. Only fixed labels and character or
//! file counts enter `app.log`; clipboard text, errors and session names never
//! do. Per-pane timers are disposed with the pane.
//! ⌘C can only report what it FOUND, so `terminal-selection` records the
//! selection's own life: `promote` / `start-ok` / `finish-ok` (or
//! `start-failed` / `update-failed` / `finish-failed` / `freeze-failed`,
//! which previously cancelled behind nothing but a toast), plus one
//! `cancel-<reason>` naming every revoke — pointer, pointer-cancel, blur,
//! hidden, input, escape, focus, live, exit, leave, dispose. That is what
//! separates a `terminal-copy keydown-none` caused by a drag that never
//! promoted from one caused by a live selection something took away. A cancel
//! with nothing to destroy stays silent, so ordinary clicks do not flood the
//! log; a caller that already logged a specific failure passes a null reason
//! instead of a second anonymous line. The two integers are a per-label count
//! (rows spanned, or 1 when a FROZEN selection died; for `finish-failed` a
//! reason code — 1 the pane had left copy-mode, 2 copy-mode kept but its
//! selection cleared, 0 other — from the backend's closed
//! `selection-missing-inactive|cleared` suffix) and the selection's age in
//! milliseconds — never text, coordinates of content, or an error string.
//! Three forensic labels attribute the dominant field failure (a completed
//! selection revoked before ⌘C arrives): `revoker-<class>` pairs with
//! `cancel-pointer` and classifies the destroying pointerdown by provenance
//! (trusted pointerType mouse/touch/pen/unknown, or synthetic when isTrusted
//! is false; its ints are click count and ms since the last pointerup — the
//! one label whose `b` is not selection age); `native-cleared` marks an xterm
//! selection appearing while Deck owned the drag (WKWebView's late
//! compatibility-mouse replay); and `terminal-copy keydown-elsewhere` replaces
//! `keydown-none` when another pane still holds a live Deck selection (count +
//! its age), separating "revoked" from "⌘C reached the wrong pane".

use std::path::PathBuf;
use std::process::Command;

use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::tmux;

/// Per-event detail policy: which detail strings an event code may log.
/// Anything outside its policy is logged as `<redacted>` — the code and the
/// integers survive, the string does not. There is deliberately NO generic
/// "looks like a slug" fallback: a token-shaped secret is still a secret.
pub(crate) enum DetailPolicy {
    /// this event never carries a detail string
    None,
    /// closed enumeration of allowed values (exact match)
    Closed(&'static [&'static str]),
    /// a bare version number: digits and dots only, ≤16 chars
    Version,
}

const JS_ERROR_CLASSES: &[&str] = &[
    "TypeError",
    "ReferenceError",
    "SyntaxError",
    "RangeError",
    "EvalError",
    "URIError",
    "AggregateError",
    "InternalError",
    "error",
];

/// CSP directive names the securitypolicyviolation listener may report.
const CSP_DIRECTIVES: &[&str] = &[
    "default-src",
    "script-src",
    "script-src-elem",
    "script-src-attr",
    "style-src",
    "style-src-elem",
    "style-src-attr",
    "img-src",
    "font-src",
    "connect-src",
    "media-src",
    "object-src",
    "worker-src",
    "frame-src",
    "form-action",
    "base-uri",
];

/// Rust→JS event names the frontend registers listeners for.
/// What the webview did with an inbound item — never what the item was.
const INBOUND_OUTCOMES: &[&str] = &[
    "created",
    "duplicate",
    "no-rule-target",
    "no-template",
    "create-fail",
    "queue-fail",
    "ack-fail",
];

const LISTEN_TARGETS: &[&str] = &[
    "deck-ping",
    "update-check",
    "update-check-manual",
    "update-download-progress",
    "menu-clear",
    "queue-changed",
    "queue-fired",
    "pty-data",
    "pty-exit",
    "inbound-changed",
];

/// Keydown CATEGORIES — the frontend classifies before sending; a raw key
/// name (let alone typed text) never crosses the bridge.
const KEY_CLASSES: &[&str] = &[
    "char",
    "plus",
    "equal",
    "minus",
    "enter",
    "backspace",
    "delete",
    "tab",
    "escape",
    "arrow",
    "mod",
    "fn",
    "nav",
    "compose",
    "other",
];

/// Foreground-process CATEGORIES for record-skip (why a typed line was not
/// recorded). Process names themselves stay out of the log.
const FG_CLASSES: &[&str] = &["no-card", "no-fg", "agent", "editor", "repl", "other"];
const SMOKE_CHECKS: &[&str] = &[
    "rename",
    "selection-up",
    "selection-markers",
    "selection-live",
    "selection-reverse",
    "selection-down",
    "selection-cancel",
    "selection-split",
    "selection-detach",
    "selection-clipboard",
    "selection-owner",
    "selection-gestures",
    "selection-repeat",
    "selection-scroll-stable",
    "selection-scroll-cursor",
    "selection-overlay",
    "selection-drag-overlay",
    "selection-native-scroll",
    "selection-resize",
    "scroll-frame",
    "link-activate",
    "link-classify",
    "ime-routing",
    "path-menu",
    "path-editor",
    "path-session-relative",
    "path-session-absolute",
    "completion",
    "completion-bottom",
    "completion-pixels",
    "completion-gap",
    "completion-scroll",
    "completion-resize",
    "completion-long",
    "completion-hidden",
    "board-concurrency",
    "board-fault",
    "theme-switch",
    "theme-rollback",
    "settings-viewport",
    "settings-navigation",
    "settings-logs",
    "button-force-touch",
    "button-context-menu",
    "surface-context-menu",
    "font-layout",
    "natural-fault",
    "completion-owner",
    "ambiguous-boot",
    "scheduler-context",
    "rename-restart",
    "done",
];

/// Terminal selection lifecycle. `terminal-copy` can only report what ⌘C
/// FOUND; these say how the selection got there or what took it away, so a
/// `keydown-none` copy can be attributed to a drag that never promoted, a
/// start tmux refused, or a specific later revoke. Labels only — no terminal
/// text, session name or error string is representable here.
///
/// `revoker-*` pairs with `cancel-pointer` and classifies the pointerdown
/// that destroyed a live selection: `synthetic` is an untrusted event, the
/// rest are the trusted pointerType. `native-cleared` records an xterm
/// selection appearing (and being cleared) while Deck owned the drag — the
/// signature of WKWebView's late compatibility mouse replay.
const SELECTION_EVENTS: &[&str] = &[
    "promote",
    "start-ok",
    "start-failed",
    "finish-ok",
    "finish-failed",
    "update-failed",
    "dimensions-changed",
    "freeze-ok",
    "freeze-failed",
    "native-cleared",
    "revoker-mouse",
    "revoker-touch",
    "revoker-pen",
    "revoker-unknown",
    "revoker-synthetic",
    "cancel-pointer",
    "cancel-pointer-cancel",
    "cancel-blur",
    "cancel-hidden",
    "cancel-input",
    "cancel-escape",
    "cancel-focus",
    "cancel-live",
    "cancel-exit",
    "cancel-leave",
    "cancel-dispose",
    "cancel-other",
];

/// The only frontend diagnostic codes the backend will log, each with its
/// closed detail policy. Anything else is dropped, so no free-form frontend
/// string (keystrokes, prompts, paths, URLs, error messages, token-shaped
/// slugs) can reach app.log even if the webview is compromised.
const UI_EVENT_SPECS: &[(&str, DetailPolicy)] = &[
    ("js-error", DetailPolicy::Closed(JS_ERROR_CLASSES)),
    ("js-reject", DetailPolicy::Closed(JS_ERROR_CLASSES)),
    ("csp-block", DetailPolicy::Closed(CSP_DIRECTIVES)),
    ("listen-fail", DetailPolicy::Closed(LISTEN_TARGETS)),
    ("ping-recv", DetailPolicy::None),
    ("ping-fail", DetailPolicy::None),
    ("update-avail", DetailPolicy::Version),
    ("update-check-fail", DetailPolicy::Closed(&["manual"])),
    ("update-install-fail", DetailPolicy::None),
    ("board-load-fail", DetailPolicy::None),
    ("settings-load-fail", DetailPolicy::None),
    ("settings-save-fail", DetailPolicy::None),
    ("inbound", DetailPolicy::Closed(INBOUND_OUTCOMES)),
    ("poll-fail", DetailPolicy::None),
    ("poll-recovered", DetailPolicy::None),
    ("clipboard-addon-fail", DetailPolicy::None),
    (
        "separator",
        DetailPolicy::Closed(&["no-marker", "at", "fail"]),
    ),
    ("mirror-desync", DetailPolicy::Closed(&["esc", "plain"])),
    ("ondata", DetailPolicy::Closed(&["desync", "ok"])),
    ("pty-write-fail", DetailPolicy::None),
    ("pty-rx", DetailPolicy::None),
    ("keydown", DetailPolicy::Closed(KEY_CLASSES)),
    ("composition", DetailPolicy::Closed(&["start", "end"])),
    (
        "terminal-copy",
        DetailPolicy::Closed(&[
            "key-capture",
            "keydown-deck",
            "keydown-native",
            "keydown-none",
            "keydown-elsewhere",
            "success",
            "selection-vanished",
            "selection-missing",
            "snapshot-failed",
            "clipboard-write-failed",
        ]),
    ),
    (
        "terminal-paste",
        DetailPolicy::Closed(&[
            "key-capture",
            "key-handler",
            "handler-missing",
            "event-text",
            "event-empty",
            "event-file",
            "event-unavailable",
            "event-missing",
            "ondata",
            "ondata-missing",
            "pty-success",
            "pty-failed",
        ]),
    ),
    ("terminal-selection", DetailPolicy::Closed(SELECTION_EVENTS)),
    (
        "clipboard-write",
        DetailPolicy::Closed(&[
            "pbcopy-success",
            "pbcopy-failed",
            "web-success",
            "web-failed",
            "web-unavailable",
        ]),
    ),
    ("record", DetailPolicy::None),
    ("record-skip", DetailPolicy::Closed(FG_CLASSES)),
    ("record-fail", DetailPolicy::None),
    ("smoke-check", DetailPolicy::Closed(SMOKE_CHECKS)),
];

fn detail_allowed(policy: &DetailPolicy, d: &str) -> bool {
    match policy {
        DetailPolicy::None => false,
        DetailPolicy::Closed(set) => set.contains(&d),
        DetailPolicy::Version => {
            !d.is_empty() && d.len() <= 16 && d.chars().all(|c| c.is_ascii_digit() || c == '.')
        }
    }
}

/// Pure formatter so the sanitization rules are unit-testable: whitelisted
/// code, detail vetted by that code's OWN policy (closed enum / version
/// pattern — never a generic slug), plus up to two integers.
pub(crate) fn format_ui_event(
    code: &str,
    detail: Option<&str>,
    a: Option<i64>,
    b: Option<i64>,
) -> Option<String> {
    let (_, policy) = UI_EVENT_SPECS.iter().find(|(c, _)| *c == code)?;
    let mut s = format!("[ui] {code}");
    if let Some(d) = detail {
        if detail_allowed(policy, d) {
            s.push(' ');
            s.push_str(d);
        } else {
            s.push_str(" <redacted>");
        }
    }
    if let Some(a) = a {
        s.push_str(&format!(" a={a}"));
    }
    if let Some(b) = b {
        s.push_str(&format!(" b={b}"));
    }
    Some(s)
}

#[tauri::command]
pub(crate) fn ui_event(code: String, detail: Option<String>, a: Option<i64>, b: Option<i64>) {
    match format_ui_event(&code, detail.as_deref(), a, b) {
        Some(line) => applog(&line),
        None => applog("[ui] unknown-event"),
    }
}

#[tauri::command]
pub(crate) fn debug_logging_enabled() -> bool {
    storage::command_flag("--debug-logging")
}

/// Build the export text. EVERY line — the environment header and the log
/// body alike — goes through the log sanitizer on the way out: an export is
/// meant to be mailed to someone, so it must not be able to inherit anything
/// an older deck (or a future call site) left in app.log.
pub(crate) fn build_export(header: &str, log: &str) -> String {
    let mut out = String::with_capacity(header.len() + log.len() + 32);
    for line in header.lines() {
        out.push_str(&storage::sanitize_log(line));
        out.push('\n');
    }
    out.push_str("\n===== app.log =====\n");
    for line in log.lines() {
        out.push_str(&storage::sanitize_log(line));
        out.push('\n');
    }
    out
}

#[tauri::command]
pub(crate) fn log_size() -> Result<u64, String> {
    storage::log_size_at(&storage::deck_dir())
}

#[tauri::command]
pub(crate) fn reset_logs() -> Result<(), String> {
    storage::reset_logs_at(&storage::deck_dir())
}

#[tauri::command]
pub(crate) fn export_logs() -> Result<PathBuf, String> {
    let data_dir = storage::deck_dir();
    let dir = data_dir.join("exports");
    storage::create_private_dir(&dir)?;
    let name = format!("deck-log-{}.txt", now_epoch());
    let path = dir.join(name);

    let mut header = String::new();
    header.push_str(&format!("deck {}\n", env!("CARGO_PKG_VERSION")));
    if let Ok(o) = Command::new("sw_vers").output() {
        header.push_str(&String::from_utf8_lossy(&o.stdout));
    }
    if let Ok(o) = Command::new("uname").arg("-m").output() {
        header.push_str(&format!("arch: {}", String::from_utf8_lossy(&o.stdout)));
    }
    // classification only — the absolute tmux path stays out of exports
    header.push_str(&format!("tmux: {}\n", crate::tmux::tmux_kind()));
    header.push_str(&format!(
        "sessions: {}\n",
        tmux(&["list-sessions", "-F", "#{session_name}"])
            .map(|s| s.lines().count())
            .unwrap_or(0)
    ));
    let log = std::fs::read_to_string(data_dir.join("app.log")).unwrap_or_default();
    // created 0600 from the first byte — never world-readable-then-chmod
    storage::write_private(&path, build_export(&header, &log).as_bytes())?;
    let _ = Command::new("open").arg("-R").arg(&path).status();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_events_admit_no_free_form_content() {
        // unknown codes never reach the log line
        assert!(format_ui_event("rm -rf", None, None, None).is_none());
        assert!(format_ui_event("", None, None, None).is_none());
        // per-code closed values + numbers pass
        assert_eq!(
            format_ui_event("js-error", Some("TypeError"), Some(42), None).unwrap(),
            "[ui] js-error TypeError a=42"
        );
        assert_eq!(
            format_ui_event("keydown", Some("arrow"), Some(0), None).unwrap(),
            "[ui] keydown arrow a=0"
        );
        assert_eq!(
            format_ui_event("csp-block", Some("script-src"), None, None).unwrap(),
            "[ui] csp-block script-src"
        );
        assert_eq!(
            format_ui_event("listen-fail", Some("pty-data"), None, None).unwrap(),
            "[ui] listen-fail pty-data"
        );
        assert_eq!(
            format_ui_event("record-skip", Some("agent"), None, None).unwrap(),
            "[ui] record-skip agent"
        );
        assert_eq!(
            format_ui_event("update-avail", Some("0.4.27"), None, None).unwrap(),
            "[ui] update-avail 0.4.27"
        );
        assert_eq!(
            format_ui_event("terminal-copy", Some("snapshot-failed"), None, None).unwrap(),
            "[ui] terminal-copy snapshot-failed"
        );
        assert_eq!(
            format_ui_event("terminal-paste", Some("ondata"), Some(42), None).unwrap(),
            "[ui] terminal-paste ondata a=42"
        );
        assert_eq!(
            format_ui_event("clipboard-write", Some("pbcopy-success"), Some(42), None).unwrap(),
            "[ui] clipboard-write pbcopy-success a=42"
        );
        assert_eq!(
            format_ui_event("terminal-selection", Some("promote"), Some(3), Some(0)).unwrap(),
            "[ui] terminal-selection promote a=3 b=0"
        );
        assert_eq!(
            format_ui_event(
                "terminal-selection",
                Some("cancel-blur"),
                Some(1),
                Some(4200)
            )
            .unwrap(),
            "[ui] terminal-selection cancel-blur a=1 b=4200"
        );
        // a revoke reason the frontend never defines must not become a log line
        assert_eq!(
            format_ui_event("terminal-selection", Some("cancel-"), None, None).unwrap(),
            "[ui] terminal-selection <redacted>"
        );
        // anything that could carry prose, prompts, paths, URLs or a
        // token-SHAPED slug (the old loophole) is redacted per event code
        for bad in [
            "my secret prompt text",
            "/Users/example/private",
            "https://example.com/x",
            "file:///secret",
            "key=$AWS_SECRET",
            "ghp_AbCdEf0123456789",
            "sk_live_4242424242",
            "distinctive-secret-9f8e",
            "TypeErrorX", // near-miss of a closed value
            "line1\nline2",
            "词语",
        ] {
            for code in [
                "js-error",
                "keydown",
                "record-skip",
                "separator",
                "terminal-copy",
                "terminal-paste",
                "terminal-selection",
                "clipboard-write",
            ] {
                let line = format_ui_event(code, Some(bad), None, None).unwrap();
                assert_eq!(line, format!("[ui] {code} <redacted>"), "leaked: {bad}");
                assert!(!line.contains("secret") && !line.contains("ghp_"));
            }
        }
        // codes with no detail policy redact ANY detail
        assert_eq!(
            format_ui_event("poll-fail", Some("anything"), None, None).unwrap(),
            "[ui] poll-fail <redacted>"
        );
        // version policy admits only bare dotted numbers
        for bad in ["0.4.27-nightly", "v0.4.27", "1.2.3.4.5.6.7.8.9.10.11", ""] {
            assert!(format_ui_event("update-avail", Some(bad), None, None)
                .unwrap()
                .ends_with("<redacted>"));
        }
    }

    /// The cross-language contract behind every diagnostic line. Each
    /// (code, detail) pair the frontend can emit is pushed through the REAL
    /// formatter: the code must be whitelisted and the detail must survive,
    /// because a label the backend redacts is a diagnostic that says nothing.
    /// Call sites are harvested from the production modules; the checks run
    /// against `format_ui_event`, not against source text.
    #[test]
    fn every_frontend_event_label_survives_the_formatter() {
        let ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/js");
        let mut sources: Vec<(String, String)> = std::fs::read_dir(&ui)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "js"))
            .map(|p| {
                (
                    p.file_name().unwrap().to_string_lossy().into_owned(),
                    std::fs::read_to_string(&p).unwrap(),
                )
            })
            .collect();
        sources.sort();
        assert!(sources.len() > 10, "frontend modules found");

        // `marker('code'` → the single-quoted literal that follows.
        fn literal_after<'a>(text: &'a str, marker: &str) -> Vec<(&'a str, &'a str)> {
            text.match_indices(marker)
                .filter_map(|(at, _)| {
                    let rest = &text[at + marker.len()..];
                    let code = rest.split('\'').next()?;
                    let after = rest[code.len() + 1..].trim_start_matches([',', ' ']);
                    let detail = after.strip_prefix('\'').and_then(|d| d.split('\'').next());
                    Some((code, detail.unwrap_or("")))
                })
                .collect()
        }
        let mut sites = 0;
        let mut pairs: Vec<(String, String, String)> = Vec::new();
        for (file, text) in &sources {
            for marker in ["uev('", "duev('"] {
                for (code, detail) in literal_after(text, marker) {
                    sites += 1;
                    let line = format_ui_event(code, None, None, None);
                    assert!(
                        line.is_some(),
                        "{file}: event code {code:?} is not whitelisted"
                    );
                    if !detail.is_empty() {
                        pairs.push((file.clone(), code.into(), detail.into()));
                    }
                }
            }
            // Labels that reach `uev` through a local wrapper or a builder.
            let indirect: &[(&str, &str, &str)] = &[
                ("selection.js", "sev('", "terminal-selection"),
                ("pure.js", "emit('", "terminal-paste"),
            ];
            for (owner, marker, code) in indirect {
                if file == owner {
                    for (label, _) in literal_after(text, marker) {
                        pairs.push((file.clone(), code.to_string(), label.into()));
                    }
                }
            }
            for (at, _) in text.match_indices("'revoker-") {
                let word: String = text[at + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                    .collect();
                pairs.push((file.clone(), "terminal-selection".into(), word));
            }
            // `cancel(…, 'reason')` becomes the `cancel-<reason>` label.
            for marker in [
                "cancel(",
                "cancelTerminalSelection(",
                "cancelAllTerminalSelections(",
            ] {
                for (at, _) in text.match_indices(marker) {
                    let call = &text[at..text.len().min(at + 90)];
                    let Some(call) = call.split(')').next() else {
                        continue;
                    };
                    let reason = call.rsplit('\'').nth(1).filter(|r| {
                        !r.is_empty() && r.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                    });
                    if let Some(reason) = reason {
                        pairs.push((
                            file.clone(),
                            "terminal-selection".into(),
                            format!("cancel-{reason}"),
                        ));
                    }
                }
            }
            if file == "selection.js" {
                for label in ["selection-missing", "snapshot-failed"] {
                    assert!(text.contains(label), "copy failure code {label} vanished");
                    pairs.push((file.clone(), "terminal-copy".into(), label.into()));
                }
            }
        }
        assert!(sites > 20, "event call sites found: {sites}");
        assert!(pairs.len() > 40, "labelled sites found: {}", pairs.len());
        for (file, code, detail) in &pairs {
            let line = format_ui_event(code, Some(detail), None, None)
                .unwrap_or_else(|| panic!("{file}: {code} is not whitelisted"));
            assert!(
                line.ends_with(&format!(" {detail}")),
                "{file}: the backend would redact {code} {detail:?} → {line:?}"
            );
        }
        for reason in [
            "pointer",
            "pointer-cancel",
            "blur",
            "hidden",
            "input",
            "escape",
            "focus",
            "live",
            "exit",
            "leave",
            "dispose",
            "other",
        ] {
            assert!(
                format_ui_event(
                    "terminal-selection",
                    Some(&format!("cancel-{reason}")),
                    None,
                    None
                )
                .is_some_and(|l| !l.contains("<redacted>")),
                "revoke reason cancel-{reason} must stay loggable"
            );
        }
    }

    #[test]
    fn debug_logging_is_the_launch_flag_and_nothing_else() {
        assert_eq!(
            debug_logging_enabled(),
            storage::command_flag("--debug-logging")
        );
    }

    #[test]
    fn an_export_is_sanitized_again_on_its_way_out() {
        // an export is meant to be sent to someone else, so it may not
        // inherit anything a PRE-0.4.29 app.log still holds
        let stale = "1787814001 [tmux] using /Users/example/private/deck.app/tmux\n\
                     1787814002 [pty] attached deck-quarterly-report-ab12 (80x24)\n\
                     1787814003 [queue] ghp_AbCdEf0123456789xyz\n\
                     1787814004 [poll] session listing recovered\n";
        let out = build_export("deck 0.4.29\ntmux: sidecar\nsessions: 3\n", stale);
        for m in [
            "/Users/example/private",
            "deck-quarterly-report-ab12",
            "ghp_AbCdEf0123456789xyz",
        ] {
            assert!(!out.contains(m), "export leaked {m}:\n{out}");
        }
        assert!(out.contains("deck 0.4.29") && out.contains("tmux: sidecar"));
        assert!(out.contains("===== app.log ====="));
        assert!(out.contains("[poll] session listing recovered"), "{out}");
    }

    /// The backend log-side error classifier: raw io/tmux/storage errors map
    /// to stable codes and their original text (paths included) never
    /// survives into the returned category.
    #[test]
    fn err_codes_are_stable_and_path_free() {
        use crate::storage::err_code;
        let real_io = std::fs::read_to_string("/no/such/deck-test-file")
            .unwrap_err()
            .to_string();
        assert_eq!(err_code(&real_io), "missing");
        let cases = [
            ("Permission denied (os error 13)", "perm"),
            ("could not create temp file (permission denied)", "perm"),
            ("Not a directory (os error 20)", "not-dir"),
            ("not a directory: /Users/example/private", "not-dir"),
            ("No space left on device (os error 28)", "disk-full"),
            (
                "deck.json was written by a newer deck (schema v9)",
                "newer-schema",
            ),
            (
                "refusing to save invalid JSON: expected value",
                "invalid-doc",
            ),
            ("wrong structure: missing field `projects`", "invalid-doc"),
            ("tmux send-keys failed: can't find session: x", "no-session"),
            (
                "tmux not runnable: No such file or directory",
                "tmux-missing",
            ),
            ("tmux new-session failed: server exited", "tmux"),
            ("another deck instance is already running", "locked"),
            ("something entirely different", "other"),
        ];
        for (input, want) in cases {
            let got = crate::storage::err_code(input);
            assert_eq!(got, want, "{input}");
            // categories are single tokens, never echoing the input
            assert!(!got.contains('/') && got.len() <= 16);
        }
        // zero-hit guarantee: distinctive markers never survive classification
        for marker in [
            "ghp_AbCdEf0123456789",
            "sk_live_4242",
            "/Users/example/private",
            "file:///secret",
        ] {
            let code = err_code(&format!("open failed for {marker}"));
            assert!(!code.contains(marker));
        }
    }
}
