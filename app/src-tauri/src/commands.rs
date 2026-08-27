//! The non-scheduler Tauri command surface: board/settings persistence,
//! session lifecycle, polling, link opening, diagnostics.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Emitter};

use crate::storage;
use crate::storage::{applog, now_epoch};
use crate::tmux::{
    expand_tilde, init_deck_server, pane_target, session_target, tmux, tmux_bin,
    validate_session_name,
};

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
    validate_session_name(&name)?;
    let dir = expand_tilde(&dir);
    if !std::path::Path::new(&dir).is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
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
    validate_session_name(&name)?;
    let t = pane_target(&name);
    // one query for both facts (was two subprocesses per wheel tick)
    let stat =
        tmux(&["display", "-p", "-t", &t, "#{pane_in_mode} #{history_size}"]).unwrap_or_default();
    let mut it = stat.split_whitespace();
    let in_mode = it.next() == Some("1");
    let hist: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if lines < 0 {
        if !in_mode {
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
    if validate_session_name(&name).is_err() {
        return;
    }
    let t = pane_target(&name);
    let _ = tmux(&["clear-history", "-t", &t]);
}

#[tauri::command]
pub(crate) fn kill_session(name: String) -> Result<(), String> {
    validate_session_name(&name)?;
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

/// Per-poll ceiling on `capture-pane` targets. Every capture is pane-content
/// I/O through the tmux server; an unbounded board would make poll cost grow
/// with visible-card count. Boards with more on-screen cards than this get
/// previews for the first MAX_TAIL_SESSIONS only (frontend sends visible
/// cards in board order, so the truncation is stable, not flickering).
const MAX_TAIL_SESSIONS: usize = 16;

/// Marker line separating per-session segments in a batched capture. \x01 is
/// never produced by capture-pane for ordinary pane text lines.
const TAIL_MARK: &str = "\u{1}deck-tail\u{1}";

/// One pane-listing line → (session, pane pid, activity epoch, fg command).
/// Every tmux session has at least one pane, so this listing doubles as the
/// liveness set — no separate `list-sessions` round-trip.
pub(crate) fn parse_panes(text: &str) -> HashMap<String, (u32, u64, String)> {
    let mut panes: HashMap<String, (u32, u64, String)> = HashMap::new();
    for line in text.lines() {
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
    panes
}

/// Split batched `display-message ; capture-pane` output back into per-session
/// tails: each segment starts with a TAIL_MARK line naming the session, and
/// keeps the last `lines` non-empty lines of its capture.
pub(crate) fn parse_tail_batches(text: &str, lines: usize) -> HashMap<String, Vec<String>> {
    let mut tails: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix(TAIL_MARK) {
            current = Some(name.to_string());
            tails.entry(name.to_string()).or_default();
        } else if let Some(name) = &current {
            if !line.trim().is_empty() {
                tails.get_mut(name).unwrap().push(line.to_string());
            }
        }
    }
    for v in tails.values_mut() {
        let skip = v.len().saturating_sub(lines);
        v.drain(..skip);
    }
    tails
}

/// Fetch tail previews for many sessions in ONE tmux invocation: a command
/// batch of `display-message -p <marker+name> ; capture-pane -p …` pairs.
/// Ran per-session before (one subprocess per visible card, every 2.5s).
pub(crate) fn capture_tails(names: &[&String], lines: usize) -> HashMap<String, Vec<String>> {
    if names.is_empty() {
        return HashMap::new();
    }
    let mut args: Vec<String> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        if i > 0 {
            args.push(";".into());
        }
        args.push("display-message".into());
        args.push("-p".into());
        args.push(format!("{TAIL_MARK}{}", crate::tmux::fmt_escape(name)));
        args.push(";".into());
        args.push("capture-pane".into());
        args.push("-p".into());
        args.push("-t".into());
        args.push(pane_target(name));
        args.push("-S".into());
        args.push("-30".into());
    }
    parse_tail_batches(&crate::tmux::tmux_batch(&args), lines)
}

/// Single poll for everything the board needs: liveness, output recency,
/// process-tree memory, and (for the sessions on screen) tail previews.
/// Cost is bounded: 2 tmux subprocesses + 1 ps per poll, independent of
/// session count (was 2 + one capture-pane per visible card).
///
/// PERF (examples/poll_bench.rs, M-series, release, 2026-08): per poll at
/// 5/20/50 sessions — old pattern 14/45/108 ms with 7/22/52 subprocesses;
/// batched pattern 4.3/4.6/5.1 ms with a constant 2 (+1 ps here).
#[tauri::command]
pub(crate) fn poll_sessions(names: Vec<String>, tail_for: Vec<String>) -> Vec<SessInfo> {
    // one listing supplies liveness + activity + pid + fg for every session
    let panes = parse_panes(
        &tmux(&[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_pid}\t#{window_activity}\t#{pane_current_command}",
        ])
        .unwrap_or_default(),
    );

    let roots: HashMap<String, u32> = names
        .iter()
        .filter_map(|n| panes.get(n).map(|(pid, _, _)| (n.clone(), *pid)))
        .collect();
    let mem = tree_mem(&roots);

    // captures only for sessions that are both requested AND alive — a dead
    // target inside the batch would abort the remaining commands
    let want_tails: Vec<&String> = tail_for
        .iter()
        .filter(|n| panes.contains_key(*n))
        .take(MAX_TAIL_SESSIONS)
        .collect();
    if tail_for.len() > MAX_TAIL_SESSIONS {
        applog(&format!(
            "[poll] tail previews capped at {MAX_TAIL_SESSIONS} of {}",
            tail_for.len()
        ));
    }
    let mut tails = capture_tails(&want_tails, 2);
    let now = now_epoch();

    names
        .into_iter()
        .map(|name| {
            let pane = panes.get(&name);
            SessInfo {
                alive: pane.is_some(),
                idle_secs: pane.map(|(_, act, _)| now.saturating_sub(*act)),
                mem_mb: mem.get(&name).copied(),
                tail: tails.remove(&name).unwrap_or_default(),
                fg: pane.map(|(_, _, fg)| fg.clone()),
                name,
            }
        })
        .collect()
}

// ---------- open path / url ----------------------------------------------------

/// What open_target is allowed to hand to `open`, decided BEFORE any
/// subprocess spawns. `open` treats its argument as a URL when it parses as
/// one — an unvalidated "url" click could reach file:// or an arbitrary app
/// scheme; a relative path could resolve outside the card's cwd view. Rules:
/// urls must be http(s); paths must resolve absolute and exist.
pub(crate) fn validate_open(kind: &str, value: &str, resolved: &str) -> Result<(), String> {
    match kind {
        "url" => {
            let lower = value.trim().to_ascii_lowercase();
            if lower.starts_with("http://") || lower.starts_with("https://") {
                Ok(())
            } else {
                Err(format!("only http(s) links open externally: {value}"))
            }
        }
        "editor" | "reveal" => {
            if !resolved.starts_with('/') {
                return Err(format!("path did not resolve absolute: {resolved}"));
            }
            if !std::path::Path::new(resolved).exists() {
                return Err(format!("no such path: {resolved}"));
            }
            Ok(())
        }
        _ => Err(format!("unknown kind: {kind}")),
    }
}

#[tauri::command]
pub(crate) fn open_target(kind: String, value: String, cwd: String) -> Result<(), String> {
    let resolved = {
        let v = expand_tilde(&value);
        // strip a trailing :line[:col] suffix before hitting the filesystem
        let stripped = regex_strip_lineno(&v);
        if stripped.starts_with('/') {
            stripped
        } else {
            format!("{}/{}", expand_tilde(&cwd).trim_end_matches('/'), stripped)
        }
    };
    validate_open(&kind, &value, &resolved)?;
    let status = match kind.as_str() {
        "url" => Command::new("open").arg(value.trim()).status(),
        "editor" => match editor_app() {
            Some(app) => Command::new("open").args(["-a", &app, &resolved]).status(),
            None => Command::new("open").args(["-t", &resolved]).status(),
        },
        "reveal" => Command::new("open").args(["-R", &resolved]).status(),
        _ => unreachable!("validate_open rejects unknown kinds"),
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
    fn parse_panes_basic_and_malformed() {
        let text =
            "alpha\t100\t1700000000\tzsh\nbeta\t200\t1700000005\tclaude\njunk-line\nempty\t\t\t\n";
        let p = parse_panes(text);
        assert_eq!(p.len(), 2);
        assert_eq!(p["alpha"], (100, 1700000000, "zsh".into()));
        assert_eq!(p["beta"], (200, 1700000005, "claude".into()));
    }

    #[test]
    fn parse_panes_first_pane_wins() {
        // multi-pane session: the first listed pane is the representative one
        let text = "s\t10\t111\tzsh\ns\t20\t222\tvim\n";
        assert_eq!(parse_panes(text)["s"], (10, 111, "zsh".into()));
    }

    #[test]
    fn tail_batches_split_and_trim() {
        let text =
            format!("{TAIL_MARK}a\nline1\n\nline2\nline3\n{TAIL_MARK}b\n\n{TAIL_MARK}c\nonly\n");
        let t = parse_tail_batches(&text, 2);
        assert_eq!(t["a"], vec!["line2", "line3"]);
        assert!(t["b"].is_empty(), "empty pane still yields an entry");
        assert_eq!(t["c"], vec!["only"]);
    }

    #[test]
    fn tail_batches_ignore_preamble() {
        // output before the first marker (e.g. a stray error line) is dropped
        let t = parse_tail_batches(&format!("noise\n{TAIL_MARK}a\nx\n"), 2);
        assert_eq!(t.len(), 1);
        assert_eq!(t["a"], vec!["x"]);
    }

    #[test]
    fn open_validation_gates_urls_and_paths() {
        assert!(validate_open("url", "https://example.com/x", "").is_ok());
        assert!(validate_open("url", "HTTP://example.com", "").is_ok());
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://host",
            "x-apple.systempreferences:",
            "/etc/passwd",
        ] {
            assert!(validate_open("url", bad, "").is_err(), "{bad}");
        }
        assert!(validate_open("reveal", "", "/tmp").is_ok());
        assert!(validate_open("reveal", "", "relative/path").is_err());
        assert!(validate_open("editor", "", "/no/such/path/deck-test").is_err());
        assert!(validate_open("shell", "", "/tmp").is_err(), "unknown kind");
    }

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
