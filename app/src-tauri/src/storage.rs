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

/// Warnings produced before the webview exists (e.g. corrupt files found at
/// boot); the frontend fetches and toasts them via the `storage_warnings`
/// command. Request-path loads return their warning in-band instead.
pub static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn warn(msg: String) {
    applog(&format!("[storage] {msg}"));
    WARNINGS.lock().unwrap().push(msg);
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
        Ok(()) => format!(" — the damaged file was kept at {}", corrupt.display()),
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
            applog(&format!("[storage] {warning}"));
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
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
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
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
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
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let f = std::fs::File::create(dir.join("deck.lock")).map_err(|e| e.to_string())?;
    // LOCK_EX | LOCK_NB
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err("another deck instance is already running".into());
    }
    std::mem::forget(f); // keep the fd (and the lock) until the process exits
    Ok(())
}

// ---------- logging -------------------------------------------------------------

/// Append a line to ~/.deck/app.log (the app may be launched via `open`,
/// where stderr goes nowhere useful). The log is chmod 0600: it must never
/// carry user content, but even metadata stays private to the user.
pub(crate) fn applog(msg: &str) {
    if cfg!(test) {
        return; // unit tests must not write into the user's real app.log
    }
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("app.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = now_epoch();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{ts} {msg}");
        restrict_to_user(&path);
    }
}

/// chmod 0600 — best-effort, silent (used on logs, exports and history).
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
                let _ = std::fs::write(&path, keep);
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

    #[test]
    fn second_instance_lock_is_refused() {
        let d = tdir("lock");
        acquire_instance_lock(&d).unwrap();
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
