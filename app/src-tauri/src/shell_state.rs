//! Bounded shell recovery across a tmux/server or machine restart.
//!
//! tmux remains the live terminal authority.  This module checkpoints only
//! panes whose foreground process is a shell, and stores:
//! - the pane's current working directory;
//! - a bounded, plain-text tail of tmux scrollback.
//!
//! Recovery never replays input and never attempts to serialize processes,
//! jobs, environment variables, shell options, or an agent TUI.  A restored
//! transcript is loaded into a one-use private tmux buffer. A system shell in
//! the NEW pane prints that buffer to stdout before it execs the user's login
//! shell. It therefore becomes ordinary tmux history without ever entering
//! shell stdin. The signed deck executable is deliberately never a pane
//! process: macOS Local Network Privacy would otherwise attribute the login
//! shell and its descendants to deck after a machine restart.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::storage::{self, applog, now_epoch};
use crate::tmux::{pane_target, tmux, validate_session_name};

pub(crate) const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;
const MAX_TRANSCRIPT_LINES: usize = 3000;
const CHECKPOINT_INTERVAL_SECS: u64 = 15;
const MAX_CHECKPOINTS_PER_TICK: usize = 2;
const MAX_SNAPSHOT_FILES: usize = 128;
const MAX_SNAPSHOT_AGE_SECS: u64 = 7 * 24 * 60 * 60;
const RESTORE_BOUNDARY: &[u8] = b"\n---------------- deck restart ----------------\n";
static BOOTSTRAP_NONCE: AtomicU64 = AtomicU64::new(0);
pub(crate) const RESTORE_EXECUTABLE: &str = "/bin/sh";
pub(crate) const RESTORE_SCRIPT: &str = include_str!("shell_restore.sh");

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "ShellSnapshotRaw")]
pub(crate) struct ShellSnapshot {
    pub(crate) session: String,
    pub(crate) cwd: String,
    pub(crate) transcript: String,
    pub(crate) updated: u64,
}

#[derive(Deserialize)]
struct ShellSnapshotRaw {
    session: String,
    cwd: String,
    transcript: String,
    updated: u64,
}

impl TryFrom<ShellSnapshotRaw> for ShellSnapshot {
    type Error = String;

    fn try_from(raw: ShellSnapshotRaw) -> Result<Self, String> {
        validate_session_name(&raw.session)?;
        if !valid_cwd(&raw.cwd) {
            return Err("snapshot cwd must be a bounded absolute path without controls".into());
        }
        if raw.transcript.len() > MAX_TRANSCRIPT_BYTES
            || raw
                .transcript
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return Err("snapshot transcript is unsafe or exceeds its size limit".into());
        }
        Ok(Self {
            session: raw.session,
            cwd: raw.cwd,
            transcript: raw.transcript,
            updated: raw.updated,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ShellObservation {
    pub(crate) session: String,
    pub(crate) activity: u64,
    pub(crate) cwd: String,
    pub(crate) foreground: String,
}

#[derive(Clone)]
struct SavedObservation {
    activity: u64,
    cwd: String,
    at: u64,
}

#[derive(Default)]
struct Tracker {
    busy: bool,
    last_schedule: u64,
    epoch: u64,
    saved: HashMap<String, SavedObservation>,
}

static TRACKER: LazyLock<Mutex<Tracker>> = LazyLock::new(|| Mutex::new(Tracker::default()));
static SNAPSHOT_IO: Mutex<()> = Mutex::new(());

fn snapshot_dir() -> PathBuf {
    storage::deck_dir().join("shell-state")
}

fn snapshot_path_in(dir: &Path, session: &str) -> Result<PathBuf, String> {
    validate_session_name(session)?;
    Ok(dir.join(format!("{session}.json")))
}

fn snapshot_path(session: &str) -> Result<PathBuf, String> {
    snapshot_path_in(&snapshot_dir(), session)
}

fn valid_cwd(cwd: &str) -> bool {
    !cwd.is_empty()
        && cwd.len() <= 4096
        && Path::new(cwd).is_absolute()
        && !cwd.chars().any(char::is_control)
}

fn checkpoint_eligible(observation: &ShellObservation) -> bool {
    crate::context::shell_process(Some(&observation.foreground)) && valid_cwd(&observation.cwd)
}

/// Strip every terminal control (including ESC/OSC and CR), retain only
/// printable text, tabs and LF, then keep a bounded tail.  A persisted pane
/// can therefore never execute OSC 52, hyperlinks, title changes, or other
/// terminal actions when printed into a restored pane.
pub(crate) fn sanitize_transcript(raw: &str) -> String {
    let printable: String = raw
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let mut lines: Vec<&str> = printable.rsplit('\n').take(MAX_TRANSCRIPT_LINES).collect();
    lines.reverse();
    let mut private_key_block = false;
    let redacted = lines
        .into_iter()
        .filter_map(|line| {
            if line.contains("-----BEGIN ") && line.contains(" PRIVATE KEY-----") {
                private_key_block = true;
                return Some("<redacted private key block>".to_string());
            }
            if private_key_block {
                if line.contains("-----END ") && line.contains(" PRIVATE KEY-----") {
                    private_key_block = false;
                }
                return None;
            }
            Some(storage::redact_credentials(line))
        })
        .collect::<Vec<_>>()
        .join("\n");
    bound_tail(&redacted, MAX_TRANSCRIPT_BYTES)
}

fn bound_tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    // Prefer not to begin with half of a very long output line.
    match tail.find('\n') {
        Some(i) if i + 1 < tail.len() => tail[i + 1..].to_string(),
        _ => tail.to_string(),
    }
}

fn load_snapshot_from(path: &Path) -> Result<Option<ShellSnapshot>, String> {
    let Some(outcome) = storage::load_typed::<ShellSnapshot>(path)? else {
        return Ok(None);
    };
    if let Some(warning) = outcome.warning {
        storage::warn(format!("shell recovery snapshot recovered: {warning}"));
    }
    let snapshot: ShellSnapshot =
        serde_json::from_str(&outcome.payload).map_err(|e| e.to_string())?;
    if now_epoch().saturating_sub(snapshot.updated) > MAX_SNAPSHOT_AGE_SECS {
        if let Some(dir) = path.parent() {
            remove_snapshot_files_in(dir, &snapshot.session)?;
        }
        return Ok(None);
    }
    Ok(Some(snapshot))
}

pub(crate) fn load_snapshot(session: &str) -> Result<Option<ShellSnapshot>, String> {
    let snapshot = load_snapshot_from(&snapshot_path(session)?)?;
    if snapshot
        .as_ref()
        .map(|saved| saved.session.as_str() != session)
        .unwrap_or(false)
    {
        return Err("shell snapshot belongs to a different session".into());
    }
    Ok(snapshot)
}

/// Optional recovery data for a user-opened, command-less shell.  A launch
/// command is intentionally excluded: agent/program startup owns its own
/// recovery semantics and deck must not put an old shell transcript over it.
pub(crate) fn snapshot_for_start(session: &str, cmd: &str, enabled: bool) -> Option<ShellSnapshot> {
    if !enabled || !cmd.trim().is_empty() {
        return None;
    }
    match load_snapshot(session) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            applog(&format!(
                "[shell-state] recovery unavailable for {} ({})",
                storage::session_tag(session),
                storage::err_code(&error)
            ));
            None
        }
    }
}

pub(crate) fn note_recovered(session: &str) {
    let mut tracker = TRACKER.lock().unwrap();
    // The tmux generation is new even if activity happens to reuse the same
    // epoch second.  Force its first shell checkpoint.
    tracker.saved.remove(session);
}

pub(crate) struct ShellBootstrap {
    pub(crate) buffer: String,
    pub(crate) output: Vec<u8>,
    pub(crate) shell: String,
    pub(crate) login_name: String,
}

fn executable_shell(path: &Path) -> bool {
    path.is_absolute()
        && path
            .metadata()
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

fn passwd_shell() -> Option<PathBuf> {
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        if libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
            || entry.pw_shell.is_null()
        {
            return None;
        }
        let bytes = CStr::from_ptr(entry.pw_shell).to_bytes();
        (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
}

fn login_shell() -> Result<PathBuf, String> {
    let inherited = std::env::var_os("SHELL").map(PathBuf::from);
    inherited
        .into_iter()
        .chain(passwd_shell())
        .chain([PathBuf::from("/bin/zsh"), PathBuf::from("/bin/sh")])
        .find(|path| executable_shell(path))
        .ok_or_else(|| "could not find an executable login shell".into())
}

fn restore_buffer_name(attempt: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "deck-restore-{:x}-{nanos:x}-{attempt:x}",
        std::process::id()
    )
}

/// Build a one-use private tmux buffer payload. The snapshot JSON itself is
/// never exposed to the pane, and the transcript is sanitized again here so
/// this path remains safe if its caller changes later.
pub(crate) fn prepare_bootstrap(snapshot: &ShellSnapshot) -> Result<ShellBootstrap, String> {
    let transcript = sanitize_transcript(&snapshot.transcript);
    if transcript.trim().is_empty() {
        return Err("shell snapshot transcript is empty".into());
    }
    let to_utf8 = |path: PathBuf, kind: &str| {
        path.into_os_string()
            .into_string()
            .map_err(|_| format!("{kind} path is not UTF-8"))
    };
    let shell = to_utf8(login_shell()?, "shell")?;
    let login_name = Path::new(&shell)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("-{name}"))
        .unwrap_or_else(|| "-sh".into());
    let mut output = Vec::with_capacity(transcript.len() + RESTORE_BOUNDARY.len() + 1);
    output.extend_from_slice(transcript.as_bytes());
    if !transcript.ends_with('\n') {
        output.push(b'\n');
    }
    output.extend_from_slice(RESTORE_BOUNDARY);
    let nonce = BOOTSTRAP_NONCE.fetch_add(1, Ordering::Relaxed);
    Ok(ShellBootstrap {
        buffer: restore_buffer_name(nonce),
        output,
        shell,
        login_name,
    })
}

/// Compatibility cleanup for one-use `.restore-*` files left by deck <=
/// 0.5.1. New recovery uses an in-memory tmux buffer and creates no temp file.
pub(crate) fn cleanup_restore_temps() {
    let dir = snapshot_dir();
    storage::prune_old_files(&dir, MAX_SNAPSHOT_AGE_SECS);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".restore-")
            && name.ends_with(".txt")
            && entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(".bak");
    PathBuf::from(os)
}

fn remove_snapshot_files_in(dir: &Path, session: &str) -> Result<(), String> {
    let main = snapshot_path_in(dir, session)?;
    for path in [main.clone(), backup_path(&main)] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("could not remove shell snapshot ({})", e.kind())),
        }
    }
    // Also erase quarantined copies for this session.  They can contain the
    // same private terminal text even though they are no longer loadable.
    let corrupt_prefix = format!("{session}.corrupt-");
    let restore_prefix = format!(".restore-{session}-");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with(&corrupt_prefix) || name.starts_with(&restore_prefix))
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

pub(crate) fn clear_snapshot(session: &str) -> Result<(), String> {
    validate_session_name(session)?;
    {
        let mut tracker = TRACKER.lock().unwrap();
        tracker.epoch = tracker.epoch.wrapping_add(1);
        tracker.saved.remove(session);
    }
    let _io = SNAPSHOT_IO.lock().unwrap();
    remove_snapshot_files_in(&snapshot_dir(), session)
}

/// Privacy control: erase main, backup and quarantined snapshot files.  The
/// epoch + IO lock makes disabling race-safe with an already-running capture.
pub(crate) fn clear_all() -> Result<(), String> {
    {
        let mut tracker = TRACKER.lock().unwrap();
        tracker.epoch = tracker.epoch.wrapping_add(1);
        tracker.saved.clear();
    }
    let _io = SNAPSHOT_IO.lock().unwrap();
    let dir = snapshot_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue; // never follow a symlink out of the private directory
        }
        std::fs::remove_file(entry.path())
            .map_err(|e| format!("could not clear shell snapshots ({})", e.kind()))?;
    }
    Ok(())
}

fn prune_snapshot_count(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut mains: Vec<_> = entries
        .flatten()
        .filter(|entry| {
            entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && entry.path().extension().and_then(|x| x.to_str()) == Some("json")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    mains.sort_by_key(|(modified, _)| *modified);
    while mains.len() >= MAX_SNAPSHOT_FILES {
        let (_, oldest) = mains.remove(0);
        if oldest == keep {
            continue;
        }
        let _ = std::fs::remove_file(&oldest);
        let _ = std::fs::remove_file(backup_path(&oldest));
    }
}

fn save_snapshot(snapshot: &ShellSnapshot, epoch: u64) -> Result<bool, String> {
    let path = snapshot_path(&snapshot.session)?;
    let _io = SNAPSHOT_IO.lock().unwrap();
    if TRACKER.lock().unwrap().epoch != epoch {
        return Ok(false); // disabled/cleared/closed while capture was running
    }
    if let Some(dir) = path.parent() {
        storage::create_private_dir(dir)?;
        prune_snapshot_count(dir, &path);
    }
    let raw = serde_json::to_string(snapshot).map_err(|e| e.to_string())?;
    storage::save_typed_ephemeral::<ShellSnapshot>(&path, &raw)?;
    Ok(true)
}

fn capture(observation: &ShellObservation) -> Result<ShellSnapshot, String> {
    let raw = tmux(&[
        "capture-pane",
        "-p",
        "-J",
        "-t",
        &pane_target(&observation.session),
        "-S",
        &format!("-{MAX_TRANSCRIPT_LINES}"),
    ])?;
    Ok(ShellSnapshot {
        session: observation.session.clone(),
        cwd: observation.cwd.clone(),
        transcript: sanitize_transcript(&raw),
        updated: now_epoch(),
    })
}

/// Select a small fair batch and checkpoint it off the poll request thread.
/// The caller invokes this every 2.5s, but disk/capture work runs at most once
/// per 15s and at most two sessions per run.  Only changed idle-shell panes
/// are eligible; command output becomes eligible after the command returns to
/// the shell, while agent/full-screen processes are never captured.
pub(crate) fn schedule_checkpoints(observations: Vec<ShellObservation>, enabled: bool) {
    if !enabled {
        return;
    }
    let now = now_epoch();
    let (epoch, work): (u64, Vec<ShellObservation>) = {
        let mut tracker = TRACKER.lock().unwrap();
        if tracker.busy || now.saturating_sub(tracker.last_schedule) < CHECKPOINT_INTERVAL_SECS {
            return;
        }
        tracker.last_schedule = now;
        let mut dirty: Vec<_> = observations
            .into_iter()
            .filter(checkpoint_eligible)
            .filter(|o| {
                tracker.saved.get(&o.session).is_none_or(|saved| {
                    saved.activity != o.activity
                        || saved.cwd != o.cwd
                        || !snapshot_path(&o.session)
                            .map(|p| p.exists())
                            .unwrap_or(false)
                })
            })
            .collect();
        // Least-recently-checkpointed first prevents two noisy shells from
        // starving a quiet card that has no first snapshot yet.
        dirty.sort_by_key(|o| {
            (
                tracker.saved.get(&o.session).map(|s| s.at).unwrap_or(0),
                o.session.clone(),
            )
        });
        dirty.truncate(MAX_CHECKPOINTS_PER_TICK);
        if dirty.is_empty() {
            return;
        }
        tracker.busy = true;
        let epoch = tracker.epoch;
        (epoch, dirty)
    };

    std::thread::spawn(move || {
        let mut saved_observations = Vec::new();
        for observation in work {
            match capture(&observation)
                .and_then(|snapshot| save_snapshot(&snapshot, epoch).map(|saved| (snapshot, saved)))
            {
                Ok((snapshot, true)) => {
                    applog(&format!(
                        "[shell-state] checkpointed {} bytes={}B",
                        storage::session_tag(&snapshot.session),
                        snapshot.transcript.len()
                    ));
                    saved_observations.push((observation, snapshot.updated));
                }
                Ok((_, false)) => break,
                Err(error) => applog(&format!(
                    "[shell-state] checkpoint failed for {} ({})",
                    storage::session_tag(&observation.session),
                    storage::err_code(&error)
                )),
            }
        }
        let mut tracker = TRACKER.lock().unwrap();
        if tracker.epoch == epoch {
            for (observation, at) in saved_observations {
                tracker.saved.insert(
                    observation.session,
                    SavedObservation {
                        activity: observation.activity,
                        cwd: observation.cwd,
                        at,
                    },
                );
            }
        }
        tracker.busy = false;
    });
}

#[tauri::command]
pub(crate) fn shell_snapshots_clear() -> Result<(), String> {
    clear_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deck-shell-state-{label}-{}-{}",
            std::process::id(),
            now_epoch()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn transcript_is_inert_and_bounded_to_the_newest_output() {
        let hostile = format!("old\x1b]52;c;secret\x07\r\n{}new", "x\n".repeat(4000));
        let clean = sanitize_transcript(&hostile);
        assert!(!clean.contains('\x1b'));
        assert!(!clean.contains('\x07'));
        assert!(!clean.contains('\r'));
        assert!(clean.ends_with("new"));
        assert!(clean.len() <= MAX_TRANSCRIPT_BYTES);
        assert!(clean.lines().count() <= MAX_TRANSCRIPT_LINES);
    }

    #[test]
    fn transcript_redacts_credentials_but_keeps_useful_paths_and_urls() {
        let raw = concat!(
            "cwd=/Users/example/project\n",
            "docs=https://example.com/guide\n",
            "OPENAI_API_KEY=sk-example12345678901234567890\n",
            "Authorization: Bearer github_pat_example12345678901234567890\n",
            "-----BEGIN OPENSSH PRIVATE KEY-----\n",
            "private-material-12345678901234567890\n",
            "-----END OPENSSH PRIVATE KEY-----\n",
            "done\n",
        );
        let clean = sanitize_transcript(raw);
        assert!(clean.contains("/Users/example/project"));
        assert!(clean.contains("https://example.com/guide"));
        assert!(!clean.contains("sk-example"));
        assert!(!clean.contains("github_pat_"));
        assert!(!clean.contains("private-material"));
        assert!(clean.contains("<redacted private key block>"));
        assert!(clean.ends_with("done\n"));
    }

    #[test]
    fn bootstrap_buffer_is_bounded_sanitized_and_argument_safe() {
        let snapshot = ShellSnapshot {
            session: "deck-shell-test".into(),
            cwd: "/tmp".into(),
            transcript: "literal: echo should-not-run\nred\x1b[31m\r\n".into(),
            updated: 1,
        };
        let bootstrap = prepare_bootstrap(&snapshot).expect("bootstrap");
        let restored = String::from_utf8(bootstrap.output).expect("utf8 output");
        assert!(restored.contains("echo should-not-run"));
        assert!(restored.contains("deck restart"));
        assert!(!restored.contains('\x1b'));
        assert!(!restored.contains('\r'));
        assert!(bootstrap.buffer.starts_with("deck-restore-"));
        assert!(bootstrap
            .buffer
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'));
        assert_eq!(bootstrap.shell, login_shell().unwrap().to_string_lossy());
        assert!(bootstrap.login_name.starts_with('-'));
    }

    #[test]
    fn typed_snapshot_is_private_and_clear_removes_backup_and_quarantine() {
        let dir = temp_dir("private");
        let path = snapshot_path_in(&dir, "deck-shell-test").unwrap();
        let snapshot = ShellSnapshot {
            session: "deck-shell-test".into(),
            cwd: "/tmp".into(),
            transcript: "one".into(),
            updated: 1,
        };
        let raw = serde_json::to_string(&snapshot).unwrap();
        storage::save_typed::<ShellSnapshot>(&path, &raw).unwrap();
        let snapshot2 = ShellSnapshot {
            transcript: "two".into(),
            ..snapshot
        };
        storage::save_typed::<ShellSnapshot>(&path, &serde_json::to_string(&snapshot2).unwrap())
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let corrupt = dir.join("deck-shell-test.corrupt-1");
        let restore = dir.join(".restore-deck-shell-test-1.txt");
        std::fs::write(&corrupt, "private").unwrap();
        std::fs::write(&restore, "private").unwrap();
        remove_snapshot_files_in(&dir, "deck-shell-test").unwrap();
        assert!(!path.exists());
        assert!(!backup_path(&path).exists());
        assert!(!corrupt.exists());
        assert!(!restore.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ephemeral_snapshot_save_removes_legacy_backup() {
        let dir = temp_dir("ephemeral");
        let path = snapshot_path_in(&dir, "deck-shell-test").unwrap();
        let snapshot = ShellSnapshot {
            session: "deck-shell-test".into(),
            cwd: "/tmp".into(),
            transcript: "one".into(),
            updated: now_epoch(),
        };
        let raw = serde_json::to_string(&snapshot).unwrap();
        storage::save_typed::<ShellSnapshot>(&path, &raw).unwrap();
        storage::save_typed::<ShellSnapshot>(&path, &raw).unwrap();
        assert!(backup_path(&path).exists());
        storage::save_typed_ephemeral::<ShellSnapshot>(&path, &raw).unwrap();
        assert!(path.exists());
        assert!(!backup_path(&path).exists());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn expired_snapshot_is_removed_instead_of_restored() {
        let dir = temp_dir("expired");
        let path = snapshot_path_in(&dir, "deck-shell-test").unwrap();
        let snapshot = ShellSnapshot {
            session: "deck-shell-test".into(),
            cwd: "/tmp".into(),
            transcript: "old output".into(),
            updated: now_epoch().saturating_sub(MAX_SNAPSHOT_AGE_SECS + 1),
        };
        storage::save_typed_ephemeral::<ShellSnapshot>(
            &path,
            &serde_json::to_string(&snapshot).unwrap(),
        )
        .unwrap();
        assert!(load_snapshot_from(&path).unwrap().is_none());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_snapshot_structure_is_rejected() {
        assert!(serde_json::from_str::<ShellSnapshot>(
            r#"{"session":"../escape","cwd":"/tmp","transcript":"ok","updated":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ShellSnapshot>(
            r#"{"session":"safe","cwd":"relative","transcript":"ok","updated":1}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ShellSnapshot>(
            "{\"session\":\"safe\",\"cwd\":\"/tmp\",\"transcript\":\"\\u001b[31m\",\"updated\":1}"
        )
        .is_err());
    }

    #[test]
    fn only_an_idle_shell_is_checkpoint_eligible() {
        let observation = |foreground: &str, cwd: &str| ShellObservation {
            session: "deck-shell-test".into(),
            activity: 1,
            cwd: cwd.into(),
            foreground: foreground.into(),
        };
        assert!(checkpoint_eligible(&observation("zsh", "/tmp")));
        assert!(checkpoint_eligible(&observation("bash", "/tmp")));
        assert!(!checkpoint_eligible(&observation("codex", "/tmp")));
        assert!(!checkpoint_eligible(&observation("claude", "/tmp")));
        assert!(!checkpoint_eligible(&observation("zsh", "relative")));
    }

    #[test]
    fn snapshot_paths_tail_bounds_and_shell_detection_are_defensive() {
        let dir = temp_dir("paths");
        assert_eq!(
            snapshot_path_in(&dir, "deck-shell-safe").unwrap(),
            dir.join("deck-shell-safe.json")
        );
        for bad in ["", "../escape", "has:colon", "has space"] {
            assert!(snapshot_path_in(&dir, bad).is_err(), "accepted {bad:?}");
        }
        for bad in [
            "",
            "relative",
            "/tmp/line\nbreak",
            &format!("/{}", "x".repeat(4096)),
        ] {
            assert!(!valid_cwd(bad), "accepted cwd {bad:?}");
        }
        assert!(valid_cwd("/tmp/project"));

        assert_eq!(bound_tail("short", 10), "short");
        assert_eq!(bound_tail("old line\nnew line", 9), "new line");
        let unicode = bound_tail("prefix\nαβγδε", 9);
        assert!(std::str::from_utf8(unicode.as_bytes()).is_ok());
        assert!(unicode.len() <= 9);

        assert!(executable_shell(Path::new("/bin/sh")));
        let plain = dir.join("plain");
        std::fs::write(&plain, "not executable").unwrap();
        assert!(!executable_shell(&plain));
        assert!(!executable_shell(Path::new("relative")));
        let name = restore_buffer_name(42);
        assert!(name.starts_with("deck-restore-"));
        assert!(name.ends_with("-2a"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn current_snapshot_round_trips_and_missing_snapshot_is_none() {
        let dir = temp_dir("load");
        let path = snapshot_path_in(&dir, "deck-shell-current").unwrap();
        assert!(load_snapshot_from(&path).unwrap().is_none());
        let snapshot = ShellSnapshot {
            session: "deck-shell-current".into(),
            cwd: "/tmp".into(),
            transcript: "recent output".into(),
            updated: now_epoch(),
        };
        storage::save_typed_ephemeral::<ShellSnapshot>(
            &path,
            &serde_json::to_string(&snapshot).unwrap(),
        )
        .unwrap();
        let loaded = load_snapshot_from(&path).unwrap().unwrap();
        assert_eq!(loaded.session, snapshot.session);
        assert_eq!(loaded.cwd, snapshot.cwd);
        assert_eq!(loaded.transcript, snapshot.transcript);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_pruning_keeps_the_directory_strictly_bounded() {
        let dir = temp_dir("prune");
        let keep = dir.join("keep.json");
        std::fs::write(&keep, "keep").unwrap();
        for index in 0..MAX_SNAPSHOT_FILES + 3 {
            std::fs::write(dir.join(format!("snapshot-{index:03}.json")), "x").unwrap();
        }
        std::fs::write(dir.join("ignored.txt"), "x").unwrap();
        prune_snapshot_count(&dir, &keep);
        let json_count = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count();
        assert!(json_count <= MAX_SNAPSHOT_FILES);
        assert!(keep.exists(), "the in-flight destination is never pruned");
        assert!(dir.join("ignored.txt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recovered_sessions_are_removed_from_checkpoint_deduplication() {
        let name = format!("deck-shell-recovered-{}", std::process::id());
        TRACKER.lock().unwrap().saved.insert(
            name.clone(),
            SavedObservation {
                activity: 1,
                cwd: "/tmp".into(),
                at: 1,
            },
        );
        note_recovered(&name);
        assert!(!TRACKER.lock().unwrap().saved.contains_key(&name));
    }

    #[test]
    fn recovery_short_circuits_before_global_io_when_disabled_or_inapplicable() {
        assert!(snapshot_for_start("deck-shell-safe", "", false).is_none());
        assert!(snapshot_for_start("deck-shell-safe", "codex", true).is_none());
        assert!(clear_snapshot("bad:name").is_err());

        let empty = ShellSnapshot {
            session: "deck-shell-safe".into(),
            cwd: "/tmp".into(),
            transcript: "\n\t".into(),
            updated: now_epoch(),
        };
        assert_eq!(
            prepare_bootstrap(&empty).err().unwrap(),
            "shell snapshot transcript is empty"
        );

        let missing =
            std::env::temp_dir().join(format!("deck-shell-no-prune-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        prune_snapshot_count(&missing, &missing.join("keep.json"));
    }
}
