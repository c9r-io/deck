//! The private data directory (`~/.deck`, or the debug-only smoke root).
//!
//! Everything under it can carry user content (board titles, prompts, shell
//! commands), so the tree is user-only BY CONSTRUCTION: the directory 0700
//! and every file 0600 from creation — never "create world-readable, chmod
//! later" (that window is a real race). Renames preserve the creation mode,
//! so the atomic-write temp being 0600 makes main files and `.bak` files
//! 0600 too. `atomic_write` is unique temp → fsync → rename → parent fsync.
//! `harden_data_dir` migrates a tree an older deck left 0644 at boot, and
//! `prune_old_files` ages out transient drops and snapshots.

use crate::launch_args::debug_arg;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Open `path` for writing (create-or-truncate) with user-only permissions
/// applied at creation time.
pub(crate) fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    // the mode above only applies when the file is CREATED; a pre-existing
    // file (e.g. written by an older deck as 0644) keeps its old bits
    restrict_to_user(path);
    Ok(f)
}

/// Write a whole file with user-only permissions (non-atomic; for files that
/// are not load-bearing data, e.g. exports).
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = open_private(path).map_err(|e| format!("could not create file ({})", e.kind()))?;
    f.write_all(bytes)
        .map_err(|e| format!("could not write file ({})", e.kind()))
}

/// Create `dir` (and parents) and restrict it to the user (0700).
pub(crate) fn create_private_dir(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("could not create data dir ({})", e.kind()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("could not restrict data dir ({})", e.kind()))
}

/// Boot-time, idempotent permission migration for the whole data tree:
/// directories → 0700, regular files → 0600 (covers main files, .bak,
/// .corrupt-*, app.log, exports and anything an older deck left 0644).
/// Symlinks are left untouched — chmod would follow them out of the tree.
/// Errors are counted and reported without embedding any path.
pub(crate) fn harden_data_dir(dir: &Path) -> Result<(), String> {
    let mut errs = 0u32;
    fn set(p: &Path, mode: u32, errs: &mut u32) {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).is_err() {
            *errs += 1;
        }
    }
    if !dir.exists() {
        return create_private_dir(dir);
    }
    set(dir, 0o700, &mut errs);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            errs += 1;
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => {
                    set(&p, 0o700, &mut errs);
                    stack.push(p);
                }
                Ok(t) if t.is_file() => set(&p, 0o600, &mut errs),
                _ => {} // symlink or unknown: never chmod through it
            }
        }
    }
    if errs > 0 {
        Err(format!(
            "could not restrict {errs} data file(s) to user-only access"
        ))
    } else {
        Ok(())
    }
}

/// Write `bytes` to `path` atomically: unique same-directory temp file →
/// fsync → rename → fsync of the parent directory (so the rename itself
/// survives power loss). Unique temp names keep concurrent saves of the
/// same file from trampling each other's temp file.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or("data path has no parent directory")?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    {
        use std::os::unix::fs::OpenOptionsExt;
        // create_new + mode: the temp file is 0600 from its first instant,
        // and the rename below preserves that mode on the final file
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("could not create temp file ({})", e.kind()))?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Best-effort cleanup of transient files older than `max_age_secs`
/// (~/.deck/drops — files saved from drag/paste so their path could be
/// typed into a session). Missing dir is fine; failures are silent.
pub(crate) fn prune_old_files(dir: &Path, max_age_secs: u64) {
    let now = now_epoch();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| now.saturating_sub(d.as_secs()) > max_age_secs)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

pub(crate) fn deck_dir() -> PathBuf {
    if let Some(root) = debug_arg("--smoke-data-dir") {
        let path = PathBuf::from(root);
        if path.is_absolute() {
            return path;
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
}

/// chmod 0600 — best-effort, silent (fixes pre-existing lax modes; new files
/// are already created 0600 via open_private/atomic_write).
pub(crate) fn restrict_to_user(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

pub(crate) fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deck-datadir-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn write_private_restricts_even_a_preexisting_lax_file() {
        let d = tdir("permw");
        let p = d.join("out.txt");
        std::fs::write(&p, "old").unwrap();
        set_mode(&p, 0o644);
        write_private(&p, b"new").unwrap();
        assert_eq!(mode_of(&p), 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    }

    #[test]
    fn harden_migrates_legacy_modes_idempotently() {
        let d = tdir("harden");
        // simulate an older deck's layout: lax dir + files, incl. backup,
        // quarantine, log and an exports subdirectory
        for name in [
            "deck.json",
            "deck.json.bak",
            "queue.json.corrupt-1",
            "app.log",
        ] {
            let p = d.join(name);
            std::fs::write(&p, "x").unwrap();
            set_mode(&p, 0o644);
        }
        let exports = d.join("exports");
        std::fs::create_dir(&exports).unwrap();
        let exp_file = exports.join("deck-log-1.txt");
        std::fs::write(&exp_file, "x").unwrap();
        set_mode(&exp_file, 0o644);
        set_mode(&exports, 0o755);
        set_mode(&d, 0o755);

        harden_data_dir(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700);
        assert_eq!(mode_of(&exports), 0o700);
        for name in [
            "deck.json",
            "deck.json.bak",
            "queue.json.corrupt-1",
            "app.log",
        ] {
            assert_eq!(mode_of(&d.join(name)), 0o600, "{name}");
        }
        assert_eq!(mode_of(&exp_file), 0o600);
        // idempotent: a second run changes nothing and still succeeds
        harden_data_dir(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700);
    }

    #[test]
    fn harden_creates_a_missing_dir_private() {
        let d = tdir("hardennew").join("fresh");
        harden_data_dir(&d).unwrap();
        assert_eq!(mode_of(&d), 0o700);
    }
}
