//! Terminal link targets: resolving clicked paths against the pane cwd in
//! Rust (no shell) and the validated `open_target` command. Hover discovery
//! is frontend-only; a path is checked when an action is taken on it.
//! A failed open or parent resolution returns one of the closed
//! `LINK_FAILURES` codes as its whole message (never the path, the URL or a
//! sentence): the wire carries only the message, so the code is what lets
//! the link menu say which of the four reasons applied. The codes are
//! mirrored in `ui/test/fixtures/limits.json` (`link_failures`).
//! Voice setup opens only three fixed System Settings destinations; terminal
//! URLs still accept only http(s). Opening settings never grants permissions.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

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

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), DeckError> {
    open_with_editor(&kind, &value, &cwd, crate::documents::editor_app)
}

/// `open_target` with the editor choice supplied by the caller, read only
/// by the two editor actions and only after the target resolved.
fn open_with_editor(
    kind: &str,
    value: &str,
    cwd: &str,
    editor: impl Fn() -> Option<String>,
) -> Result<(), DeckError> {
    let resolved = if kind == "url" {
        String::new()
    } else if kind == "editor-parent" {
        resolve_clicked_parent(value, cwd)?.directory
    } else {
        let raw = unquote_clicked_path(value);
        let literal = absolute_clicked_path(&raw, cwd)?;
        match std::fs::canonicalize(&literal) {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(_) => {
                let stripped = regex_strip_lineno(&raw);
                std::fs::canonicalize(absolute_clicked_path(&stripped, cwd)?)
                    .map_err(|_| path_missing())?
                    .to_string_lossy()
                    .into_owned()
            }
        }
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
            Some(app) => Command::new("/usr/bin/open")
                .args(["-a", &app, &resolved])
                .status(),
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
}
