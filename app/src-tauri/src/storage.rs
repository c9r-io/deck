//! One reliable persistence layer for every deck data file
//! (deck.json / queue.json / history.json / settings.json).
//!
//! Guarantees:
//! - every save is atomic: temp file in the same directory → fsync → rename;
//! - the previous good version is kept as `<file>.bak` before each save;
//! - files carry `{"schema_version": N, "data": …}`; legacy version-less
//!   files are read as v0 and upgraded in place on their next save;
//! - a corrupt file is NEVER overwritten with defaults: we fall back to the
//!   `.bak`, and if that is also unreadable the corrupt original is set
//!   aside as `<file>.corrupt-<ts>` and the caller is told (surface to UI);
//! - a single flock guards against two deck instances fighting over the
//!   same files (and double-firing the scheduler).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SCHEMA_VERSION: u64 = 1;

/// Warnings produced before the webview exists (e.g. corrupt files found at
/// boot); the frontend fetches and toasts them via the `storage_warnings`
/// command.
pub static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn warn(msg: String) {
    crate::applog(&format!("[storage] {msg}"));
    WARNINGS.lock().unwrap().push(msg);
}

fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn bak_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(".bak");
    PathBuf::from(os)
}

/// Unwrap `{"schema_version": N, "data": …}`; a version-less document IS the
/// data (legacy v0). Returns the payload serialized back to a string.
fn unwrap_envelope(raw: &str) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    let payload = match (v.get("schema_version"), v.get("data")) {
        (Some(sv), Some(data)) => {
            let n = sv.as_u64().unwrap_or(0);
            if n > SCHEMA_VERSION {
                return Err(format!(
                    "written by a newer deck (schema v{n}, this build reads v{SCHEMA_VERSION})"
                ));
            }
            data.clone()
        }
        _ => v, // legacy v0: the whole document is the payload
    };
    serde_json::to_string(&payload).map_err(|e| e.to_string())
}

/// Load the payload of a data file. `Ok(None)` = file does not exist.
/// Corruption falls back to `.bak`; if both are unreadable the original is
/// renamed to `<file>.corrupt-<ts>` (never clobbered) and an error returns.
pub fn load(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|raw| unwrap_envelope(&raw))
    {
        Ok(payload) => Ok(Some(payload)),
        Err(main_err) => {
            let bak = bak_path(path);
            if let Ok(payload) = std::fs::read_to_string(&bak)
                .map_err(|e| e.to_string())
                .and_then(|raw| unwrap_envelope(&raw))
            {
                warn(format!(
                    "{name} was unreadable ({main_err}); recovered from its .bak backup"
                ));
                return Ok(Some(payload));
            }
            let corrupt = path.with_extension(format!("corrupt-{}", now_epoch()));
            let _ = std::fs::rename(path, &corrupt);
            Err(format!(
                "{name} is unreadable ({main_err}) and has no usable backup; \
                 the damaged file was kept at {}",
                corrupt.display()
            ))
        }
    }
}

/// Atomically save `payload` (a JSON document) wrapped in the version
/// envelope, keeping the previous version as `.bak`.
pub fn save(path: &Path, payload: &str) -> Result<(), String> {
    let data: serde_json::Value =
        serde_json::from_str(payload).map_err(|e| format!("refusing to save invalid JSON: {e}"))?;
    let doc = serde_json::json!({ "schema_version": SCHEMA_VERSION, "data": data });
    let out = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;

    let dir = path.parent().ok_or("data path has no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if path.exists() {
        std::fs::copy(path, bak_path(path)).map_err(|e| format!("backup failed: {e}"))?;
    }
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(out.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deck-storage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_and_envelope() {
        let d = tdir("rt");
        let p = d.join("x.json");
        save(&p, r#"{"a":1}"#).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("schema_version"), "file carries the envelope");
        let got = load(&p).unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&got).unwrap(),
            serde_json::json!({"a": 1})
        );
    }

    #[test]
    fn legacy_versionless_file_reads_as_payload() {
        let d = tdir("legacy");
        let p = d.join("x.json");
        std::fs::write(&p, r#"{"items":[1,2]}"#).unwrap();
        let got = load(&p).unwrap().unwrap();
        assert!(got.contains("items"));
        // next save upgrades in place
        save(&p, &got).unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().contains("schema_version"));
    }

    #[test]
    fn save_keeps_a_bak_and_corrupt_main_recovers_from_it() {
        let d = tdir("bak");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap(); // .bak now holds v1
        std::fs::write(&p, "{garbage").unwrap();
        let got = load(&p).unwrap().unwrap();
        assert!(got.contains("\"v\":1") || got.contains("\"v\": 1"), "recovered from .bak: {got}");
    }

    #[test]
    fn corrupt_without_backup_is_set_aside_never_clobbered() {
        let d = tdir("corrupt");
        let p = d.join("x.json");
        std::fs::write(&p, "{garbage").unwrap();
        let err = load(&p).unwrap_err();
        assert!(err.contains("unreadable"), "{err}");
        assert!(!p.exists(), "main moved aside");
        let kept = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("corrupt-"));
        assert!(kept, "damaged bytes preserved as .corrupt-<ts>");
    }

    #[test]
    fn newer_schema_is_refused_not_destroyed() {
        let d = tdir("newer");
        let p = d.join("x.json");
        std::fs::write(&p, r#"{"schema_version": 99, "data": {}}"#).unwrap();
        let err = load(&p).unwrap_err();
        assert!(err.contains("newer deck"), "{err}");
    }

    #[test]
    fn invalid_payload_is_refused_before_touching_disk() {
        let d = tdir("invalid");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        assert!(save(&p, "{not json").is_err());
        assert!(load(&p).unwrap().unwrap().contains("\"v\""));
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
