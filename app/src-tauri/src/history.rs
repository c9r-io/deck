//! Command history for completion: deck records commands typed in its own
//! shells (zsh history only flushes on shell exit, so it can't be the live
//! source); recency is frequency-boosted.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::storage;
use crate::storage::now_epoch;

/// history.json is read-modify-write; without this in-process lock two quick
/// successive commands could each read the same base and lose an update.
static HIST_LOCK: Mutex<()> = Mutex::new(());

// ---------- command history -----------------------------------------------------

pub(crate) fn deck_history_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("history.json")
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
#[tauri::command]
pub(crate) fn recent_commands(limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |cmd: &str| {
        let c = cmd.trim().to_string();
        if usable_command(&c) && seen.insert(c.clone()) {
            out.push(c);
        }
    };

    let mut own = read_deck_history();
    own.sort_by_key(|e| std::cmp::Reverse(hist_score(e)));
    for e in own {
        push(&e.cmd);
    }
    if let Some(home) = dirs::home_dir() {
        for file in [".zsh_history", ".bash_history"] {
            // zsh history can contain non-UTF-8 (metafied) bytes — read lossily
            if let Ok(bytes) = std::fs::read(home.join(file)) {
                let text = String::from_utf8_lossy(&bytes);
                // newest entries are at the end; skip continuation lines
                for line in text.lines().rev().take(400) {
                    if line.ends_with('\\') || line.contains('\u{fffd}') {
                        continue;
                    }
                    push(strip_zsh_prefix(line));
                }
                break;
            }
        }
    }
    out.truncate(limit.max(1));
    out
}

/// Record a command run in a deck shell (typed, completed, or injected).
/// deck owns its history: shells inside tmux sessions stay alive, so the
/// user's ~/.zsh_history only fills on shell exit — too late for completion.
pub(crate) fn record_into(path: &Path, cmd: &str) -> Result<(), String> {
    let _g = HIST_LOCK.lock().unwrap(); // serialize read-modify-write
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
    storage::save(path, &raw)?;
    // full shell commands are user content — keep the file private
    storage::restrict_to_user(path);
    Ok(())
}

#[tauri::command]
pub(crate) fn record_command(cmd: String) -> Result<(), String> {
    if !usable_command(&cmd) {
        return Ok(());
    }
    record_into(&deck_history_path(), cmd.trim())
}

/// Wipe deck's own command history (file + backup) — Settings → privacy.
#[tauri::command]
pub(crate) fn history_clear() -> Result<(), String> {
    let _g = HIST_LOCK.lock().unwrap();
    let p = deck_history_path();
    storage::save(&p, "[]")?;
    storage::restrict_to_user(&p);
    // the .bak still holds the old commands — clearing means clearing
    let mut bak = p.as_os_str().to_owned();
    bak.push(".bak");
    let _ = std::fs::remove_file(PathBuf::from(bak));
    Ok(())
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
}
