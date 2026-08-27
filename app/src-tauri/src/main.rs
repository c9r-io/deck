// deck — Tauri backend. tmux owns the sessions (they survive app restarts);
// this process is the projection layer: board persistence, low-frequency
// polling for the kanban, and a PTY bridge (`tmux attach`) for the one
// session the user has open.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- tmux helpers ----------------------------------------------------

/// Absolute path to tmux. The bundled sidecar comes first (zero-dependency
/// installs — a statically linked tmux ships inside the .app); Homebrew /
/// MacPorts are fallbacks for source builds. Apps launched from Finder get
/// launchd's PATH (no /opt/homebrew/bin), so plain "tmux" is last resort.
fn tmux_bin() -> &'static str {
    static BIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("tmux").display().to_string());
            }
        }
        for p in [
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/opt/local/bin/tmux",
        ] {
            candidates.push(p.to_string());
        }
        for c in candidates {
            if std::path::Path::new(&c).exists() {
                applog(&format!("[tmux] using {c}"));
                return c;
            }
        }
        applog("[tmux] falling back to PATH lookup");
        "tmux".to_string()
    })
}

/// deck runs its own tmux server (socket "deck"): the bundled binary never
/// clashes with a user-installed tmux of a different version, and deck's
/// sessions stay out of the user's personal `tmux ls`.
const SOCKET: &str = "deck";

/// Run tmux (on the deck server) with output captured — stray stderr must
/// never reach a terminal.
fn tmux(args: &[&str]) -> Result<String, String> {
    let out = Command::new(tmux_bin())
        .args(["-L", SOCKET])
        .args(args)
        .output()
        .map_err(|e| format!("tmux not runnable: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Exact-match session target. Pane-level commands need the trailing colon.
fn session_target(name: &str) -> String {
    format!("={name}")
}
fn pane_target(name: &str) -> String {
    format!("={name}:")
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), rest);
        }
    }
    path.to_string()
}

// ---------- logging -------------------------------------------------------------

/// Append a line to ~/.deck/app.log (the app may be launched via `open`,
/// where stderr goes nowhere useful).
fn applog(msg: &str) {
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("app.log");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = now_epoch();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{ts} {msg}");
    }
}

/// Frontend log sink — the webview console is invisible outside devtools.
#[tauri::command]
fn ui_log(msg: String) {
    applog(&format!("[ui] {msg}"));
}

/// Keep app.log bounded so support bundles stay small.
fn rotate_log() {
    let path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("app.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2 * 1024 * 1024 {
            if let Ok(data) = std::fs::read(&path) {
                let keep = &data[data.len().saturating_sub(512 * 1024)..];
                let _ = std::fs::write(&path, keep);
            }
        }
    }
}

/// Bundle diagnostics into a timestamped file and reveal it in Finder —
/// the "Export Logs" support flow for non-technical users.
fn export_logs() -> Result<PathBuf, String> {
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
    out.push_str(
        &std::fs::read_to_string(home.join(".deck").join("app.log")).unwrap_or_default(),
    );
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    let _ = Command::new("open").arg("-R").arg(&path).status();
    Ok(path)
}

/// Rust→JS event self-test: the frontend calls this after registering a
/// listener; if the pong never arrives, the event bus is the broken link.
#[tauri::command]
fn ping_event(app: AppHandle) {
    let r = app.emit("deck-ping", "pong");
    applog(&format!("[evt] ping emitted ok={:?}", r.is_ok()));
}

// ---------- board persistence ------------------------------------------------

fn board_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("deck.json")
}

#[tauri::command]
fn load_board() -> Result<String, String> {
    let path = board_path();
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_board(data: String) -> Result<(), String> {
    let path = board_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[tauri::command]
fn default_dir() -> String {
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "~".into())
}

#[tauri::command]
fn tmux_available() -> bool {
    Command::new(tmux_bin())
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------- session lifecycle ------------------------------------------------

/// Start a detached session: login shell in `dir`, then type `cmd` into it,
/// so the shell (and scrollback) survives the agent exiting.
#[tauri::command]
fn start_session(name: String, dir: String, cmd: String) -> Result<(), String> {
    let dir = expand_tilde(&dir);
    init_deck_server();
    tmux(&["new-session", "-d", "-s", &name, "-c", &dir])?;
    if !cmd.trim().is_empty() {
        tmux(&["send-keys", "-t", &pane_target(&name), &cmd, "Enter"])?;
    }
    Ok(())
}

/// Server-wide defaults for the deck tmux server. Idempotent; called at app
/// boot and before creating sessions so updates also apply to a server that
/// is already running.
fn init_deck_server() {
    let _ = tmux(&["start-server"]);
    // truecolor for agent processes (iTerm/Warp advertise the same)
    let _ = tmux(&["set-environment", "-g", "COLORTERM", "truecolor"]);
    // deck's pane headers already identify sessions — tmux's own status bar
    // ("[name] host HH:MM date") is noise and costs a terminal row
    let _ = tmux(&["set", "-g", "status", "off"]);
}

#[tauri::command]
fn kill_session(name: String) -> Result<(), String> {
    tmux(&["kill-session", "-t", &session_target(&name)]).map(|_| ())
}

// ---------- polling ------------------------------------------------------------

#[derive(Serialize)]
struct SessInfo {
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

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One `ps` snapshot → per-root-pid process-tree RSS sums (MB).
fn tree_mem(roots: &HashMap<String, u32>) -> HashMap<String, f64> {
    let mut result = HashMap::new();
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,ppid=,rss="]).output() else {
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

fn capture_tail(name: &str, lines: usize) -> Vec<String> {
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
fn poll_sessions(names: Vec<String>, tail_for: Vec<String>) -> Vec<SessInfo> {
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
                    panes.entry(s.to_string()).or_insert((pid, act, fg.to_string()));
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
                fg: panes.get(&name).map(|(_, _, fg)| fg.clone()).filter(|_| is_alive),
                name,
            }
        })
        .collect()
}

// ---------- PTY bridge (the open session) -------------------------------------

struct PtyEntry {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
}

#[derive(Default)]
struct PtyState {
    map: Mutex<HashMap<String, PtyEntry>>,
    counter: Mutex<u64>,
}

#[derive(Clone, Serialize)]
struct PtyData {
    name: String,
    data: String, // base64
}

#[derive(Clone, Serialize)]
struct PtyExit {
    name: String,
}

/// Attach = subscribe to the session's byte stream. The tmux session keeps
/// running whether or not anyone is attached; detach just closes the stream.
#[tauri::command]
fn attach_session(
    app: AppHandle,
    state: State<'_, PtyState>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    // replace any previous attachment for this session
    if let Some(mut old) = state.map.lock().unwrap().remove(&name) {
        let _ = old.child.kill();
    }

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(tmux_bin());
    cmd.args(["-L", SOCKET, "attach-session", "-t", &session_target(&name)]);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", "en_US.UTF-8");
    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        applog(&format!("[pty] attach spawn failed for {name}: {e}"));
        e.to_string()
    })?;
    drop(pair.slave);
    applog(&format!("[pty] attached {name} ({cols}x{rows})"));

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let generation = {
        let mut c = state.counter.lock().unwrap();
        *c += 1;
        *c
    };
    state.map.lock().unwrap().insert(
        name.clone(),
        PtyEntry {
            writer,
            master: pair.master,
            child,
            generation,
        },
    );

    let thread_app = app.clone();
    let thread_name = name.clone();
    std::thread::spawn(move || {
        applog(&format!("[pty] reader started for {thread_name}"));
        let mut buf = [0u8; 8192];
        let mut chunks: u64 = 0;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    chunks += 1;
                    if chunks <= 3 || chunks % 200 == 0 {
                        applog(&format!("[pty] read chunk #{chunks} {n}B from {thread_name}"));
                    }
                    let r = thread_app.emit(
                        "pty-data",
                        PtyData {
                            name: thread_name.clone(),
                            data: B64.encode(&buf[..n]),
                        },
                    );
                    if chunks == 1 {
                        applog(&format!("[pty] first emit result: {:?}", r.is_ok()));
                    }
                }
            }
        }
        // clean up only if this attachment is still the current one
        let state = thread_app.state::<PtyState>();
        let mut map = state.map.lock().unwrap();
        if map.get(&thread_name).map(|e| e.generation) == Some(generation) {
            map.remove(&thread_name);
            drop(map);
            applog(&format!("[pty] stream ended for {thread_name}"));
            let _ = thread_app.emit("pty-exit", PtyExit { name: thread_name });
        }
    });

    Ok(())
}

#[tauri::command]
fn pty_write(state: State<'_, PtyState>, name: String, data_b64: String) -> Result<(), String> {
    let bytes = B64.decode(data_b64).map_err(|e| e.to_string())?;
    let mut map = state.map.lock().unwrap();
    let entry = map.get_mut(&name).ok_or("not attached")?;
    entry.writer.write_all(&bytes).map_err(|e| e.to_string())?;
    entry.writer.flush().map_err(|e| e.to_string())
}

#[tauri::command]
fn pty_resize(state: State<'_, PtyState>, name: String, cols: u16, rows: u16) -> Result<(), String> {
    let map = state.map.lock().unwrap();
    let entry = map.get(&name).ok_or("not attached")?;
    entry
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn detach_session(state: State<'_, PtyState>, name: String) {
    if let Some(mut entry) = state.map.lock().unwrap().remove(&name) {
        let _ = entry.child.kill();
    }
}

// ---------- command history -----------------------------------------------------

fn deck_history_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("history.json")
}

#[derive(Serialize, Deserialize, Clone)]
struct HistEntry {
    cmd: String,
    n: u32,
    last: u64,
}

/// Frequency-boosted recency: each past use is worth an hour of freshness.
fn hist_score(e: &HistEntry) -> u64 {
    e.last + (e.n as u64) * 3600
}

fn read_deck_history() -> Vec<HistEntry> {
    let Some(raw) = std::fs::read_to_string(deck_history_path()).ok() else {
        return Vec::new();
    };
    if let Ok(v) = serde_json::from_str::<Vec<HistEntry>>(&raw) {
        return v;
    }
    // migrate v1 (plain string array)
    let now = now_epoch();
    serde_json::from_str::<Vec<String>>(&raw)
        .map(|v| v.into_iter().map(|cmd| HistEntry { cmd, n: 1, last: now }).collect())
        .unwrap_or_default()
}

/// Strip zsh EXTENDED_HISTORY prefix ": 1756…:0;cmd" → "cmd".
fn strip_zsh_prefix(line: &str) -> &str {
    if line.starts_with(": ") {
        if let Some(i) = line.find(';') {
            return &line[i + 1..];
        }
    }
    line
}

fn usable_command(cmd: &str) -> bool {
    let c = cmd.trim();
    c.len() >= 2 && c.len() <= 120 && !c.contains('\n')
}

/// Candidates for the quick-command chips: commands deck itself launched
/// (most recent first), then the user's shell history, deduped.
#[tauri::command]
fn recent_commands(limit: usize) -> Vec<String> {
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
fn record_command(cmd: String) -> Result<(), String> {
    if !usable_command(&cmd) {
        return Ok(());
    }
    let cmd = cmd.trim().to_string();
    let mut hist = read_deck_history();
    if let Some(e) = hist.iter_mut().find(|e| e.cmd == cmd) {
        e.n += 1;
        e.last = now_epoch();
    } else {
        hist.push(HistEntry { cmd, n: 1, last: now_epoch() });
    }
    hist.sort_by_key(|e| std::cmp::Reverse(hist_score(e)));
    hist.truncate(500);
    let path = deck_history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&hist).unwrap()).map_err(|e| e.to_string())
}

// ---------- scheduled prompts ----------------------------------------------------
// Queue prompts to be typed into a session later — the rate-limit workflow:
// "when my Claude quota window resets in 5h, send these tasks in order".
// tmux send-keys needs no attached client, so this works fully detached.
// The scheduler lives in a Rust thread (webview timers get frozen by App Nap).

#[derive(Serialize, Deserialize, Clone)]
struct QueueItem {
    id: String,
    session: String,
    dir: String,
    cmd: String,
    text: String,
    /// "at" = fire at `at` (epoch secs); "chain" = fire once the session has
    /// been quiet for CHAIN_QUIET_SECS after the previous send
    mode: String,
    at: Option<u64>,
    added: u64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct QueueState {
    items: Vec<QueueItem>,
    /// session → when we last injected a prompt
    last_fired: HashMap<String, u64>,
}

struct Queues(Mutex<QueueState>);

const CHAIN_QUIET_SECS: u64 = 180;
const CHAIN_MIN_GAP_SECS: u64 = 60;

fn queue_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
        .join("queue.json")
}

fn load_queue() -> QueueState {
    std::fs::read_to_string(queue_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_queue(q: &QueueState) {
    let path = queue_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(q).unwrap());
}

#[tauri::command]
fn queue_list(state: State<'_, Queues>) -> QueueState {
    state.0.lock().unwrap().clone()
}

#[tauri::command]
fn queue_add(
    state: State<'_, Queues>,
    app: AppHandle,
    session: String,
    dir: String,
    cmd: String,
    text: String,
    mode: String,
    at: Option<u64>,
) -> Result<(), String> {
    let text = text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err("empty prompt".into());
    }
    let mut q = state.0.lock().unwrap();
    let id = format!("q{}-{}", now_epoch(), q.items.len());
    q.items.push(QueueItem {
        id,
        session,
        dir,
        cmd,
        text,
        mode,
        at,
        added: now_epoch(),
    });
    save_queue(&q);
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
fn queue_update(
    state: State<'_, Queues>,
    app: AppHandle,
    id: String,
    text: String,
) -> Result<(), String> {
    let text = text.replace(['\n', '\r'], " ").trim().to_string();
    if text.is_empty() {
        return Err("empty prompt".into());
    }
    let mut q = state.0.lock().unwrap();
    if let Some(item) = q.items.iter_mut().find(|i| i.id == id) {
        item.text = text;
    }
    save_queue(&q);
    let _ = app.emit("queue-changed", ());
    Ok(())
}

#[tauri::command]
fn queue_remove(state: State<'_, Queues>, app: AppHandle, id: String) {
    let mut q = state.0.lock().unwrap();
    q.items.retain(|i| i.id != id);
    save_queue(&q);
    let _ = app.emit("queue-changed", ());
}

/// Drop all queued prompts for a session — called when its card closes.
#[tauri::command]
fn queue_clear_session(state: State<'_, Queues>, app: AppHandle, session: String) {
    let mut q = state.0.lock().unwrap();
    q.items.retain(|i| i.session != session);
    q.last_fired.remove(&session);
    save_queue(&q);
    let _ = app.emit("queue-changed", ());
}

#[derive(Clone, Serialize)]
struct QueueFired {
    session: String,
    text: String,
}

/// Inject one prompt into its session, starting the session if needed.
fn fire_item(item: &QueueItem) -> Result<(), String> {
    let alive: HashSet<String> = tmux(&["list-sessions", "-F", "#{session_name}"])
        .map(|o| o.lines().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if !alive.contains(&item.session) {
        start_session(item.session.clone(), item.dir.clone(), item.cmd.clone())?;
        std::thread::sleep(std::time::Duration::from_millis(2500));
    }
    // -l = literal text (no key-name interpretation), then a real Enter
    tmux(&["send-keys", "-t", &pane_target(&item.session), "-l", &item.text])?;
    tmux(&["send-keys", "-t", &pane_target(&item.session), "Enter"])?;
    Ok(())
}

fn spawn_scheduler(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(20));
        let state = app.state::<Queues>();
        let due: Vec<QueueItem> = {
            let q = state.0.lock().unwrap();
            if q.items.is_empty() {
                continue;
            }
            let now = now_epoch();
            // pane activity for chain-mode quiet checks
            let mut activity: HashMap<String, u64> = HashMap::new();
            if let Ok(out) = tmux(&["list-panes", "-a", "-F", "#{session_name}\t#{window_activity}"]) {
                for line in out.lines() {
                    let mut it = line.split('\t');
                    if let (Some(s), Some(a)) = (it.next(), it.next()) {
                        if let Ok(a) = a.parse() {
                            activity.entry(s.to_string()).or_insert(a);
                        }
                    }
                }
            }
            // only the head item of each session's queue is a candidate
            let mut seen: HashSet<String> = HashSet::new();
            q.items
                .iter()
                .filter(|i| seen.insert(i.session.clone()))
                .filter(|i| match i.mode.as_str() {
                    "at" => i.at.map(|t| now >= t).unwrap_or(false),
                    "chain" => {
                        let gap_ok = q
                            .last_fired
                            .get(&i.session)
                            .map(|t| now >= t + CHAIN_MIN_GAP_SECS)
                            .unwrap_or(true);
                        let quiet_ok = activity
                            .get(&i.session)
                            .map(|a| now >= a + CHAIN_QUIET_SECS)
                            .unwrap_or(true); // dead session = quiet; fire_item restarts it
                        gap_ok && quiet_ok
                    }
                    _ => false,
                })
                .cloned()
                .collect()
        };
        for item in due {
            match fire_item(&item) {
                Ok(()) => {
                    applog(&format!("[queue] sent to {}: {}", item.session, item.text));
                    let mut q = state.0.lock().unwrap();
                    q.items.retain(|i| i.id != item.id);
                    q.last_fired.insert(item.session.clone(), now_epoch());
                    save_queue(&q);
                    drop(q);
                    let _ = app.emit(
                        "queue-fired",
                        QueueFired { session: item.session.clone(), text: item.text.clone() },
                    );
                    let _ = app.emit("queue-changed", ());
                }
                Err(e) => {
                    applog(&format!("[queue] send FAILED for {}: {e} (will retry)", item.session));
                }
            }
        }
    });
}

// ---------- open path / url ----------------------------------------------------

#[tauri::command]
fn open_target(kind: String, value: String, cwd: String) -> Result<(), String> {
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
        "editor" => Command::new("open").args(["-t", &resolve()]).status(),
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

fn regex_strip_lineno(path: &str) -> String {
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

// ---------- main ---------------------------------------------------------------

fn main() {
    rotate_log();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(PtyState::default())
        .manage(Queues(Mutex::new(load_queue())))
        .setup(|app| {
            std::thread::spawn(init_deck_server);
            spawn_scheduler(app.handle().clone());
            // Update-check heartbeat from a Rust thread: webview timers are
            // frozen by App Nap when the app is backgrounded, so a JS
            // setInterval would effectively never fire. One latest.json
            // fetch (~1.4 KB) per 30 min is the entire cost.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(30 * 60));
                    let _ = handle.emit("update-check", ());
                });
            }
            // Native menu: the default set restores all standard macOS
            // shortcuts (⌘C/V/A/Z/Q/H/M/W…); Terminal→Clear adds ⌘K.
            let handle = app.handle();
            let menu = tauri::menu::Menu::default(handle)?;
            let clear = tauri::menu::MenuItemBuilder::with_id("clear", "Clear")
                .accelerator("Cmd+K")
                .build(app)?;
            let export = tauri::menu::MenuItemBuilder::with_id("export-logs", "Export Logs…")
                .build(app)?;
            let term_menu = tauri::menu::SubmenuBuilder::new(app, "Terminal")
                .item(&clear)
                .separator()
                .item(&export)
                .build()?;
            menu.append(&term_menu)?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, e| {
                if e.id() == "clear" {
                    let _ = app.emit("menu-clear", ());
                }
                if e.id() == "export-logs" {
                    match export_logs() {
                        Ok(p) => applog(&format!("[export] logs → {}", p.display())),
                        Err(err) => applog(&format!("[export] FAILED: {err}")),
                    }
                }
            });
            Ok(())
        })
        .on_page_load(|webview, payload| {
            // The window is created hidden (no white flash). Reveal it from
            // the Rust side once content is loaded — the JS show() alone
            // doesn't reliably surface a relaunched-by-updater instance.
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let w = webview.window();
                let _ = w.show();
                let _ = w.set_focus();
            }
        })
        .on_window_event(|window, event| {
            // ⌘W / red button hides instead of destroying the only window;
            // the Dock icon (Reopen) brings it back.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_board,
            save_board,
            default_dir,
            tmux_available,
            start_session,
            kill_session,
            poll_sessions,
            attach_session,
            pty_write,
            pty_resize,
            detach_session,
            open_target,
            recent_commands,
            record_command,
            ui_log,
            ping_event,
            queue_list,
            queue_add,
            queue_update,
            queue_remove,
            queue_clear_session,
        ])
        .build(tauri::generate_context!())
        .expect("error while building deck")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            let _ = (app, &event);
        });
}
