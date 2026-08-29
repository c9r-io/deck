//! The non-scheduler Tauri command surface: board/settings persistence,
//! session lifecycle, polling, link opening, diagnostics.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::{
    expand_tilde, init_deck_server, pane_target, session_target, tmux, tmux_bin, tmux_owned,
    validate_session_name,
};

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
const LISTEN_TARGETS: &[&str] = &[
    "deck-ping",
    "update-check",
    "update-check-manual",
    "menu-clear",
    "queue-changed",
    "queue-fired",
    "pty-data",
    "pty-exit",
];

/// Keydown CATEGORIES — the frontend classifies before sending; a raw key
/// name (let alone typed text) never crosses the bridge.
const KEY_CLASSES: &[&str] = &[
    "char",
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
    "selection-overlay",
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
    "natural-fault",
    "completion-owner",
    "ambiguous-boot",
    "scheduler-context",
    "rename-restart",
    "done",
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
            "success",
            "selection-missing",
            "snapshot-failed",
            "clipboard-write-failed",
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

// ---------- dropped files -------------------------------------------------------

/// Max bytes accepted from one dropped/pasted file — screenshots are
/// hundreds of KB; the cap only guards against absurd payloads.
const MAX_DROP_BYTES: usize = 32 * 1024 * 1024;

/// Keep the original extension and a recognizable stem, but only characters
/// that are safe inside a quoted shell path; leading dots are stripped so a
/// drop can never create a hidden file.
pub(crate) fn sanitize_drop_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == '-' || c == '.');
    let mut out = if trimmed.is_empty() {
        "file".to_string()
    } else {
        trimmed.to_string()
    };
    if out.len() > 72 {
        let ext = out
            .rsplit_once('.')
            .map(|(_, e)| e)
            .filter(|e| !e.is_empty() && e.len() <= 8)
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        out.truncate(64);
        let stem = out.trim_end_matches('.').to_string();
        out = format!("{stem}{ext}");
    }
    out
}

/// Pure core of save_dropped_file (unit-tested against a temp dir).
pub(crate) fn save_drop_into(
    dir: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    if bytes.is_empty() {
        return Err("empty file".into());
    }
    if bytes.len() > MAX_DROP_BYTES {
        return Err("file too large (32MB max)".into());
    }
    storage::create_private_dir(dir)?;
    let safe = sanitize_drop_name(name);
    let mut path = dir.join(format!("{}-{safe}", now_epoch()));
    let mut n = 0u32;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{}-{n}-{safe}", now_epoch()));
    }
    storage::write_private(&path, bytes)?;
    Ok(path)
}

/// Persist a file dragged/pasted into a terminal pane so its PATH can be
/// typed into the session (the Warp-style "drop a screenshot at the agent"
/// flow). WKWebView surfaces dropped files as content, never as a usable
/// path, hence the round-trip. Files land 0600 in ~/.deck/drops (0700,
/// pruned of week-old entries at boot); neither name nor content is logged.
#[tauri::command]
pub(crate) fn save_dropped_file(name: String, data_b64: String) -> Result<String, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64)
        .map_err(|_| "malformed file payload".to_string())?;
    let path = save_drop_into(&storage::deck_dir().join("drops"), &name, &bytes)?;
    applog(&format!("[drop] saved {}B", bytes.len()));
    Ok(path.display().to_string())
}

/// Rust→JS event self-test: the frontend calls this after registering a
/// listener; if the pong never arrives, the event bus is the broken link.
#[tauri::command]
pub(crate) fn ping_event(app: AppHandle) {
    let r = app.emit("deck-ping", "pong");
    applog(&format!("[evt] ping emitted ok={:?}", r.is_ok()));
}

// ---------- board persistence ------------------------------------------------

/// Business-structure validation for deck.json — ONE rule set shared by
/// load and save: `BoardDoc` deserializes via `try_from`, so
/// `storage::load_typed::<BoardDoc>` (quarantine/backup recovery on
/// failure) and `save_board` (reject before touching disk) both run the
/// full referential checks below. Unknown extension fields are tolerated
/// (serde ignores them; save persists the original string, so they
/// round-trip untouched).
#[derive(serde::Deserialize)]
pub(crate) struct BoardDocRaw {
    projects: Vec<BoardProject>,
    cards: Vec<BoardCard>,
}

#[derive(serde::Deserialize)]
#[serde(try_from = "BoardDocRaw")]
pub(crate) struct BoardDoc(#[allow(dead_code)] BoardDocRaw);

impl TryFrom<BoardDocRaw> for BoardDoc {
    type Error = String;
    fn try_from(raw: BoardDocRaw) -> Result<Self, String> {
        validate_board(&raw)?;
        Ok(BoardDoc(raw))
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct BoardProject {
    id: String,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    columns: Vec<BoardColumn>,
}
#[derive(serde::Deserialize)]
pub(crate) struct BoardColumn {
    id: String,
    #[allow(dead_code)]
    name: String,
}
#[derive(serde::Deserialize)]
pub(crate) struct BoardCard {
    id: String,
    #[serde(rename = "projectId")]
    project_id: String,
    #[serde(rename = "columnId")]
    column_id: String,
    #[allow(dead_code)]
    title: String,
    /// runtime fields the UI cannot operate a card without
    #[allow(dead_code)]
    cmd: String,
    #[allow(dead_code)]
    dir: String,
    session: String,
}

/// The referential rules a usable board must satisfy. Errors carry ids
/// (deck-generated), never titles/commands/paths — they end up in recovery
/// warnings.
fn validate_board(b: &BoardDocRaw) -> Result<(), String> {
    let mut project_ids = HashSet::new();
    for p in &b.projects {
        if p.id.trim().is_empty() {
            return Err("a project has an empty id".into());
        }
        if !project_ids.insert(p.id.as_str()) {
            return Err(format!("duplicate project id {}", p.id));
        }
        if p.columns.is_empty() {
            return Err(format!("project {} has no columns", p.id));
        }
        let mut col_ids = HashSet::new();
        for c in &p.columns {
            if c.id.trim().is_empty() {
                return Err(format!("project {} has a column with an empty id", p.id));
            }
            if !col_ids.insert(c.id.as_str()) {
                return Err(format!("duplicate column id {} in project {}", c.id, p.id));
            }
        }
    }
    let mut card_ids = HashSet::new();
    let mut sessions = HashSet::new();
    for c in &b.cards {
        if c.id.trim().is_empty() {
            return Err("a card has an empty id".into());
        }
        if !card_ids.insert(c.id.as_str()) {
            return Err(format!("duplicate card id {}", c.id));
        }
        // the SAME session-name rule the runtime enforces on start/attach
        crate::tmux::validate_session_name(&c.session)
            .map_err(|e| format!("card {}: {e}", c.id))?;
        if !sessions.insert(c.session.as_str()) {
            return Err(format!("card {}: session name is already used", c.id));
        }
        let Some(project) = b.projects.iter().find(|p| p.id == c.project_id) else {
            return Err(format!("card {} references a missing project", c.id));
        };
        if !project.columns.iter().any(|col| col.id == c.column_id) {
            return Err(format!(
                "card {} references a column that is not in its project",
                c.id
            ));
        }
    }
    Ok(())
}

/// Settings must be a JSON object; individual keys are optional but must
/// have the right type when present. Same try_from sharing as BoardDoc.
fn deserialize_present_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer).map(Some)
}

#[derive(serde::Deserialize)]
pub(crate) struct SettingsDocRaw {
    #[serde(default)]
    editor: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    debug: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    locale: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(try_from = "SettingsDocRaw")]
pub(crate) struct SettingsDoc(#[allow(dead_code)] SettingsDocRaw);

impl TryFrom<SettingsDocRaw> for SettingsDoc {
    type Error = String;
    fn try_from(raw: SettingsDocRaw) -> Result<Self, String> {
        if let Some(e) = &raw.editor {
            if e.len() > 200 {
                return Err("editor name is unreasonably long".into());
            }
        }
        if let Some(locale) = &raw.locale {
            if !matches!(locale.as_str(), "system" | "en" | "zh-Hans") {
                return Err("locale must be system, en, or zh-Hans".into());
            }
        }
        Ok(SettingsDoc(raw))
    }
}

/// What a typed load hands the frontend: the payload, where it came from
/// ("main" | "backup" | "none" for a first run), and — when recovery
/// happened — a warning the UI must show. A rejected promise here is a HARD
/// error (nothing loadable): the UI must surface it, never treat it as a
/// first run.
#[derive(Serialize)]
pub(crate) struct LoadedDoc {
    data: String,
    source: String,
    warning: Option<UiNotice>,
}

#[derive(Serialize)]
pub(crate) struct UiNotice {
    code: &'static str,
}

fn notice_from(note: &str) -> UiNotice {
    let code = if note.contains("privacy hardening") {
        "storage.privacy"
    } else if note.contains("scheduled prompts could not be saved") {
        "queue.persist"
    } else if note.contains("scheduled prompts could not be loaded") {
        "queue.load"
    } else if note.contains("command history could not be loaded") {
        "history.load"
    } else if note.contains("interrupted deliveries") || note.contains("delivery") {
        "queue.interrupted"
    } else {
        "storage.recovered"
    };
    UiNotice { code }
}

fn to_loaded(o: Option<storage::LoadOutcome>) -> LoadedDoc {
    match o {
        Some(o) => LoadedDoc {
            data: o.payload,
            source: o.source.into(),
            warning: o.warning.as_deref().map(notice_from),
        },
        None => LoadedDoc {
            data: String::new(),
            source: "none".into(),
            warning: None,
        },
    }
}

pub(crate) fn board_path() -> PathBuf {
    storage::deck_dir().join("deck.json")
}

#[tauri::command]
pub(crate) fn load_board() -> Result<LoadedDoc, String> {
    Ok(to_loaded(storage::load_typed::<BoardDoc>(&board_path())?))
}

/// The same full business validation as load, BEFORE anything touches disk:
/// an invalid document never overwrites the main file or rotates the .bak.
pub(crate) fn save_validated<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    data: &str,
    what: &str,
) -> Result<(), String> {
    serde_json::from_str::<T>(data).map_err(|e| format!("refusing to save invalid {what}: {e}"))?;
    storage::save_typed::<T>(path, data)
}

#[tauri::command]
pub(crate) fn save_board(data: String) -> Result<(), String> {
    if crate::smoke_faults::take("board-save") {
        return Err("injected board save failure".into());
    }
    save_validated::<BoardDoc>(&board_path(), &data, "board")
}

/// Boot-time storage notices (corruption recovered from .bak, etc.) for the
/// frontend to surface as toasts.
#[tauri::command]
pub(crate) fn storage_warnings() -> Vec<UiNotice> {
    std::mem::take(&mut *storage::WARNINGS.lock().unwrap())
        .iter()
        .map(|note| notice_from(note))
        .collect()
}

// ---------- settings ------------------------------------------------------------

pub(crate) fn settings_path() -> PathBuf {
    storage::deck_dir().join("settings.json")
}

#[tauri::command]
pub(crate) fn load_settings() -> Result<LoadedDoc, String> {
    Ok(to_loaded(storage::load_typed::<SettingsDoc>(
        &settings_path(),
    )?))
}

#[tauri::command]
pub(crate) fn save_settings(data: String) -> Result<(), String> {
    save_validated::<SettingsDoc>(&settings_path(), &data, "settings")
}

pub(crate) fn editor_app() -> Option<String> {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()??
        .payload;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let e = v.get("editor")?.as_str()?.trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

pub(crate) fn locale_setting() -> String {
    let raw = storage::load_typed::<SettingsDoc>(&settings_path())
        .ok()
        .flatten()
        .map(|o| o.payload);
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("locale")?.as_str().map(str::to_owned))
        .filter(|v| matches!(v.as_str(), "system" | "en" | "zh-Hans"))
        .unwrap_or_else(|| "system".into())
}

/// Developer editors present in /Applications or ~/Applications, offered in
/// the Settings editor picker. Names double as `open -a` targets.
#[tauri::command]
pub(crate) fn detect_editors() -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "Cursor",
        "Visual Studio Code",
        "Zed",
        "Sublime Text",
        "TextMate",
        "BBEdit",
        "Nova",
        "IntelliJ IDEA",
        "WebStorm",
        "RustRover",
        "Xcode",
    ];
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("Applications"));
    }
    CANDIDATES
        .iter()
        .filter(|c| roots.iter().any(|r| r.join(format!("{c}.app")).exists()))
        .map(|c| c.to_string())
        .collect()
}

#[tauri::command]
pub(crate) fn default_dir() -> String {
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".into())
}

#[tauri::command]
pub(crate) fn tmux_available() -> bool {
    Command::new(tmux_bin())
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Idempotent start: returns true only when it actually CREATED the session.
/// "Enter card" means "make sure it's running, then attach" — if the session
/// already lives (stale frontend status, click before the first poll, another
/// window), that is success-without-side-effects: never a duplicate-session
/// error, and never re-typing the boot cmd into a running shell.
#[tauri::command]
pub(crate) fn start_session(name: String, dir: String, cmd: String) -> Result<bool, String> {
    validate_session_name(&name)?;
    if tmux(&["has-session", "-t", &session_target(&name)]).is_ok() {
        return Ok(false);
    }
    let dir = expand_tilde(&dir);
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    tmux(&["new-session", "-d", "-s", &name, "-c", &dir])?;
    // belt & suspenders for servers started by older deck versions
    init_deck_server();
    if !cmd.trim().is_empty() {
        tmux(&["send-keys", "-t", &pane_target(&name), &cmd, "Enter"])?;
    }
    Ok(true)
}

/// Server-wide defaults for the deck tmux server. Idempotent; called at app
/// Wheel scrolling, deck-driven: xterm keeps LOCAL selection (mouse mode
/// stays off), and deck translates wheel deltas into tmux copy-mode
/// Returns whether the pane is in copy-mode AFTER the scroll, so the UI can
/// show/hide its scrollback indicator without waiting for the next poll.
#[tauri::command]
pub(crate) fn scroll_session(name: String, lines: i32) -> Result<bool, String> {
    validate_session_name(&name)?;
    let t = pane_target(&name);
    // State test, optional copy-mode entry, movement and post-state report all
    // execute in one tmux server command list. This removes two to three
    // process/IPC round trips from every display-frame scroll update.
    let after = tmux_owned(&crate::terminal_scroll::args(&t, lines))?;
    Ok(after.trim() == "1")
}

/// Leave copy-mode and return to the live view (typing, the scrollback
/// chip, or wheel-to-bottom all end here). A pane that is not in copy-mode
/// is a no-op — tmux's error for that case is deliberately swallowed.
#[tauri::command]
pub(crate) fn scroll_bottom(name: String) -> Result<(), String> {
    validate_session_name(&name)?;
    let _ = tmux(&["send-keys", "-t", &pane_target(&name), "-X", "cancel"]);
    Ok(())
}

/// Fresh shells accumulate junk history from the attach-time resize
/// reflow (blank lines pushed into scrollback), which made "empty" shells
/// scrollable. Called once for sessions deck itself just started.
#[tauri::command]
pub(crate) fn clear_history(name: String) {
    if validate_session_name(&name).is_err() {
        return;
    }
    let t = pane_target(&name);
    let _ = tmux(&["clear-history", "-t", &t]);
}

/// tmux copy-mode is the sole authority for cross-screen selection. The
/// attached PTY repaints tmux's own highlighted frame into xterm; no second
/// scrollback document or private xterm API is involved.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalSelectionStatus {
    active: bool,
    selection_present: bool,
    history_rows: u32,
    history_limit: u32,
    pane_rows: u32,
    pane_cols: u32,
    scroll_position: u32,
    cursor_row: u32,
    cursor_col: u32,
    absolute_row: u64,
    at_top: bool,
    at_bottom: bool,
    history_at_limit: bool,
    selection_start_row: u32,
    selection_start_col: u32,
    selection_end_row: u32,
    selection_end_col: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct TerminalSelectionGrid {
    cols: u32,
    rows: u32,
}

#[derive(Clone, Debug)]
enum TerminalSelectionLease {
    Cancelled {
        token: u64,
    },
    Dragging {
        token: u64,
    },
    Frozen {
        token: u64,
        text: String,
        bytes: u64,
        history_limit: u32,
        selection_start_row: u32,
        selection_start_col: u32,
        selection_end_row: u32,
        selection_end_col: u32,
    },
}

impl TerminalSelectionLease {
    fn token(&self) -> u64 {
        match self {
            Self::Cancelled { token } | Self::Dragging { token } | Self::Frozen { token, .. } => {
                *token
            }
        }
    }
}

fn terminal_selection_leases() -> &'static Mutex<HashMap<String, TerminalSelectionLease>> {
    static LEASES: OnceLock<Mutex<HashMap<String, TerminalSelectionLease>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn terminal_selection_operation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn selection_token_matches(name: &str, token: u64, frozen: bool) -> bool {
    let leases = terminal_selection_leases().lock().unwrap();
    matches!(
        leases.get(name),
        Some(TerminalSelectionLease::Dragging { token: current })
            if !frozen && *current == token
    ) || matches!(
        leases.get(name),
        Some(TerminalSelectionLease::Frozen { token: current, .. })
            if frozen && *current == token
    )
}

fn frozen_selection_status(
    name: &str,
    token: u64,
    mut status: TerminalSelectionStatus,
) -> Result<TerminalSelectionStatus, String> {
    let leases = terminal_selection_leases().lock().unwrap();
    let Some(TerminalSelectionLease::Frozen {
        token: current,
        selection_start_row,
        selection_start_col,
        selection_end_row,
        selection_end_col,
        ..
    }) = leases.get(name)
    else {
        return Err("selection-missing".into());
    };
    if *current != token {
        return Err("selection-missing".into());
    }
    status.selection_present = true;
    status.selection_start_row = *selection_start_row;
    status.selection_start_col = *selection_start_col;
    status.selection_end_row = *selection_end_row;
    status.selection_end_col = *selection_end_col;
    Ok(status)
}

fn parse_u32_or_zero(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn terminal_selection_status_for(target: &str) -> Result<TerminalSelectionStatus, String> {
    let raw = tmux(&[
        "display-message",
        "-p",
        "-t",
        target,
        "#{pane_in_mode}\t#{selection_present}\t#{history_size}\t#{history_limit}\t#{pane_height}\t#{pane_width}\t#{scroll_position}\t#{copy_cursor_y}\t#{copy_cursor_x}\t#{selection_start_y}\t#{selection_start_x}\t#{selection_end_y}\t#{selection_end_x}",
    ])?;
    let mut f = raw.trim_end().split('\t');
    let active = f.next() == Some("1");
    let selection_present = f.next() == Some("1");
    let history_rows = parse_u32_or_zero(f.next());
    let history_limit = parse_u32_or_zero(f.next());
    let pane_rows = parse_u32_or_zero(f.next());
    let pane_cols = parse_u32_or_zero(f.next());
    let scroll_position = parse_u32_or_zero(f.next());
    let cursor_row = parse_u32_or_zero(f.next());
    let cursor_col = parse_u32_or_zero(f.next());
    let selection_start_row = parse_u32_or_zero(f.next());
    let selection_start_col = parse_u32_or_zero(f.next());
    let selection_end_row = parse_u32_or_zero(f.next());
    let selection_end_col = parse_u32_or_zero(f.next());
    if pane_rows == 0 || pane_cols == 0 {
        return Err("tmux returned invalid terminal dimensions".into());
    }
    let visible_start = history_rows.saturating_sub(scroll_position) as u64;
    let absolute_row = visible_start.saturating_add(cursor_row as u64);
    let last_row = history_rows as u64 + pane_rows.saturating_sub(1) as u64;
    Ok(TerminalSelectionStatus {
        active,
        selection_present,
        history_rows,
        history_limit,
        pane_rows,
        pane_cols,
        scroll_position,
        cursor_row,
        cursor_col,
        absolute_row,
        at_top: absolute_row == 0,
        at_bottom: absolute_row >= last_row,
        history_at_limit: history_limit > 0 && history_rows >= history_limit,
        selection_start_row,
        selection_start_col,
        selection_end_row,
        selection_end_col,
    })
}

fn require_terminal_selection_dimensions(
    actual_cols: u32,
    actual_rows: u32,
    expected_cols: u32,
    expected_rows: u32,
) -> Result<(), String> {
    if actual_cols == expected_cols && actual_rows == expected_rows {
        Ok(())
    } else {
        Err("selection-dimensions-changed".into())
    }
}

fn push_tmux_command(batch: &mut Vec<String>, command: &[String]) {
    if !batch.is_empty() {
        batch.push(";".into());
    }
    batch.extend(command.iter().cloned());
}

fn push_copy_cursor(batch: &mut Vec<String>, target: &str, row: u32, horizontal_steps: u32) {
    for action in ["top-line", "start-of-line"] {
        push_tmux_command(
            batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.into(),
                "-X".into(),
                action.into(),
            ],
        );
    }
    if row > 0 {
        push_tmux_command(
            batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.into(),
                "-X".into(),
                "-N".into(),
                row.to_string(),
                "cursor-down".into(),
            ],
        );
    }
    if horizontal_steps > 0 {
        push_tmux_command(
            batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.into(),
                "-X".into(),
                "-N".into(),
                horizontal_steps.to_string(),
                "cursor-right".into(),
            ],
        );
    }
}

fn copy_cursor_steps(
    target: &str,
    scroll_position: u32,
    row: u32,
    col: u32,
) -> Result<u32, String> {
    let coord = row as i64 - scroll_position as i64;
    let captured = tmux(&[
        "capture-pane",
        "-p",
        "-J",
        "-S",
        &coord.to_string(),
        "-E",
        &coord.to_string(),
        "-t",
        target,
    ])?;
    let row_text = captured.strip_suffix('\n').unwrap_or(&captured);
    Ok(crate::terminal_selection::cursor_steps_for_cell(
        row_text, col,
    ))
}

#[tauri::command]
pub(crate) fn terminal_selection_start(
    name: String,
    token: u64,
    anchor_row: u32,
    anchor_col: u32,
    active_row: u32,
    active_col: u32,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    if terminal_selection_leases()
        .lock()
        .unwrap()
        .get(&name)
        .is_some_and(|lease| lease.token() >= token)
    {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let dims = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(dims.pane_cols, dims.pane_rows, grid.cols, grid.rows)?;
    let clamp_row = |row: u32| row.min(dims.pane_rows.saturating_sub(1));
    let clamp_col = |col: u32| col.min(dims.pane_cols.saturating_sub(1));
    let anchor_row = clamp_row(anchor_row);
    let anchor_col = clamp_col(anchor_col);
    let active_row = clamp_row(active_row);
    let active_col = clamp_col(active_col);
    let anchor_steps = copy_cursor_steps(&target, dims.scroll_position, anchor_row, anchor_col)?;
    let active_steps = copy_cursor_steps(&target, dims.scroll_position, active_row, active_col)?;
    let mut batch = Vec::new();
    // A wheel-scrolled pane is already in copy-mode at the user's chosen
    // history position. Re-entering copy-mode here jumps it back to the live
    // frame and makes a downward cross-screen drag impossible.
    if !dims.active {
        push_tmux_command(
            &mut batch,
            &["copy-mode".into(), "-H".into(), "-t".into(), target.clone()],
        );
    } else if dims.selection_present {
        // begin-selection is a toggle in tmux: invoking it while an older
        // selection is still present clears that selection instead of moving
        // its anchor. This can happen when a second physical drag starts
        // before the first start reply has crossed the webview boundary.
        // Clear explicitly so every start command has restart semantics.
        push_tmux_command(
            &mut batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.clone(),
                "-X".into(),
                "clear-selection".into(),
            ],
        );
    }
    push_copy_cursor(&mut batch, &target, anchor_row, anchor_steps);
    push_tmux_command(
        &mut batch,
        &[
            "send-keys".into(),
            "-t".into(),
            target.clone(),
            "-X".into(),
            "begin-selection".into(),
        ],
    );
    push_copy_cursor(&mut batch, &target, active_row, active_steps);
    tmux_owned(&batch).map_err(|e| {
        format!(
            "terminal selection could not start ({})",
            storage::err_code(&e)
        )
    })?;
    let status = terminal_selection_status_for(&target)?;
    terminal_selection_leases()
        .lock()
        .unwrap()
        .insert(name, TerminalSelectionLease::Dragging { token });
    Ok(status)
}

#[tauri::command]
pub(crate) fn terminal_selection_update(
    name: String,
    token: u64,
    row: u32,
    col: u32,
    edge_lines: i32,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let before = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(
        before.pane_cols,
        before.pane_rows,
        grid.cols,
        grid.rows,
    )?;
    // A freshly begun selection has no selected cells until its cursor first
    // leaves the anchor, so selection_present=0 is valid while a drag is
    // still inside that cell. Moving the copy cursor is what makes it present.
    if !before.active {
        return Err("terminal selection is no longer active".into());
    }
    let row = row.min(before.pane_rows.saturating_sub(1));
    let col = col.min(before.pane_cols.saturating_sub(1));
    let horizontal_steps = copy_cursor_steps(&target, before.scroll_position, row, col)?;
    let mut batch = Vec::new();
    push_copy_cursor(&mut batch, &target, row, horizontal_steps);
    if edge_lines != 0 {
        push_tmux_command(
            &mut batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.clone(),
                "-X".into(),
                "-N".into(),
                edge_lines.unsigned_abs().clamp(1, 8).to_string(),
                if edge_lines < 0 {
                    "cursor-up".into()
                } else {
                    "cursor-down".into()
                },
            ],
        );
    }
    tmux_owned(&batch).map_err(|e| {
        format!(
            "terminal selection could not move ({})",
            storage::err_code(&e)
        )
    })?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    terminal_selection_status_for(&target)
}

const MAX_TERMINAL_SELECTION_BYTES: u64 = 64 * 1024 * 1024;
static TERMINAL_SELECTION_BUFFER_NONCE: AtomicU64 = AtomicU64::new(0);

fn terminal_selection_buffer_prefix(token: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let nonce = TERMINAL_SELECTION_BUFFER_NONCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "deck-copy-{:x}-{token:x}-{nanos:x}-{nonce:x}-",
        std::process::id()
    )
}

#[derive(Serialize)]
pub(crate) struct TerminalSelectionCopy {
    text: String,
    bytes: u64,
    history_limit: u32,
}

#[tauri::command]
pub(crate) fn terminal_selection_finish(
    name: String,
    token: u64,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let status = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(
        status.pane_cols,
        status.pane_rows,
        grid.cols,
        grid.rows,
    )?;
    if !status.active || !status.selection_present {
        return Err("selection-missing".into());
    }
    let prefix = terminal_selection_buffer_prefix(token);
    let text = crate::terminal_selection::snapshot_selection(&target, &prefix, tmux_owned)
        .map_err(|_| "snapshot-failed".to_string())?;
    let bytes = text.len() as u64;
    if bytes > MAX_TERMINAL_SELECTION_BYTES {
        return Err(
            "terminal selection exceeds the 64 MiB clipboard limit; narrow the selection".into(),
        );
    }
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    // The snapshot above is the immutable selection authority from now on.
    // Clear tmux's cursor-bound highlight but keep copy-mode and its viewport;
    // the frontend renders the frozen content coordinates with public cell
    // geometry, so later scroll commands cannot move either endpoint.
    tmux(&["send-keys", "-t", &target, "-X", "clear-selection"])
        .map_err(|_| "snapshot-failed".to_string())?;
    terminal_selection_leases().lock().unwrap().insert(
        name.clone(),
        TerminalSelectionLease::Frozen {
            token,
            text,
            bytes,
            history_limit: status.history_limit,
            selection_start_row: status.selection_start_row,
            selection_start_col: status.selection_start_col,
            selection_end_row: status.selection_end_row,
            selection_end_col: status.selection_end_col,
        },
    );
    let viewport = terminal_selection_status_for(&target)?;
    frozen_selection_status(&name, token, viewport)
}

#[tauri::command]
pub(crate) fn terminal_selection_copy(
    name: String,
    token: u64,
) -> Result<TerminalSelectionCopy, String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    let lease = terminal_selection_leases()
        .lock()
        .unwrap()
        .get(&name)
        .cloned();
    match lease {
        Some(TerminalSelectionLease::Frozen {
            token: current,
            text,
            bytes,
            history_limit,
            ..
        }) if current == token => Ok(TerminalSelectionCopy {
            text,
            bytes,
            history_limit,
        }),
        Some(TerminalSelectionLease::Dragging { token: current }) if current == token => {
            let target = pane_target(&name);
            let status = terminal_selection_status_for(&target)?;
            if !status.active || !status.selection_present {
                return Err("selection-missing".into());
            }
            let prefix = terminal_selection_buffer_prefix(token);
            let text = crate::terminal_selection::snapshot_selection(&target, &prefix, tmux_owned)
                .map_err(|_| "snapshot-failed".to_string())?;
            let bytes = text.len() as u64;
            if bytes > MAX_TERMINAL_SELECTION_BYTES {
                return Err("snapshot-failed".into());
            }
            Ok(TerminalSelectionCopy {
                text,
                bytes,
                history_limit: status.history_limit,
            })
        }
        _ => Err("selection-missing".into()),
    }
}

#[tauri::command]
pub(crate) fn terminal_selection_scroll(
    name: String,
    token: u64,
    lines: i32,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, true) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    tmux_owned(&crate::terminal_scroll::args(&target, lines))?;
    let viewport = terminal_selection_status_for(&target)?;
    frozen_selection_status(&name, token, viewport)
}

#[tauri::command]
pub(crate) fn terminal_selection_cancel(name: String, token: u64) -> Result<(), String> {
    let _operation = terminal_selection_operation_lock().lock().unwrap();
    validate_session_name(&name)?;
    let should_cancel = {
        let mut leases = terminal_selection_leases().lock().unwrap();
        let matches = match leases.get(&name) {
            Some(TerminalSelectionLease::Dragging { token: current })
            | Some(TerminalSelectionLease::Frozen { token: current, .. }) => *current == token,
            Some(TerminalSelectionLease::Cancelled { .. }) => false,
            None => false,
        };
        if matches {
            leases.insert(name.clone(), TerminalSelectionLease::Cancelled { token });
        }
        matches
    };
    if !should_cancel {
        return Ok(());
    }
    let _ = tmux(&["send-keys", "-t", &pane_target(&name), "-X", "cancel"]);
    Ok(())
}

#[derive(Serialize)]
pub(crate) struct TerminalMetrics {
    history_rows: u32,
    history_limit: u32,
    pane_rows: u32,
    pane_cols: u32,
    in_copy_mode: bool,
    scroll_position: u32,
}

#[tauri::command]
pub(crate) fn terminal_metrics(name: String) -> Result<TerminalMetrics, String> {
    validate_session_name(&name)?;
    let status = terminal_selection_status_for(&pane_target(&name))?;
    Ok(TerminalMetrics {
        history_rows: status.history_rows,
        history_limit: status.history_limit,
        pane_rows: status.pane_rows,
        pane_cols: status.pane_cols,
        in_copy_mode: status.active,
        scroll_position: status.scroll_position,
    })
}

/// Native clipboard path for WKWebView. Success means pbcopy consumed all
/// bytes and exited zero; clipboard content never enters logs or errors.
#[tauri::command]
pub(crate) fn write_clipboard(text: String) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::Stdio;
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "clipboard-write-failed".to_string())?;
    child
        .stdin
        .take()
        .ok_or("clipboard-write-failed")?
        .write_all(text.as_bytes())
        .map_err(|_| "clipboard-write-failed".to_string())?;
    let status = child
        .wait()
        .map_err(|_| "clipboard-write-failed".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("clipboard-write-failed".into())
    }
}

#[tauri::command]
pub(crate) fn kill_session(name: String) -> Result<(), String> {
    validate_session_name(&name)?;
    idempotent_kill_result(tmux(&["kill-session", "-t", &session_target(&name)]))
}

pub(crate) fn idempotent_kill_result(result: Result<String, String>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        // Closing an already-gone session is the successful end state. This
        // also covers an empty deck tmux server ("no server running").
        Err(e) if matches!(storage::err_code(&e), "no-session" | "missing") => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------- polling ------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct SessInfo {
    name: String,
    alive: bool,
    /// seconds since the pane last produced output (None if unknown)
    idle_secs: Option<u64>,
    /// RSS of the whole process tree under the pane, in MB
    mem_mb: Option<f64>,
    /// last non-empty lines of the pane, for card previews
    tail: Vec<String>,
    /// foreground process in the pane (zsh, claude, node, …) — lets the
    /// frontend record shell commands but not agent prompts
    fg: Option<String>,
    /// pane is in tmux copy-mode: the VISIBLE frame is frozen scrollback,
    /// not live output — the UI must say so (a silently frozen agent TUI
    /// reads as a hung session)
    scrolled: Option<bool>,
}

pub(crate) fn tree_mem(roots: &HashMap<String, u32>) -> HashMap<String, f64> {
    let mut result = HashMap::new();
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
    else {
        return result;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut rss: HashMap<u32, u64> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(kb)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(kb)) = (pid.parse(), ppid.parse::<u32>(), kb.parse::<u64>())
        else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
        rss.insert(pid, kb);
    }
    for (session, root) in roots {
        let mut sum = 0u64;
        let mut stack = vec![*root];
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            sum += rss.get(&pid).copied().unwrap_or(0);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        result.insert(session.clone(), sum as f64 / 1024.0);
    }
    result
}

/// Per-poll ceiling on `capture-pane` targets. Every capture is pane-content
/// I/O through the tmux server; an unbounded board would make poll cost grow
/// with visible-card count. Boards with more on-screen cards than this get
/// previews for the first MAX_TAIL_SESSIONS only (frontend sends visible
/// cards in board order, so the truncation is stable, not flickering).
const MAX_TAIL_SESSIONS: usize = 16;

/// Marker line separating per-session segments in a batched capture. \x01 is
/// never produced by capture-pane for ordinary pane text lines.
const TAIL_MARK: &str = "\u{1}deck-tail\u{1}";

/// One pane-listing line → (session, pane pid, activity epoch, in copy-mode,
/// fg command). Every tmux session has at least one pane, so this listing
/// doubles as the liveness set — no separate `list-sessions` round-trip.
pub(crate) fn parse_panes(text: &str) -> HashMap<String, (u32, u64, bool, String)> {
    let mut panes: HashMap<String, (u32, u64, bool, String)> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        if let (Some(s), Some(pid), Some(act), Some(mode), Some(fg)) =
            (it.next(), it.next(), it.next(), it.next(), it.next())
        {
            if let (Ok(pid), Ok(act)) = (pid.parse(), act.parse()) {
                panes
                    .entry(s.to_string())
                    .or_insert((pid, act, mode == "1", fg.to_string()));
            }
        }
    }
    panes
}

/// Split batched `display-message ; capture-pane` output back into per-session
/// tails: each segment starts with a TAIL_MARK line naming the session, and
/// keeps the last `lines` non-empty lines of its capture.
pub(crate) fn parse_tail_batches(text: &str, lines: usize) -> HashMap<String, Vec<String>> {
    let mut tails: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix(TAIL_MARK) {
            current = Some(name.to_string());
            tails.entry(name.to_string()).or_default();
        } else if let Some(name) = &current {
            if !line.trim().is_empty() {
                tails.get_mut(name).unwrap().push(line.to_string());
            }
        }
    }
    for v in tails.values_mut() {
        let skip = v.len().saturating_sub(lines);
        v.drain(..skip);
    }
    tails
}

/// Fetch tail previews for many sessions in ONE tmux invocation: a command
/// batch of `display-message -p <marker+name> ; capture-pane -p …` pairs.
/// Ran per-session before (one subprocess per visible card, every 2.5s).
pub(crate) fn capture_tails(names: &[&String], lines: usize) -> HashMap<String, Vec<String>> {
    if names.is_empty() {
        return HashMap::new();
    }
    let mut args: Vec<String> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            args.push(";".into());
        }
        args.push("display-message".into());
        args.push("-p".into());
        args.push(format!("{TAIL_MARK}{}", crate::tmux::fmt_escape(name)));
        args.push(";".into());
        args.push("capture-pane".into());
        args.push("-p".into());
        args.push("-t".into());
        args.push(pane_target(name));
        args.push("-S".into());
        args.push("-30".into());
    }
    parse_tail_batches(&crate::tmux::tmux_batch(&args), lines)
}

/// Single poll for everything the board needs: liveness, output recency,
/// process-tree memory, and (for the sessions on screen) tail previews.
/// Cost is bounded: 2 tmux subprocesses + 1 ps per poll, independent of
/// session count (was 2 + one capture-pane per visible card).
///
/// PERF (examples/poll_bench.rs, M-series, release, 2026-08): per poll at
/// 5/20/50 sessions — old pattern 14/45/108 ms with 7/22/52 subprocesses;
/// batched pattern 4.3/4.6/5.1 ms with a constant 2 (+1 ps here).
#[tauri::command]
pub(crate) fn poll_sessions(names: Vec<String>, tail_for: Vec<String>) -> Vec<SessInfo> {
    // one listing supplies liveness + activity + pid + fg for every session
    let listing = tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}\t#{pane_pid}\t#{window_activity}\t#{pane_in_mode}\t#{pane_current_command}",
    ]);
    // a failing listing silently reads as "everything is dead" — log the
    // failure and the recovery, once per transition (tmux errors carry no
    // user content)
    static POLL_BROKEN: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    {
        let mut broken = POLL_BROKEN.lock().unwrap();
        match &listing {
            Err(e) if !*broken => {
                *broken = true;
                applog(&format!(
                    "[poll] session listing FAILED ({})",
                    storage::err_code(e)
                ));
            }
            Ok(_) if *broken => {
                *broken = false;
                applog("[poll] session listing recovered");
            }
            _ => {}
        }
    }
    let panes = parse_panes(&listing.unwrap_or_default());

    let roots: HashMap<String, u32> = names
        .iter()
        .filter_map(|n| panes.get(n).map(|(pid, _, _, _)| (n.clone(), *pid)))
        .collect();
    let mem = tree_mem(&roots);

    // captures only for sessions that are both requested AND alive — a dead
    // target inside the batch would abort the remaining commands
    let want_tails: Vec<&String> = tail_for
        .iter()
        .filter(|n| panes.contains_key(*n))
        .take(MAX_TAIL_SESSIONS)
        .collect();
    if tail_for.len() > MAX_TAIL_SESSIONS {
        applog(&format!(
            "[poll] tail previews capped at {MAX_TAIL_SESSIONS} of {}",
            tail_for.len()
        ));
    }
    let mut tails = capture_tails(&want_tails, 2);
    let now = now_epoch();

    names
        .into_iter()
        .map(|name| {
            let pane = panes.get(&name);
            SessInfo {
                alive: pane.is_some(),
                idle_secs: pane.map(|(_, act, _, _)| now.saturating_sub(*act)),
                mem_mb: mem.get(&name).copied(),
                tail: tails.remove(&name).unwrap_or_default(),
                fg: pane.map(|(_, _, _, fg)| fg.clone()),
                scrolled: pane.map(|(_, _, m, _)| *m),
                name,
            }
        })
        .collect()
}

// ---------- open path / url ----------------------------------------------------

#[derive(Serialize)]
pub(crate) struct ResolvedPathTarget {
    directory: String,
    target_is_directory: bool,
}

fn unquote_clicked_path(value: &str) -> String {
    let value = value.trim();
    for quote in ['\'', '"'] {
        if value.starts_with(quote) {
            if let Some(end) = value[1..].rfind(quote).map(|i| i + 1) {
                let suffix = &value[end + quote.len_utf8()..];
                if suffix.is_empty()
                    || suffix.strip_prefix(':').is_some_and(|s| {
                        !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == ':')
                    })
                {
                    return format!("{}{}", &value[1..end], suffix);
                }
            }
        }
    }
    value.to_string()
}

fn absolute_clicked_path(value: &str, cwd: &str) -> Result<PathBuf, String> {
    let value = expand_tilde(value);
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::fs::canonicalize(expand_tilde(cwd))
        .map_err(|_| "the session working directory is unavailable".to_string())?;
    if !cwd.is_dir() {
        return Err("the session working directory is unavailable".into());
    }
    Ok(cwd.join(path))
}

/// Resolve a clicked path without confusing a real `name:42` file with a
/// line suffix: the literal path always wins when it exists; suffix removal
/// is only a fallback after that lookup fails.
pub(crate) fn resolve_clicked_parent(value: &str, cwd: &str) -> Result<ResolvedPathTarget, String> {
    let raw = unquote_clicked_path(value);
    let literal = absolute_clicked_path(&raw, cwd)?;
    let resolved = match std::fs::canonicalize(&literal) {
        Ok(path) => path,
        Err(_) => {
            let stripped = regex_strip_lineno(&raw);
            if stripped == raw {
                return Err("the selected path does not exist or cannot be accessed".into());
            }
            std::fs::canonicalize(absolute_clicked_path(&stripped, cwd)?)
                .map_err(|_| "the selected path does not exist or cannot be accessed".to_string())?
        }
    };
    let meta = std::fs::metadata(&resolved)
        .map_err(|_| "the selected path does not exist or cannot be accessed".to_string())?;
    let target_is_directory = meta.is_dir();
    let directory = if target_is_directory {
        resolved
    } else {
        resolved
            .parent()
            .ok_or_else(|| "the selected path has no usable parent folder".to_string())?
            .to_path_buf()
    };
    if !directory.is_dir() {
        return Err("the selected path has no usable parent folder".into());
    }
    Ok(ResolvedPathTarget {
        directory: directory.to_string_lossy().into_owned(),
        target_is_directory,
    })
}

#[tauri::command]
pub(crate) fn resolve_parent_dir(value: String, cwd: String) -> Result<ResolvedPathTarget, String> {
    resolve_clicked_parent(&value, &cwd)
}

/// Link discovery is intentionally stricter than token discovery: a path-like
/// token only becomes interactive when it resolves to a real local target in
/// the pane's working directory. Actions resolve it again to avoid TOCTOU.
fn terminal_path_exists(value: &str, cwd: &str) -> bool {
    const MAX_PATH_TOKEN: usize = 4096;
    if value.is_empty()
        || value.len() > MAX_PATH_TOKEN
        || cwd.is_empty()
        || cwd.len() > MAX_PATH_TOKEN
    {
        return false;
    }
    resolve_clicked_parent(value, cwd).is_ok()
}

#[tauri::command]
pub(crate) fn terminal_paths_exist(values: Vec<String>, cwd: String) -> Vec<bool> {
    const MAX_CANDIDATES: usize = 128;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| index < MAX_CANDIDATES && terminal_path_exists(value, &cwd))
        .collect()
}

/// What open_target is allowed to hand to `open`, decided BEFORE any
/// subprocess spawns. `open` treats its argument as a URL when it parses as
/// one — an unvalidated "url" click could reach file:// or an arbitrary app
/// scheme; a relative path could resolve outside the card's cwd view. Rules:
/// urls must be http(s); paths must resolve absolute and exist.
pub(crate) fn validate_open(kind: &str, value: &str, resolved: &str) -> Result<(), String> {
    match kind {
        "url" => {
            let lower = value.trim().to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Ok(())
            } else {
                Err(format!("only http(s) links open externally: {value}"))
            }
        }
        "editor" | "editor-parent" | "reveal" => {
            if !resolved.starts_with('/') {
                return Err(format!("path did not resolve absolute: {resolved}"));
            }
            if !std::path::Path::new(resolved).exists() {
                return Err(format!("no such path: {resolved}"));
            }
            Ok(())
        }
        _ => Err(format!("unknown kind: {kind}")),
    }
}

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), String> {
    let resolved = if kind == "url" {
        String::new()
    } else if kind == "editor-parent" {
        resolve_clicked_parent(&value, &cwd)?.directory
    } else {
        let raw = unquote_clicked_path(&value);
        let literal = absolute_clicked_path(&raw, &cwd)?;
        match std::fs::canonicalize(&literal) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => {
                let stripped = regex_strip_lineno(&raw);
                std::fs::canonicalize(absolute_clicked_path(&stripped, &cwd)?)
                    .map_err(|_| {
                        "the selected path does not exist or cannot be accessed".to_string()
                    })?
                    .to_string_lossy()
                    .into_owned()
            }
        }
    };
    validate_open(&kind, &value, &resolved)?;
    let status = match kind.as_str() {
        "url" => Command::new("open").arg(value.trim()).status(),
        "editor-parent" => match editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolved]).status(),
            None => return Err("choose an editor in Settings before opening a folder".into()),
        },
        "editor" => match editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolved]).status(),
            None => Command::new("open").args(["-t", &resolved]).status(),
        },
        "reveal" => Command::new("open").args(["-R", &resolved]).status(),
        _ => unreachable!("validate_open rejects unknown kinds"),
    }
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("the selected item could not be opened".into())
    }
}

pub(crate) fn regex_strip_lineno(path: &str) -> String {
    // "src/foo.rs:42:7" → "src/foo.rs". Work from the RIGHT so a legal
    // colon elsewhere in the filename/path is untouched.
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    let Some((head, tail)) = path.rsplit_once(':') else {
        return path.to_string();
    };
    if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
        return path.to_string();
    }
    if let Some((base, line)) = head.rsplit_once(':') {
        if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) {
            return base.to_string();
        }
    }
    head.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_rejects_a_stale_frontend_grid_instead_of_clamping_it() {
        assert!(require_terminal_selection_dimensions(80, 24, 80, 24).is_ok());
        assert_eq!(
            require_terminal_selection_dimensions(79, 24, 80, 24).unwrap_err(),
            "selection-dimensions-changed"
        );
        assert_eq!(
            require_terminal_selection_dimensions(80, 23, 80, 24).unwrap_err(),
            "selection-dimensions-changed"
        );
    }

    #[test]
    fn parse_panes_basic_and_malformed() {
        let text = "alpha\t100\t1700000000\t0\tzsh\nbeta\t200\t1700000005\t1\tclaude\njunk-line\nempty\t\t\t\t\n";
        let p = parse_panes(text);
        assert_eq!(p.len(), 2);
        assert_eq!(p["alpha"], (100, 1700000000, false, "zsh".into()));
        assert_eq!(
            p["beta"],
            (200, 1700000005, true, "claude".into()),
            "copy-mode pane reported as scrolled"
        );
    }

    #[test]
    fn parse_panes_first_pane_wins() {
        // multi-pane session: the first listed pane is the representative one
        let text = "s\t10\t111\t0\tzsh\ns\t20\t222\t1\tvim\n";
        assert_eq!(parse_panes(text)["s"], (10, 111, false, "zsh".into()));
    }

    #[test]
    fn tail_batches_split_and_trim() {
        let text =
            format!("{TAIL_MARK}a\nline1\n\nline2\nline3\n{TAIL_MARK}b\n\n{TAIL_MARK}c\nonly\n");
        let t = parse_tail_batches(&text, 2);
        assert_eq!(t["a"], vec!["line2", "line3"]);
        assert!(t["b"].is_empty(), "empty pane still yields an entry");
        assert_eq!(t["c"], vec!["only"]);
    }

    #[test]
    fn tail_batches_ignore_preamble() {
        // output before the first marker (e.g. a stray error line) is dropped
        let t = parse_tail_batches(&format!("noise\n{TAIL_MARK}a\nx\n"), 2);
        assert_eq!(t.len(), 1);
        assert_eq!(t["a"], vec!["x"]);
    }

    #[test]
    fn killing_an_already_missing_session_is_idempotent_but_real_errors_survive() {
        for missing in [
            "tmux kill-session failed: can't find session: x",
            "tmux kill-session failed: no server running",
            "tmux kill-session failed: error connecting to socket (No such file or directory)",
        ] {
            assert!(idempotent_kill_result(Err(missing.into())).is_ok());
        }
        let real =
            idempotent_kill_result(Err("tmux kill-session failed: permission denied".into()));
        assert!(real.is_err());
        assert!(idempotent_kill_result(Ok(String::new())).is_ok());
    }

    #[test]
    fn open_validation_gates_urls_and_paths() {
        assert!(validate_open("url", "https://example.com/x", "").is_ok());
        assert!(validate_open("url", "HTTP://example.com", "").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://host",
            "x-apple.systempreferences:",
            "/etc/passwd",
        ] {
            assert!(validate_open("url", bad, "").is_err(), "{bad}");
        }
        assert!(validate_open("reveal", "", "/tmp").is_ok());
        assert!(validate_open("reveal", "", "relative/path").is_err());
        assert!(validate_open("editor", "", "/no/such/path/deck-test").is_err());
        assert!(validate_open("shell", "", "/tmp").is_err(), "unknown kind");
    }

    #[test]
    fn strip_lineno_suffixes() {
        assert_eq!(regex_strip_lineno("src/foo.rs:42:7"), "src/foo.rs");
        assert_eq!(regex_strip_lineno("src/foo.rs:42"), "src/foo.rs");
        assert_eq!(regex_strip_lineno("src/foo.rs"), "src/foo.rs");
        // a colon followed by non-digits is part of the path, not a lineno
        assert_eq!(regex_strip_lineno("a:b/c"), "a:b/c");
        assert_eq!(regex_strip_lineno("a:b/c.rs:9"), "a:b/c.rs");
        assert_eq!(regex_strip_lineno("http://x/y:8080"), "http://x/y:8080");
    }

    #[test]
    fn clicked_paths_resolve_files_directories_unicode_quotes_and_suffixes() {
        let root = std::env::temp_dir().join(format!(
            "deck-parent-resolve-{}-{}",
            std::process::id(),
            now_epoch()
        ));
        let dir = root.join("空 格😀");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("code.rs");
        std::fs::write(&file, b"fn main() {}\n").unwrap();
        let colon_file = dir.join("actual:42");
        std::fs::write(&colon_file, b"literal colon\n").unwrap();

        let relative =
            resolve_clicked_parent("\"空 格😀/code.rs\":12:3", &root.to_string_lossy()).unwrap();
        assert_eq!(
            PathBuf::from(relative.directory),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(!relative.target_is_directory);

        let absolute = resolve_clicked_parent(&dir.to_string_lossy(), "/tmp").unwrap();
        assert_eq!(
            PathBuf::from(absolute.directory),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(absolute.target_is_directory);

        let literal = resolve_clicked_parent(&colon_file.to_string_lossy(), "/tmp").unwrap();
        assert_eq!(
            PathBuf::from(literal.directory),
            std::fs::canonicalize(&dir).unwrap()
        );
        assert!(
            !literal.target_is_directory,
            "an existing :42 filename wins over suffix parsing"
        );

        let root_target = resolve_clicked_parent("/", "/tmp").unwrap();
        assert_eq!(root_target.directory, "/");
        assert!(root_target.target_is_directory);
        if let Some(home) = dirs::home_dir() {
            let tilde = resolve_clicked_parent("~", "/tmp").unwrap();
            assert_eq!(
                PathBuf::from(tilde.directory),
                std::fs::canonicalize(home).unwrap()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = root.join("locked");
            std::fs::create_dir(&locked).unwrap();
            std::fs::write(locked.join("secret.txt"), b"secret").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            assert!(
                resolve_clicked_parent("locked/secret.txt", &root.to_string_lossy()).is_err(),
                "an unsearchable parent has a safe failure"
            );
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        assert!(resolve_clicked_parent("missing.txt", &root.to_string_lossy()).is_err());
        assert!(resolve_clicked_parent("file.txt", "/definitely/missing/deck-cwd").is_err());
        let cwd = root.to_string_lossy().into_owned();
        assert_eq!(
            terminal_paths_exist(
                vec![
                    "\"空 格😀/code.rs\":12:3".into(),
                    "memcache.go:265".into(),
                    "x".repeat(4097),
                ],
                cwd,
            ),
            vec![true, false, false]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    // ---------- dropped files ----------

    #[test]
    fn drop_names_are_sanitized_but_recognizable() {
        assert_eq!(
            sanitize_drop_name("Screenshot 2026-08-28 at 5.12.03 PM.png"),
            "Screenshot-2026-08-28-at-5.12.03-PM.png"
        );
        assert_eq!(sanitize_drop_name("../../etc/passwd"), "passwd");
        assert_eq!(
            sanitize_drop_name(".hidden"),
            "hidden",
            "never creates dotfiles"
        );
        assert_eq!(
            sanitize_drop_name("测试截图.png"),
            "png",
            "non-ascii collapses, ext survives"
        );
        assert_eq!(
            sanitize_drop_name("///"),
            "file",
            "degenerate names fall back"
        );
        let long = sanitize_drop_name(&format!("{}.png", "a".repeat(200)));
        assert!(long.len() <= 72 && long.ends_with(".png"), "{long}");
    }

    #[test]
    fn dropped_files_land_private_unique_and_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("deck-drops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p1 = save_drop_into(&d, "shot.png", b"AAAA").unwrap();
        let p2 = save_drop_into(&d, "shot.png", b"BBBB").unwrap();
        assert_ne!(p1, p2, "same name twice → distinct files");
        assert_eq!(std::fs::read(&p1).unwrap(), b"AAAA");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p1), 0o600);
        assert_eq!(mode(&d), 0o700);
        assert!(save_drop_into(&d, "x", b"").is_err(), "empty refused");
        let big = vec![0u8; MAX_DROP_BYTES + 1];
        assert!(save_drop_into(&d, "x", &big).is_err(), "oversize refused");
    }

    #[test]
    fn old_drops_are_pruned_fresh_ones_kept() {
        let d = std::env::temp_dir().join(format!("deck-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let old = d.join("old.png");
        let fresh = d.join("fresh.png");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&fresh, "x").unwrap();
        // age the old file via touch (std cannot set mtime)
        std::process::Command::new("touch")
            .args(["-t", "202001010000", old.to_str().unwrap()])
            .status()
            .unwrap();
        crate::storage::prune_old_files(&d, 7 * 24 * 3600);
        assert!(!old.exists(), "week-old drop removed");
        assert!(fresh.exists(), "fresh drop kept");
        // missing dir is a no-op, not a panic
        crate::storage::prune_old_files(&d.join("nope"), 60);
    }

    // ---------- board / settings business validation ----------

    /// A minimal valid board matching what persistence.js actually writes.
    fn board(cards: &str) -> String {
        format!(
            r#"{{"projects":[{{"id":"P1","name":"main","columns":[
                 {{"id":"C1","name":"Attention"}},{{"id":"C2","name":"Working"}}]}},
                 {{"id":"P2","name":"side","columns":[{{"id":"C9","name":"Only"}}]}}],
               "cards":[{cards}]}}"#
        )
    }
    fn card(id: &str, project: &str, column: &str, session: &str) -> String {
        format!(
            r#"{{"id":"{id}","projectId":"{project}","columnId":"{column}",
                 "title":"t","desc":"","cmd":"claude","dir":"~/w","session":"{session}"}}"#
        )
    }

    #[test]
    fn board_validation_accepts_real_shape_and_unknown_extensions() {
        let ok = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        assert!(serde_json::from_str::<BoardDoc>(&ok).is_ok());
        // future extension fields anywhere must not break loading
        let extended = ok
            .replacen(
                "{\"projects\"",
                "{\"futureTopLevel\":{\"x\":1},\"projects\"",
                1,
            )
            .replacen("\"title\":\"t\"", "\"title\":\"t\",\"pinned\":true", 1);
        assert!(
            serde_json::from_str::<BoardDoc>(&extended).is_ok(),
            "unknown fields are tolerated"
        );
        // empty board is a valid first save
        assert!(serde_json::from_str::<BoardDoc>(r#"{"projects":[],"cards":[]}"#).is_ok());
    }

    #[test]
    fn board_validation_rejects_broken_documents() {
        let fail = |doc: &str, why: &str, needle: &str| {
            let e = match serde_json::from_str::<BoardDoc>(doc) {
                Err(e) => e.to_string(),
                Ok(_) => panic!("{why}: invalid document was accepted"),
            };
            assert!(e.contains(needle), "{why}: wrong error {e}");
        };
        // missing runtime field (no session)
        let no_session =
            board(r#"{"id":"s1","projectId":"P1","columnId":"C1","title":"t","cmd":"","dir":""}"#);
        fail(&no_session, "missing session", "session");
        // duplicate project id
        let dup_proj = r#"{"projects":[
            {"id":"P1","name":"a","columns":[{"id":"C1","name":"x"}]},
            {"id":"P1","name":"b","columns":[{"id":"C2","name":"y"}]}],"cards":[]}"#;
        fail(dup_proj, "dup project", "duplicate project id");
        // duplicate column id within a project
        let dup_col = r#"{"projects":[{"id":"P1","name":"a","columns":[
            {"id":"C1","name":"x"},{"id":"C1","name":"y"}]}],"cards":[]}"#;
        fail(dup_col, "dup column", "duplicate column id");
        // a project with no columns cannot hold cards
        let no_cols = r#"{"projects":[{"id":"P1","name":"a","columns":[]}],"cards":[]}"#;
        fail(no_cols, "no columns", "no columns");
        // duplicate card ids
        let dup_card = board(&format!(
            "{},{}",
            card("s1", "P1", "C1", "deck-a-1111"),
            card("s1", "P1", "C2", "deck-b-2222")
        ));
        fail(&dup_card, "dup card", "duplicate card id");
        // dangling project reference
        fail(
            &board(&card("s1", "PX", "C1", "deck-a-1111")),
            "dangling project",
            "missing project",
        );
        // column exists but belongs to ANOTHER project
        fail(
            &board(&card("s1", "P1", "C9", "deck-a-1111")),
            "wrong-project column",
            "not in its project",
        );
        // session name breaking the runtime rule (tmux target separators)
        fail(
            &board(&card("s1", "P1", "C1", "has:colon")),
            "illegal session",
            "session name",
        );
        // two cards sharing one tmux session
        let dup_sess = board(&format!(
            "{},{}",
            card("s1", "P1", "C1", "deck-a-1111"),
            card("s2", "P1", "C2", "deck-a-1111")
        ));
        fail(&dup_sess, "dup session", "already used");
    }

    #[test]
    fn settings_validation_type_checks_optional_keys() {
        assert!(serde_json::from_str::<SettingsDoc>(r#"{}"#).is_ok());
        assert!(
            serde_json::from_str::<SettingsDoc>(r#"{"editor":"Zed","debug":true,"future":1}"#)
                .is_ok()
        );
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"editor":123}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"debug":"yes"}"#).is_err());
        for locale in ["system", "en", "zh-Hans"] {
            assert!(
                serde_json::from_str::<SettingsDoc>(&format!(r#"{{"locale":"{locale}"}}"#)).is_ok()
            );
        }
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":"zh-CN"}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":false}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"{"locale":null}"#).is_err());
        assert!(serde_json::from_str::<SettingsDoc>(r#"[1,2]"#).is_err());
    }

    #[test]
    fn locale_setting_persists_with_unknown_fields_and_rejects_atomically() {
        let d = std::env::temp_dir().join(format!("deck-settings-locale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("settings.json");
        let good = r#"{"editor":"Zed","debug":true,"locale":"zh-Hans","future":{"kept":1}}"#;
        save_validated::<SettingsDoc>(&p, good, "settings").unwrap();
        let loaded = storage::load_typed::<SettingsDoc>(&p)
            .unwrap()
            .unwrap()
            .payload;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&loaded).unwrap(),
            serde_json::from_str::<serde_json::Value>(good).unwrap()
        );
        let before = std::fs::read_to_string(&p).unwrap();
        assert!(save_validated::<SettingsDoc>(
            &p,
            r#"{"locale":"zh-CN","future":{"kept":2}}"#,
            "settings"
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn save_rejection_touches_neither_main_nor_backup() {
        let d = std::env::temp_dir().join(format!("deck-savereject-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("deck.json");
        let good = board(&card("s1", "P1", "C1", "deck-t-ab12"));
        save_validated::<BoardDoc>(&p, &good, "board").unwrap();
        let before = std::fs::read_to_string(&p).unwrap();

        let bad = board(&card("s1", "PX", "C1", "deck-t-ab12")); // dangling ref
        let err = save_validated::<BoardDoc>(&p, &bad, "board").unwrap_err();
        assert!(err.contains("refusing to save"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            before,
            "main untouched"
        );
        let mut bak = p.as_os_str().to_owned();
        bak.push(".bak");
        assert!(
            !std::path::PathBuf::from(bak).exists(),
            "backup not rotated by a rejected save"
        );
        // a valid save afterwards still works (rejection left no debris)
        save_validated::<BoardDoc>(&p, &good, "board").unwrap();
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
