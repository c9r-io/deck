//! Structured, content-free diagnostics: the closed `ui_event` whitelist
//! (code + per-code detail policy + two ints + optional numeric terminal IDs),
//! log size/reset, and sanitized exports. Nothing free-form from the webview ever reaches `app.log`.
//!
//! # Contract
//! `~/.deck/app.log` (0600) collects backend + frontend diagnostics.
//! Maintainer-only verbose frontend events are enabled at launch with
//! `app/run.sh --debug-logging`; there is no user setting for them. Frontend
//! logging is STRUCTURED ONLY: the `ui_event` command takes a whitelisted
//! code + a detail vetted by that code's OWN closed policy (enum values /
//! version pattern — no generic slug rule) + two ints, and redacts everything
//! else. Terminal events optionally add run/pane/selection/attempt integer IDs
//! (no text or session name) — never add a free-form log channel (log_privacy tests
//! enforce this). Backend log lines never interpolate raw error Display text
//! or a raw session NAME: `crate::error::err_code()` maps errors to stable
//! path-free categories (the full error goes only to the operation's caller)
//! and `crate::applog::session_tag()` gives a per-RUN, non-reversible tag.
//! Every line is redacted again by `redact::sanitize_log` on its way to disk;
//! exports sanitize their own header AND body instead of trusting app.log;
//! `applog::sanitize_existing_logs` migrates logs/exports an older deck wrote,
//! in place, at boot. The runtime privacy tests write REAL files through
//! `applog_to` into temp dirs — never stub the writer and call it proven.
//! (How to run the app, and the private-by-construction data directory, are
//! documented once: CLAUDE.md "Run and gates" and `datadir.rs`.)
//!
//! Clipboard diagnostics are always structured and content-free. Copy records
//! Deck/native/no-selection routing, snapshot loss and
//! a `pbcopy`/Web Clipboard writer FAILURE (a successful write is silent:
//! the v0.5.7 success lines never revealed the empty-pasteboard bug below,
//! `terminal-copy success` already marks a copy that completed). `pbcopy` is spawned with
//! `LANG=en_US.UTF-8` (`pbcopy_command`): a GUI-launched deck has no locale,
//! and under the C locale pbcopy writes an EMPTY pasteboard item for any
//! non-ASCII input while exiting 0 — every copy from an agent pane (Chinese,
//! box-drawing, `⏺`) "succeeded" and pasted nothing. The v0.5.7 paste-chain
//! trace (key capture → handler → paste event → `onData` → PTY write, with
//! missing-stage timers) was retired once its two findings landed (Enter sent
//! separately after a bracketed paste; a fresh agent settles before the first
//! paste): a PTY write that fails is still `pty-write-fail`. Only fixed labels
//! and character counts enter `app.log`; clipboard text, errors and session
//! names never do. The redundant key-capture line is retired. Each copy emits
//! a route and one outcome (or just no-selection); drag finish failures no
//! longer masquerade as copy failures. IDs are captured before async work,
//! so cancellations and overlapping copies cannot misattribute the outcome.
//! `pointer-context`: a flags = window focused(1), terminal focused(2), inside
//! highlight(4), frozen(8); b = ms since window focus (-1 if unknown).
//! `empty-range`: a/b are frontend
//! row/column deltas; backend `[selection] empty` supplies content deltas and
//! whether distinct content endpoints collapsed to the same tmux placement.
//! `copy-empty-gesture`: a = no release(0), click(1), promoted drag(2), pending
//! press(3); b = ms since that pane's last release (press for 3). Emitted only on an empty copy, not per click.
//! `source-elsewhere` carries the source pane's IDs and a/b = destination pane
//! and copy attempt; it diagnoses focus mistakes without copying another pane.
//! ⌘C can only report what it FOUND, so `terminal-selection` records the
//! selection's own life: `promote` / `start-ok` / `finish-ok` (or
//! `start-failed` / `update-failed` / `finish-failed` / `freeze-failed`,
//! which previously cancelled behind nothing but a toast), plus one
//! `cancel-<reason>` naming every revoke — pointer, pointer-cancel, blur,
//! hidden (both only for a drag still in progress), input, escape, focus,
//! live, exit, leave, dispose, and empty (a drag whose endpoints walked to
//! the same tmux position ended as the click it was). That is what
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
//! is false; its ints are click count and ms since this pane's last pointerup — the
//! one label whose `b` is not selection age); `native-cleared` marks an xterm
//! selection appearing while Deck owned the drag (WKWebView's late
//! compatibility-mouse replay); and `terminal-copy keydown-elsewhere` replaces
//! `keydown-none` when another pane still holds a live Deck/native selection (count +
//! its age), separating "revoked" from "⌘C reached the wrong pane".
//! A native xterm word/line selection has its own lifecycle: `native-select`
//! (a = rows, b = click count of the press that made it, 0 when none) and one
//! `native-end-<reason>` (a = rows, b = age ms) — pointer, input, output,
//! buffer, deck, dispose, other — so a `keydown-none` that followed a native
//! selection says what took it away.

use std::path::PathBuf;
use std::process::Command;

use crate::applog::applog;
use crate::datadir::now_epoch;
use crate::error::DeckError;
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
    "blocked",
    "create-fail",
    "queue-fail",
    "channel-queue-fail",
    "ack-fail",
    "busy",
    "run-closed",
    "run-close-fail",
    "rule-orphaned",
];

const LISTEN_TARGETS: &[&str] = &[
    "notify-open",
    "voice-window-hidden",
    "update-check",
    "update-check-manual",
    "update-download-progress",
    "menu-clear",
    "queue-changed",
    "queue-fired",
    "pty-data",
    "pty-exit",
    "inbound-changed",
    "channel-changed",
    "connector-changed",
    "mcp-changed",
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
    "selection-events-ready",
    "resume-capture-0",
    "resume-capture-1",
    "resume-priority",
    "resume-chip",
    "resume-ghost",
    "resume-no-execution",
    "resume-restored",
    "resume-pane-isolation",
    "resume-agent-hidden",
    "resume-cleared",
    "resume-done",
    "resume-exception",
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
    "selection-multiclick-drag",
    "selection-repeat",
    "selection-blur",
    "selection-empty-click",
    "selection-forensic-compat",
    "selection-forensic-up",
    "selection-forensic-revoke",
    "selection-forensic-empty",
    "selection-copy-unavailable",
    "selection-scroll-stable",
    "selection-scroll-cursor",
    "selection-overlay",
    "selection-drag-overlay",
    "selection-native-scroll",
    "selection-resize",
    "selection-up-range",
    "selection-clipboard-expect",
    "selection-clipboard-range",
    "selection-clipboard-copy",
    "selection-down-scroll",
    "scroll-frame",
    "link-activate",
    "link-classify",
    "link-repaint",
    "link-scan-bounded",
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
    "entry-automations",
    "entry-chip",
    "entry-empty",
    "entry-head",
    "entry-menu",
    "entry-menu-dismiss",
    "entry-project-menu",
    "entry-templates",
    "entry-vocabulary",
    "defaults-menu",
    "defaults-create",
    "defaults-shell-only",
    "defaults-not-dir",
    "defaults-here",
    "defaults-editor",
    "defaults-dialog",
    "defaults-empty",
    "defaults-persistence",
    "defaults-exit",
    "review-default",
    "review-atomic-list",
    "review-no-bypass",
    "review-save-failure",
    "review-idempotent",
    "review-edit-revokes",
    "review-signals",
    "review-dialog",
    "review-cancel-dialog",
    "review-layout",
    "review-second",
    "review-last",
    "review-final-hold",
    "review-restart",
    "review-stage",
    "attention-attach-fail",
    "attention-exit-before-reply",
    "attention-poll-followup",
    "attention-reopen-detached",
    "attention-reorder-focus",
    "attention-board",
    "attention-attach-pending",
    "attention-exit-generation",
    "attention-fixture",
    "attention-followed-save",
    "attention-followed-toggle",
    "attention-followed-viewed",
    "attention-followed-unknown",
    "attention-input-seen",
    "attention-keyed-focus",
    "attention-layout",
    "attention-locate-only",
    "attention-origin",
    "attention-partial",
    "attention-persistence",
    "attention-pointer",
    "attention-return",
    "attention-seen",
    "attention-split-fail",
    "split-picker-button",
    "split-picker-create",
    "split-picker-layout",
    "split-picker-pty",
    "split-picker-shortcut",
    "split-picker-existing",
    "attention-stage",
    "attention-stale",
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
    "command-without-pane",
    "completion-owner",
    "ambiguous-boot",
    "scheduler-context",
    "multiline-prompt",
    "buffer-board-stopped",
    "buffer-cas",
    "buffer-queue-copy",
    "buffer-natural-exit-retained",
    "buffer-visual-fixture",
    "channel-ui",
    "channel-route",
    "channel-network",
    "channel-scope",
    "channel-fault-recovery",
    "connector-route",
    "connector-no-agent",
    "connector-settings",
    "connector-apply",
    "connector-transport-ready",
    "voice-workspace",
    "voice-button-idle",
    "voice-type-visible",
    "voice-type-separator",
    "voice-type-no-enter",
    "voice-foreground-program",
    "voice-target-ended",
    "voice-leave",
    "voice-theme-light",
    "voice-theme-high-contrast",
    "voice-theme-deck-dark",
    "voice-preferences-defaults",
    "voice-preferences-layout",
    "voice-preferences-saved",
    "voice-preferences-single",
    "voice-preferences-last",
    "voice-preferences-restore",
    "voice-exception-0",
    "voice-exception-1",
    "voice-exception-2",
    "voice-exception-3",
    "voice-exception-4",
    "dropdown",
    "automation",
    "rename-restart",
    "done",
];

/// Terminal selection lifecycle. `terminal-copy` can only report what ⌘C
/// FOUND; these say how the selection got there or what took it away, so a
/// `keydown-none` copy can be attributed to a drag that never promoted, a
/// start tmux refused, or a specific later revoke. Labels only — no terminal
/// text, session name or error string is representable here.
///
/// `native-select` / `native-end-*` follow an xterm word/line selection Deck
/// does not own; both spend `a` on rows, `b` on click count and age.
///
/// `revoker-*` pairs with `cancel-pointer` and classifies the pointerdown
/// that destroyed a live selection: `synthetic` is an untrusted event, the
/// rest are the trusted pointerType. `native-cleared` records an xterm
/// selection appearing (and being cleared) while Deck owned the drag — the
/// signature of WKWebView's late compatibility mouse replay.
///
/// Two labels spend BOTH integers on their own numbers instead of the
/// selection's age. `span-mismatch` (a = rows the pointer crossed, b = rows
/// tmux selected) is the frontend half of the drift the backend's
/// `[selection]` lines count; `update-rtt` (a = ms for one backend update,
/// b = pointer moves folded into it) is the frontend half of drag lag and is
/// verbose enough to stay behind --debug-logging.
const SELECTION_EVENTS: &[&str] = &[
    "pointer-context",
    "empty-range",
    "copy-empty-gesture",
    "copy-no-selection-no-gesture",
    "copy-no-selection-gesture-active",
    "copy-no-selection-gesture-cancelled",
    "copy-no-selection-same-cell",
    "copy-no-selection-native-gesture-no-range",
    "copy-no-selection-native-range-ended",
    "copy-no-selection-promoted-empty",
    "copy-no-selection-promoted-start-failed",
    "copy-no-selection-promoted-finish-failed",
    "copy-no-selection-selection-revoked-pointer",
    "copy-no-selection-selection-revoked-input",
    "copy-no-selection-selection-revoked-focus",
    "copy-no-selection-selection-revoked-lifecycle",
    "copy-no-selection-selection-revoked-other",
    "copy-gesture-flags",
    "copy-gesture-pointer",
    "copy-gesture-compat",
    "copy-gesture-up",
    "copy-gesture-post-up-mousemove",
    "copy-promotion-pointer",
    "copy-promotion-compat",
    "copy-promotion-up",
    "copy-selection-promoted-pending",
    "copy-selection-finished-and-live",
    "copy-selection-promoted-empty",
    "copy-selection-promoted-start-failed",
    "copy-selection-promoted-finish-failed",
    "copy-selection-revoked-pointer",
    "copy-selection-revoked-input",
    "copy-selection-revoked-focus",
    "copy-selection-revoked-live",
    "copy-selection-revoked-exit",
    "copy-selection-revoked-dispose",
    "copy-selection-revoked-leave",
    "copy-selection-revoked-blur",
    "copy-selection-revoked-hidden",
    "copy-selection-revoked-escape",
    "copy-selection-revoked-pointer-cancel",
    "copy-selection-revoked-other",
    "copy-native-live",
    "copy-native-adopted",
    "copy-native-native-end-pointer",
    "copy-native-native-end-input",
    "copy-native-native-end-output",
    "copy-native-native-end-buffer",
    "copy-native-native-end-deck",
    "copy-native-native-end-dispose",
    "copy-native-native-end-other",
    "event-down",
    "event-mousedown",
    "event-pointer",
    "event-compat",
    "event-up",
    "event-promote-pointer",
    "event-promote-compat",
    "event-promote-up",
    "event-end",
    "event-post-up-mousemove",
    "promote",
    "span-mismatch",
    "update-rtt",
    "start-ok",
    "start-failed",
    "finish-ok",
    "finish-failed",
    "update-failed",
    "dimensions-changed",
    "freeze-ok",
    "freeze-failed",
    "native-cleared",
    "native-select",
    "native-end-pointer",
    "native-end-input",
    "native-end-output",
    "native-end-buffer",
    "native-end-deck",
    "native-end-dispose",
    "native-end-other",
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
    "cancel-empty",
    "cancel-other",
];

/// Link attempts share the terminal numeric context (selection = 0).
/// press/menu/blocked: a = candidate UTF-16 length, b = click count for press,
/// otherwise elapsed ms. miss = no candidate on a click. scan-slow: a = ms,
/// b = scanned UTF-16 length, limited to one per pane per five seconds.
/// action-*: a = copy(1), URL(2), editor(3), editor-parent(4), session-parent(5),
/// reveal(6); b = elapsed ms. No candidate, path, URL or error text is accepted.
const LINK_EVENTS: &[&str] = &[
    "press-path",
    "press-url",
    "menu-path",
    "menu-url",
    "miss",
    "drag",
    "viewport",
    "outside",
    "changed",
    "cancelled",
    "scan-slow",
    "action-start",
    "action-retry",
    "action-ok",
    "action-failed",
    "action-stale",
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
    (
        "update-install-fail",
        DetailPolicy::Closed(&["not-writable"]),
    ),
    // away notifications (notify.rs): the authorization word after a
    // settings change, and a click that opened a card
    (
        "notify-status",
        DetailPolicy::Closed(&[
            "unsupported",
            "not-determined",
            "denied",
            "authorized",
            "provisional",
        ]),
    ),
    ("notify-open", DetailPolicy::None),
    ("board-load-fail", DetailPolicy::None),
    ("settings-load-fail", DetailPolicy::None),
    ("settings-save-fail", DetailPolicy::None),
    ("inbound", DetailPolicy::Closed(INBOUND_OUTCOMES)),
    ("poll-fail", DetailPolicy::None),
    ("poll-recovered", DetailPolicy::None),
    ("separator", DetailPolicy::Closed(&["no-marker", "fail"])),
    ("mirror-desync", DetailPolicy::Closed(&["esc", "plain"])),
    ("ondata", DetailPolicy::Closed(&["desync", "ok"])),
    ("pty-write-fail", DetailPolicy::None),
    ("pty-rx", DetailPolicy::None),
    ("keydown", DetailPolicy::Closed(KEY_CLASSES)),
    ("composition", DetailPolicy::Closed(&["start", "end"])),
    (
        "terminal-copy",
        DetailPolicy::Closed(&[
            "keydown-deck",
            "keydown-native",
            "keydown-none",
            "keydown-elsewhere",
            "source-elsewhere",
            "success",
            "selection-vanished",
            "selection-missing",
            "snapshot-failed",
            "clipboard-write-failed",
        ]),
    ),
    ("terminal-selection", DetailPolicy::Closed(SELECTION_EVENTS)),
    ("terminal-link", DetailPolicy::Closed(LINK_EVENTS)),
    (
        "clipboard-write",
        DetailPolicy::Closed(&["pbcopy-failed", "web-failed", "web-unavailable"]),
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

/// Terminal-only correlation; numeric, ephemeral frontend IDs, never a
/// session name or content hash. Unknown fields and non-integers are refused.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TerminalEventContext {
    run: u64,
    pane: u32,
    selection: u32,
    #[serde(default)]
    gesture: u32,
    #[serde(default)]
    attempt: u32,
}

fn format_scoped_ui_event(
    code: &str,
    detail: Option<&str>,
    a: Option<i64>,
    b: Option<i64>,
    context: Option<&TerminalEventContext>,
) -> Option<String> {
    let mut line = format_ui_event(code, detail, a, b)?;
    if matches!(
        code,
        "terminal-copy" | "terminal-selection" | "terminal-link" | "clipboard-write"
    ) {
        if let Some(c) = context {
            line.push_str(&format!(
                " run={} pane={} selection={} gesture={} attempt={}",
                c.run, c.pane, c.selection, c.gesture, c.attempt
            ));
        }
    }
    Some(line)
}

#[tauri::command]
pub(crate) fn ui_event(
    code: String,
    detail: Option<String>,
    a: Option<i64>,
    b: Option<i64>,
    context: Option<TerminalEventContext>,
) {
    match format_scoped_ui_event(&code, detail.as_deref(), a, b, context.as_ref()) {
        Some(line) => applog(&line),
        None => applog("[ui] unknown-event"),
    }
}

#[tauri::command]
pub(crate) fn debug_logging_enabled() -> bool {
    crate::launch_args::command_flag("--debug-logging")
}

/// Build the export text. EVERY line — the environment header and the log
/// body alike — goes through the log sanitizer on the way out: an export is
/// meant to be mailed to someone, so it must not be able to inherit anything
/// an older deck (or a future call site) left in app.log.
pub(crate) fn build_export(header: &str, log: &str) -> String {
    let mut out = String::with_capacity(header.len() + log.len() + 32);
    for line in header.lines() {
        out.push_str(&crate::redact::sanitize_log(line));
        out.push('\n');
    }
    out.push_str("\n===== app.log =====\n");
    for line in log.lines() {
        out.push_str(&crate::redact::sanitize_log(line));
        out.push('\n');
    }
    out
}

#[tauri::command]
pub(crate) fn log_size() -> Result<u64, DeckError> {
    crate::applog::log_size_at(&crate::datadir::deck_dir())
}

#[tauri::command]
pub(crate) fn reset_logs() -> Result<(), DeckError> {
    crate::applog::reset_logs_at(&crate::datadir::deck_dir())
}

#[tauri::command]
pub(crate) fn export_logs() -> Result<PathBuf, DeckError> {
    let data_dir = crate::datadir::deck_dir();
    let dir = data_dir.join("exports");
    crate::datadir::create_private_dir(&dir)?;
    let name = format!("deck-log-{}.txt", now_epoch());
    let path = dir.join(name);

    let mut header = String::new();
    header.push_str(&format!("deck {}\n", env!("CARGO_PKG_VERSION")));
    if let Ok(o) = Command::new("/usr/bin/sw_vers").output() {
        header.push_str(&String::from_utf8_lossy(&o.stdout));
    }
    if let Ok(o) = Command::new("/usr/bin/uname").arg("-m").output() {
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
    crate::datadir::write_private(&path, build_export(&header, &log).as_bytes())?;
    let _ = Command::new("/usr/bin/open").arg("-R").arg(&path).status();
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ui/test/fixtures/smoke-manifest.json` lists what every smoke mode must
    /// emit (judged by `scripts/smoke-verdict`); its names are exactly the
    /// closed smoke-check vocabulary. A manifest-only name would log as
    /// `<redacted>`; a SMOKE_CHECKS-only name is dead.
    #[test]
    fn smoke_checks_are_exactly_the_smoke_manifest() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../ui/test/fixtures/smoke-manifest.json"))
                .unwrap();
        let mut listed = std::collections::BTreeSet::new();
        for mode in manifest["modes"].as_object().unwrap().values() {
            if let Some(terminal) = mode["terminal"].as_str() {
                listed.insert(terminal.to_owned());
            }
            for name in mode["checks"].as_object().unwrap().keys() {
                listed.insert(name.clone());
            }
        }
        let closed: std::collections::BTreeSet<String> =
            SMOKE_CHECKS.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(
            closed.len(),
            SMOKE_CHECKS.len(),
            "a smoke check listed twice"
        );
        let dead: Vec<_> = closed.difference(&listed).collect();
        let unknown: Vec<_> = listed.difference(&closed).collect();
        assert!(
        dead.is_empty() && unknown.is_empty(),
        "SMOKE_CHECKS without a smoke mode: {dead:?}; manifest names SMOKE_CHECKS would redact: {unknown:?}"
    );
    }

    /// The launcher side of the smoke manifest: main.rs dispatches exactly
    /// the manifest's modes, each to an entry wk-smoke.mjs exports, and any
    /// other value runs `run`.
    #[test]
    fn smoke_launcher_dispatches_exactly_the_manifest_modes() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../ui/test/fixtures/smoke-manifest.json"))
                .unwrap();
        let modes: std::collections::BTreeSet<&str> = manifest["modes"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let launched: std::collections::BTreeSet<&str> =
            crate::SMOKE_ENTRIES.iter().map(|(mode, _)| *mode).collect();
        assert_eq!(
            launched.len(),
            crate::SMOKE_ENTRIES.len(),
            "a mode listed twice"
        );
        assert_eq!(launched, modes);
        let carrier = include_str!("../../ui/test/wk-smoke.mjs");
        for (mode, entry) in crate::SMOKE_ENTRIES {
            let function = entry
                .strip_prefix("m.")
                .and_then(|call| call.split('(').next())
                .unwrap_or_else(|| panic!("{mode}: entry is not m.<fn>(…)"));
            assert!(
                carrier.contains(&format!("export async function {function}(")),
                "{mode}: wk-smoke.mjs exports no {function}"
            );
        }
        assert_eq!(crate::smoke_entry("1"), "m.run()");
        assert_eq!(crate::smoke_entry("review-restart"), "m.verifyReview(true)");
    }

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
            format_ui_event("clipboard-write", Some("pbcopy-failed"), Some(42), None).unwrap(),
            "[ui] clipboard-write pbcopy-failed a=42"
        );
        // retired probes stay retired: no code, no line
        assert!(format_ui_event("terminal-paste", Some("ondata"), None, None).is_none());
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
                "terminal-selection",
                "terminal-link",
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
            let mut markers = vec!["uev('", "duev('"];
            if file == "terminal-clipboard.js" {
                markers.push("log('");
            }
            for marker in markers {
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
                // `sevPair(` / `dsevPair(` — the two-integer probes.
                ("selection.js", "sevPair('", "terminal-selection"),
                ("selection.js", "dsevPair('", "terminal-selection"),
                ("terminal-links.js", "log('", "terminal-link"),
                ("terminal-links.js", "outcome('", "terminal-link"),
                ("terminal-links.js", "cancelPress('", "terminal-link"),
            ];
            for (owner, marker, code) in indirect {
                if file == owner {
                    for (label, _) in literal_after(text, marker) {
                        pairs.push((file.clone(), code.to_string(), label.into()));
                    }
                }
            }
            for prefix in ["'native-end-", "'native-select'", "'native-cleared'"] {
                for (at, _) in text.match_indices(prefix) {
                    let word: String = text[at + 1..]
                        .chars()
                        .take_while(|c| c.is_ascii_lowercase() || *c == '-')
                        .collect();
                    pairs.push((file.clone(), "terminal-selection".into(), word));
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
            // Copy outcomes are returned by the clipboard adapter.
            if file == "terminal-clipboard.js" {
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
            "empty",
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
    fn link_events_are_closed_and_keep_numeric_correlation() {
        let c = TerminalEventContext {
            run: 123,
            pane: 2,
            selection: 0,
            gesture: 7,
            attempt: 4,
        };
        for detail in LINK_EVENTS {
            let line =
                format_scoped_ui_event("terminal-link", Some(detail), Some(18), Some(5), Some(&c))
                    .unwrap();
            assert_eq!(
                line,
                format!(
                    "[ui] terminal-link {detail} a=18 b=5 run=123 pane=2 selection=0 gesture=7 attempt=4"
                )
            );
            assert_eq!(crate::redact::sanitize_log(&line), line);
        }
    }

    #[test]
    fn terminal_context_is_numeric_scoped_and_preserves_redaction() {
        let c: TerminalEventContext = serde_json::from_value(serde_json::json!({
            "run": 123456, "pane": 2, "selection": 19, "gesture": 42, "attempt": 3
        }))
        .unwrap();
        let line =
            format_scoped_ui_event("terminal-copy", Some("keydown-none"), None, None, Some(&c))
                .unwrap();
        assert_eq!(
            line,
            "[ui] terminal-copy keydown-none run=123456 pane=2 selection=19 gesture=42 attempt=3"
        );
        // The disk/export sanitizer must preserve correlation IDs, unlike
        // credential-shaped keys such as `token`, which it intentionally hides.
        assert_eq!(crate::redact::sanitize_log(&line), line);
        let empty = "[selection] sess-abcde empty selection=19 rows=0 cols=1 collapsed=1";
        assert_eq!(crate::redact::sanitize_log(empty), empty);
        assert!(
            format_scoped_ui_event("terminal-copy", Some("secret-text"), None, None, Some(&c))
                .unwrap()
                .contains("<redacted>")
        );
        assert_eq!(
            format_scoped_ui_event("ping-recv", None, None, None, Some(&c)).unwrap(),
            "[ui] ping-recv"
        );
        for bad in [
            serde_json::json!({"run": 1, "pane": "private-session", "selection": 1}),
            serde_json::json!({"run": 1, "pane": 2, "selection": 1, "text": "secret"}),
            serde_json::json!({"run": 1, "pane": -1, "selection": 1}),
            serde_json::json!({"run": 1, "pane": 2, "selection": 1.5}),
        ] {
            assert!(serde_json::from_value::<TerminalEventContext>(bad).is_err());
        }
        assert!(
            format_ui_event("terminal-copy", Some("key-capture"), None, None)
                .unwrap()
                .contains("<redacted>")
        );
    }

    #[test]
    fn debug_logging_is_the_launch_flag_and_nothing_else() {
        assert_eq!(
            debug_logging_enabled(),
            crate::launch_args::command_flag("--debug-logging")
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
        use crate::error::err_code;
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
            let got = crate::error::err_code(input);
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
