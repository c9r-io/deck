//! Terminal link targets: resolving clicked paths against the pane cwd in
//! Rust (no shell) and the validated `open_target` command. Hover discovery
//! is frontend-only; a path is checked when an action is taken on it.
//! A failed open or parent resolution returns one of the closed
//! `LINK_FAILURES` codes as its whole message (never the path, the URL or a
//! sentence): the wire carries only the message, so the code is what lets
//! the link menu say which of the four reasons applied. The codes are
//! mirrored in `ui/test/fixtures/limits.json` (`link_failures`).
//! A clicked `file:line[:column]` reaches the editor as a position only for
//! the editors in `EDITOR_LINE_SCHEMES`, through that editor's own URL
//! scheme: the URL is built here from the scheme in the table, the
//! canonical path (percent-encoded) and the parsed numbers, and handed to
//! `open -a <the chosen editor>`. No clicked text becomes part of a scheme;
//! an editor outside the table, or a click without a line, opens exactly as
//! before. The editor may ask before opening such a URL; that is its own.
//! Voice setup opens only three fixed System Settings destinations; terminal
//! URLs still accept only http(s). Opening settings never grants permissions.
//!
//! # A pane whose program is `ssh`
//! What such a pane prints are paths of ANOTHER machine: resolving them here
//! finds nothing, and says so correctly. `link_remote` reads, when a link
//! is clicked and never before, the foreground program of the session's
//! pane; when that is a direct `ssh`, its own arguments name the
//! destination (`ssh_destination`). That word changes what the link menu
//! offers and nothing else: it is read from the process's argv, proves
//! nothing about where a path lives (a second `ssh` typed inside the first
//! is invisible), is not kept, written or logged, and lifts no hold.
//! `open_remote_target` reads it AGAIN — a host the webview names is never
//! used — and, for an editor in the closed `REMOTE_EDITOR_SCHEMES` table,
//! hands `open -a <that editor>` its own remote URL: built here from the
//! table's scheme, a destination of `[A-Za-z0-9._-]` only, the clicked
//! absolute path (percent-encoded) and parsed numbers. Only an `ssh` whose
//! arguments are the destination and switches that take no value is opened
//! this way: the editor connects by the user's own ssh configuration, and
//! what was given on a command line (a port, a user, a jump host, a key)
//! it would not know. Deck opens no connection, starts no `ssh`, reads no
//! ssh configuration or key, and cannot say whether the file exists.
//! A failed open is one app.log line with its closed code and nothing of
//! what was clicked.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::tmux::expand_tilde;

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VoiceSettings {
    Microphone,
    Speech,
    Dictation,
}

impl VoiceSettings {
    fn url(&self) -> &'static str {
        match self {
            Self::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Self::Speech => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition"
            }
            Self::Dictation => "x-apple.systempreferences:com.apple.preference.keyboard?Dictation",
        }
    }
}

#[tauri::command]
pub(crate) fn voice_open_settings(kind: VoiceSettings) -> Result<(), DeckError> {
    let status = Command::new("/usr/bin/open")
        .arg(kind.url())
        .status()
        .map_err(DeckError::from)?;
    if status.success() {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::Other,
            "system-settings-unavailable",
        ))
    }
}

// ---------- open path / url ----------------------------------------------------

/// Why `open_target` / `resolve_parent_dir` refused or failed, as the UI
/// may tell it apart. Each is the error's entire message.
pub(crate) const LINK_NO_EDITOR: &str = "link-no-editor";
pub(crate) const LINK_PATH_MISSING: &str = "link-path-missing";
pub(crate) const LINK_NOT_ALLOWED: &str = "link-not-allowed";
pub(crate) const LINK_OPEN_FAILED: &str = "link-open-failed";
#[cfg(test)]
const LINK_FAILURES: [&str; 4] = [
    LINK_NO_EDITOR,
    LINK_PATH_MISSING,
    LINK_NOT_ALLOWED,
    LINK_OPEN_FAILED,
];

/// The clicked path, its parent or the session directory it is relative to
/// is not there or cannot be read.
fn path_missing() -> DeckError {
    DeckError::new(ErrorKind::Missing, LINK_PATH_MISSING)
}

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
                    || suffix == ":"
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

fn absolute_clicked_path(value: &str, cwd: &str) -> Result<PathBuf, DeckError> {
    let value = expand_tilde(value);
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::fs::canonicalize(expand_tilde(cwd)).map_err(|_| path_missing())?;
    if !cwd.is_dir() {
        return Err(path_missing());
    }
    Ok(cwd.join(path))
}

/// Resolve a clicked path without confusing a real `name:42` file with a
/// line suffix: the literal path always wins when it exists; suffix removal
/// is only a fallback after that lookup fails.
pub(crate) fn resolve_clicked_parent(
    value: &str,
    cwd: &str,
) -> Result<ResolvedPathTarget, DeckError> {
    let raw = unquote_clicked_path(value);
    let literal = absolute_clicked_path(&raw, cwd)?;
    let resolved = match std::fs::canonicalize(&literal) {
        Ok(path) => path,
        Err(_) => {
            let stripped = regex_strip_lineno(&raw);
            if stripped == raw {
                return Err(path_missing());
            }
            std::fs::canonicalize(absolute_clicked_path(&stripped, cwd)?)
                .map_err(|_| path_missing())?
        }
    };
    let meta = std::fs::metadata(&resolved).map_err(|_| path_missing())?;
    let target_is_directory = meta.is_dir();
    let directory = if target_is_directory {
        resolved
    } else {
        resolved.parent().ok_or_else(path_missing)?.to_path_buf()
    };
    if !directory.is_dir() {
        return Err(path_missing());
    }
    Ok(ResolvedPathTarget {
        directory: directory.to_string_lossy().into_owned(),
        target_is_directory,
    })
}

#[tauri::command]
pub(crate) fn resolve_parent_dir(
    value: String,
    cwd: String,
) -> Result<ResolvedPathTarget, DeckError> {
    resolve_clicked_parent(&value, &cwd)
}

/// What open_target is allowed to hand to `open`, decided BEFORE any
/// subprocess spawns. `open` treats its argument as a URL when it parses as
/// one — an unvalidated "url" click could reach file:// or an arbitrary app
/// scheme; a relative path could resolve outside the card's cwd view. Rules:
/// urls must be http(s); paths must resolve absolute and exist. A refusal
/// names its reason and repeats nothing of what was clicked.
pub(crate) fn validate_open(kind: &str, value: &str, resolved: &str) -> Result<(), DeckError> {
    let not_allowed = || DeckError::new(ErrorKind::Invalid, LINK_NOT_ALLOWED);
    match kind {
        "url" => {
            let lower = value.trim().to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Ok(())
            } else {
                Err(not_allowed())
            }
        }
        "editor" | "editor-parent" | "reveal" => {
            if !resolved.starts_with('/') {
                return Err(not_allowed());
            }
            if !std::path::Path::new(resolved).exists() {
                return Err(path_missing());
            }
            Ok(())
        }
        _ => Err(not_allowed()),
    }
}

/// A clicked `:line[:column]`, both 1-based as the terminal printed them.
type ClickedPosition = (u32, Option<u32>);

/// Editors that open a file at a position through their own URL scheme,
/// `<scheme>://file<absolute path>:<line>[:<column>]`. Closed, and only
/// what was seen working: VS Code documents the form ("Opening VS Code with
/// URLs"), Cursor follows it. Keys are the `detect_editors` names.
const EDITOR_LINE_SCHEMES: &[(&str, &str)] =
    &[("Visual Studio Code", "vscode"), ("Cursor", "cursor")];

/// The `:line[:column]` suffix `regex_strip_lineno` removes, as numbers.
/// Digits only; a zero or an overflowing line is no position at all, and
/// such a column is dropped.
fn clicked_position(raw: &str) -> Option<ClickedPosition> {
    let number = |digits: &str| {
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
            .then(|| digits.parse::<u32>().ok().filter(|n| *n > 0))
    };
    let (head, tail) = raw.rsplit_once(':')?;
    let last = number(tail)?;
    match head.rsplit_once(':').and_then(|(_, line)| number(line)) {
        Some(line) => Some((line?, last)),
        None => Some((last?, None)),
    }
}

/// The file a click names, canonical, and the position it carried. The
/// literal path wins when it exists, so a real `name:42` file is that file
/// and has no position; the suffix is read only after that lookup failed.
fn resolve_clicked_file(
    value: &str,
    cwd: &str,
) -> Result<(String, Option<ClickedPosition>), DeckError> {
    let raw = unquote_clicked_path(value);
    let literal = absolute_clicked_path(&raw, cwd)?;
    if let Ok(path) = std::fs::canonicalize(&literal) {
        return Ok((path.to_string_lossy().into_owned(), None));
    }
    let stripped = regex_strip_lineno(&raw);
    let path = std::fs::canonicalize(absolute_clicked_path(&stripped, cwd)?)
        .map_err(|_| path_missing())?;
    Ok((path.to_string_lossy().into_owned(), clicked_position(&raw)))
}

/// What `open -a <editor>` is handed for a resolved file: the path, or for
/// an `EDITOR_LINE_SCHEMES` editor and a click that carried a line, that
/// editor's URL. Everything but unreserved characters and `/` is
/// percent-encoded, so nothing in a file name can end the path early.
fn editor_target(editor: &str, resolved: &str, position: Option<ClickedPosition>) -> String {
    let scheme = EDITOR_LINE_SCHEMES
        .iter()
        .find(|(name, _)| *name == editor)
        .map(|(_, scheme)| *scheme);
    let (Some(scheme), Some((line, column))) = (scheme, position) else {
        return resolved.to_string();
    };
    let mut url = format!("{scheme}://file");
    push_encoded_path(&mut url, resolved);
    url.push_str(&format!(":{line}"));
    if let Some(column) = column {
        url.push_str(&format!(":{column}"));
    }
    url
}

/// The ONE place a chosen editor is handed a target it opens itself: a
/// resolved file of this Mac (or that editor's URL for it at a line), or,
/// for a path clicked in a pane whose program is `ssh`, that editor's
/// remote URL (`remote_open`). Always `open -a <editor> <one argument>`.
fn open_in_editor(app: &str, target: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("/usr/bin/open")
        .args(["-a", app, target])
        .status()
}

/// `path` with everything but unreserved characters and `/`
/// percent-encoded: nothing in a file name can end a URL's path early.
fn push_encoded_path(url: &mut String, path: &str) {
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                url.push(byte as char)
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
}

/// One line for a link action that failed: its closed code, nothing of what
/// was clicked.
fn logged(result: Result<(), DeckError>) -> Result<(), DeckError> {
    if let Err(error) = &result {
        let code = error.message();
        let code = if LINK_CODES.contains(&code) {
            code
        } else {
            "other"
        };
        applog(&format!("[link] failed ({code})"));
    }
    result
}

/// The codes `logged` may write.
const LINK_CODES: [&str; 4] = [
    LINK_NO_EDITOR,
    LINK_PATH_MISSING,
    LINK_NOT_ALLOWED,
    LINK_OPEN_FAILED,
];

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), DeckError> {
    logged(open_with_editor(
        &kind,
        &value,
        &cwd,
        crate::documents::editor_app,
    ))
}

// ---------- a pane whose program is ssh ------------------------------------------

/// Editors that open a file of another machine through their own URL
/// scheme, `<scheme>://vscode-remote/ssh-remote+<host><absolute path>`,
/// followed by `:<line>[:<column>]` for a FILE — without a trailing number
/// VS Code opens the path as a folder (read in VS Code 1.146.0's own
/// handler and seen opening a file at its line, 2026-11-18). Closed, and
/// only what was seen working. Keys are the `detect_editors` names.
const REMOTE_EDITOR_SCHEMES: &[(&str, &str)] = &[("Visual Studio Code", "vscode")];

/// `ssh` switches that take no value, and those that take one.
const SSH_SWITCHES: &str = "46AaCfGgKkMNnqsTtVvXxYy";
const SSH_SWITCHES_WITH_VALUE: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// What a pane's `ssh` was asked to connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SshDestination {
    /// The destination word as typed: a host or an alias, or `user@host`.
    host: String,
    /// The command line holds nothing an editor's own connection would not
    /// know: the destination is a bare host or alias and every switch is
    /// one that takes no value.
    plain: bool,
}

/// The destination of an `ssh` command line, or `None` when the program is
/// not `ssh` or its arguments cannot be read with certainty (a switch this
/// table does not have, no destination, a destination that is not a plain
/// word). A remote command after the destination changes nothing.
fn ssh_destination(argv: &[String]) -> Option<SshDestination> {
    let program = std::path::Path::new(argv.first()?).file_name()?.to_str()?;
    if program != "ssh" {
        return None;
    }
    let mut plain = true;
    let mut at = 1;
    while let Some(argument) = argv.get(at) {
        if argument == "--" {
            at += 1;
            break;
        }
        let Some(switches) = argument.strip_prefix('-').filter(|rest| !rest.is_empty()) else {
            break;
        };
        for (index, switch) in switches.char_indices() {
            if SSH_SWITCHES.contains(switch) {
                continue;
            }
            if !SSH_SWITCHES_WITH_VALUE.contains(switch) {
                return None;
            }
            plain = false;
            // its value is the rest of this word, or the next word
            if index + switch.len_utf8() == switches.len() {
                at += 1;
            }
            break;
        }
        at += 1;
    }
    let host = argv.get(at)?;
    let word = |text: &str| {
        !text.is_empty()
            && text.len() <= 255
            && !text.starts_with('-')
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    let named = match host.split_once('@') {
        Some((user, host)) => {
            plain = false;
            word(user) && word(host)
        }
        None => word(host),
    };
    named.then(|| SshDestination {
        host: host.clone(),
        plain,
    })
}

/// The destination of the `ssh` in front of `session`'s pane right now.
fn remote_of(session: &str) -> Option<SshDestination> {
    crate::tmux::validate_session_name(session).ok()?;
    let rows = crate::tmux::list_panes().ok()?;
    let targets = crate::agent_status::signal_targets(&rows);
    let pane = targets.get(session)?;
    let foreground = crate::agent_status::live_generation(pane.pane_pid)?;
    ssh_destination(&crate::procinfo::argv(foreground.pid())?)
}

/// What the link menu may say about a card whose program is `ssh`.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub(crate) struct RemoteHost {
    /// The destination as the user typed it. Shown and copied; never a
    /// URL's part unless `open` is true.
    host: String,
    /// The chosen editor can open a path of that machine (module header).
    open: bool,
}

/// The editor's scheme for a remote open, when the chosen editor has one.
fn remote_scheme(editor: Option<&str>) -> Option<(&'static str, &'static str)> {
    let editor = editor?;
    REMOTE_EDITOR_SCHEMES
        .iter()
        .find(|(name, _)| *name == editor)
        .copied()
}

#[tauri::command]
pub(crate) fn link_remote(session: String) -> Option<RemoteHost> {
    let remote = remote_of(&session)?;
    let editor = crate::documents::editor_app();
    Some(RemoteHost {
        open: remote.plain && remote_scheme(editor.as_deref()).is_some(),
        host: remote.host,
    })
}

/// The editor and the URL a remote open hands to `open -a`. The clicked
/// text gives the path and its numbers and nothing else: it must be
/// absolute (Deck does not know the remote shell's directory or home), a
/// path that ends in `/` is a folder, and any other gets a line — `:1`
/// when none was clicked — because the editor reads a path without one as
/// a folder.
fn remote_open(
    remote: &SshDestination,
    value: &str,
    editor: Option<&str>,
) -> Result<(&'static str, String), DeckError> {
    let not_allowed = || DeckError::new(ErrorKind::Invalid, LINK_NOT_ALLOWED);
    let (app, scheme) =
        remote_scheme(editor).ok_or_else(|| DeckError::new(ErrorKind::Missing, LINK_NO_EDITOR))?;
    if !remote.plain {
        return Err(not_allowed());
    }
    let raw = unquote_clicked_path(value);
    let position = clicked_position(&raw);
    let path = regex_strip_lineno(&raw);
    if !path.starts_with('/') || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(not_allowed());
    }
    let mut url = format!("{scheme}://vscode-remote/ssh-remote+{}", remote.host);
    push_encoded_path(&mut url, &path);
    if !path.ends_with('/') {
        let (line, column) = position.unwrap_or((1, None));
        url.push_str(&format!(":{line}"));
        if let Some(column) = column {
            url.push_str(&format!(":{column}"));
        }
    }
    Ok((app, url))
}

/// Open a clicked path of the machine `session`'s `ssh` is connected to, in
/// the chosen editor's remote mode. The destination is read here, now.
#[tauri::command]
pub(crate) fn open_remote_target(session: String, value: String) -> Result<(), DeckError> {
    logged((|| {
        let remote = remote_of(&session)
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, LINK_NOT_ALLOWED))?;
        let editor = crate::documents::editor_app();
        let (app, url) = remote_open(&remote, &value, editor.as_deref())?;
        let status = open_in_editor(app, &url)
            .map_err(|e| DeckError::new(ErrorKind::io(e.kind()), LINK_OPEN_FAILED))?;
        if status.success() {
            Ok(())
        } else {
            Err(DeckError::new(ErrorKind::Other, LINK_OPEN_FAILED))
        }
    })())
}

/// `open_target` with the editor choice supplied by the caller, read only
/// by the two editor actions and only after the target resolved.
fn open_with_editor(
    kind: &str,
    value: &str,
    cwd: &str,
    editor: impl Fn() -> Option<String>,
) -> Result<(), DeckError> {
    let (resolved, position) = if kind == "url" {
        (String::new(), None)
    } else if kind == "editor-parent" {
        (resolve_clicked_parent(value, cwd)?.directory, None)
    } else {
        resolve_clicked_file(value, cwd)?
    };
    validate_open(kind, value, &resolved)?;
    let status = match kind {
        "url" => Command::new("/usr/bin/open").arg(value.trim()).status(),
        "editor-parent" => match editor() {
            Some(app) => Command::new("/usr/bin/open")
                .args(["-a", &app, &resolved])
                .status(),
            None => return Err(DeckError::new(ErrorKind::Missing, LINK_NO_EDITOR)),
        },
        "editor" => match editor() {
            Some(app) => open_in_editor(&app, &editor_target(&app, &resolved, position)),
            None => Command::new("/usr/bin/open")
                .args(["-t", &resolved])
                .status(),
        },
        "reveal" => Command::new("/usr/bin/open")
            .args(["-R", &resolved])
            .status(),
        _ => unreachable!("validate_open rejects unknown kinds"),
    }
    // `open` could not be started: the log keeps the io kind, the caller
    // gets the same reason as an `open` that ran and failed.
    .map_err(|e| DeckError::new(ErrorKind::io(e.kind()), LINK_OPEN_FAILED))?;
    if status.success() {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Other, LINK_OPEN_FAILED))
    }
}

pub(crate) fn regex_strip_lineno(path: &str) -> String {
    // "src/foo.rs:42:7" or "src/foo.rs:" → "src/foo.rs". Work from the
    // RIGHT so a legal colon elsewhere in the filename/path is untouched.
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    if let Some(head) = path.strip_suffix(':') {
        return head.to_string();
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
    fn voice_settings_accept_only_fixed_setup_destinations() {
        for (kind, suffix) in [
            ("microphone", "security?Privacy_Microphone"),
            ("speech", "security?Privacy_SpeechRecognition"),
            ("dictation", "keyboard?Dictation"),
        ] {
            let target: VoiceSettings = serde_json::from_value(serde_json::json!(kind)).unwrap();
            assert_eq!(
                target.url(),
                format!("x-apple.systempreferences:com.apple.preference.{suffix}")
            );
        }
        for kind in ["", "url", "file:///tmp", "x-apple.systempreferences:other"] {
            assert!(serde_json::from_value::<VoiceSettings>(serde_json::json!(kind)).is_err());
        }
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
        assert_eq!(regex_strip_lineno("src/foo.rs:"), "src/foo.rs");
        assert_eq!(regex_strip_lineno("src/foo.rs"), "src/foo.rs");
        // a colon followed by non-digits is part of the path, not a lineno
        assert_eq!(regex_strip_lineno("a:b/c"), "a:b/c");
        assert_eq!(regex_strip_lineno("a:b/c.rs:9"), "a:b/c.rs");
        assert_eq!(regex_strip_lineno("a:b/c.rs:"), "a:b/c.rs");
        assert_eq!(regex_strip_lineno("http://x/y:8080"), "http://x/y:8080");
    }

    #[test]
    fn clicked_paths_resolve_files_directories_unicode_quotes_and_suffixes() {
        let root = std::env::temp_dir().join(format!(
            "deck-parent-resolve-{}-{}",
            std::process::id(),
            crate::datadir::now_epoch()
        ));
        let dir = root.join("空 格😀");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("code.rs");
        std::fs::write(&file, b"fn main() {}\n").unwrap();
        let colon_file = dir.join("actual:42");
        std::fs::write(&colon_file, b"literal colon\n").unwrap();
        let trailing_colon_file = dir.join("actual:");
        std::fs::write(&trailing_colon_file, b"literal trailing colon\n").unwrap();

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
        let literal =
            resolve_clicked_parent(&trailing_colon_file.to_string_lossy(), "/tmp").unwrap();
        assert!(
            !literal.target_is_directory,
            "an existing trailing colon wins over punctuation fallback"
        );
        let punctuated =
            resolve_clicked_parent("空 格😀/code.rs:", &root.to_string_lossy()).unwrap();
        assert_eq!(
            PathBuf::from(punctuated.directory),
            std::fs::canonicalize(&dir).unwrap()
        );
        let quoted =
            resolve_clicked_parent("\"空 格😀/code.rs\":", &root.to_string_lossy()).unwrap();
        assert_eq!(
            PathBuf::from(quoted.directory),
            std::fs::canonicalize(&dir).unwrap()
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
        assert!(resolve_clicked_parent("\"空 格😀/code.rs\":12:3", &cwd).is_ok());
        assert!(resolve_clicked_parent("memcache.go:265", &cwd).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Every failure before `open(1)` runs carries one closed reason code as
    /// its whole message: no path, no URL, no sentence the UI would have to
    /// parse. The kinds the log has always recorded are unchanged.
    #[test]
    fn open_failures_name_a_closed_reason_and_carry_no_content() {
        let root = std::env::temp_dir().join(format!(
            "deck-link-failure-{}-{}",
            std::process::id(),
            crate::datadir::now_epoch()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let open = |kind: &str, value: &str, cwd: &str| {
            open_target(kind.into(), value.into(), cwd.into()).unwrap_err()
        };
        for (kind, value, cwd, code, error_kind) in [
            (
                "editor",
                "absent.rs:12",
                cwd.as_str(),
                "link-path-missing",
                ErrorKind::Missing,
            ),
            (
                "reveal",
                "nested/absent",
                cwd.as_str(),
                "link-path-missing",
                ErrorKind::Missing,
            ),
            (
                "editor-parent",
                "absent/file.rs",
                cwd.as_str(),
                "link-path-missing",
                ErrorKind::Missing,
            ),
            (
                "editor",
                "file.rs",
                "/definitely/missing/deck-cwd",
                "link-path-missing",
                ErrorKind::Missing,
            ),
            (
                "url",
                "file:///etc/passwd",
                cwd.as_str(),
                "link-not-allowed",
                ErrorKind::Invalid,
            ),
            (
                "url",
                "x-private-scheme://secret-token",
                cwd.as_str(),
                "link-not-allowed",
                ErrorKind::Invalid,
            ),
        ] {
            let error = open(kind, value, cwd);
            assert_eq!(error.message(), code, "{kind} {value}");
            assert_eq!(error.kind(), error_kind, "{kind} {value}");
        }
        for (value, cwd) in [
            ("absent.rs", cwd.as_str()),
            ("file.rs", "/definitely/missing/deck-cwd"),
        ] {
            let error = resolve_parent_dir(value.into(), cwd.into())
                .map(|_| ())
                .unwrap_err();
            assert_eq!(error.message(), "link-path-missing");
            assert_eq!(error.kind(), ErrorKind::Missing);
        }
        // A folder that resolved, with no editor chosen: refused before any
        // spawn. The file action has a system default and is not refused.
        let no_editor = open_with_editor("editor-parent", "nested", &cwd, || None).unwrap_err();
        assert_eq!(no_editor.message(), "link-no-editor");
        assert_eq!(no_editor.kind(), ErrorKind::Missing);
        // Resolution is checked first, so a missing path is never reported
        // as a missing editor.
        assert_eq!(
            open_with_editor("editor-parent", "absent/file.rs", &cwd, || None)
                .unwrap_err()
                .message(),
            "link-path-missing"
        );
        assert_eq!(
            validate_open("shell", "", "/tmp").unwrap_err().message(),
            "link-not-allowed"
        );
        assert_eq!(
            validate_open("reveal", "", "relative/path")
                .unwrap_err()
                .message(),
            "link-not-allowed"
        );
        assert_eq!(
            validate_open("editor", "", "/no/such/path/deck-test")
                .unwrap_err()
                .message(),
            "link-path-missing"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_clicked_suffix_is_a_position_only_as_plain_positive_numbers() {
        assert_eq!(clicked_position("src/foo.rs:42:7"), Some((42, Some(7))));
        assert_eq!(clicked_position("src/foo.rs:42"), Some((42, None)));
        assert_eq!(clicked_position("a:b/c.rs:9"), Some((9, None)));
        for none in [
            "src/foo.rs",
            "src/foo.rs:",
            "a:b/c",
            "src/foo.rs:0",
            "src/foo.rs:0:3",
            "src/foo.rs:99999999999",
            "src/foo.rs:+4",
            "src/foo.rs:４２",
        ] {
            assert_eq!(clicked_position(none), None, "{none}");
        }
        // a column that is not usable is dropped, the line stays
        assert_eq!(clicked_position("src/foo.rs:42:0"), Some((42, None)));
        assert_eq!(
            clicked_position("src/foo.rs:42:99999999999"),
            Some((42, None))
        );
    }

    /// The URL is the table's scheme, the canonical path and two numbers.
    /// Nothing else, and only for a listed editor and a click with a line.
    #[test]
    fn only_listed_editors_get_a_line_url_built_from_the_canonical_path() {
        let path = "/tmp/deck f03 空格/sample#1.txt";
        assert_eq!(
            editor_target("Visual Studio Code", path, Some((9, None))),
            "vscode://file/tmp/deck%20f03%20%E7%A9%BA%E6%A0%BC/sample%231.txt:9"
        );
        assert_eq!(
            editor_target("Cursor", "/tmp/deck-f03-sample.txt", Some((42, Some(7)))),
            "cursor://file/tmp/deck-f03-sample.txt:42:7"
        );
        // characters that could end or reshape the URL never survive
        let hostile = "/tmp/a:1?x=y&z%20\"<>'`|{}[]^\n;@=+,$!*()";
        let url = editor_target("Cursor", hostile, Some((3, None)));
        let encoded = url
            .strip_prefix("cursor://file")
            .and_then(|rest| rest.strip_suffix(":3"))
            .unwrap();
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~/%".contains(c)),
            "{encoded}"
        );
        // no line, or an editor that is not listed: the path, byte for byte
        assert_eq!(editor_target("Cursor", path, None), path);
        for editor in [
            "Zed",
            "Sublime Text",
            "TextMate",
            "BBEdit",
            "Nova",
            "IntelliJ IDEA",
            "WebStorm",
            "RustRover",
            "Xcode",
            "cursor",
            "Cursor ",
            "vscode://evil",
            "",
        ] {
            assert_eq!(
                editor_target(editor, path, Some((9, Some(2)))),
                path,
                "{editor}"
            );
        }
        for (name, scheme) in EDITOR_LINE_SCHEMES {
            assert!(scheme.chars().all(|c| c.is_ascii_lowercase()), "{name}");
        }
    }

    #[test]
    fn a_position_comes_only_from_a_suffix_the_filesystem_did_not_claim() {
        let root = std::env::temp_dir().join(format!(
            "deck-link-position-{}-{}",
            std::process::id(),
            crate::datadir::now_epoch()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("code.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(root.join("name:12"), b"literal colon\n").unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let canonical = |name: &str| {
            std::fs::canonicalize(root.join(name))
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };

        assert_eq!(
            resolve_clicked_file("code.rs:42:7", &cwd).unwrap(),
            (canonical("code.rs"), Some((42, Some(7))))
        );
        assert_eq!(
            resolve_clicked_file("\"code.rs\":42", &cwd).unwrap(),
            (canonical("code.rs"), Some((42, None)))
        );
        assert_eq!(
            resolve_clicked_file("code.rs", &cwd).unwrap(),
            (canonical("code.rs"), None)
        );
        assert_eq!(
            resolve_clicked_file("code.rs:", &cwd).unwrap(),
            (canonical("code.rs"), None)
        );
        // a literal existing colon-number filename wins: it is the file
        assert_eq!(
            resolve_clicked_file("name:12", &cwd).unwrap(),
            (canonical("name:12"), None)
        );
        // A line on that file was never resolved (both numeric suffixes are
        // removed together) and still is not: unchanged, it stays missing.
        assert_eq!(
            resolve_clicked_file("name:12:5", &cwd)
                .unwrap_err()
                .message(),
            LINK_PATH_MISSING
        );
        assert_eq!(
            resolve_clicked_file("absent.rs:3", &cwd)
                .unwrap_err()
                .message(),
            LINK_PATH_MISSING
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn link_failure_codes_are_the_shared_list() {
        let limits: serde_json::Value =
            serde_json::from_str(include_str!("../../ui/test/fixtures/limits.json")).unwrap();
        let shared: Vec<&str> = limits["link_failures"]
            .as_array()
            .expect("limits.json link_failures")
            .iter()
            .map(|code| code.as_str().unwrap())
            .collect();
        assert_eq!(shared, LINK_FAILURES);
    }

    #[test]
    fn filesystem_command_adapter_resolves_parents() {
        let dir = std::env::temp_dir().join(format!("deck-path-command-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/file.rs"), "fn main() {}\n").unwrap();

        let resolved =
            resolve_parent_dir("nested/file.rs:12:3".into(), dir.display().to_string()).unwrap();
        assert_eq!(
            std::path::Path::new(&resolved.directory),
            std::fs::canonicalize(dir.join("nested")).unwrap()
        );
        assert!(!resolved.target_is_directory);
        let gone = resolve_parent_dir("nested/absent.rs".into(), dir.display().to_string())
            .map(|_| ())
            .unwrap_err();
        assert_eq!(
            gone.kind(),
            ErrorKind::Missing,
            "a path that is not there is Missing, so the log can tell it from a bad request"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| word.to_string()).collect()
    }

    fn destination(words: &[&str]) -> Option<(String, bool)> {
        ssh_destination(&argv(words)).map(|remote| (remote.host, remote.plain))
    }

    /// The destination is read from ssh's own grammar, or not at all.
    #[test]
    fn an_ssh_command_line_names_its_destination_or_nothing() {
        let plain = |host: &str| Some((host.to_string(), true));
        let named = |host: &str| Some((host.to_string(), false));
        assert_eq!(destination(&["ssh", "air2"]), plain("air2"));
        assert_eq!(destination(&["/usr/bin/ssh", "air2"]), plain("air2"));
        assert_eq!(
            destination(&["ssh", "build-01.example.com"]),
            plain("build-01.example.com")
        );
        // switches that take no value change nothing; a remote command neither
        assert_eq!(destination(&["ssh", "-t", "-A", "air2"]), plain("air2"));
        assert_eq!(
            destination(&["ssh", "-tt", "air2", "tmux", "attach"]),
            plain("air2")
        );
        assert_eq!(
            destination(&["ssh", "-qv4", "air2", "ls", "-la", "/tmp"]),
            plain("air2")
        );
        assert_eq!(destination(&["ssh", "--", "air2"]), plain("air2"));
        // what an editor's own connection would not know: named, never opened
        assert_eq!(destination(&["ssh", "-p", "2222", "air2"]), named("air2"));
        assert_eq!(destination(&["ssh", "-p2222", "air2"]), named("air2"));
        assert_eq!(
            destination(&["ssh", "-i", "/keys/id", "air2"]),
            named("air2")
        );
        assert_eq!(destination(&["ssh", "-J", "jump", "air2"]), named("air2"));
        assert_eq!(
            destination(&["ssh", "-o", "ProxyCommand=nc %h %p", "air2"]),
            named("air2")
        );
        assert_eq!(destination(&["ssh", "-tl", "me", "air2"]), named("air2"));
        assert_eq!(
            destination(&["ssh", "-L", "8080:localhost:80", "-N", "air2"]),
            named("air2")
        );
        assert_eq!(destination(&["ssh", "me@air2"]), named("me@air2"));
        // a value is never taken for the destination
        assert_eq!(destination(&["ssh", "-p", "air2"]), None);
        assert_eq!(destination(&["ssh", "-l", "me"]), None);
        // not ssh, no destination, a switch this table does not have, a word that is no host
        for words in [
            &["zsh"][..],
            &["mosh", "air2"],
            &["sshd", "air2"],
            &["ssh-add", "air2"],
            &["ssh"],
            &["ssh", "-t"],
            &["ssh", "-Z", "air2"],
            &["ssh", "-", "air2"],
            &["ssh", "ssh://air2:22"],
            &["ssh", "air2:22"],
            &["ssh", "air 2"],
            &["ssh", "air2;id"],
            &["ssh", "$(id)"],
            &["ssh", "a/b"],
            &["ssh", "@air2"],
            &["ssh", "me@"],
            &["ssh", "me@air2@x"],
            &["ssh", "höst"],
            &[],
        ] {
            assert_eq!(destination(words), None, "{words:?}");
        }
        assert_eq!(
            destination(&["ssh", &"a".repeat(255)]),
            plain(&"a".repeat(255))
        );
        assert_eq!(destination(&["ssh", &"a".repeat(256)]), None);
    }

    /// The remote URL is the table's scheme, the destination, the clicked
    /// absolute path and its numbers: a file always gets a line, a folder never.
    #[test]
    fn a_remote_open_is_built_from_the_table_the_destination_and_the_clicked_path() {
        let remote = |host: &str, plain: bool| SshDestination {
            host: host.into(),
            plain,
        };
        let air2 = remote("air2", true);
        let vscode = Some("Visual Studio Code");
        let url = |value: &str| {
            remote_open(&air2, value, vscode).map(|(app, url)| {
                assert_eq!(app, "Visual Studio Code");
                url
            })
        };
        let base = "vscode://vscode-remote/ssh-remote+air2";
        // exactly what was seen opening a file at its line (2026-11-18)
        assert_eq!(
            url("/Users/chenhan/.zshenv:3").unwrap(),
            format!("{base}/Users/chenhan/.zshenv:3")
        );
        assert_eq!(
            url("/src/main.rs:42:7").unwrap(),
            format!("{base}/src/main.rs:42:7")
        );
        // no line: the editor would read the path as a folder, so a file gets one
        assert_eq!(
            url("/src/main.rs").unwrap(),
            format!("{base}/src/main.rs:1")
        );
        assert_eq!(
            url("/src/main.rs:").unwrap(),
            format!("{base}/src/main.rs:1")
        );
        assert_eq!(
            url("/src/main.rs:0").unwrap(),
            format!("{base}/src/main.rs:1")
        );
        // a path that ends in a slash is a folder, with or without a number behind it
        assert_eq!(url("/srv/app/").unwrap(), format!("{base}/srv/app/"));
        // nothing in a name ends the path or becomes a part of the URL
        assert_eq!(
            url("'/tmp/a b?x=1#frag%41.rs':9").unwrap(),
            format!("{base}/tmp/a%20b%3Fx%3D1%23frag%2541.rs:9")
        );
        assert_eq!(
            url("/tmp/名.rs").unwrap(),
            format!("{base}/tmp/%E5%90%8D.rs:1")
        );
        assert_eq!(
            url("/tmp/a:b/c.rs").unwrap(),
            format!("{base}/tmp/a%3Ab/c.rs:1")
        );
        // only an absolute path: Deck knows neither the remote directory nor the remote home
        for value in [
            "src/main.rs",
            "./a.rs",
            "~/a.rs",
            "",
            "a.rs:3",
            "https://x/y",
            "/tmp/a\nb",
            "/tmp/a\u{7}",
        ] {
            assert_eq!(
                url(value).unwrap_err().message(),
                LINK_NOT_ALLOWED,
                "{value:?}"
            );
        }
        assert_eq!(
            url(&format!("/{}", "a".repeat(4096)))
                .unwrap_err()
                .message(),
            LINK_NOT_ALLOWED
        );
        // a destination with anything beside it on the command line is never opened
        assert_eq!(
            remote_open(&remote("air2", false), "/a.rs", vscode)
                .unwrap_err()
                .message(),
            LINK_NOT_ALLOWED
        );
        // an editor outside the table, or none: its own reason, and no URL
        for editor in [
            Some("Cursor"),
            Some("Zed"),
            Some("visual studio code"),
            None,
        ] {
            assert_eq!(
                remote_open(&air2, "/a.rs", editor).unwrap_err().message(),
                LINK_NO_EDITOR,
                "{editor:?}"
            );
            assert_eq!(remote_scheme(editor), None);
        }
        // the log's codes are the failures' codes
        assert_eq!(LINK_CODES, LINK_FAILURES);
    }
}
