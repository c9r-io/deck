//! The non-scheduler Tauri command surface: board/settings persistence,
//! session lifecycle, polling, link opening, diagnostics.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Emitter};

use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::{expand_tilde, init_deck_server, pane_target, session_target, tmux, tmux_bin};

#[tauri::command]
pub(crate) fn ui_log(msg: String) {
    applog(&format!("[ui] {msg}"));
}

pub(crate) fn export_logs() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("no home dir")?;
    let dir = home.join(".deck").join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("deck-log-{}.txt", now_epoch());
    let path = dir.join(name);

    let mut out = String::new();
    out.push_str(&format!("deck {}\n", env!("CARGO_PKG_VERSION")));
    if let Ok(o) = Command::new("sw_vers").output() {
        out.push_str(&String::from_utf8_lossy(&o.stdout));
    }
    if let Ok(o) = Command::new("uname").arg("-m").output() {
        out.push_str(&format!("arch: {}", String::from_utf8_lossy(&o.stdout)));
    }
    out.push_str(&format!("tmux: {}\n", tmux_bin()));
    out.push_str(&format!(
        "sessions: {}\n",
        tmux(&["list-sessions", "-F", "#{session_name}"])
            .map(|s| s.lines().count())
            .unwrap_or(0)
    ));
    out.push_str("\n===== app.log =====\n");
    out.push_str(&std::fs::read_to_string(home.join(".deck").join("app.log")).unwrap_or_default());
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    let _ = Command::new("open").arg("-R").arg(&path).status();
    Ok(path)
}

/// Rust→JS event self-test: the frontend calls this after registering a
/// listener; if the pong never arrives, the event bus is the broken link.
#[tauri::command]
pub(crate) fn ping_event(app: AppHandle) {
    let r = app.emit("deck-ping", "pong");
    applog(&format!("[evt] ping emitted ok={:?}", r.is_ok()));
}

// ---------- board persistence ------------------------------------------------

pub(crate) fn board_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("deck.json")
}

#[tauri::command]
pub(crate) fn load_board() -> Result<String, String> {
    Ok(storage::load(&board_path())?.unwrap_or_default())
}

#[tauri::command]
pub(crate) fn save_board(data: String) -> Result<(), String> {
    storage::save(&board_path(), &data)
}

/// Boot-time storage notices (corruption recovered from .bak, etc.) for the
/// frontend to surface as toasts.
#[tauri::command]
pub(crate) fn storage_warnings() -> Vec<String> {
    std::mem::take(&mut *storage::WARNINGS.lock().unwrap())
}

// ---------- settings ------------------------------------------------------------

pub(crate) fn settings_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("settings.json")
}

#[tauri::command]
pub(crate) fn load_settings() -> Result<String, String> {
    Ok(storage::load(&settings_path())?.unwrap_or_default())
}

#[tauri::command]
pub(crate) fn save_settings(data: String) -> Result<(), String> {
    storage::save(&settings_path(), &data)
}

pub(crate) fn editor_app() -> Option<String> {
    let raw = storage::load(&settings_path()).ok()??;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let e = v.get("editor")?.as_str()?.trim().to_string();
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

/// Developer editors present in /Applications or ~/Applications, offered in
/// the Settings editor picker. Names double as `open -a` targets.
#[tauri::command]
pub(crate) fn detect_editors() -> Vec<String> {
    const CANDIDATES: &[&str] = &[
        "Cursor",
        "Visual Studio Code",
        "Zed",
        "Sublime Text",
        "TextMate",
        "BBEdit",
        "Nova",
        "IntelliJ IDEA",
        "WebStorm",
        "RustRover",
        "Xcode",
    ];
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("Applications"));
    }
    CANDIDATES
        .iter()
        .filter(|c| roots.iter().any(|r| r.join(format!("{c}.app")).exists()))
        .map(|c| c.to_string())
        .collect()
}

#[tauri::command]
pub(crate) fn default_dir() -> String {
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".into())
}

#[tauri::command]
pub(crate) fn tmux_available() -> bool {
    Command::new(tmux_bin())
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tauri::command]
pub(crate) fn start_session(name: String, dir: String, cmd: String) -> Result<(), String> {
    let dir = expand_tilde(&dir);
    tmux(&["new-session", "-d", "-s", &name, "-c", &dir])?;
    // belt & suspenders for servers started by older deck versions
    init_deck_server();
    if !cmd.trim().is_empty() {
        tmux(&["send-keys", "-t", &pane_target(&name), &cmd, "Enter"])?;
    }
    Ok(())
}

/// Server-wide defaults for the deck tmux server. Idempotent; called at app
/// Wheel scrolling, deck-driven: xterm keeps LOCAL selection (mouse mode
/// stays off), and deck translates wheel deltas into tmux copy-mode
#[tauri::command]
pub(crate) fn scroll_session(name: String, lines: i32) -> Result<(), String> {
    let t = pane_target(&name);
    let in_mode = tmux(&["display", "-p", "-t", &t, "#{pane_in_mode}"])
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    if lines < 0 {
        if !in_mode {
            let hist = tmux(&["display", "-p", "-t", &t, "#{history_size}"])
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(0);
            if hist == 0 {
                return Ok(());
            }
            tmux(&["copy-mode", "-e", "-t", &t])?;
        }
        let n = (-lines).clamp(1, 60).to_string();
        let _ = tmux(&["send-keys", "-t", &t, "-X", "-N", &n, "scroll-up"]);
    } else if lines > 0 && in_mode {
        let n = lines.clamp(1, 60).to_string();
        let _ = tmux(&["send-keys", "-t", &t, "-X", "-N", &n, "scroll-down"]);
    }
    Ok(())
}

/// Fresh shells accumulate junk history from the attach-time resize
/// reflow (blank lines pushed into scrollback), which made "empty" shells
/// scrollable. Called once for sessions deck itself just started.
#[tauri::command]
pub(crate) fn clear_history(name: String) {
    let t = pane_target(&name);
    let _ = tmux(&["clear-history", "-t", &t]);
}

#[tauri::command]
pub(crate) fn kill_session(name: String) -> Result<(), String> {
    tmux(&["kill-session", "-t", &session_target(&name)]).map(|_| ())
}

// ---------- polling ------------------------------------------------------------

#[derive(Serialize)]
pub(crate) struct SessInfo {
    name: String,
    alive: bool,
    /// seconds since the pane last produced output (None if unknown)
    idle_secs: Option<u64>,
    /// RSS of the whole process tree under the pane, in MB
    mem_mb: Option<f64>,
    /// last non-empty lines of the pane, for card previews
    tail: Vec<String>,
    /// foreground process in the pane (zsh, claude, node, …) — lets the
    /// frontend record shell commands but not agent prompts
    fg: Option<String>,
}

pub(crate) fn tree_mem(roots: &HashMap<String, u32>) -> HashMap<String, f64> {
    let mut result = HashMap::new();
    let Ok(out) = Command::new("ps")
        .args(["-axo", "pid=,ppid=,rss="])
        .output()
    else {
        return result;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut rss: HashMap<u32, u64> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(kb)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(kb)) = (pid.parse(), ppid.parse::<u32>(), kb.parse::<u64>())
        else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
        rss.insert(pid, kb);
    }
    for (session, root) in roots {
        let mut sum = 0u64;
        let mut stack = vec![*root];
        let mut seen = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) {
                continue;
            }
            sum += rss.get(&pid).copied().unwrap_or(0);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        result.insert(session.clone(), sum as f64 / 1024.0);
    }
    result
}

pub(crate) fn capture_tail(name: &str, lines: usize) -> Vec<String> {
    match tmux(&["capture-pane", "-p", "-t", &pane_target(name), "-S", "-30"]) {
        Ok(text) => {
            let non_empty: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect();
            let skip = non_empty.len().saturating_sub(lines);
            non_empty.into_iter().skip(skip).collect()
        }
        Err(_) => Vec::new(),
    }
}

/// Single poll for everything the board needs: liveness, output recency,
/// process-tree memory, and (for the sessions on screen) tail previews.
#[tauri::command]
pub(crate) fn poll_sessions(names: Vec<String>, tail_for: Vec<String>) -> Vec<SessInfo> {
    let alive: HashSet<String> = tmux(&["list-sessions", "-F", "#{session_name}"])
        .map(|o| o.lines().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    // session name → (pane pid, last activity epoch, foreground command)
    let mut panes: HashMap<String, (u32, u64, String)> = HashMap::new();
    if let Ok(out) = tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}\t#{pane_pid}\t#{window_activity}\t#{pane_current_command}",
    ]) {
        for line in out.lines() {
            let mut it = line.split('\t');
            if let (Some(s), Some(pid), Some(act), Some(fg)) =
                (it.next(), it.next(), it.next(), it.next())
            {
                if let (Ok(pid), Ok(act)) = (pid.parse(), act.parse()) {
                    panes
                        .entry(s.to_string())
                        .or_insert((pid, act, fg.to_string()));
                }
            }
        }
    }

    let roots: HashMap<String, u32> = names
        .iter()
        .filter(|n| alive.contains(*n))
        .filter_map(|n| panes.get(n).map(|(pid, _, _)| (n.clone(), *pid)))
        .collect();
    let mem = tree_mem(&roots);
    let tails: HashSet<&String> = tail_for.iter().collect();
    let now = now_epoch();

    names
        .into_iter()
        .map(|name| {
            let is_alive = alive.contains(&name);
            let idle = panes
                .get(&name)
                .map(|(_, act, _)| now.saturating_sub(*act))
                .filter(|_| is_alive);
            SessInfo {
                alive: is_alive,
                idle_secs: idle,
                mem_mb: mem.get(&name).copied(),
                tail: if is_alive && tails.contains(&name) {
                    capture_tail(&name, 2)
                } else {
                    Vec::new()
                },
                fg: panes
                    .get(&name)
                    .map(|(_, _, fg)| fg.clone())
                    .filter(|_| is_alive),
                name,
            }
        })
        .collect()
}

// ---------- open path / url ----------------------------------------------------

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), String> {
    let resolve = || {
        let v = expand_tilde(&value);
        // strip a trailing :line[:col] suffix before hitting the filesystem
        let stripped = regex_strip_lineno(&v);
        if stripped.starts_with('/') {
            stripped
        } else {
            format!("{}/{}", expand_tilde(&cwd).trim_end_matches('/'), stripped)
        }
    };
    let status = match kind.as_str() {
        "url" => Command::new("open").arg(&value).status(),
        "editor" => match editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolve()]).status(),
            None => Command::new("open").args(["-t", &resolve()]).status(),
        },
        "reveal" => Command::new("open").args(["-R", &resolve()]).status(),
        _ => return Err(format!("unknown kind: {kind}")),
    }
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open failed for {value}"))
    }
}

pub(crate) fn regex_strip_lineno(path: &str) -> String {
    // "src/foo.rs:42:7" → "src/foo.rs" (without pulling in the regex crate)
    let mut parts = path.splitn(2, ':');
    let head = parts.next().unwrap_or(path);
    match parts.next() {
        Some(rest) if rest.chars().all(|c| c.is_ascii_digit() || c == ':') && !rest.is_empty() => {
            head.to_string()
        }
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_lineno_suffixes() {
        assert_eq!(regex_strip_lineno("src/foo.rs:42:7"), "src/foo.rs");
        assert_eq!(regex_strip_lineno("src/foo.rs:42"), "src/foo.rs");
        assert_eq!(regex_strip_lineno("src/foo.rs"), "src/foo.rs");
        // a colon followed by non-digits is part of the path, not a lineno
        assert_eq!(regex_strip_lineno("a:b/c"), "a:b/c");
        assert_eq!(regex_strip_lineno("http://x/y:8080"), "http://x/y:8080");
    }
}
