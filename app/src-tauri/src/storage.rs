//! One reliable persistence layer for every deck data file
//! (deck.json / queue.json / history.json / settings.json).
//!
//! Guarantees:
//! - every write is atomic: unique temp file in the same directory → fsync →
//!   rename → parent-directory fsync (both the main file and its `.bak`);
//! - the previous good version is kept as `<file>.bak` before each save;
//! - files carry `{"schema_version": N, "data": …}`; legacy version-less
//!   files are read as v0 and upgraded in place on their next save. The
//!   version written is the lowest one whose readers understand the
//!   content (`save_checked_locked`), and it never goes back down (sticky),
//!   so an older deck refuses the file untouched instead of misreading it:
//!   v1 ordinary data; v2 queue.json and settings.json that use inspection
//!   checkpoints (a card's `origin.reviewEach` alone does not lock the whole
//!   Board); v3 deck.json, queue.json or settings.json holding a card
//!   scratchpad that is collecting or has entries, a channel or Connector
//!   run, a frozen inbound plan, a task preset, a channel rule, an enabled
//!   channel connection or an idempotent queue operation; v4 queue.json and
//!   settings.json that use a clock first send; v5 a Board that holds a
//!   card reminder or a blocked retirement identity. A settings v2 barrier
//!   precedes the first reviewed queue save, so an old automation finish
//!   rule never takes a refused queue for an empty one;
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
//! - only a file's OWNER moves it. The owner is the code that loads the
//!   document into memory and saves it back; `load_typed` is its door. A
//!   file that others also read (settings.json: the scheduler's authority
//!   check, the pollers, the notification, locale and translation switches)
//!   gives those readers `read_typed`, which never renames or writes: a main
//!   file with damaged content is answered from the validated `.bak`, a main
//!   file that cannot be READ is an error meaning "unknown" (not damage, no
//!   fallback), and a main file an earlier recovery set aside is answered
//!   from the `.bak` until the owner's next save puts it back
//!   (`was_quarantined`). Its owner loads with `load_as_owner`, so the same
//!   state is not a first run on the next start either and the warning is
//!   produced once, and saves with `save_typed_as_owner`, which sets aside a
//!   main file damaged while deck runs instead of refusing the save. The one
//!   backend writer of settings.json, `ensure_review_schema`, loads the same
//!   way, leaves a main file it merely cannot read alone, and never writes a
//!   recovered document as the main file;
//! - a `.bak` is the version BEFORE the last save, not the last save.
//!   `LoadOutcome::source` says which one answered, and it stays "backup"
//!   until the owner saves: this layer reports provenance and decides
//!   nothing with it. A reader for which the previous version must not stand
//!   in for the current one checks it (`inbound::read_config_strict`);
//! - nothing removes a copy a recovery set aside: `kept_copies` lists them,
//!   newest first, and `newest_valid_copy` reads them without moving one,
//!   skipping a copy it read and found unusable and stopping at one it could
//!   not read (the Board's way out of a load that left nothing loadable,
//!   `documents.rs`);
//! - a single flock guards against two deck instances fighting over the
//!   same files (and double-firing the scheduler).
//!
//! The pieces this used to own live beside it: `datadir.rs` (private
//! directory, 0600/0700 creation, atomic writes), `applog.rs` (the log),
//! `redact.rs` (the sanitizer), `instance_lock.rs`, `launch_args.rs`.
//!
//! # Contract
//! `~/.deck/deck.json` is owned by the frontend; the persist-before-commit
//! Board transaction queue every mutation goes through is documented in
//! `ui/js/persistence.js`, not here.
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

// v2 protects opt-in human checkpoints; v3 protects retained scratchpad and
// channel/idempotency fields; v4 protects clock readiness origins/policy;
// v5 protects card reminders and blocked retirement identities; v6 protects
// a phone task preset's first-send choice and approval and their queue
// origins.
// Ordinary documents keep v1; upgrades are sticky.
pub const SCHEMA_VERSION: u64 = 6;

/// The two documents whose review fields an old reader could misinterpret as
/// ordinary state (queue rows it would resend; finish rules it would apply).
fn review_gated(name: &str) -> bool {
    matches!(name, "queue.json" | "settings.json")
}

fn uses_review(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(o) => o.iter().any(|(k, v)| {
            ((k == "review_each" || k == "reviewEach") && v.as_bool() == Some(true))
                || (k == "review" && v.is_object())
                || ((k == "reviews" || k == "review_completed")
                    && v.as_array().is_some_and(|a| !a.is_empty()))
                || uses_review(v)
        }),
        serde_json::Value::Array(a) => a.iter().any(uses_review),
        _ => false,
    }
}

// Clock readiness introduces a closed persisted origin and settings that
// v3 readers reject. Refuse those documents on old builds, never quarantine.
fn uses_clock_first_send(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(o) => {
            (o.get("source").and_then(|v| v.as_str()) == Some("clock")
                && o.get("firstSendWithoutReadiness").and_then(|v| v.as_bool()) == Some(true))
                || o.get("readiness_override")
                    .is_some_and(|v| v["trigger"] == "clock")
                || o.values().any(uses_clock_first_send)
        }
        serde_json::Value::Array(a) => a.iter().any(uses_clock_first_send),
        _ => false,
    }
}

// A phone task preset's first-send choice and approval (deck.json) and the
// row origins they admit (queue.json: a row's override, a row's or a
// delivery record's authority). A v5 reader decodes the closed origins as
// damage, and its webview rebuilds presets from the fields it knows, so its
// next Board save would drop both without a word: it must refuse both files
// untouched.
fn uses_phone_task_policy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(o) => {
            o.get("presets")
                .and_then(|v| v.as_array())
                .is_some_and(|presets| {
                    presets
                        .iter()
                        .any(|p| p["firstSend"] == true || p["autoSend"].is_object())
                })
                || ["readiness_override", "authority"]
                    .iter()
                    .any(|key| o.get(*key).is_some_and(|v| v["trigger"] == "connector"))
                || o.values().any(uses_phone_task_policy)
        }
        serde_json::Value::Array(a) => a.iter().any(uses_phone_task_policy),
        _ => false,
    }
}

fn uses_buffer(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(o) => {
            o.get("buffer").is_some_and(|buffer| {
                buffer.get("collecting").and_then(|v| v.as_bool()) == Some(true)
                    || buffer
                        .get("entries")
                        .and_then(|v| v.as_array())
                        .is_some_and(|entries| !entries.is_empty())
            }) || o.get("channelRun").is_some_and(|run| run.is_object())
                || o.get("connectorRun").is_some_and(|run| run.is_object())
                || o.get("inboundPlan").is_some_and(|plan| plan.is_object())
                || o.get("presets")
                    .and_then(|v| v.as_array())
                    .is_some_and(|presets| !presets.is_empty())
                || o.get("channelRules")
                    .and_then(|v| v.as_array())
                    .is_some_and(|rules| !rules.is_empty())
                || o.get("channelConnection").is_some_and(|connection| {
                    connection.get("enabled").and_then(|v| v.as_bool()) == Some(true)
                })
                || o.get("operation_id").is_some_and(|v| v.is_string())
                || o.get("operations")
                    .and_then(|v| v.as_array())
                    .is_some_and(|operations| !operations.is_empty())
                || o.values().any(uses_buffer)
        }
        serde_json::Value::Array(a) => a.iter().any(uses_buffer),
        _ => false,
    }
}
static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// The settings fence: a settings.json write (`documents::save_settings`)
/// and a decision that must observe ONE settings version before an
/// irreversible side effect (the scheduler's automation-authority check,
/// held until the firing intent is persisted — `scheduler/authority.rs`)
/// serialize on it. So once a settings write that revokes an approval has
/// returned, no automatic send can still begin under the revoked grant.
/// Lock order: this fence, then the queue lock, then `SAVE_LOCK`; nothing
/// that holds the queue lock or `SAVE_LOCK` may take it.
static SETTINGS_FENCE: Mutex<()> = Mutex::new(());

pub(crate) fn settings_fence() -> std::sync::MutexGuard<'static, ()> {
    SETTINGS_FENCE.lock_or_recover()
}

/// Test probe: whether the fence is held right now (by anyone).
#[cfg(test)]
pub(crate) fn settings_fence_busy() -> bool {
    SETTINGS_FENCE.try_lock().is_err()
}

/// What a boot/storage notice is about: the closed codes the webview
/// translates (`translateNotice`). These are notice categories, not errors —
/// the emitter names one; nothing infers it from the note's wording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageNotice {
    /// ~/.deck permission hardening or log redaction did not complete.
    Privacy,
    /// queue.json could not be saved after an irreversible send.
    QueuePersist,
    /// queue.json could not be read at all.
    QueueLoad,
    /// the command history could not be read at all.
    HistoryLoad,
    /// deliveries interrupted by a crash await acknowledge or retry.
    QueueInterrupted,
    /// a document was restored from its .bak backup.
    Recovered,
    /// a Board that is not the user's last save was put in its place, and
    /// the choices on its task presets were turned off (`documents.rs`).
    ChoicesWithdrawn,
}

impl StorageNotice {
    #[cfg(test)]
    pub(crate) const ALL: [StorageNotice; 7] = [
        StorageNotice::Privacy,
        StorageNotice::QueuePersist,
        StorageNotice::QueueLoad,
        StorageNotice::HistoryLoad,
        StorageNotice::QueueInterrupted,
        StorageNotice::Recovered,
        StorageNotice::ChoicesWithdrawn,
    ];

    /// The webview's notice code (i18n `notice.*`).
    pub(crate) fn code(self) -> &'static str {
        match self {
            StorageNotice::Privacy => "storage.privacy",
            StorageNotice::QueuePersist => "queue.persist",
            StorageNotice::QueueLoad => "queue.load",
            StorageNotice::HistoryLoad => "history.load",
            StorageNotice::QueueInterrupted => "queue.interrupted",
            StorageNotice::Recovered => "storage.recovered",
            StorageNotice::ChoicesWithdrawn => "storage.choices-withdrawn",
        }
    }
}

/// Warnings produced before the webview exists (e.g. corrupt files found at
/// boot); the frontend fetches and toasts them via the `storage_warnings`
/// command. Request-path loads return their warning in-band instead.
#[cfg(not(test))]
static WARNINGS: Mutex<Vec<StorageNotice>> = Mutex::new(Vec::new());
// Unit tests run in parallel threads and several of them (scheduler boot and
// persist-failure paths) raise notices; each test thread keeps its own, so a
// test that drains notices sees exactly the ones it raised.
#[cfg(test)]
thread_local! {
    static WARNINGS: std::cell::RefCell<Vec<StorageNotice>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The webview gets only the notice's closed code; the log gets only a
/// stable category code of the note — notes can embed serde detail and
/// quarantine file names, which stay out of app.log. The note itself is not
/// kept.
pub(crate) fn warn(notice: StorageNotice, note: String) {
    applog(&format!("[storage] warning ({})", err_code(&note)));
    #[cfg(not(test))]
    WARNINGS.lock_or_recover().push(notice);
    #[cfg(test)]
    WARNINGS.with(|notices| notices.borrow_mut().push(notice));
}

/// Every notice raised since the last call, oldest first.
pub(crate) fn take_notices() -> Vec<StorageNotice> {
    #[cfg(not(test))]
    return std::mem::take(&mut *WARNINGS.lock_or_recover());
    #[cfg(test)]
    return WARNINGS.with(|notices| notices.take());
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
    envelope_payload_for(v, SCHEMA_VERSION)
}

fn envelope_payload_for(
    v: &serde_json::Value,
    supported: u64,
) -> Result<serde_json::Value, DocErr> {
    match (v.get("schema_version"), v.get("data")) {
        (None, None) => Ok(v.clone()), // legacy v0: the whole document
        (Some(sv), data) => {
            let n = sv.as_u64().ok_or_else(|| {
                DocErr::Bad(format!(
                    "schema_version must be a non-negative integer, found {}",
                    type_name_of(sv)
                ))
            })?;
            if n > supported {
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

/// Move a file this build cannot use to a unique `.corrupt-<ts>` beside it.
/// The bytes are kept, never overwritten. They may hold user content and a
/// pre-migration 0644 mode would survive the rename, so the kept file is
/// restricted to the user explicitly.
fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let corrupt = unique_corrupt_path(path);
    std::fs::rename(path, &corrupt)?;
    restrict_to_user(&corrupt);
    Ok(corrupt)
}

/// Every copy a recovery set aside for this file — the `<stem>.corrupt-<ts>`
/// siblings `quarantine` names (`<ts>-<n>` when several fall in one second)
/// — newest first. Nothing removes them, so the list survives restarts.
pub(crate) fn kept_copies(path: &Path) -> Result<Vec<PathBuf>, DeckError> {
    let Some(parent) = path.parent() else {
        return Ok(Vec::new());
    };
    let prefix = format!(
        "{}.corrupt-",
        path.file_stem().unwrap_or_default().to_string_lossy()
    );
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut kept = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let Some(suffix) = name.strip_prefix(&prefix) else {
            continue;
        };
        if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            continue;
        }
        // the timestamp, then the same-second counter
        let order: Vec<u64> = suffix
            .split('-')
            .map(|part| part.parse().unwrap_or(0))
            .collect();
        kept.push((order, parent.join(&name)));
    }
    kept.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(kept.into_iter().map(|(_, path)| path).collect())
}

/// Whether a recovery ever set this file aside (`kept_copies` is not empty).
pub(crate) fn was_quarantined(path: &Path) -> Result<bool, DeckError> {
    Ok(!kept_copies(path)?.is_empty())
}

/// The newest copy a recovery set aside that passes full validation as `T`
/// now, read and never moved. A copy that was read and is unusable is
/// skipped; one that cannot be READ ends the search with an error — it may
/// be intact, and it is newer than every copy after it. `Ok(None)`: every
/// copy was read and none is usable.
pub(crate) fn newest_valid_copy<T: DeserializeOwned>(
    path: &Path,
) -> Result<Option<(PathBuf, String)>, DeckError> {
    for kept in kept_copies(path)? {
        match held::<T>(&kept) {
            Held::Good(payload) => return Ok(Some((kept, payload))),
            Held::Unreadable(kind) => {
                let message =
                    format!("a kept copy could not be read ({kind}); it was left untouched");
                return Err(DeckError::new(ErrorKind::io(kind), message));
            }
            Held::Nothing | Held::Newer | Held::Damaged(_) => {}
        }
    }
    Ok(None)
}

/// What one file holds, as far as a reader that will not move it can tell.
enum Held {
    /// no file at that path
    Nothing,
    /// a fully validated payload
    Good(String),
    /// written by a newer deck
    Newer,
    /// its CONTENT cannot be used: not UTF-8, not JSON, a broken envelope or
    /// the wrong structure (the reason, for the log's category code)
    Damaged(String),
    /// the READ failed — permissions, descriptors, a failing disk; the file
    /// may be perfectly fine
    Unreadable(std::io::ErrorKind),
}

fn held<T: DeserializeOwned>(path: &Path) -> Held {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Held::Nothing,
        Err(error) => return Held::Unreadable(error.kind()),
    };
    let Ok(raw) = std::str::from_utf8(&bytes) else {
        return Held::Damaged("invalid UTF-8".into());
    };
    match parse_doc::<T>(raw) {
        Ok(payload) => Held::Good(payload),
        Err(DocErr::Newer(_)) => Held::Newer,
        Err(DocErr::Bad(reason)) => Held::Damaged(reason),
    }
}

/// The read for everything that does not OWN the file: the best validated
/// copy, with the disk left exactly as it was found. Nothing is renamed,
/// created or rewritten, however often this runs and whoever runs first.
///
/// - A usable main file is the answer.
/// - A main file written by a newer deck is refused, never read around.
/// - A main file that is there but cannot be READ is an error meaning
///   "unknown": that is not damage, and the backup does not stand in.
/// - A main file whose CONTENT is damaged is answered from the backup (the
///   same full validation); an error when that is unusable too.
/// - No main file is `Ok(None)`, a first run — unless a recovery set it
///   aside and its backup still loads: then the backup answers until the
///   owner's next save puts the main file back.
///
/// Moving a damaged file and telling the user are the owner's
/// (`load_as_owner`, `save_typed_as_owner`); an outcome from here never
/// carries a warning, and its errors name the file, never its content.
pub(crate) fn read_typed<T: DeserializeOwned>(
    path: &Path,
) -> Result<Option<LoadOutcome>, DeckError> {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unreadable = |kind: std::io::ErrorKind| {
        DeckError::new(
            ErrorKind::io(kind),
            format!("{name} could not be read ({kind}); it was left untouched"),
        )
    };
    let newer = || {
        DeckError::new(
            ErrorKind::NewerSchema,
            format!("{name} was written by a newer deck — update deck; it was left untouched"),
        )
    };
    let main_is_gone = match held::<T>(path) {
        Held::Good(payload) => {
            return Ok(Some(LoadOutcome {
                payload,
                source: "main",
                warning: None,
            }))
        }
        Held::Newer => return Err(newer()),
        Held::Unreadable(kind) => return Err(unreadable(kind)),
        Held::Damaged(_) => false,
        Held::Nothing => true,
    };
    let bak = bak_path(path);
    // no backup, or nothing ever set aside: an ordinary first run
    if main_is_gone && !(bak.exists() && was_quarantined(path)?) {
        return Ok(None);
    }
    match held::<T>(&bak) {
        Held::Good(payload) => Ok(Some(LoadOutcome {
            payload,
            source: "backup",
            warning: None,
        })),
        Held::Newer => Err(newer()),
        Held::Unreadable(kind) => Err(unreadable(kind)),
        // nothing loadable is left of a file whose loss was already reported
        Held::Nothing | Held::Damaged(_) if main_is_gone => Ok(None),
        Held::Nothing | Held::Damaged(_) => Err(DeckError::new(
            ErrorKind::Recovery,
            format!("{name} is damaged and its backup is unusable"),
        )),
    }
}

/// The load for the OWNER of a file that has readers beside it (settings):
/// `load_typed` — the one place a damaged main file is set aside and its
/// warning produced — and, when that finds no main file, `read_typed`, so a
/// file set aside by an earlier recovery and not saved since is still not a
/// first run. The warning comes once, from the load that moved the file;
/// later starts answer from the backup without one.
pub(crate) fn load_as_owner<T: DeserializeOwned>(
    path: &Path,
) -> Result<Option<LoadOutcome>, DeckError> {
    match load_typed::<T>(path)? {
        Some(doc) => Ok(Some(doc)),
        None => read_typed::<T>(path),
    }
}

/// A projection/save fence may inspect the main file but must never quarantine
/// it or consume recovery before the authoritative Board loader sees warnings.
pub(crate) fn peek_typed<T: DeserializeOwned>(path: &Path) -> Result<Option<String>, DeckError> {
    match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(raw) => parse_doc::<T>(&raw).map(Some).map_err(|error| match error {
            DocErr::Newer(_) => {
                DeckError::new(ErrorKind::NewerSchema, "board requires a newer schema")
            }
            DocErr::Bad(_) => DeckError::new(
                ErrorKind::InvalidDoc,
                "board is unreadable; recovery is required",
            ),
        }),
    }
}

/// Load and fully validate a data file as document type `T`, for its owner.
/// `Ok(None)` = file does not exist (a genuine first run).
/// A bad main file is quarantined, then the `.bak` (same validation) is
/// tried; success carries a warning for the UI, failure is a hard error the
/// caller must surface — NOT to be treated as an empty first run. A main
/// file the owner cannot read is set aside the same way: the owner goes on
/// to save from memory, and only a file that is out of the way is safe from
/// that save. Code that does not own the file uses `read_typed`.
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
    let kept_at = match quarantine(path) {
        // Only the file NAME is reported (it sits beside the original) — the
        // absolute path never enters warnings or logs.
        Ok(corrupt) => format!(
            " — the damaged file was kept as {}",
            corrupt
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
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
    minimum_version: u64,
    validate_existing: impl Fn(&serde_json::Value) -> Result<(), DeckError>,
) -> Result<(), DeckError> {
    // Scheduler workers and UI commands can save concurrently. Serialize the
    // validate → backup → replace sequence so one writer cannot validate
    // bytes another writer replaces before its backup is taken.
    let _save_guard = SAVE_LOCK.lock_or_recover();
    save_checked_locked(
        path,
        payload,
        keep_backup,
        minimum_version,
        validate_existing,
    )
}

// Caller owns SAVE_LOCK, including any read used to derive this write.
fn save_checked_locked(
    path: &Path,
    payload: &str,
    keep_backup: bool,
    minimum_version: u64,
    validate_existing: impl Fn(&serde_json::Value) -> Result<(), DeckError>,
) -> Result<(), DeckError> {
    let data: serde_json::Value = serde_json::from_str(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save invalid JSON: {e}")))?;
    // Never clobber a file this build does not understand. `load_typed`
    // quarantines a damaged main file before anything can save over it, so
    // reaching here with a broken envelope means the file was never loaded
    // (or was replaced behind our back) — refuse rather than destroy it.
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let feature_version =
        if matches!(name.as_ref(), "deck.json" | "queue.json") && uses_phone_task_policy(&data) {
            6
        } else if name == "deck.json"
            && data
                .get("cards")
                .and_then(|v| v.as_array())
                .is_some_and(|cards| {
                    cards.iter().any(|c| {
                        c.get("reminder").is_some() || c.get("reminderRetirements").is_some()
                    })
                })
        {
            5
        } else if matches!(name.as_ref(), "queue.json" | "settings.json")
            && uses_clock_first_send(&data)
        {
            4
        } else if matches!(name.as_ref(), "deck.json" | "queue.json" | "settings.json")
            && uses_buffer(&data)
        {
            3
        } else if review_gated(&name) && uses_review(&data) {
            2
        } else {
            1
        };
    let mut version = minimum_version.max(feature_version);
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
            version = version.max(
                v.get("schema_version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1),
            );
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
    let doc = serde_json::json!({ "schema_version": version, "data": data });
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
    save_checked(path, payload, true, 1, |_| Ok(()))
}

/// Typed save used by every app data file. It validates both the new payload
/// and any concurrently replaced existing payload under the same save lock,
/// so malformed business structure cannot be silently overwritten.
pub(crate) fn save_typed<T: DeserializeOwned>(path: &Path, payload: &str) -> Result<(), DeckError> {
    save_typed_version::<T>(path, payload, 1)
}

pub(crate) fn save_typed_version<T: DeserializeOwned>(
    path: &Path,
    payload: &str,
    minimum_version: u64,
) -> Result<(), DeckError> {
    if !(1..=SCHEMA_VERSION).contains(&minimum_version) {
        return Err(DeckError::new(
            ErrorKind::NewerSchema,
            "unsupported schema version",
        ));
    }
    serde_json::from_str::<T>(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save wrong structure: {e}")))?;
    save_checked(path, payload, true, minimum_version, |existing| {
        serde_json::from_value::<T>(existing.clone())
            .map(|_| ())
            .map_err(DeckError::from)
    })
}

/// The save for the OWNER of a file that has readers beside it (settings).
/// Those readers never move anything (`read_typed`), so a main file damaged
/// while deck runs would refuse every later save. The owner holds the
/// document in memory: the damaged file is set aside — kept as
/// `.corrupt-<ts>`, never made the backup — and the save goes ahead. A main
/// file written by a newer deck or one that cannot be read is refused,
/// exactly as `save_typed` refuses it; a payload that is not a `T` is
/// refused before anything is moved.
pub(crate) fn save_typed_as_owner<T: DeserializeOwned>(
    path: &Path,
    payload: &str,
) -> Result<(), DeckError> {
    serde_json::from_str::<T>(payload)
        .map_err(|e| DeckError::classified(format!("refusing to save wrong structure: {e}")))?;
    let _save_guard = SAVE_LOCK.lock_or_recover();
    if let Held::Damaged(reason) = held::<T>(path) {
        // a failed move leaves the file where it is; the save below refuses it
        if quarantine(path).is_ok() {
            applog(&format!(
                "[storage] {} was damaged when saving; kept aside ({})",
                path.file_name().unwrap_or_default().to_string_lossy(),
                err_code(&reason)
            ));
        }
    }
    save_checked_locked(path, payload, true, 1, |existing| {
        serde_json::from_value::<T>(existing.clone())
            .map(|_| ())
            .map_err(DeckError::from)
    })
}

/// Upgrade only the envelope while holding the same lock as settings saves.
/// Reading outside this lock could restore stale user settings during opt-in.
/// It loads the settings as their owner would: a damaged main file is set
/// aside (raising the recovery notice the webview's own load will no longer
/// see). It never makes a recovered document the current one, though: when
/// only the backup answers, the barrier is raised on the backup where it
/// lies, and the main file stays absent until its owner saves — so a reader
/// can still tell the previous save from the current one
/// (`LoadOutcome::source`), and nothing but `{}` on a real first run is ever
/// written here that the owner did not save. It runs on the scheduler's
/// thread, not for the owner: a main file it merely cannot READ is left
/// where it is, and the queue save that asked for the barrier fails and is
/// retried.
pub(crate) fn ensure_review_schema<T: DeserializeOwned>(path: &Path) -> Result<(), DeckError> {
    let _save_guard = SAVE_LOCK.lock_or_recover();
    let version_at = |path: &Path| {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("schema_version").and_then(|n| n.as_u64()))
    };
    if version_at(path) == Some(2) {
        return Ok(());
    }
    if let Held::Unreadable(kind) = held::<T>(path) {
        return Err(DeckError::new(
            ErrorKind::io(kind),
            format!(
                "{} could not be read ({kind}); it was left untouched",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        ));
    }
    let (payload, recovered) = match load_as_owner::<T>(path)? {
        Some(doc) => {
            if let Some(note) = doc.warning {
                warn(StorageNotice::Recovered, note);
            }
            (doc.payload, doc.source == "backup")
        }
        None => ("{}".into(), false),
    };
    serde_json::from_str::<T>(&payload).map_err(DeckError::from)?;
    let bak = bak_path(path);
    if recovered && version_at(&bak).is_some_and(|version| version >= 2) {
        return Ok(());
    }
    // the backup is rewritten in place (same payload, higher envelope) and
    // keeps no backup of its own
    let target = if recovered { bak.as_path() } else { path };
    save_checked_locked(target, &payload, !recovered, 2, |existing| {
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
    save_checked(path, payload, false, 1, |existing| {
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
    #[test]
    fn review_documents_upgrade_only_on_opt_in_and_never_silently_downgrade() {
        let dir = tdir("review-version");
        let path = dir.join("queue.json");
        save_typed::<serde_json::Value>(&path, r#"{"items":[],"reviews":[]}"#).unwrap();
        let first: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(first["schema_version"], 1);
        assert!(envelope_payload_for(&first, 1).is_ok());
        save_typed::<serde_json::Value>(&path, r#"{"items":[{"review_each":true}]}"#).unwrap();
        let reviewed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reviewed["schema_version"], 2);
        assert!(matches!(
            envelope_payload_for(&reviewed, 1),
            Err(DocErr::Newer(2))
        ));
        assert!(load_typed::<serde_json::Value>(&path).unwrap().is_some());
        save_typed::<serde_json::Value>(&path, r#"{"items":[]}"#).unwrap();
        let empty: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            empty["schema_version"], 2,
            "removing the last checkpoint never permits an old reader to resurrect its v1 backup"
        );
        let settings = dir.join("settings.json");
        save_typed::<serde_json::Value>(&settings, r#"{"editor":"Zed"}"#).unwrap();
        ensure_review_schema::<serde_json::Value>(&settings).unwrap();
        save_typed::<serde_json::Value>(&settings, r#"{"editor":"Zed"}"#).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert!(matches!(
            envelope_payload_for(&raw, 1),
            Err(DocErr::Newer(2))
        ));
        assert_eq!(raw["data"]["editor"], "Zed");
        // The Board never joins the door: an opted-in run origin stays v1.
        let board = dir.join("deck.json");
        save_typed::<serde_json::Value>(
            &board,
            r#"{"cards":[{"origin":{"source":"clock","reviewEach":true}}]}"#,
        )
        .unwrap();
        let board_raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&board).unwrap()).unwrap();
        assert_eq!(board_raw["schema_version"], 1);
        assert!(envelope_payload_for(&board_raw, 1).is_ok());
    }

    #[test]
    fn buffer_documents_upgrade_to_sticky_v3_and_old_readers_refuse_them() {
        let dir = tdir("buffer-version");
        let board = dir.join("deck.json");
        save_typed::<serde_json::Value>(&board, r#"{"cards":[]}"#).unwrap();
        let plain: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&board).unwrap()).unwrap();
        assert_eq!(plain["schema_version"], 1);
        save_typed::<serde_json::Value>(
            &board,
            r#"{"cards":[{"buffer":{"revision":1,"collecting":true,"entries":[]}}]}"#,
        )
        .unwrap();
        let buffered: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&board).unwrap()).unwrap();
        assert_eq!(buffered["schema_version"], 3);
        assert!(matches!(
            envelope_payload_for(&buffered, 2),
            Err(DocErr::Newer(3))
        ));
        let inbound_dir = tdir("inbound-plan-version");
        let inbound_board = inbound_dir.join("deck.json");
        save_typed::<serde_json::Value>(
            &inbound_board,
            r#"{"cards":[{"inboundPlan":{"operationId":"B1","initialQueued":false}}]}"#,
        )
        .unwrap();
        let inbound: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&inbound_board).unwrap()).unwrap();
        assert_eq!(inbound["schema_version"], 3);
        assert!(matches!(
            envelope_payload_for(&inbound, 2),
            Err(DocErr::Newer(3))
        ));
        let preset_dir = tdir("preset-version");
        let preset_board = preset_dir.join("deck.json");
        save_typed::<serde_json::Value>(
            &preset_board,
            r#"{"projects":[{"presets":[{"id":"R1"}]}],"cards":[]}"#,
        )
        .unwrap();
        let preset: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&preset_board).unwrap()).unwrap();
        assert_eq!(preset["schema_version"], 3);
        assert!(matches!(
            envelope_payload_for(&preset, 2),
            Err(DocErr::Newer(3))
        ));
        save_typed::<serde_json::Value>(&board, r#"{"cards":[]}"#).unwrap();
        let cleared: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&board).unwrap()).unwrap();
        assert_eq!(
            cleared["schema_version"], 3,
            "clearing the buffer cannot make an older writer safe"
        );

        let queue = dir.join("queue.json");
        save_typed::<serde_json::Value>(&queue, r#"{"operations":[{"id":"B1"}]}"#).unwrap();
        let operation: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&queue).unwrap()).unwrap();
        assert_eq!(operation["schema_version"], 3);
        assert!(matches!(
            envelope_payload_for(&operation, 2),
            Err(DocErr::Newer(3))
        ));

        let settings = dir.join("settings.json");
        save_typed::<serde_json::Value>(
            &settings,
            r#"{"channelConnection":{"enabled":true},"channelRules":[{"id":"R1"}]}"#,
        )
        .unwrap();
        let channel: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(channel["schema_version"], 3);
        assert!(matches!(
            envelope_payload_for(&channel, 2),
            Err(DocErr::Newer(3))
        ));
    }

    #[test]
    fn review_envelope_barrier_never_overwrites_concurrent_settings() {
        let path = tdir("review-settings-race").join("settings.json");
        save_typed::<Doc>(&path, r#"{"v":0}"#).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for value in 1..=50 {
                    save_typed::<Doc>(&path, &format!("{{\"v\":{value}}}")).unwrap();
                }
            });
            scope.spawn(|| {
                for _ in 0..50 {
                    ensure_review_schema::<Doc>(&path).unwrap();
                }
            });
        });
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(raw["schema_version"], 2);
        assert_eq!(raw["data"]["v"], 50);
    }
    /// A preset's first-send choice and the queue origin it admits are
    /// refused by a v5 reader instead of being dropped or read as damage.
    #[test]
    fn phone_task_first_send_documents_upgrade_to_sticky_v6_only_when_used() {
        let dir = tdir("connector-readiness-version");
        for (case, (name, data, unused)) in [
            (
                "deck.json",
                serde_json::json!({"projects":[{"presets":[{"id":"R1", "firstSend":true}]}]}),
                serde_json::json!({"projects":[{"presets":[{"id":"R1"}, {"id":"R2", "firstSend":false}]}]}),
            ),
            (
                "queue.json",
                serde_json::json!({"items":[{"readiness_override":{"rule":"R1", "trigger":"connector"}}]}),
                serde_json::json!({"items":[{"readiness_override":{"rule":"R", "trigger":"clock"}}]}),
            ),
            // the approval: on the preset, on a row, on a delivery record
            (
                "deck.json",
                serde_json::json!({"projects":[{"presets":[{"id":"R1", "autoSend":{"digest":"d"}}]}]}),
                serde_json::json!({"projects":[{"presets":[{"id":"R1"}]}]}),
            ),
            (
                "queue.json",
                serde_json::json!({"items":[{"authority":{"rule":"R1", "trigger":"connector"}}]}),
                serde_json::json!({"items":[{"authority":{"rule":"R", "trigger":"slack-badge"}}]}),
            ),
            (
                "queue.json",
                serde_json::json!({"deliveries":[{"authority":{"rule":"R1", "trigger":"connector"}}]}),
                serde_json::json!({"deliveries":[{"authority":{"rule":"R", "trigger":"slack-badge"}}]}),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            // one directory per case: the version is sticky per file
            let case = dir.join(case.to_string());
            std::fs::create_dir_all(&case).unwrap();
            let p = case.join(name);
            save_typed::<serde_json::Value>(&p, &unused.to_string()).unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert!(raw["schema_version"].as_u64().unwrap() < 6, "{name}");
            save_typed::<serde_json::Value>(&p, &data.to_string()).unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], 6, "{name}");
            assert!(envelope_payload_for(&raw, 5).is_err(), "{name}");
            // sticky: the choice withdrawn, the version stays
            save_typed::<serde_json::Value>(&p, &unused.to_string()).unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], 6, "{name}");
        }
        // settings.json has no such field: it never takes v6 for one
        let p = dir.join("settings.json");
        save_typed::<serde_json::Value>(
            &p,
            &serde_json::json!({"presets":[{"firstSend":true}]}).to_string(),
        )
        .unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(raw["schema_version"].as_u64().unwrap() < 6);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn clock_readiness_documents_upgrade_to_sticky_v4_only_when_used() {
        let dir = tdir("clock-readiness-version");
        for (name, data) in [
            (
                "settings.json",
                serde_json::json!({"rules":[{"source":"clock", "firstSendWithoutReadiness":true}]}),
            ),
            (
                "queue.json",
                serde_json::json!({"items":[{"readiness_override":{"rule":"R", "trigger":"clock"}}]}),
            ),
        ] {
            let p = dir.join(name);
            save_typed::<serde_json::Value>(&p, &data.to_string()).unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], 4);
            // The old reader's envelope refusal precedes closed-enum decode.
            assert!(envelope_payload_for(&raw, 3).is_err());
            save_typed::<serde_json::Value>(&p, "{}").unwrap();
            let raw: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
            assert_eq!(raw["schema_version"], 4);
        }
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn reminder_board_requires_sticky_v5_and_old_readers_refuse_protection() {
        let dir = tdir("reminder-version");
        let path = dir.join("deck.json");
        save_typed::<serde_json::Value>(&path, r#"{"cards":[]}"#).unwrap();
        let read =
            || serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(read()["schema_version"], 1);
        save_typed::<serde_json::Value>(&path, r#"{"cards":[{"reminder":{"id":"identity"}}]}"#)
            .unwrap();
        assert_eq!(read()["schema_version"], 5);
        assert!(envelope_payload_for(&read(), 4).is_err());
        save_typed::<serde_json::Value>(&path, r#"{"cards":[]}"#).unwrap();
        assert_eq!(read()["schema_version"], 5);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn peek_never_consumes_corruption_or_future_schema_before_the_owner() {
        let dir = tdir("reminder-peek");
        let path = dir.join("deck.json");
        for bytes in ["broken", r#"{"schema_version":99,"data":{}}"#] {
            std::fs::write(&path, bytes).unwrap();
            assert!(peek_typed::<serde_json::Value>(&path).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    // ---------- the reader's door (`read_typed`) and the owner's doors ----------

    /// Every entry of `d` with its bytes (nothing for one that cannot be
    /// read): equal listings mean nothing was moved, created, removed or
    /// rewritten.
    fn listing(d: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap_or_default(),
                )
            })
            .collect();
        files.sort();
        files
    }

    fn envelope(v: u64) -> Vec<u8> {
        format!(r#"{{"schema_version":1,"data":{{"v":{v}}}}}"#).into_bytes()
    }

    const NEWER: &[u8] = br#"{"schema_version":99,"data":{"v":9}}"#;

    /// What a reader is owed in one cell of main × backup × set-aside.
    #[derive(Debug, PartialEq)]
    enum Answer {
        Main(u64),
        Backup(u64),
        FirstRun,
        Fails(ErrorKind),
    }

    /// One cell: its name, the main file, the backup, whether something was
    /// set aside before, and the answer.
    type Cell<'a> = (&'a str, Option<&'a [u8]>, Option<&'a [u8]>, bool, Answer);

    fn answer(p: &Path) -> Answer {
        match read_typed::<Doc>(p) {
            Ok(None) => Answer::FirstRun,
            Ok(Some(doc)) => {
                assert!(doc.warning.is_none(), "a reader is never the one told");
                let v = serde_json::from_str::<serde_json::Value>(&doc.payload).unwrap()["v"]
                    .as_u64()
                    .unwrap();
                match doc.source {
                    "main" => Answer::Main(v),
                    "backup" => Answer::Backup(v),
                    other => panic!("unknown source {other}"),
                }
            }
            Err(error) => Answer::Fails(error.kind()),
        }
    }

    /// The whole matrix, one row per cell. In every cell the reader gets the
    /// best validated copy (or an honest "no"), twice in a row, and the
    /// directory is byte for byte what it was.
    #[test]
    fn a_reader_gets_the_best_validated_copy_and_never_changes_the_disk() {
        use Answer::{Backup, Fails, FirstRun, Main};
        let broken: &[u8] = b"{broken";
        let not_utf8: &[u8] = &[0xff, 0xfe, b'{', b'}'];
        let wrong_type: &[u8] = br#"{"schema_version":1,"data":{"v":"text"}}"#;
        let half_envelope: &[u8] = br#"{"schema_version":1}"#;
        let (one, two) = (envelope(1), envelope(2));
        let cells: [Cell; 19] = [
            ("usable main", Some(&two), Some(&one), false, Main(2)),
            ("usable main, no backup", Some(&two), None, false, Main(2)),
            (
                "usable main, damaged backup",
                Some(&two),
                Some(broken),
                false,
                Main(2),
            ),
            (
                "usable main, set aside long ago",
                Some(&two),
                Some(&one),
                true,
                Main(2),
            ),
            (
                "damaged main, usable backup",
                Some(broken),
                Some(&one),
                false,
                Backup(1),
            ),
            (
                "not UTF-8, usable backup",
                Some(not_utf8),
                Some(&one),
                false,
                Backup(1),
            ),
            (
                "wrong structure, usable backup",
                Some(wrong_type),
                Some(&one),
                false,
                Backup(1),
            ),
            (
                "half an envelope, usable backup",
                Some(half_envelope),
                Some(&one),
                false,
                Backup(1),
            ),
            (
                "damaged main, damaged backup",
                Some(broken),
                Some(broken),
                false,
                Fails(ErrorKind::Recovery),
            ),
            (
                "damaged main, no backup",
                Some(broken),
                None,
                false,
                Fails(ErrorKind::Recovery),
            ),
            (
                "damaged main, newer backup",
                Some(broken),
                Some(NEWER),
                false,
                Fails(ErrorKind::NewerSchema),
            ),
            (
                "newer main, usable backup",
                Some(NEWER),
                Some(&one),
                false,
                Fails(ErrorKind::NewerSchema),
            ),
            (
                "newer main, set aside before",
                Some(NEWER),
                Some(&one),
                true,
                Fails(ErrorKind::NewerSchema),
            ),
            ("no main, nothing else", None, None, false, FirstRun),
            (
                "no main, a backup, never set aside",
                None,
                Some(&one),
                false,
                FirstRun,
            ),
            (
                "no main, set aside, usable backup",
                None,
                Some(&one),
                true,
                Backup(1),
            ),
            (
                "no main, set aside, damaged backup",
                None,
                Some(broken),
                true,
                FirstRun,
            ),
            ("no main, set aside, no backup", None, None, true, FirstRun),
            (
                "no main, set aside, newer backup",
                None,
                Some(NEWER),
                true,
                Fails(ErrorKind::NewerSchema),
            ),
        ];
        for (k, (cell, main, backup, set_aside, want)) in cells.into_iter().enumerate() {
            let d = tdir(&format!("read-{k}"));
            let p = d.join("x.json");
            if let Some(bytes) = main {
                std::fs::write(&p, bytes).unwrap();
            }
            if let Some(bytes) = backup {
                std::fs::write(bak_path(&p), bytes).unwrap();
            }
            if set_aside {
                std::fs::write(d.join("x.corrupt-1700000000"), b"kept").unwrap();
            }
            let before = listing(&d);
            for round in 0..2 {
                assert_eq!(answer(&p), want, "{cell}, read {round}");
            }
            assert_eq!(listing(&d), before, "{cell}: the read changed the disk");
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    /// A file that is there but cannot be READ is "unknown" to a reader: not
    /// a first run, not damage, and a good backup does not stand in for it.
    /// The same holds for the backup of a damaged or set-aside main file.
    #[test]
    fn a_reader_reports_an_unreadable_file_as_unknown_and_never_reads_around_it() {
        let unknown = |p: &Path, cell: &str| {
            let kind = match read_typed::<Doc>(p) {
                Err(error) => error.kind(),
                Ok(found) => panic!("{cell}: answered {:?}", found.map(|doc| doc.source)),
            };
            assert!(
                !matches!(kind, ErrorKind::Recovery | ErrorKind::NewerSchema),
                "{cell}: {kind:?} says damaged or newer"
            );
        };
        // a directory where the file should be: the read fails for any user
        let d = tdir("read-unreadable");
        let p = d.join("x.json");
        std::fs::create_dir(&p).unwrap();
        std::fs::write(bak_path(&p), envelope(1)).unwrap();
        let before = listing(&d);
        for _ in 0..2 {
            unknown(&p, "unreadable main, usable backup");
        }
        assert_eq!(listing(&d), before);
        assert!(p.is_dir(), "not moved");

        // a permission failure, where the test user is not privileged
        let d = tdir("read-denied");
        let p = d.join("x.json");
        std::fs::write(&p, envelope(2)).unwrap();
        std::fs::write(bak_path(&p), envelope(1)).unwrap();
        set_mode(&p, 0o000);
        if std::fs::read(&p).is_err() {
            assert_eq!(
                read_typed::<Doc>(&p).unwrap_err().kind(),
                ErrorKind::Perm,
                "the kind says why"
            );
            assert_eq!(listing(&d).len(), 2, "nothing set aside");
        }
        set_mode(&p, 0o600);
        assert_eq!(answer(&p), Answer::Main(2), "readable again, untouched");

        // the backup cannot be read: unknown too, never "nothing there"
        for (cell, main, set_aside) in [
            (
                "damaged main, unreadable backup",
                Some(&b"{broken"[..]),
                false,
            ),
            ("no main, set aside, unreadable backup", None, true),
        ] {
            let d = tdir("read-unreadable-backup");
            let p = d.join("x.json");
            if let Some(bytes) = main {
                std::fs::write(&p, bytes).unwrap();
            }
            if set_aside {
                std::fs::write(d.join("x.corrupt-1700000000"), b"kept").unwrap();
            }
            std::fs::create_dir(bak_path(&p)).unwrap();
            let before = listing(&d);
            unknown(&p, cell);
            assert_eq!(listing(&d), before, "{cell}");
        }
    }

    /// "Set aside before" is exactly the name `load_typed` gives a file it
    /// moves — `<stem>.corrupt-<digits>` with an optional `-<n>` — for THIS
    /// file, and nothing else in the directory.
    #[test]
    fn only_a_recovery_s_own_file_name_counts_as_set_aside() {
        let d = tdir("marker");
        let p = d.join("settings.json");
        assert!(!was_quarantined(&p).unwrap(), "an empty directory");
        for other in [
            "settings.corrupt-",
            "settings.corrupt-abc",
            "settings.corrupt-17x",
            "settings.json.corrupt-1700000000",
            "deck.corrupt-1700000000",
            "settings.json.bak",
        ] {
            std::fs::write(d.join(other), b"x").unwrap();
            assert!(!was_quarantined(&p).unwrap(), "{other} is not a marker");
        }
        std::fs::write(d.join("settings.corrupt-1700000000-1"), b"x").unwrap();
        assert!(was_quarantined(&p).unwrap(), "the numbered form counts");
        // the real thing: whatever name the quarantine picks is recognised
        let d = tdir("marker-real");
        let p = d.join("settings.json");
        std::fs::write(&p, "{broken").unwrap();
        assert!(load_doc(&p).is_err());
        assert!(was_quarantined(&p).unwrap());
        assert!(!was_quarantined(&d.join("deck.json")).unwrap());
        // a directory that is not there has nothing set aside
        assert!(!was_quarantined(&d.join("missing").join("settings.json")).unwrap());
    }

    /// The owner's load across a restart: the load that moves the damaged
    /// file carries the warning; the next ones, with the main file still
    /// gone, answer from the backup without one; a save ends the episode.
    #[test]
    fn the_owner_is_warned_once_and_a_set_aside_file_is_never_a_first_run() {
        let d = tdir("owner-load");
        let p = d.join("x.json");
        save(&p, r#"{"v":1}"#).unwrap();
        save(&p, r#"{"v":2}"#).unwrap(); // .bak holds v1
        std::fs::write(&p, "{broken").unwrap();
        let first = load_as_owner::<Doc>(&p).unwrap().unwrap();
        assert_eq!(first.source, "backup");
        assert!(first.warning.is_some());
        for start in 0..2 {
            let later = load_as_owner::<Doc>(&p).unwrap().unwrap();
            assert_eq!(later.source, "backup", "start {start}");
            assert!(later.payload.contains("\"v\":1"), "{}", later.payload);
            assert!(later.warning.is_none(), "start {start}: told once");
        }
        assert!(!p.exists(), "loading never writes");
        save(&p, &first.payload).unwrap();
        let rebuilt = load_as_owner::<Doc>(&p).unwrap().unwrap();
        assert_eq!((rebuilt.source, rebuilt.warning), ("main", None));
        // with no usable backup: a hard error once, then a first run
        let d = tdir("owner-load-lost");
        let p = d.join("x.json");
        std::fs::write(&p, "{broken").unwrap();
        assert_eq!(
            load_as_owner::<Doc>(&p).unwrap_err().kind(),
            ErrorKind::Recovery
        );
        assert!(load_as_owner::<Doc>(&p).unwrap().is_none());
    }

    /// The owner's save: a main file whose content is damaged is set aside
    /// and the save goes ahead; everything `save_typed` refuses for another
    /// reason is still refused, with the disk untouched.
    #[test]
    fn the_owners_save_sets_a_damaged_main_aside_and_refuses_what_save_refuses() {
        let d = tdir("owner-save");
        let p = d.join("x.json");
        save_typed::<Doc>(&p, r#"{"v":1}"#).unwrap();
        save_typed::<Doc>(&p, r#"{"v":2}"#).unwrap(); // .bak holds v1
        let backup = std::fs::read(bak_path(&p)).unwrap();

        // usable main: an ordinary save, the previous version becomes the backup
        save_typed_as_owner::<Doc>(&p, r#"{"v":3}"#).unwrap();
        assert_eq!(answer(&p), Answer::Main(3));
        assert_ne!(std::fs::read(bak_path(&p)).unwrap(), backup);
        assert_eq!(listing(&d).len(), 2, "nothing set aside");
        let backup = std::fs::read(bak_path(&p)).unwrap(); // v2

        // a payload that is not the document: refused before anything moves
        std::fs::write(&p, "{broken").unwrap();
        let before = listing(&d);
        assert!(save_typed_as_owner::<Doc>(&p, r#"{"v":"text"}"#).is_err());
        assert_eq!(listing(&d), before);

        // damaged content: set aside with its bytes, saved, backup untouched
        save_typed_as_owner::<Doc>(&p, r#"{"v":4}"#).unwrap();
        assert_eq!(answer(&p), Answer::Main(4));
        assert_eq!(std::fs::read(bak_path(&p)).unwrap(), backup);
        let kept: Vec<_> = listing(&d)
            .into_iter()
            .filter(|(name, _)| name.starts_with("x.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].1, b"{broken");
        assert_eq!(mode_of(&d.join(&kept[0].0)), 0o600);

        // written by a newer deck: refused untouched
        std::fs::write(&p, NEWER).unwrap();
        let before = listing(&d);
        assert_eq!(
            save_typed_as_owner::<Doc>(&p, r#"{"v":5}"#)
                .unwrap_err()
                .kind(),
            ErrorKind::NewerSchema
        );
        assert_eq!(listing(&d), before);

        // cannot be read: refused untouched, like any save
        std::fs::remove_file(&p).unwrap();
        std::fs::create_dir(&p).unwrap();
        let before = listing(&d);
        assert!(save_typed_as_owner::<Doc>(&p, r#"{"v":5}"#).is_err());
        assert_eq!(listing(&d), before);
        assert!(p.is_dir());
    }

    /// The review barrier never writes `{}` over settings that can be
    /// recovered, reports a recovery it performs itself, and never makes a
    /// recovered document the main file: while only the backup loads, the
    /// envelope is raised on the backup and every reader still sees "backup".
    #[test]
    fn the_review_barrier_keeps_recoverable_settings_and_never_makes_them_current() {
        let d = tdir("review-recovery");
        let p = d.join("settings.json");
        let on_disk = |p: &Path| {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(p).unwrap()).unwrap()
        };
        save_typed::<Doc>(&p, r#"{"v":1}"#).unwrap();
        save_typed::<Doc>(&p, r#"{"v":2}"#).unwrap(); // .bak holds v1

        // set aside by its owner's load, not saved since
        std::fs::write(&p, "{broken").unwrap();
        assert_eq!(load_as_owner::<Doc>(&p).unwrap().unwrap().source, "backup");
        take_notices();
        let raised = |p: &Path| {
            assert!(!p.exists(), "the barrier wrote a main file");
            assert_eq!(on_disk(&bak_path(p))["schema_version"], 2);
            assert_eq!(on_disk(&bak_path(p))["data"]["v"], 1);
            assert!(!bak_path(&bak_path(p)).exists());
            let read = read_typed::<Doc>(p).unwrap().unwrap();
            assert_eq!(
                (read.source, read.payload.as_str()),
                ("backup", r#"{"v":1}"#)
            );
        };
        ensure_review_schema::<Doc>(&p).unwrap();
        raised(&p);
        assert!(take_notices().is_empty(), "its owner was already told");
        // a second barrier finds it raised and writes nothing
        let before = listing(&d);
        ensure_review_schema::<Doc>(&p).unwrap();
        assert_eq!(listing(&d), before);

        // the owner's save is what makes the recovered document current
        save_typed_as_owner::<Doc>(&p, r#"{"v":1}"#).unwrap();
        assert_eq!(read_typed::<Doc>(&p).unwrap().unwrap().source, "main");
        save_typed_as_owner::<Doc>(&p, r#"{"v":3}"#).unwrap(); // .bak holds v1

        // damaged and not yet seen by its owner: set aside here, told once
        std::fs::write(&p, "{broken again").unwrap();
        ensure_review_schema::<Doc>(&p).unwrap();
        raised(&p);
        assert_eq!(take_notices(), [StorageNotice::Recovered]);
        let loaded = load_as_owner::<Doc>(&p).unwrap().unwrap();
        assert_eq!((loaded.source, loaded.warning), ("backup", None));
        let kept = listing(&d)
            .into_iter()
            .filter(|(name, _)| name.starts_with("settings.corrupt-"))
            .count();
        assert_eq!(kept, 2, "both damaged files are kept");

        // a real first run still gets the empty barrier
        let fresh = tdir("review-first").join("settings.json");
        ensure_review_schema::<serde_json::Value>(&fresh).unwrap();
        assert_eq!(on_disk(&fresh)["schema_version"], 2);
        assert_eq!(on_disk(&fresh)["data"], serde_json::json!({}));

        // a main file that cannot be read right now is not this thread's to
        // move: the barrier fails (the queue save is retried), nothing is set
        // aside, and no backup is written in its place
        let d = tdir("review-unreadable");
        let p = d.join("settings.json");
        std::fs::create_dir(&p).unwrap();
        std::fs::write(bak_path(&p), envelope(1)).unwrap();
        let before = listing(&d);
        take_notices();
        let refused = ensure_review_schema::<Doc>(&p).unwrap_err();
        assert!(
            !matches!(refused.kind(), ErrorKind::Recovery),
            "not damage: {refused}"
        );
        assert_eq!(listing(&d), before);
        assert!(p.is_dir(), "left where it was");
        assert!(take_notices().is_empty());
    }
}
