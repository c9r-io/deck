//! Bounded, inert shell recovery across a tmux/server or machine restart.
//!
//! tmux remains the live terminal authority.  This module checkpoints only
//! panes whose foreground process is a shell, and stores:
//! - the pane's current working directory;
//! - a bounded, plain-text tail of tmux scrollback.
//!
//! Recovery never replays input and never attempts to serialize processes,
//! jobs, environment variables, shell options, or an agent TUI.  The saved
//! transcript is returned to the frontend as inert xterm scrollback before a
//! fresh tmux client attaches.  Every snapshot is a separate typed/atomic
//! 0600 file, so a busy shell does not rewrite every other session's output.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::storage::{self, applog, now_epoch};
use crate::tmux::{pane_target, tmux, validate_session_name};

pub(crate) const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;
const MAX_TRANSCRIPT_LINES: usize = 3000;
const CHECKPOINT_INTERVAL_SECS: u64 = 15;
const MAX_CHECKPOINTS_PER_TICK: usize = 2;
const MAX_SNAPSHOT_FILES: usize = 128;

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
    /// A transcript recovered into a NEW shell is kept as a prefix while
    /// that shell lives.  Later checkpoints become prefix + current tmux
    /// capture, so another restart does not immediately lose the older part.
    recovered_prefixes: HashMap<String, String>,
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
/// terminal actions when written back into xterm.
pub(crate) fn sanitize_transcript(raw: &str) -> String {
    let printable: String = raw
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let mut lines: Vec<&str> = printable.rsplit('\n').take(MAX_TRANSCRIPT_LINES).collect();
    lines.reverse();
    bound_tail(&lines.join("\n"), MAX_TRANSCRIPT_BYTES)
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

pub(crate) fn merge_transcripts(prefix: &str, current: &str) -> String {
    match (prefix.trim().is_empty(), current.trim().is_empty()) {
        (true, _) => sanitize_transcript(current),
        (_, true) => sanitize_transcript(prefix),
        (false, false) => sanitize_transcript(&format!("{prefix}\n\n{current}")),
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

pub(crate) fn note_recovered(snapshot: &ShellSnapshot) {
    let mut tracker = TRACKER.lock().unwrap();
    if snapshot.transcript.is_empty() {
        tracker.recovered_prefixes.remove(&snapshot.session);
    } else {
        tracker
            .recovered_prefixes
            .insert(snapshot.session.clone(), snapshot.transcript.clone());
    }
    // The tmux generation is new even if activity happens to reuse the same
    // epoch second.  Force its first shell checkpoint.
    tracker.saved.remove(&snapshot.session);
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
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&corrupt_prefix)
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
        tracker.recovered_prefixes.remove(session);
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
        tracker.recovered_prefixes.clear();
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
    storage::save_typed::<ShellSnapshot>(&path, &raw)?;
    Ok(true)
}

fn capture(observation: &ShellObservation, prefix: &str) -> Result<ShellSnapshot, String> {
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
        transcript: merge_transcripts(prefix, &raw),
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
    let (epoch, work): (u64, Vec<(ShellObservation, String)>) = {
        let mut tracker = TRACKER.lock().unwrap();
        if tracker.busy || now.saturating_sub(tracker.last_schedule) < CHECKPOINT_INTERVAL_SECS {
            return;
        }
        tracker.last_schedule = now;
        let mut dirty: Vec<_> = observations
            .into_iter()
            .filter(|o| checkpoint_eligible(o))
            .filter(|o| {
                tracker.saved.get(&o.session).map_or(true, |saved| {
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
        let work = dirty
            .into_iter()
            .map(|o| {
                let prefix = tracker
                    .recovered_prefixes
                    .get(&o.session)
                    .cloned()
                    .unwrap_or_default();
                (o, prefix)
            })
            .collect();
        (epoch, work)
    };

    std::thread::spawn(move || {
        let mut saved_observations = Vec::new();
        for (observation, prefix) in work {
            match capture(&observation, &prefix)
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
pub(crate) fn load_shell_snapshot(name: String) -> Result<Option<ShellSnapshot>, String> {
    load_snapshot(&name)
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
    fn cumulative_recovery_keeps_old_and_new_tails_under_one_cap() {
        let merged = merge_transcripts("before reboot", "after reboot");
        assert!(merged.contains("before reboot"));
        assert!(merged.contains("after reboot"));
        let large = merge_transcripts(&"a".repeat(MAX_TRANSCRIPT_BYTES), "latest");
        assert!(large.len() <= MAX_TRANSCRIPT_BYTES);
        assert!(large.ends_with("latest"));
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
        std::fs::write(&corrupt, "private").unwrap();
        remove_snapshot_files_in(&dir, "deck-shell-test").unwrap();
        assert!(!path.exists());
        assert!(!backup_path(&path).exists());
        assert!(!corrupt.exists());
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
}
