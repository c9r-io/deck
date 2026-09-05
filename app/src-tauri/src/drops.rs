//! Dropped/pasted files: WKWebView hands the frontend bytes with no path,
//! so the bytes are persisted 0600 under `~/.deck/drops` and the path is
//! typed into the session.

use std::path::PathBuf;

use crate::storage;
use crate::storage::{applog, now_epoch};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
