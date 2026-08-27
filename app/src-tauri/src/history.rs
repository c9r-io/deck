//! Command history for completion: deck records commands typed in its own
//! shells (zsh history only flushes on shell exit, so it can't be the live
//! source); recency is frequency-boosted.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::storage;
use crate::storage::now_epoch;

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

pub(crate) fn read_deck_history() -> Vec<HistEntry> {
    let Some(raw) = storage::load(&deck_history_path()).ok().flatten() else {
        return Vec::new();
    };
    if let Ok(v) = serde_json::from_str::<Vec<HistEntry>>(&raw) {
        return v;
    }
    // migrate v1 (plain string array)
    let now = now_epoch();
    serde_json::from_str::<Vec<String>>(&raw)
        .map(|v| {
            v.into_iter()
                .map(|cmd| HistEntry {
                    cmd,
                    n: 1,
                    last: now,
                })
                .collect()
        })
        .unwrap_or_default()
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
#[tauri::command]
pub(crate) fn record_command(cmd: String) -> Result<(), String> {
    if !usable_command(&cmd) {
        return Ok(());
    }
    let cmd = cmd.trim().to_string();
    let mut hist = read_deck_history();
    if let Some(e) = hist.iter_mut().find(|e| e.cmd == cmd) {
        e.n += 1;
        e.last = now_epoch();
    } else {
        hist.push(HistEntry {
            cmd,
            n: 1,
            last: now_epoch(),
        });
    }
    hist.sort_by_key(|e| std::cmp::Reverse(hist_score(e)));
    hist.truncate(500);
    let raw = serde_json::to_string(&hist).map_err(|e| e.to_string())?;
    storage::save(&deck_history_path(), &raw)
}
