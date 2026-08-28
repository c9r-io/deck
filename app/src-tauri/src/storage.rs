//! One reliable persistence layer for every deck data file
//! (deck.json / queue.json / history.json / settings.json).
//!
//! Guarantees:
//! - every write is atomic: unique temp file in the same directory → fsync →
//!   rename → parent-directory fsync (both the main file and its `.bak`);
//! - the previous good version is kept as `<file>.bak` before each save;
//! - files carry `{"schema_version": N, "data": …}`; legacy version-less
//!   files are read as v0 and upgraded in place on their next save;
//! - loading is TYPED: a file must parse as JSON, carry a readable envelope
//!   AND deserialize into its document type — valid JSON with the wrong
//!   business structure goes through the same recovery as garbage bytes;
//! - a damaged main file is quarantined to a unique `.corrupt-<ts>` BEFORE
//!   the `.bak` (which gets the same full validation) is consulted, so the
//!   damaged bytes are never overwritten and the caller learns exactly what
//!   happened via `LoadOutcome::warning` (returned in-band, not queued);
//! - a file written by a NEWER deck is refused verbatim: never moved, never
//!   marked corrupt, and `save` refuses to overwrite it;
//! - recovery itself never writes: recovered data only reaches disk when the
//!   user actually changes something and a normal save runs;
//! - a single flock guards against two deck instances fighting over the
//!   same files (and double-firing the scheduler).

use serde::de::DeserializeOwned;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u64 = 1;

// ---------- private-by-construction file creation --------------------------------
//
// Everything under ~/.deck can carry user content (board titles, prompts,
// shell commands), so the entire tree is user-only: the directory 0700 and
// every file 0600 FROM CREATION — never "create world-readable, chmod later"
// (that window is a real race). Renames preserve the creation mode, so the
// atomic-write temp being 0600 makes main files and .bak files 0600 too.

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

/// Warnings produced before the webview exists (e.g. corrupt files found at
/// boot); the frontend fetches and toasts them via the `storage_warnings`
/// command. Request-path loads return their warning in-band instead.
pub static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The full note goes to the USER (toast via storage_warnings / in-band
/// warning); the log gets only a stable category code — notes can embed
/// serde detail and quarantine file names, which stay out of app.log.
pub fn warn(note: String) {
    applog(&format!("[storage] warning ({})", err_code(&note)));
    WARNINGS.lock().unwrap().push(note);
}

/// A successful load: the payload plus where it came from and, when it came
/// from the backup, a user-facing account of what happened to the original.
#[derive(Debug)]
pub struct LoadOutcome {
    pub payload: String,
    pub source: &'static str, // "main" | "backup"
    pub warning: Option<String>,
}

enum DocErr {
    /// written by a newer deck — leave the file alone, tell the user to update
    Newer(u64),
    /// unreadable or wrong business structure — recovery material
    Bad(String),
}

fn bak_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(".bak");
    PathBuf::from(os)
}

/// Full validation of one file's bytes: JSON → envelope (schema version) →
/// the document type `T`. Returns the payload serialized back to a string.
fn parse_doc<T: DeserializeOwned>(raw: &str) -> Result<String, DocErr> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| DocErr::Bad(format!("invalid JSON: {e}")))?;
    let payload = match (v.get("schema_version"), v.get("data")) {
        (Some(sv), Some(data)) => {
            let n = sv.as_u64().unwrap_or(0);
            if n > SCHEMA_VERSION {
                return Err(DocErr::Newer(n));
            }
            data.clone()
        }
        _ => v, // legacy v0: the whole document is the payload
    };
    let raw_payload = serde_json::to_string(&payload).map_err(|e| DocErr::Bad(e.to_string()))?;
    serde_json::from_str::<T>(&raw_payload)
        .map_err(|e| DocErr::Bad(format!("wrong structure: {e}")))?;
    Ok(raw_payload)
}

/// A quarantine path that is guaranteed not to exist yet.
fn unique_corrupt_path(path: &Path) -> PathBuf {
    let ts = now_epoch();
    let mut n = 0u32;
    loop {
        let ext = if n == 0 {
            format!("corrupt-{ts}")
        } else {
            format!("corrupt-{ts}-{n}")
        };
        let p = path.with_extension(ext);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Load and fully validate a data file as document type `T`.
/// `Ok(None)` = file does not exist (a genuine first run).
/// A bad main file is quarantined, then the `.bak` (same validation) is
/// tried; success carries a warning for the UI, failure is a hard error the
/// caller must surface — NOT to be treated as an empty first run.
pub fn load_typed<T: DeserializeOwned>(path: &Path) -> Result<Option<LoadOutcome>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let main = std::fs::read_to_string(path)
        .map_err(|e| DocErr::Bad(e.to_string()))
        .and_then(|raw| parse_doc::<T>(&raw));
    let main_err = match main {
        Ok(payload) => {
            return Ok(Some(LoadOutcome {
                payload,
                source: "main",
                warning: None,
            }))
        }
        Err(DocErr::Newer(n)) => {
            return Err(format!(
                "{name} was written by a newer deck (schema v{n}, this build reads v{SCHEMA_VERSION}) — update deck; the file was left untouched"
            ))
        }
        Err(DocErr::Bad(e)) => e,
    };
    // quarantine the damaged original FIRST — it is preserved, never clobbered
    let corrupt = unique_corrupt_path(path);
    let kept_at = match std::fs::rename(path, &corrupt) {
        Ok(()) => {
            // the damaged bytes may hold user content; a pre-migration 0644
            // mode would survive the rename, so restrict explicitly. Only
            // the file NAME is reported (it sits beside the original) — the
            // absolute path never enters warnings or logs.
            restrict_to_user(&corrupt);
            format!(
                " — the damaged file was kept as {}",
                corrupt
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )
        }
        Err(e) => format!(" (quarantining it also failed: {e})"),
    };
    let bak = bak_path(path);
    match std::fs::read_to_string(&bak)
        .map_err(|e| DocErr::Bad(e.to_string()))
        .and_then(|raw| parse_doc::<T>(&raw))
    {
        Ok(payload) => {
            let warning =
                format!("{name} was unreadable ({main_err}); recovered from its .bak backup{kept_at}");
            // detail goes to the caller; the log gets file name + category
            applog(&format!(
                "[storage] {name} recovered from backup ({})",
                err_code(&main_err)
            ));
            Ok(Some(LoadOutcome {
                payload,
                source: "backup",
                warning: Some(warning),
            }))
        }
        Err(DocErr::Newer(n)) => Err(format!(
            "{name} is unreadable ({main_err}) and its backup was written by a newer deck (schema v{n}) — update deck{kept_at}"
        )),
        Err(DocErr::Bad(bak_err)) => Err(format!(
            "{name} is unreadable ({main_err}) and its backup is unusable ({bak_err}){kept_at}"
        )),
    }
}

/// Write `bytes` to `path` atomically: unique same-directory temp file →
/// fsync → rename → fsync of the parent directory (so the rename itself
/// survives power loss). Unique temp names keep concurrent saves of the
/// same file from trampling each other's temp file.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
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

/// Atomically save `payload` (a JSON document) wrapped in the version
/// envelope, keeping the previous version as `.bak` (also written
/// atomically). Refuses to overwrite a file written by a newer deck.
pub fn save(path: &Path, payload: &str) -> Result<(), String> {
    let data: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("refusing to save invalid JSON: {e}"))?;
    if let Ok(existing) = std::fs::read_to_string(path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&existing) {
            let n = v
                .get("schema_version")
                .and_then(|s| s.as_u64())
                .unwrap_or(0);
            if n > SCHEMA_VERSION {
                return Err(format!(
                    "refusing to overwrite {} — it was written by a newer deck (schema v{n}); update deck first",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    let doc = serde_json::json!({ "schema_version": SCHEMA_VERSION, "data": data });
    let out = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;

    let dir = path.parent().ok_or("data path has no parent directory")?;
    create_private_dir(dir)?;
    if let Ok(cur) = std::fs::read(path) {
        atomic_write(&bak_path(path), &cur).map_err(|e| format!("backup failed: {e}"))?;
    }
    atomic_write(path, out.as_bytes())
}

/// Hold an exclusive advisory lock for the app's lifetime. A second deck
/// instance would double-fire scheduled prompts and race every data file,
/// so it must not start.
pub fn acquire_instance_lock(dir: &Path) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    create_private_dir(dir)?;
    let f = open_private(&dir.join("deck.lock")).map_err(|e| e.to_string())?;
    // LOCK_EX | LOCK_NB
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err("another deck instance is already running".into());
    }
    std::mem::forget(f); // keep the fd (and the lock) until the process exits
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

// ---------- logging -------------------------------------------------------------

/// Stable, content-free category for an error message. The FULL error text
/// goes back to the current operation's caller (command result → UI toast);
/// the log gets only this code — raw io/tmux/storage/serde Display texts can
/// embed absolute paths, directories or file contents and must never be
/// interpolated into app.log.
pub(crate) fn err_code(e: &str) -> &'static str {
    let l = e.to_ascii_lowercase();
    if l.contains("permission denied") || l.contains("read-only") {
        "perm"
    } else if l.contains("already running") {
        "locked"
    } else if l.contains("not a directory") {
        "not-dir"
    } else if l.contains("tmux not runnable") {
        "tmux-missing"
    } else if l.contains("no such file") || l.contains("not found") || l.contains("no such path") {
        "missing"
    } else if l.contains("no space") || l.contains("quota") {
        "disk-full"
    } else if l.contains("newer deck") {
        "newer-schema"
    } else if l.contains("invalid json")
        || l.contains("wrong structure")
        || l.contains("refusing to save")
        || l.contains("expected")
    {
        "invalid-doc"
    } else if l.contains("no server")
        || l.contains("can't find session")
        || l.contains("can't find pane")
        || l.contains("no such session")
    {
        "no-session"
    } else if l.contains("tmux") {
        "tmux"
    } else if l.contains("unreadable") || l.contains("backup") || l.contains("corrupt") {
        "recovery"
    } else {
        "other"
    }
}

/// Append a line to ~/.deck/app.log (the app may be launched via `open`,
/// where stderr goes nowhere useful). The log is chmod 0600: it must never
/// carry user content, but even metadata stays private to the user.
pub(crate) fn applog(msg: &str) {
    if cfg!(test) {
        return; // unit tests must not write into the user's real app.log
    }
    use std::os::unix::fs::OpenOptionsExt;
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("app.log");
    if let Some(dir) = path.parent() {
        let _ = create_private_dir(dir);
    }
    let ts = now_epoch();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600) // applies at creation; legacy 0644 logs are fixed at boot
        .open(&path)
    {
        let _ = writeln!(f, "{ts} {msg}");
    }
}

/// chmod 0600 — best-effort, silent (fixes pre-existing lax modes; new files
/// are already created 0600 via open_private/atomic_write).
pub(crate) fn restrict_to_user(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

pub(crate) fn rotate_log() {
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("app.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2 * 1024 * 1024 {
            if let Ok(data) = std::fs::read(&path) {
                let keep = &data[data.len().saturating_sub(512 * 1024)..];
                let _ = write_private(&path, keep);
            }
        }
    }
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

    #[derive(serde::Deserialize)]
    struct Doc {
        #[allow(dead_code)]
        v: u64,
    }

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deck-storage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn load_doc(p: &Path) -> Result<Option<LoadOutcome>, String> {
        load_typed::<Doc>(p)
    }

    #[test]
    fn roundtrip_and_envelope() {
        let d = tdir("rt");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("schema_version"), "file carries the envelope");
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "main");
        assert!(got.warning.is_none());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&got.payload).unwrap(),
            serde_json::json!({"v": 1})
        );
    }

    #[test]
    fn legacy_versionless_file_reads_as_payload() {
        let d = tdir("legacy");
        let p = d.join("x.json");
        std::fs::write(&p, r#"{"v":7}"#).unwrap();
        let got = load_doc(&p).unwrap().unwrap();
        assert!(got.payload.contains("\"v\""));
        // next save upgrades in place
        save(&p, &got.payload).unwrap();
        assert!(std::fs::read_to_string(&p)
            .unwrap()
            .contains("schema_version"));
    }

    #[test]
    fn corrupt_main_recovers_from_bak_and_is_quarantined() {
        let d = tdir("bak");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap(); // .bak now holds v1
        std::fs::write(&p, "{garbage").unwrap();
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "backup");
        assert!(
            got.payload.contains("\"v\":1"),
            "recovered v1: {}",
            got.payload
        );
        let w = got.warning.unwrap();
        assert!(w.contains("recovered") && w.contains("corrupt-"), "{w}");
        // the damaged original is preserved under a unique quarantine name…
        assert!(!p.exists(), "main was moved aside, not overwritten");
        let kept = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .count();
        assert_eq!(kept, 1);
        // …and recovery itself wrote NOTHING: only a real save recreates main
        save(&p, &got.payload).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn valid_json_wrong_structure_goes_through_recovery() {
        let d = tdir("shape");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap();
        // valid JSON + valid envelope, but Doc requires {"v": number} —
        // save() doesn't type-check (the frontend owns some shapes), load must
        save(&p, r#"{"projects":"nope"}"#).unwrap(); // .bak now holds v2
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "backup");
        assert!(got.payload.contains("\"v\":2"), "{}", got.payload);
        assert!(got.warning.unwrap().contains("wrong structure"));
    }

    #[test]
    fn both_corrupt_is_a_hard_error_not_a_first_run() {
        let d = tdir("corrupt");
        let p = d.join("x.json");
        std::fs::write(&p, "{garbage").unwrap();
        std::fs::write(bak_path(&p), "{worse").unwrap();
        let err = load_doc(&p).unwrap_err();
        assert!(
            err.contains("unreadable") && err.contains("backup is unusable"),
            "{err}"
        );
        assert!(
            err.contains("corrupt-"),
            "tells the user where the bytes are: {err}"
        );
        assert!(!p.exists(), "main moved aside");
    }

    #[test]
    fn missing_file_is_the_only_first_run_signal() {
        let d = tdir("first");
        assert!(load_doc(&d.join("x.json")).unwrap().is_none());
    }

    #[test]
    fn newer_schema_is_refused_untouched_and_save_wont_overwrite() {
        let d = tdir("newer");
        let p = d.join("x.json");
        std::fs::write(&p, r#"{"schema_version": 99, "data": {"v":1}}"#).unwrap();
        let err = load_doc(&p).unwrap_err();
        assert!(err.contains("newer deck"), "{err}");
        assert!(p.exists(), "file left in place");
        assert!(
            !std::fs::read_dir(&d).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("corrupt-")),
            "never marked corrupt"
        );
        let err = save(&p, r#"{"v":2}"#).unwrap_err();
        assert!(err.contains("newer deck"), "save refuses too: {err}");
        assert!(std::fs::read_to_string(&p).unwrap().contains("99"));
    }

    #[test]
    fn invalid_payload_is_refused_before_touching_disk() {
        let d = tdir("invalid");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        assert!(save(&p, "{not json").is_err());
        assert!(load_doc(&p).unwrap().unwrap().payload.contains("\"v\""));
    }

    #[test]
    fn concurrent_saves_use_unique_temp_files_and_leave_a_valid_file() {
        let d = tdir("race");
        let p = d.join("x.json");
        std::thread::scope(|s| {
            for k in 0..4 {
                let p = p.clone();
                s.spawn(move || {
                    for i in 0..25 {
                        save(&p, &format!("{{\"v\":{}}}", k * 100 + i)).unwrap();
                    }
                });
            }
        });
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "main", "no torn writes: {}", got.payload);
        // no temp litter left behind
        assert!(
            !std::fs::read_dir(&d).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp.")),
            "temp files all consumed"
        );
    }

    // ---------- permissions: behavioral checks against real fs metadata ----------

    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn every_saved_artifact_is_user_only() {
        let d = tdir("perm");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap(); // first write
        assert_eq!(mode_of(&p), 0o600, "main file");
        assert_eq!(mode_of(&d), 0o700, "data dir");
        save(&p, r#"{"v":2}"#).unwrap(); // second write creates the backup
        assert_eq!(mode_of(&p), 0o600, "main after rewrite");
        assert_eq!(mode_of(&bak_path(&p)), 0o600, "backup file");
        // no temp litter, so no temp modes to check — creation itself is 0600
        assert!(!std::fs::read_dir(&d).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp.")));
    }

    #[test]
    fn quarantined_corrupt_file_is_user_only() {
        let d = tdir("permq");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap();
        // a damaged main file left world-readable by an older deck
        std::fs::write(&p, "{garbage").unwrap();
        set_mode(&p, 0o644);
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "backup");
        let corrupt = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .expect("quarantine file exists")
            .path();
        assert_eq!(mode_of(&corrupt), 0o600, "quarantine restricted");
    }

    #[test]
    fn recovery_warning_names_files_but_never_paths() {
        let d = tdir("noleak");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap();
        std::fs::write(&p, "{garbage").unwrap();
        let w = load_doc(&p).unwrap().unwrap().warning.unwrap();
        assert!(
            w.contains("x.corrupt-"),
            "quarantine stays discoverable: {w}"
        );
        assert!(
            !w.contains(d.to_str().unwrap()),
            "no absolute path in the user-facing warning: {w}"
        );
    }

    #[test]
    fn concurrent_saves_stay_user_only() {
        let d = tdir("permrace");
        let p = d.join("x.json");
        std::thread::scope(|s| {
            for k in 0..4 {
                let p = p.clone();
                s.spawn(move || {
                    for i in 0..15 {
                        save(&p, &format!("{{\"v\":{}}}", k * 100 + i)).unwrap();
                    }
                });
            }
        });
        assert_eq!(mode_of(&p), 0o600);
        assert_eq!(mode_of(&bak_path(&p)), 0o600);
        assert_eq!(mode_of(&d), 0o700);
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

    #[test]
    fn second_instance_lock_is_refused() {
        let d = tdir("lock");
        acquire_instance_lock(&d).unwrap();
        assert_eq!(mode_of(&d.join("deck.lock")), 0o600, "lock file private");
        // same-process flock on a fresh fd of the same file: macOS grants it
        // (locks are per-open-file but merge per process), so exercise the
        // failure path from a child process instead.
        let out = std::process::Command::new("python3")
            .arg("-c")
            .arg(format!(
                "import fcntl,sys;f=open('{}','w')\ntry:\n fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);print('got')\nexcept OSError:\n print('refused')",
                d.join("deck.lock").display()
            ))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("refused"),
            "child process must not obtain the lock"
        );
    }
}
