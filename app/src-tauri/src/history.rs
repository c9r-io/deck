//! Command history for completion: deck records commands typed in its own
//! shells (zsh history only flushes on shell exit, so it can't be the live
//! source); recency is frequency-boosted.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::storage;
use crate::storage::now_epoch;
use crate::sync::LockRecover;

/// history.json is read-modify-write; without this in-process lock two quick
/// successive commands could each read the same base and lose an update.
static HIST_LOCK: Mutex<()> = Mutex::new(());

// ---------- command history -----------------------------------------------------

pub(crate) fn deck_history_path() -> PathBuf {
    storage::deck_dir().join("history.json")
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct HistEntry {
    cmd: String,
    n: u32,
    last: u64,
}

/// Frequency-boosted recency: each past use is worth an hour of freshness.
pub(crate) fn hist_score(e: &HistEntry) -> u64 {
    e.last + (e.n as u64) * 3600
}

/// Either current entries or the legacy v1 plain string array.
#[derive(Deserialize)]
#[serde(untagged)]
enum HistDoc {
    V2(Vec<HistEntry>),
    V1(Vec<String>),
}

fn read_history_from(path: &Path) -> Vec<HistEntry> {
    let raw = match storage::load_typed::<HistDoc>(path) {
        Ok(Some(o)) => {
            if let Some(w) = o.warning {
                storage::warn(w);
            }
            o.payload
        }
        Ok(None) => return Vec::new(),
        Err(e) => {
            storage::warn(format!("command history could not be loaded: {e}"));
            return Vec::new();
        }
    };
    let now = now_epoch();
    match serde_json::from_str::<HistDoc>(&raw) {
        Ok(HistDoc::V2(v)) => v,
        Ok(HistDoc::V1(v)) => v
            .into_iter()
            .map(|cmd| HistEntry {
                cmd,
                n: 1,
                last: now,
            })
            .collect(),
        Err(_) => Vec::new(), // unreachable: load_typed already validated
    }
}

pub(crate) fn read_deck_history() -> Vec<HistEntry> {
    read_history_from(&deck_history_path())
}

/// Strip zsh EXTENDED_HISTORY prefix ": 1756…:0;cmd" → "cmd".
pub(crate) fn strip_zsh_prefix(line: &str) -> &str {
    if line.starts_with(": ") {
        if let Some(i) = line.find(';') {
            return &line[i + 1..];
        }
    }
    line
}

pub(crate) fn usable_command(cmd: &str) -> bool {
    let c = cmd.trim();
    c.len() >= 2 && c.len() <= 120 && !c.contains('\n')
}

/// Candidates for the quick-command chips: commands deck itself launched
/// (most recent first), then the user's shell history, deduped.
fn merge_recent_commands(
    mut own: Vec<HistEntry>,
    shell_history: Option<&[u8]>,
    limit: usize,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |cmd: &str| {
        let c = cmd.trim().to_string();
        if usable_command(&c) && seen.insert(c.clone()) {
            out.push(c);
        }
    };

    own.sort_by_key(|e| std::cmp::Reverse(hist_score(e)));
    for e in own {
        push(&e.cmd);
    }
    if let Some(bytes) = shell_history {
        let text = String::from_utf8_lossy(bytes);
        // newest entries are at the end; skip continuation lines
        for line in text.lines().rev().take(400) {
            if line.ends_with('\\') || line.contains('\u{fffd}') {
                continue;
            }
            push(strip_zsh_prefix(line));
        }
    }
    out.truncate(limit.max(1));
    out
}

#[tauri::command]
pub(crate) fn recent_commands(limit: usize) -> Vec<String> {
    let shell_history = dirs::home_dir().and_then(|home| {
        [".zsh_history", ".bash_history"]
            .into_iter()
            .find_map(|file| std::fs::read(home.join(file)).ok())
    });
    merge_recent_commands(read_deck_history(), shell_history.as_deref(), limit)
}

/// Record a command run in a deck shell (typed, completed, or injected).
/// deck owns its history: shells inside tmux sessions stay alive, so the
/// user's ~/.zsh_history only fills on shell exit — too late for completion.
pub(crate) fn record_into(path: &Path, cmd: &str) -> Result<(), String> {
    let _g = HIST_LOCK.lock_or_recover(); // serialize read-modify-write
    let mut hist = read_history_from(path);
    if let Some(e) = hist.iter_mut().find(|e| e.cmd == cmd) {
        e.n += 1;
        e.last = now_epoch();
    } else {
        hist.push(HistEntry {
            cmd: cmd.to_string(),
            n: 1,
            last: now_epoch(),
        });
    }
    hist.sort_by_key(|e| std::cmp::Reverse(hist_score(e)));
    hist.truncate(500);
    let raw = serde_json::to_string(&hist).map_err(|e| e.to_string())?;
    // full shell commands are user content; storage::save creates every
    // artifact (main, .bak, temp) 0600 by construction
    storage::save_typed::<Vec<HistEntry>>(path, &raw)
}

#[tauri::command]
pub(crate) fn record_command(cmd: String) -> Result<(), String> {
    if !usable_command(&cmd) {
        return Ok(());
    }
    record_into(&deck_history_path(), cmd.trim())
}

/// Wipe history at `path`: empty the main file AND delete the .bak (which
/// still holds the old commands — clearing means clearing).
pub(crate) fn clear_into(path: &Path) -> Result<(), String> {
    let _g = HIST_LOCK.lock_or_recover();
    storage::save_typed::<Vec<HistEntry>>(path, "[]")?;
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let _ = std::fs::remove_file(PathBuf::from(bak));
    Ok(())
}

/// Wipe deck's own command history (file + backup) — Settings → privacy.
#[tauri::command]
pub(crate) fn history_clear() -> Result<(), String> {
    clear_into(&deck_history_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_records_lose_no_update() {
        let d = std::env::temp_dir().join(format!("deck-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("history.json");
        std::thread::scope(|s| {
            for k in 0..4 {
                let p = p.clone();
                s.spawn(move || {
                    for i in 0..10 {
                        record_into(&p, &format!("cmd-{k}-{i}")).unwrap();
                    }
                });
            }
        });
        assert_eq!(read_history_from(&p).len(), 40, "every record survived");
    }

    #[test]
    fn history_files_are_user_only_and_clear_removes_backup() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let d = std::env::temp_dir().join(format!("deck-hist-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("history.json");
        record_into(&p, "first-command").unwrap();
        record_into(&p, "second-command").unwrap(); // creates the .bak
        let bak = d.join("history.json.bak");
        assert_eq!(mode(&p), 0o600, "history main");
        assert_eq!(mode(&bak), 0o600, "history backup");
        // clearing wipes the main file AND removes the backup's old commands
        clear_into(&p).unwrap();
        assert!(!bak.exists(), "backup removed on clear");
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(!after.contains("first-command"), "content gone");
        assert_eq!(mode(&p), 0o600, "cleared file still private");
    }

    #[test]
    fn legacy_v1_string_array_migrates() {
        let d = std::env::temp_dir().join(format!("deck-hist-v1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("history.json");
        std::fs::write(&p, r#"["ls","make"]"#).unwrap();
        let v = read_history_from(&p);
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|e| e.n == 1));
    }

    #[test]
    fn recent_candidates_rank_dedupe_filter_and_bound_shell_history() {
        let own = vec![
            HistEntry {
                cmd: "cargo test".into(),
                n: 1,
                last: 10,
            },
            HistEntry {
                cmd: "git status".into(),
                n: 3,
                last: 1,
            },
            HistEntry {
                cmd: "x".into(),
                n: 99,
                last: 99,
            },
        ];
        let shell = b"old command\n: 1756000000:0;cargo test\ncontinued\\\n\xffbad\nnew command\n";
        let merged = merge_recent_commands(own, Some(shell), 4);
        assert_eq!(
            merged,
            ["git status", "cargo test", "new command", "old command"]
        );
        assert_eq!(
            merge_recent_commands(Vec::new(), None, 0),
            Vec::<String>::new()
        );
    }

    #[test]
    fn history_syntax_and_scoring_are_closed_and_duplicate_records_accumulate() {
        assert_eq!(strip_zsh_prefix(": 1756000000:0;cargo test"), "cargo test");
        assert_eq!(strip_zsh_prefix(": malformed"), ": malformed");
        assert_eq!(strip_zsh_prefix("plain"), "plain");
        assert!(usable_command("ok"));
        assert!(!usable_command("x"));
        assert!(!usable_command("line one\nline two"));
        assert!(!usable_command(&"x".repeat(121)));
        assert_eq!(
            hist_score(&HistEntry {
                cmd: "x".into(),
                n: 2,
                last: 10,
            }),
            7210
        );

        let dir = std::env::temp_dir().join(format!("deck-history-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        record_into(&path, "cargo test").unwrap();
        record_into(&path, "cargo test").unwrap();
        let loaded = read_history_from(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].cmd, "cargo test");
        assert_eq!(loaded[0].n, 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
