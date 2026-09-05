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
//!
//! The pieces this used to own live beside it: `datadir.rs` (private
//! directory, 0600/0700 creation, atomic writes), `applog.rs` (the log),
//! `redact.rs` (the sanitizer), `instance_lock.rs`, `launch_args.rs`.
//!
//! # Contract
//! Board persistence: `~/.deck/deck.json` (frontend owns the state). EVERY
//! mutation enters one global persist-before-commit transaction queue and builds
//! its candidate from the latest committed Board only when it reaches the head;
//! debounced mutations enter that same queue before an immediate-operation
//! barrier. A rejected mutation or failed write cannot poison the following
//! transaction, resurrect a removed card, or overwrite a concurrent rename/move.
//! Runtime-only card fields are merged from the newest live state at commit.
//! `storage.rs` is TYPED and durable for every persistent JSON document
//! (deck/queue/history/settings and per-session shell snapshots): JSON + version envelope + business-structure
//! validation on load — BoardDoc/SettingsDoc validate via `try_from`
//! (referential rules: unique ids, cards reference an existing project and a
//! column of that project, ≥1 column per project, runtime fields present,
//! session names by the same tmux rule the runtime enforces), and
//! save_board/save_settings run the SAME validation before touching disk;
//! unknown extension fields round-trip untouched. Damaged main quarantined to
//! a unique `.corrupt-<ts>`
//! BEFORE the fully-validated `.bak` is tried; recovery warnings returned
//! in-band (`LoadedDoc {data, source, warning}`); future schema versions
//! refused untouched (save refuses to overwrite them too); recovery never
//! writes; a load FAILURE is surfaced, never treated as a first run — the UI
//! must never auto-save defaults over an existing file. Writes: unique temp +
//! fsync + rename + parent-dir fsync, `.bak` written the same way. The
//! envelope is validated STRICTLY: only a document carrying neither
//! `schema_version` nor `data` is legacy v0; once either appears the file
//! must be a COMPLETE envelope with a non-negative INTEGER version (string /
//! fractional / negative / null version, or a version without data, is
//! damage → recovery), and `save` refuses to overwrite a malformed or future
//! envelope.

use crate::applog::applog;
use crate::datadir::{atomic_write, create_private_dir, now_epoch, restrict_to_user};
use crate::error::err_code;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SCHEMA_VERSION: u64 = 1;
static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// Warnings produced before the webview exists (e.g. corrupt files found at
/// boot); the frontend fetches and toasts them via the `storage_warnings`
/// command. Request-path loads return their warning in-band instead.
pub static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The full note goes to the USER (toast via storage_warnings / in-band
/// warning); the log gets only a stable category code — notes can embed
/// serde detail and quarantine file names, which stay out of app.log.
pub fn warn(note: String) {
    applog(&format!("[storage] warning ({})", err_code(&note)));
    WARNINGS.lock_or_recover().push(note);
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

/// Envelope check, strictly: a document is legacy v0 ONLY when it carries
/// neither marker. The moment `schema_version` or `data` appears, the file
/// claims to be enveloped and must be a COMPLETE, well-formed envelope —
/// a string/float/negative/null version, or a version without data, is a
/// damaged file (recovery material), never "probably v0". Reading a half
/// envelope as v0 would hand the caller the wrapper object as if it were
/// the payload, and let `save` overwrite a file it never understood.
fn envelope_payload(v: &serde_json::Value) -> Result<serde_json::Value, DocErr> {
    match (v.get("schema_version"), v.get("data")) {
        (None, None) => Ok(v.clone()), // legacy v0: the whole document
        (Some(sv), data) => {
            let n = sv.as_u64().ok_or_else(|| {
                DocErr::Bad(format!(
                    "schema_version must be a non-negative integer, found {}",
                    type_name_of(sv)
                ))
            })?;
            if n > SCHEMA_VERSION {
                return Err(DocErr::Newer(n));
            }
            data.cloned()
                .ok_or_else(|| DocErr::Bad("version envelope has no data field".into()))
        }
        (None, Some(_)) => Err(DocErr::Bad(
            "version envelope has no schema_version field".into(),
        )),
    }
}

/// JSON type name for an envelope diagnostic (never the VALUE — a data file
/// can hold user content, and this text reaches the user-facing warning).
fn type_name_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(n) if n.is_f64() => "a fractional number",
        serde_json::Value::Number(_) => "a negative number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Full validation of one file's bytes: JSON → envelope (schema version) →
/// the document type `T`. Returns the payload serialized back to a string.
fn parse_doc<T: DeserializeOwned>(raw: &str) -> Result<String, DocErr> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| DocErr::Bad(format!("invalid JSON: {e}")))?;
    let payload = envelope_payload(&v)?;
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
pub fn load_typed<T: DeserializeOwned>(path: &Path) -> Result<Option<LoadOutcome>, DeckError> {
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
            return Err(DeckError::new(
                ErrorKind::NewerSchema,
                format!(
                "{name} was written by a newer deck (schema v{n}, this build reads v{SCHEMA_VERSION}) — update deck; the file was left untouched"
            )))
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
        Err(DocErr::Newer(n)) => Err(DeckError::new(
                ErrorKind::NewerSchema,
                format!(
            "{name} is unreadable ({main_err}) and its backup was written by a newer deck (schema v{n}) — update deck{kept_at}"
        ))),
        Err(DocErr::Bad(bak_err)) => Err(DeckError::new(
            ErrorKind::Recovery,
            format!("{name} is unreadable ({main_err}) and its backup is unusable ({bak_err}){kept_at}"),
        )),
    }
}

/// Atomically save `payload` (a JSON document) wrapped in the version
/// envelope, keeping the previous version as `.bak` (also written
/// atomically). Refuses to overwrite a file written by a newer deck.
fn save_checked(
    path: &Path,
    payload: &str,
    keep_backup: bool,
    validate_existing: impl Fn(&serde_json::Value) -> Result<(), DeckError>,
) -> Result<(), DeckError> {
    // Scheduler workers and UI commands can save concurrently. Serialize the
    // validate → backup → replace sequence so one writer cannot validate
    // bytes another writer replaces before its backup is taken.
    let _save_guard = SAVE_LOCK.lock_or_recover();
    let data: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save invalid JSON: {e}")))?;
    // Never clobber a file this build does not understand. `load_typed`
    // quarantines a damaged main file before anything can save over it, so
    // reaching here with a broken envelope means the file was never loaded
    // (or was replaced behind our back) — refuse rather than destroy it.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let existing = match std::fs::read(path) {
        Ok(bytes) => {
            let raw = std::str::from_utf8(&bytes).map_err(|_| {
                DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("refusing to overwrite {name} — the existing file is not valid UTF-8"),
                )
            })?;
            let v = serde_json::from_str::<serde_json::Value>(raw).map_err(|_| {
                DeckError::new(
                    ErrorKind::InvalidDoc,
                    format!("refusing to overwrite {name} — the existing file is invalid JSON"),
                )
            })?;
            match envelope_payload(&v) {
                Ok(existing_payload) => validate_existing(&existing_payload).map_err(|e| {
                    DeckError::new(
                        ErrorKind::InvalidDoc,
                        format!("refusing to overwrite {name} — the existing file has the wrong structure ({e})"),
                    )
                })?,
                Err(DocErr::Newer(n)) => {
                    return Err(DeckError::new(
                ErrorKind::NewerSchema,
                format!(
                        "refusing to overwrite {name} — it was written by a newer deck (schema v{n}); update deck first"
                    )))
                }
                Err(DocErr::Bad(e)) => {
                    return Err(DeckError::new(
                        ErrorKind::Recovery,
                        format!("refusing to overwrite {name} — its version envelope is unreadable ({e}); move the file aside first"),
                    ))
                }
            }
            Some(bytes)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return Err(DeckError::new(
                ErrorKind::Other,
                format!("refusing to overwrite {name} — the existing file could not be read"),
            ))
        }
    };
    let doc = serde_json::json!({ "schema_version": SCHEMA_VERSION, "data": data });
    let out = serde_json::to_string_pretty(&doc).map_err(DeckError::from)?;

    let dir = path.parent().ok_or(DeckError::new(
        ErrorKind::Other,
        "data path has no parent directory",
    ))?;
    create_private_dir(dir)?;
    if keep_backup {
        if let Some(cur) = existing {
            atomic_write(&bak_path(path), &cur)
                .map_err(|e| DeckError::classified(format!("backup failed: {e}")))?;
        }
    } else {
        match std::fs::remove_file(bak_path(path)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DeckError::new(
                    ErrorKind::io(error.kind()),
                    format!("could not remove transient backup ({})", error.kind()),
                ))
            }
        }
    }
    atomic_write(path, out.as_bytes())
}

#[allow(dead_code)] // low-level envelope tests intentionally exercise this directly
pub fn save(path: &Path, payload: &str) -> Result<(), DeckError> {
    save_checked(path, payload, true, |_| Ok(()))
}

/// Typed save used by every app data file. It validates both the new payload
/// and any concurrently replaced existing payload under the same save lock,
/// so malformed business structure cannot be silently overwritten.
pub(crate) fn save_typed<T: DeserializeOwned>(path: &Path, payload: &str) -> Result<(), DeckError> {
    serde_json::from_str::<T>(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save wrong structure: {e}")))?;
    save_checked(path, payload, true, |existing| {
        serde_json::from_value::<T>(existing.clone())
            .map(|_| ())
            .map_err(DeckError::from)
    })
}

/// Typed atomic save for bounded, disposable privacy-sensitive state. It
/// retains all structure/future-schema checks but deliberately creates no
/// `.bak`, and removes a legacy backup before replacing the main file.
pub(crate) fn save_typed_ephemeral<T: DeserializeOwned>(
    path: &Path,
    payload: &str,
) -> Result<(), DeckError> {
    serde_json::from_str::<T>(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save wrong structure: {e}")))?;
    save_checked(path, payload, false, |existing| {
        serde_json::from_value::<T>(existing.clone())
            .map(|_| ())
            .map_err(DeckError::from)
    })
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

    fn load_doc(p: &Path) -> Result<Option<LoadOutcome>, DeckError> {
        load_typed::<Doc>(p)
    }

    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[derive(serde::Deserialize)]
    struct Doc {
        #[allow(dead_code)]
        v: u64,
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
            err.message().contains("unreadable") && err.message().contains("backup is unusable"),
            "{err}"
        );
        assert!(
            err.message().contains("corrupt-"),
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
        assert!(err.message().contains("newer deck"), "{err}");
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
        assert!(
            err.message().contains("newer deck"),
            "save refuses too: {err}"
        );
        assert!(std::fs::read_to_string(&p).unwrap().contains("99"));
    }

    /// Anything that CLAIMS to be enveloped must be a complete envelope.
    /// The old rule (`as_u64().unwrap_or(0)`) read every one of these as a
    /// legacy v0 document and handed the WRAPPER back as the payload.
    #[test]
    fn a_half_or_mistyped_envelope_is_damage_not_a_legacy_file() {
        let d = tdir("envelope");
        for (k, doc) in [
            r#"{"schema_version":"99","data":{"v":1}}"#, // string version
            r#"{"schema_version":1.5,"data":{"v":1}}"#,  // fractional
            r#"{"schema_version":-1,"data":{"v":1}}"#,   // negative
            r#"{"schema_version":null,"data":{"v":1}}"#, // null
            r#"{"schema_version":true,"data":{"v":1}}"#, // boolean
            r#"{"schema_version":1}"#,                   // version, no data
            r#"{"data":{"v":1}}"#,                       // data, no version
        ]
        .iter()
        .enumerate()
        {
            let p = d.join(format!("x{k}.json"));
            std::fs::write(&p, doc).unwrap();
            let err = load_doc(&p).unwrap_err();
            assert!(
                err.message().contains("unreadable")
                    && err.message().contains("backup is unusable"),
                "{doc} → {err}"
            );
            assert!(!p.exists(), "{doc}: damaged main file was quarantined");
            // …and the quarantined bytes are exactly the original
            let kept = std::fs::read_dir(&d)
                .unwrap()
                .flatten()
                .find(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("x{k}.corrupt-"))
                })
                .expect("quarantine file")
                .path();
            assert_eq!(&std::fs::read_to_string(kept).unwrap(), doc);
        }
    }

    #[test]
    fn a_valid_envelope_and_a_valid_legacy_file_both_load() {
        let d = tdir("envelope-ok");
        let legacy = d.join("legacy.json");
        std::fs::write(&legacy, r#"{"v":7}"#).unwrap(); // no markers at all
        assert!(load_doc(&legacy)
            .unwrap()
            .unwrap()
            .payload
            .contains("\"v\""));
        let current = d.join("current.json");
        std::fs::write(&current, r#"{"schema_version":1,"data":{"v":8}}"#).unwrap();
        let got = load_doc(&current).unwrap().unwrap();
        assert_eq!(got.source, "main");
        assert!(got.payload.contains("\"v\":8"), "{}", got.payload);
        // v0 spelled out explicitly is still valid
        let zero = d.join("zero.json");
        std::fs::write(&zero, r#"{"schema_version":0,"data":{"v":9}}"#).unwrap();
        assert!(load_doc(&zero)
            .unwrap()
            .unwrap()
            .payload
            .contains("\"v\":9"));
    }

    #[test]
    fn a_malformed_envelope_recovers_from_a_valid_backup() {
        let d = tdir("envelope-bak");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap(); // .bak now holds a valid v1 envelope
        std::fs::write(&p, r#"{"schema_version":"99","data":{"v":3}}"#).unwrap();
        let got = load_doc(&p).unwrap().unwrap();
        assert_eq!(got.source, "backup");
        assert!(got.payload.contains("\"v\":1"), "{}", got.payload);
        assert!(got.warning.unwrap().contains("schema_version must be"));
    }

    #[test]
    fn a_backup_with_a_malformed_envelope_is_refused_too() {
        let d = tdir("envelope-bakbad");
        let p = d.join("x.json");
        std::fs::write(&p, r#"{"schema_version":1}"#).unwrap();
        std::fs::write(bak_path(&p), r#"{"data":{"v":1}}"#).unwrap();
        let err = load_doc(&p).unwrap_err();
        assert!(err.message().contains("backup is unusable"), "{err}");
        assert!(err.message().contains("no schema_version field"), "{err}");
    }

    #[test]
    fn save_refuses_a_file_whose_envelope_it_cannot_read() {
        let d = tdir("envelope-save");
        for doc in [
            r#"{"schema_version":"99","data":{"v":1}}"#,
            r#"{"schema_version":1.5,"data":{"v":1}}"#,
            r#"{"schema_version":1}"#,
            r#"{"data":{"v":1}}"#,
        ] {
            let p = d.join("x.json");
            std::fs::write(&p, doc).unwrap();
            let bak = bak_path(&p);
            std::fs::write(&bak, doc).unwrap();
            let err = save(&p, r#"{"v":2}"#).unwrap_err();
            assert!(
                err.message().contains("refusing to overwrite"),
                "{doc} → {err}"
            );
            assert_eq!(std::fs::read_to_string(&p).unwrap(), doc, "main untouched");
            assert_eq!(
                std::fs::read_to_string(&bak).unwrap(),
                doc,
                "backup not rotated"
            );
        }
    }

    #[test]
    fn save_refuses_invalid_json_main_without_poisoning_valid_backup() {
        let d = tdir("invalid-main-save");
        let p = d.join("x.json");
        let bak = bak_path(&p);
        let good_backup = r#"{"schema_version":1,"data":{"v":7}}"#;
        std::fs::write(&p, b"{broken main").unwrap();
        std::fs::write(&bak, good_backup).unwrap();
        let err = save(&p, r#"{"v":8}"#).unwrap_err();
        assert!(
            err.message().contains("refusing to overwrite")
                && err.message().contains("invalid JSON")
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"{broken main");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), good_backup);
    }

    #[test]
    fn typed_save_refuses_wrong_existing_structure_and_preserves_backup() {
        let d = tdir("wrong-structure-save");
        let p = d.join("x.json");
        let bak = bak_path(&p);
        let malformed = r#"{"schema_version":1,"data":{"v":"not-a-number"}}"#;
        let good_backup = r#"{"schema_version":1,"data":{"v":7}}"#;
        std::fs::write(&p, malformed).unwrap();
        std::fs::write(&bak, good_backup).unwrap();
        let err = save_typed::<Doc>(&p, r#"{"v":8}"#).unwrap_err();
        assert!(err.message().contains("wrong structure"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), malformed);
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), good_backup);
    }

    #[test]
    fn unreadable_or_wrong_kind_main_is_never_treated_as_missing() {
        let d = tdir("wrong-kind-save");
        let p = d.join("x.json");
        std::fs::create_dir(&p).unwrap();
        let err = save(&p, r#"{"v":1}"#).unwrap_err();
        assert!(err.message().contains("refusing to overwrite"));
        assert!(p.is_dir(), "existing main object was untouched");
    }

    #[test]
    fn failed_atomic_main_replace_leaves_no_temp_and_preserves_target() {
        let d = tdir("main-write-fail");
        let target = d.join("x.json");
        std::fs::create_dir(&target).unwrap(); // rename(temp, non-empty-dir) must fail
        std::fs::write(target.join("keep"), b"sentinel").unwrap();
        assert!(atomic_write(&target, b"replacement").is_err());
        assert_eq!(std::fs::read(target.join("keep")).unwrap(), b"sentinel");
        assert!(
            !std::fs::read_dir(&d)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains(".tmp.")),
            "failed main write cleaned its private temp"
        );
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
}
