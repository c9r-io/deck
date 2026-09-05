//! Terminal link targets: resolving clicked paths against the pane cwd in
//! Rust (no shell), existence checks for link candidates, and the validated
//! `open_target` command.

use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;

use crate::error::DeckError;
use crate::tmux::expand_tilde;

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

fn absolute_clicked_path(value: &str, cwd: &str) -> Result<PathBuf, DeckError> {
    let value = expand_tilde(value);
    let path = PathBuf::from(&value);
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::fs::canonicalize(expand_tilde(cwd))
        .map_err(|_| DeckError::from("the session working directory is unavailable"))?;
    if !cwd.is_dir() {
        return Err("the session working directory is unavailable".into());
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
                return Err("the selected path does not exist or cannot be accessed".into());
            }
            std::fs::canonicalize(absolute_clicked_path(&stripped, cwd)?).map_err(|_| {
                DeckError::from("the selected path does not exist or cannot be accessed")
            })?
        }
    };
    let meta = std::fs::metadata(&resolved)
        .map_err(|_| DeckError::from("the selected path does not exist or cannot be accessed"))?;
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
pub(crate) fn resolve_parent_dir(
    value: String,
    cwd: String,
) -> Result<ResolvedPathTarget, DeckError> {
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
pub(crate) fn validate_open(kind: &str, value: &str, resolved: &str) -> Result<(), DeckError> {
    match kind {
        "url" => {
            let lower = value.trim().to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Ok(())
            } else {
                Err(format!("only http(s) links open externally: {value}").into())
            }
        }
        "editor" | "editor-parent" | "reveal" => {
            if !resolved.starts_with('/') {
                return Err(format!("path did not resolve absolute: {resolved}").into());
            }
            if !std::path::Path::new(resolved).exists() {
                return Err(format!("no such path: {resolved}").into());
            }
            Ok(())
        }
        _ => Err(format!("unknown kind: {kind}").into()),
    }
}

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), DeckError> {
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
                        DeckError::from("the selected path does not exist or cannot be accessed")
                    })?
                    .to_string_lossy()
                    .into_owned()
            }
        }
    };
    validate_open(&kind, &value, &resolved)?;
    let status = match kind.as_str() {
        "url" => Command::new("open").arg(value.trim()).status(),
        "editor-parent" => match crate::documents::editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolved]).status(),
            None => return Err("choose an editor in Settings before opening a folder".into()),
        },
        "editor" => match crate::documents::editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolved]).status(),
            None => Command::new("open").args(["-t", &resolved]).status(),
        },
        "reveal" => Command::new("open").args(["-R", &resolved]).status(),
        _ => unreachable!("validate_open rejects unknown kinds"),
    }
    .map_err(DeckError::from)?;
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
            crate::datadir::now_epoch()
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

    #[test]
    fn filesystem_command_adapters_bound_candidates_and_resolve_parents() {
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

        let mut candidates = vec!["nested/file.rs".to_string(), "missing.rs".to_string()];
        candidates.extend((0..128).map(|_| "nested".to_string()));
        let exists = terminal_paths_exist(candidates, dir.display().to_string());
        assert!(exists[0]);
        assert!(!exists[1]);
        assert!(exists[127]);
        assert!(!exists[128]);
        assert!(!exists[129]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
