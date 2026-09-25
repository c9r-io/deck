//! The remaining Tauri command surface: event self-test, tmux mode style,
//! editor discovery, session start/kill, clipboard write and the one poll
//! command that feeds card status/memory/preview.
//!
//! # Contract
//! One poll command (`poll_sessions`) reads one `tmux::list_panes()` and
//! returns liveness + `window_activity` recency + process-tree footprint
//! (pane_pid → libproc tree walk) + the last six non-empty pane rows
//! for fixed-height, bottom-aligned card previews. Frontend polls every 2.5s and
//! diffs into granular UI events (status/mem/output) — never full re-renders on
//! output.
//! Poll IO runs on the blocking pool with a tmux deadline. A failed listing
//! rejects the poll, never reports dead sessions; unusable cwd metadata is
//! omitted independently of liveness. Closing an already-absent session is
//! successful, including tmux's `no current target` reply on a reachable
//! server with no sessions; other tmux failures still reject the close.
//!
//! # Wire naming (all commands, not only this file's)
//! A NEW struct returned to JS is `#[serde(rename_all = "camelCase")]`
//! (`QueueAddArgs`, `ContextProbeView`, every `mcp/` and `connector/` view).
//! Older returned structs stay snake_case on the wire (`SessInfo`'s
//! `idle_secs`/`mem_mb`, `QueueItem`, `TerminalScrollResult`, `PtyData`,
//! `PtyExit`, …): the frontend already reads those keys, and `QueueItem` is
//! also the on-disk `queue.json` format, so renaming it is a schema change,
//! not a style fix. Command ARGUMENTS are converted by Tauri 2's default
//! camelCase mapping: JS passes camelCase keys (`{ sessionId }`), including
//! inside an argument struct that is itself camelCase. One round trip can
//! therefore spell a field both ways (JS sends `quietSecs` in `QueueAddArgs`
//! and reads `quiet_secs` back from `QueueItem`); that is expected.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use tauri::{AppHandle, Emitter};

use crate::applog::applog;
use crate::datadir::now_epoch;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use crate::tmux::{
    expand_tilde, pane_target, session_target, tmux, tmux_program, tmux_with_stdin,
    validate_session_name, PaneRow,
};

/// Rust→JS event self-test: the frontend calls this after registering a
/// listener; if the pong never arrives, the event bus is the broken link.
#[tauri::command]
pub(crate) fn ping_event(app: AppHandle) {
    let r = app.emit("deck-ping", "pong");
    applog(&format!("[evt] ping emitted ok={:?}", r.is_ok()));
}

fn validated_palette_color(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Keep tmux's copy cursor aligned with the same closed JS registry that owns
/// CSS and xterm. The selection itself stays visually empty because the
/// frontend paints only settled selection geometry; this prevents tmux's
/// intermediate cursor motions from flashing across the terminal. Values
/// never come from a free-form UI; the strict shape check also prevents
/// option/format injection if the webview is compromised.
#[tauri::command]
pub(crate) fn set_terminal_mode_style(
    foreground: String,
    background: String,
) -> Result<(), DeckError> {
    if !validated_palette_color(&foreground) || !validated_palette_color(&background) {
        return Err(DeckError::new(
            ErrorKind::Other,
            "terminal palette colors must be six-digit hex values",
        ));
    }
    let style = format!("fg={foreground},bg={background}");
    crate::tmux::tmux(&["set", "-g", "mode-style", "none"])?;
    crate::tmux::tmux(&["set", "-g", "copy-mode-selection-style", "none"])?;
    crate::tmux::tmux(&["set", "-g", "copy-mode-position-style", &style]).map(|_| ())
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
    tmux_program().is_ok_and(|tmux_sidecar| {
        Command::new(tmux_sidecar)
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StartSessionResult {
    pub(crate) created: bool,
    pub(crate) restored: bool,
}

/// Idempotent start: `created` is true only when this call actually created
/// the session; `restored` additionally means a saved transcript was emitted
/// into the new pane's real tmux history before its login shell started.
/// "Enter card" means "make sure it's running, then attach" — if the session
/// already lives (stale frontend status, click before the first poll, another
/// window), that is success-without-side-effects: never a duplicate-session
/// error, and never re-typing the boot cmd into a running shell.
/// A created session logs one `[start]` line with per-phase milliseconds
/// (lifecycle gate, pane creation, boot cmd, total): every tmux client here
/// is one exec, and an endpoint-security agent taxes each exec (measured
/// 3ms vs 29ms per call, 2026-09-06), so the line shows whether a slow
/// "new shell" is deck's doing or the login shell's rc files. Server
/// defaults come from `-f tmux.conf` at server spawn; nothing is re-set
/// per session.
/// Restored-shell creation keeps an empty server alive, loads the sanitized
/// transcript from stdin into a private buffer, and creates the pane the
/// ORDINARY way (tmux's own login shell, no command). `new-session` prints
/// that new pane's tty from its own format context; a second fixed tmux batch
/// writes the buffer to that validated device and discards it.
/// No `/bin/sh -c`, no script, no shell argv: the earlier inline-script
/// bootstrap was an EDR signature, and the signed deck binary must never be
/// a pane executable (macOS Local Network Privacy would attribute the shell
/// tree to deck). `tmux_contract` proves the sequence against real tmux;
/// the unit test pins its shape.
pub(crate) fn restore_start_args(name: &str, dir: &str, buffer: &str) -> Vec<String> {
    [
        "start-server",
        ";",
        "load-buffer",
        "-b",
        buffer,
        "-",
        ";",
        "new-session",
        "-d",
        "-P",
        "-F",
        crate::shell_state::RESTORE_TTY_FORMAT,
        "-s",
        name,
        "-c",
        dir,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn restore_tty(output: &str) -> Result<String, DeckError> {
    let tty = output.strip_suffix('\n').unwrap_or(output);
    if tty.is_empty()
        || tty.contains(['\n', '\r', '\t'])
        || !tty.starts_with("/dev/")
        || crate::procinfo::tty_device(tty).is_none()
    {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "shell restore returned an invalid pane tty",
        ));
    }
    Ok(tty.to_string())
}

pub(crate) fn restore_emit_args(buffer: &str, tty: &str) -> Vec<String> {
    [
        "save-buffer",
        "-b",
        buffer,
        tty,
        ";",
        "delete-buffer",
        "-b",
        buffer,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// The one `[start]` log line: cumulative marks after the lifecycle gate,
/// after pane creation and at the end become per-phase durations. Only the
/// hashed session tag, numbers and a flag — never the name, dir or cmd.
fn start_timing_line(name: &str, marks_ms: [u128; 3], restored: bool) -> String {
    let [gate, created, total] = marks_ms;
    format!(
        "[start] created {} gate={gate}ms create={}ms cmd={}ms total={total}ms restored={restored}",
        crate::applog::session_tag(name),
        created.saturating_sub(gate),
        total.saturating_sub(created)
    )
}

/// A failed start's log line: hashed tag, the phase and the closed error
/// kind. The tmux stderr text stays out (it can echo the name or command).
fn start_failure_line(name: &str, stage: &str, error: &DeckError) -> String {
    format!(
        "[start] failed {} stage={stage} code={}",
        crate::applog::session_tag(name),
        error.code()
    )
}

#[tauri::command]
pub(crate) fn start_session(
    name: String,
    dir: String,
    cmd: String,
    restore_shell: bool,
) -> Result<StartSessionResult, DeckError> {
    let t0 = std::time::Instant::now();
    validate_session_name(&name)?;
    if tmux(&["has-session", "-t", &session_target(&name)]).is_ok() {
        return Ok(StartSessionResult {
            created: false,
            restored: false,
        });
    }
    // Serialize every server-creating path with upgrade replacement. The
    // second existence check closes the race with another creator while the
    // lifecycle gate was being acquired.
    let _lifecycle_guard = crate::tmux_lifecycle::session_creation_guard()?;
    if tmux(&["has-session", "-t", &session_target(&name)]).is_ok() {
        return Ok(StartSessionResult {
            created: false,
            restored: false,
        });
    }
    let gate_ms = t0.elapsed().as_millis();
    // A checkpoint is consulted only for a command-less, user-opened shell.
    // Its cwd may have advanced far beyond the card's original launch dir.
    // Missing directories fall back safely to the persisted card path.
    let recovery = crate::shell_state::snapshot_for_start(&name, &cmd, restore_shell);
    let requested_dir = expand_tilde(&dir);
    let dir = recovery
        .as_ref()
        .map(|snapshot| snapshot.cwd.clone())
        .filter(|cwd| std::path::Path::new(cwd).is_dir())
        .unwrap_or(requested_dir);
    if !std::path::Path::new(&dir).is_dir() {
        return Err(DeckError::new(
            ErrorKind::NotDir,
            format!("not a directory: {dir}"),
        ));
    }
    let bootstrap = recovery
        .as_ref()
        .filter(|snapshot| !snapshot.transcript.trim().is_empty())
        .and_then(
            |snapshot| match crate::shell_state::prepare_bootstrap(snapshot) {
                Ok(bootstrap) => Some(bootstrap),
                Err(error) => {
                    applog(&format!(
                        "[shell-state] bootstrap unavailable for {} ({})",
                        crate::applog::session_tag(&name),
                        error.code()
                    ));
                    None
                }
            },
        );
    let (start, restored) = if let Some(bootstrap) = bootstrap.as_ref() {
        // The pane is created exactly like a clean shell (tmux's own login
        // shell, no command). `new-session -P` returns THIS pane's tty; never
        // let an attached control client's ambient target choose the device.
        let args = restore_start_args(&name, &dir, &bootstrap.buffer);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let restored_start = tmux_with_stdin(&args, &bootstrap.output).and_then(|output| {
            let tty = restore_tty(&output)?;
            let emit = restore_emit_args(&bootstrap.buffer, &tty);
            let emit: Vec<&str> = emit.iter().map(String::as_str).collect();
            tmux(&emit)
        });
        match restored_start {
            Ok(output) => (Ok(output), true),
            Err(error) => {
                let _ = tmux(&["delete-buffer", "-b", &bootstrap.buffer]);
                applog(&format!(
                    "[shell-state] restore start unavailable for {} ({}); starting a clean shell",
                    crate::applog::session_tag(&name),
                    error.code()
                ));
                // the sequence may have failed after the pane already existed
                if tmux(&["has-session", "-t", &session_target(&name)]).is_ok() {
                    (Ok(String::new()), false)
                } else {
                    (tmux(&["new-session", "-d", "-s", &name, "-c", &dir]), false)
                }
            }
        }
    } else {
        (tmux(&["new-session", "-d", "-s", &name, "-c", &dir]), false)
    };
    if let Err(error) = start {
        applog(&start_failure_line(&name, "create", &error));
        return Err(error);
    }
    let created_ms = t0.elapsed().as_millis();
    if !cmd.trim().is_empty() {
        if let Err(error) = tmux(&["send-keys", "-t", &pane_target(&name), &cmd, "Enter"]) {
            // The pane exists but never got its launch command. A failed
            // start persists no card, so a surviving session would be an
            // orphan the Board can neither show nor close.
            let _ = tmux(&["kill-session", "-t", &session_target(&name)]);
            let _ = crate::shell_state::clear_snapshot(&name);
            applog(&start_failure_line(&name, "command", &error));
            return Err(error);
        }
    }
    if recovery.is_some() {
        crate::shell_state::note_recovered(&name);
    }
    applog(&start_timing_line(
        &name,
        [gate_ms, created_ms, t0.elapsed().as_millis()],
        restored,
    ));
    Ok(StartSessionResult {
        created: true,
        restored,
    })
}

/// Native clipboard path for WKWebView. Success means pbcopy consumed all
/// bytes and exited zero; clipboard content never enters logs or errors.
/// `pbcopy` decodes stdin under the process locale, and a GUI-launched deck
/// has NONE (its environment is PATH and HOME). Under the C locale pbcopy
/// writes an EMPTY `public.utf8-plain-text` item for any input containing a
/// non-ASCII byte and still exits 0 — so a Chinese word, a box-drawing rule
/// or a `⏺` from an agent pane copied "successfully" and pasted as nothing
/// (the user-reported "copy often fails"; measured 2026-09-05). Pin the same
/// UTF-8 locale tmux()/pty.rs already pin, for the same reason.
fn pbcopy_command() -> Command {
    use std::process::Stdio;
    let mut command = Command::new("/usr/bin/pbcopy");
    command
        .env("LANG", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[tauri::command]
pub(crate) fn write_clipboard(text: String) -> Result<(), DeckError> {
    use std::io::Write as _;
    let mut child = pbcopy_command()
        .spawn()
        .map_err(|_| DeckError::new(ErrorKind::Other, "clipboard-write-failed"))?;
    child
        .stdin
        .take()
        .ok_or(DeckError::new(ErrorKind::Other, "clipboard-write-failed"))?
        .write_all(text.as_bytes())
        .map_err(|_| DeckError::new(ErrorKind::Other, "clipboard-write-failed"))?;
    let status = child
        .wait()
        .map_err(|_| DeckError::new(ErrorKind::Other, "clipboard-write-failed"))?;
    if status.success() {
        Ok(())
    } else {
        Err(DeckError::new(ErrorKind::Other, "clipboard-write-failed"))
    }
}

#[tauri::command]
pub(crate) fn kill_session(name: String, mcp_admission: Option<String>) -> Result<(), DeckError> {
    crate::mcp::validate_close_admission(mcp_admission.as_deref(), std::slice::from_ref(&name))?;
    let _activity = crate::session_runtime::activity_guard()?;
    validate_session_name(&name)?;
    // An MCP-managed pane: its runner stops the job groups first (bounded).
    crate::mcp::stop_managed_jobs(&name);
    idempotent_kill_result(tmux(&["kill-session", "-t", &session_target(&name)]))?;
    // Closing a card is also a privacy deletion: its transcript, backup and
    // quarantined recovery copies must not outlive the card.
    crate::shell_state::clear_snapshot(&name)
}

pub(crate) fn idempotent_kill_result(result: Result<String, DeckError>) -> Result<(), DeckError> {
    match result {
        Ok(_) => Ok(()),
        // Closing an already-gone session is the successful end state. This
        // also covers an empty deck tmux server ("no server running"). A
        // reachable server kept alive by exit-empty=off instead responds
        // "no current target" to this exact kill-session command. Match the
        // command and stderr, not the broad Missing error category: an
        // unrelated missing resource cannot prove the session is absent.
        Err(e)
            if e.message()
                .starts_with("tmux kill-session failed: can't find session: ")
                || e.message()
                    .starts_with("tmux kill-session failed: no server running")
                || e.message() == "tmux kill-session failed: no current target"
                || (e
                    .message()
                    .starts_with("tmux kill-session failed: error connecting to ")
                    && e.message().ends_with("(No such file or directory)")) =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

// ---------- polling ------------------------------------------------------------

#[derive(Debug, Serialize)]
pub(crate) struct SessInfo {
    name: String,
    alive: bool,
    /// seconds since the pane last produced output (None if unknown)
    idle_secs: Option<u64>,
    /// physical footprint of the whole process tree under the pane, in MB
    mem_mb: Option<f64>,
    /// last non-empty lines of the pane, for card previews
    tail: Vec<String>,
    /// foreground process in the pane (zsh, claude, node, …) — lets the
    /// frontend record shell commands but not agent prompts
    fg: Option<String>,
    /// live pane cwd; the frontend persists changes into the card so even a
    /// disabled transcript checkpoint still restarts in the right directory
    cwd: Option<String>,
    /// pane is in tmux copy-mode: the VISIBLE frame is frozen scrollback,
    /// not live output — the UI must say so (a silently frozen agent TUI
    /// reads as a hung session)
    scrolled: Option<bool>,
    /// closed agent-hook state word ("working" | "needs-input" |
    /// "turn-done") projected from the session's Signal target pane (the
    /// active pane of its current window, `agent_status::signal_targets`),
    /// if that pane's current foreground generation reported one
    agent: Option<&'static str>,
    /// the Deck-local attention episode of that same observation
    /// (`agent_status::Observation`): opaque, never a source id, never
    /// authority. None without an observation.
    episode: Option<crate::agent_status::EpisodeId>,
    /// the backend's authoritative "this turn-done episode was viewed" —
    /// every attention surface converges to it (FR-SI-05)
    episode_viewed: bool,
    /// retirement evidence for the automation finish rule, paired with
    /// `agent` ("no agent state and a shell in front"): the foreground of
    /// that SAME Signal target pane, and ONLY for a single-pane session
    /// (`agent_status::finish_foregrounds`). None for a multi-pane session
    /// whichever pane is active, and without a unique target — so no
    /// pane-local shell can retire a session another pane's agent lives in.
    /// `fg` above stays the representative pane's, for the Board's other
    /// uses; the two must not be mixed in an authority decision.
    finish_fg: Option<String>,
}

pub(crate) fn tree_mem(
    table: &crate::agent_status::ProcessTable,
    roots: &HashMap<String, u32>,
) -> HashMap<String, f64> {
    crate::procinfo::tree_memory(table, roots)
}

/// Per-poll ceiling on `capture-pane` targets. Every capture is pane-content
/// I/O through the tmux server; an unbounded board would make poll cost grow
/// with visible-card count. Boards with more on-screen cards than this get
/// previews for the first MAX_TAIL_SESSIONS only (frontend sends visible
/// cards in board order, so the truncation is stable, not flickering).
const MAX_TAIL_SESSIONS: usize = 16;

/// Enough terminal context to show the end of an agent response above the
/// input/status rows that occupy the bottom of most full-screen agent UIs.
const CARD_PREVIEW_LINES: usize = 6;

/// Marker line separating per-session segments in a batched capture. \x01 is
/// never produced by capture-pane for ordinary pane text lines.
const TAIL_MARK: &str = "\u{1}deck-tail\u{1}";

/// One representative pane per session — the first tmux lists — keyed by
/// session name. Cwd validity must never determine session liveness.
/// Every tmux session has at least one pane, so this
/// doubles as the liveness set — no separate `list-sessions` round-trip.
pub(crate) fn representative_panes(rows: Vec<PaneRow>) -> HashMap<String, PaneRow> {
    let mut panes: HashMap<String, PaneRow> = HashMap::new();
    for row in rows {
        panes.entry(row.session_name.clone()).or_insert(row);
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
/// Cost is bounded: the stable list path reuses one read-only tmux control
/// client, while tail capture remains one subprocess and memory remains one
/// ps per poll, independent of session count (was 2 + one capture-pane per
/// visible card).
///
/// PERF (examples/poll_bench.rs, M-series, release, 2026-08): per poll at
/// 5/20/50 sessions — old pattern 14/45/108 ms with 7/22/52 subprocesses;
/// batched pattern 4.3/4.6/5.1 ms with a constant 2 (+1 ps here).
#[tauri::command]
pub(crate) async fn poll_sessions(
    names: Vec<String>,
    tail_for: Vec<String>,
    checkpoint_shells: bool,
) -> Result<Vec<SessInfo>, DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        let _activity = crate::session_runtime::activity_guard()?;
        let _deadline = crate::session_runtime::Deadline::until(
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        );
        poll_from_listing(
            names,
            tail_for,
            checkpoint_shells,
            crate::tmux::query_list_panes(),
            crate::procinfo::processes,
        )
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "session poll worker failed"))?
}

pub(crate) fn poll_from_listing(
    names: Vec<String>,
    tail_for: Vec<String>,
    checkpoint_shells: bool,
    listing: Result<Vec<PaneRow>, DeckError>,
    processes: impl FnOnce() -> crate::agent_status::ProcessTable,
) -> Result<Vec<SessInfo>, DeckError> {
    // one listing supplies liveness + activity + pid + fg for every session
    // Log transitions, then propagate failures before reconciling agents,
    // scheduling checkpoints or projecting liveness. The UI retains cards.
    static POLL_BROKEN: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    {
        let mut broken = POLL_BROKEN.lock_or_recover();
        match &listing {
            Err(e) if !*broken => {
                *broken = true;
                applog(&format!("[poll] session listing FAILED ({})", e.code()));
            }
            Ok(_) if *broken => {
                *broken = false;
                applog("[poll] session listing recovered");
            }
            _ => {}
        }
    }
    let rows = listing?;
    // ONE process-table snapshot per poll, shared by agent-status
    // reconciliation and the memory footprint
    let table = processes();
    // agent-hook state lives exactly as long as the pane generation and
    // foreground process generation that reported it
    crate::agent_status::reconcile(&rows, &table);
    let agents = crate::agent_status::projections(&rows);
    let mut finish = crate::agent_status::finish_foregrounds(&rows);
    let panes = representative_panes(rows);

    let roots: HashMap<String, u32> = names
        .iter()
        .filter_map(|n| panes.get(n).map(|pane| (n.clone(), pane.pane_pid)))
        .collect();
    let mem = tree_mem(&table, &roots);

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
    let mut tails = capture_tails(&want_tails, CARD_PREVIEW_LINES);
    let now = now_epoch();

    // Snapshot work is throttled and runs off-thread; this call only selects
    // the small fair batch.  No pane content enters logs or the poll payload.
    crate::shell_state::schedule_checkpoints(
        panes
            .iter()
            .map(|(session, pane)| crate::shell_state::ShellObservation {
                session: session.clone(),
                activity: pane.window_activity,
                cwd: pane.path.clone(),
                foreground: pane.command.clone(),
            })
            .collect(),
        checkpoint_shells,
    );

    Ok(names
        .into_iter()
        .map(|name| {
            let pane = panes.get(&name);
            SessInfo {
                alive: pane.is_some(),
                idle_secs: pane.map(|pane| now.saturating_sub(pane.window_activity)),
                mem_mb: mem.get(&name).copied(),
                tail: tails.remove(&name).unwrap_or_default(),
                fg: pane.map(|pane| pane.command.clone()),
                cwd: pane.and_then(|pane| usable_cwd(&pane.path).map(str::to_owned)),
                scrolled: pane.map(|pane| pane.in_mode),
                agent: pane.and(agents.get(&name)).map(|o| o.state),
                episode: pane.and(agents.get(&name)).map(|o| o.episode),
                episode_viewed: pane.and(agents.get(&name)).is_some_and(|o| o.viewed),
                finish_fg: pane.and_then(|_| finish.remove(&name)),
                name,
            }
        })
        .collect())
}

fn usable_cwd(path: &str) -> Option<&str> {
    (!path.is_empty() && !path.chars().any(char::is_control)).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A runtime check would clobber the developer's clipboard, so this pins
    /// the configured spawn instead: without a UTF-8 locale pbcopy turns any
    /// non-ASCII selection into an empty pasteboard item and exits 0.
    #[test]
    fn pbcopy_is_spawned_under_a_utf8_locale() {
        let command = pbcopy_command();
        assert_eq!(
            command.get_program(),
            std::ffi::OsStr::new("/usr/bin/pbcopy")
        );
        let lang = command
            .get_envs()
            .find(|(key, _)| *key == std::ffi::OsStr::new("LANG"))
            .and_then(|(_, value)| value)
            .expect("pbcopy must not inherit the GUI session's missing locale");
        assert_eq!(lang, std::ffi::OsStr::new("en_US.UTF-8"));
    }

    #[test]
    fn start_timing_line_carries_phase_durations_and_no_session_name() {
        let line = start_timing_line("deck-quarterly-report-ab12", [12, 40, 47], true);
        assert!(line.starts_with("[start] created sess-"), "{line}");
        assert!(line.ends_with(" gate=12ms create=28ms cmd=7ms total=47ms restored=true"));
        assert!(
            !line.contains("quarterly"),
            "the card title never reaches the log"
        );
        assert_eq!(
            crate::redact::sanitize_log(&line),
            line,
            "safe to log verbatim"
        );
    }

    #[test]
    fn start_failure_line_carries_stage_and_kind_but_no_tmux_text() {
        let error = DeckError::classified("tmux send-keys failed: client is read-only");
        let line = start_failure_line("deck-quarterly-report-ab12", "command", &error);
        assert!(line.starts_with("[start] failed sess-"), "{line}");
        assert!(
            line.ends_with(&format!(" stage=command code={}", error.code())),
            "{line}"
        );
        assert!(
            !line.contains("quarterly") && !line.contains("read-only"),
            "{line}"
        );
        assert_eq!(
            crate::redact::sanitize_log(&line),
            line,
            "safe to log verbatim"
        );
    }

    /// Both probes are read-only and answer from fixed roots: the editor
    /// list is a subset of the closed candidate table, and each name is an
    /// `open -a` target that really exists.
    #[test]
    fn editor_and_home_probes_answer_from_fixed_roots() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(default_dir(), home.display().to_string());
        for name in detect_editors() {
            let app = format!("{name}.app");
            assert!(
                std::path::Path::new("/Applications").join(&app).exists()
                    || home.join("Applications").join(&app).exists(),
                "{name}"
            );
        }
    }

    fn row(session: &str, pid: u32, activity: u64, in_mode: bool, fg: &str, path: &str) -> PaneRow {
        PaneRow {
            session_name: session.into(),
            pane_pid: pid,
            window_activity: activity,
            in_mode,
            command: fg.into(),
            path: path.into(),
            ..PaneRow::default()
        }
    }

    #[test]
    fn representative_panes_keep_live_sessions_even_without_a_usable_cwd() {
        let panes = representative_panes(vec![
            row("alpha", 100, 1700000000, false, "zsh", "/tmp/a"),
            row("beta", 200, 1700000005, true, "claude", "/tmp/b"),
            row("gone", 300, 1, false, "zsh", ""),
            row("odd", 400, 1, false, "zsh", "/tmp/x\u{7}"),
            // multi-pane session: the first listed pane is the representative one
            row("beta", 201, 1700000009, false, "vim", "/tmp/two"),
        ]);
        assert_eq!(panes.len(), 4);
        assert_eq!(usable_cwd(&panes["gone"].path), None);
        assert_eq!(usable_cwd(&panes["odd"].path), None);
        assert_eq!(usable_cwd("/tmp/a\tb"), None);
        assert_eq!(usable_cwd(&panes["alpha"].path), Some("/tmp/a"));
        assert_eq!(panes["alpha"].command, "zsh");
        assert!(panes["beta"].in_mode, "copy-mode pane reported as scrolled");
        assert_eq!(panes["beta"].pane_pid, 200);
    }

    /// One listing feeds every card: a present pane is alive with its
    /// recency, footprint, foreground and (only when usable) cwd; an absent
    /// name is dead with nothing else claimed about it. Previews are capped
    /// per poll, and a listing that recovers after a failure is logged once.
    #[test]
    fn poll_projects_liveness_footprint_and_cwd_from_one_listing() {
        // the listing drives agent_status::reconcile and the shell
        // checkpoint tracker: both are process-wide
        let _store = crate::agent_status::STORE_TEST_LOCK.lock_or_recover();
        let _tracker = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let now = now_epoch();
        let rows = vec![
            row(
                "alpha",
                std::process::id(),
                now.saturating_sub(5),
                true,
                "zsh",
                "/tmp/a",
            ),
            row("beta", u32::MAX, now, false, "claude", "/tmp/x\u{7}"),
        ];
        assert!(poll_from_listing(
            vec!["alpha".into()],
            vec![],
            false,
            Err(DeckError::new(ErrorKind::Tmux, "listing unavailable")),
            crate::procinfo::processes,
        )
        .is_err());
        let mut previews: Vec<String> = (0..MAX_TAIL_SESSIONS)
            .map(|i| format!("preview-{i}"))
            .collect();
        previews.insert(0, "alpha".into());
        let info = poll_from_listing(
            vec!["alpha".into(), "beta".into(), "gone".into()],
            previews,
            false,
            Ok(rows),
            crate::procinfo::processes,
        )
        .unwrap();
        assert_eq!(info.len(), 3);
        let alpha = &info[0];
        assert_eq!(alpha.name, "alpha");
        assert!(alpha.alive);
        assert!(
            alpha.idle_secs.is_some_and(|secs| (5..60).contains(&secs)),
            "{:?}",
            alpha.idle_secs
        );
        assert!(
            alpha.mem_mb.is_some_and(|mb| mb > 0.0),
            "{:?}",
            alpha.mem_mb
        );
        assert_eq!(alpha.fg.as_deref(), Some("zsh"));
        assert_eq!(alpha.cwd.as_deref(), Some("/tmp/a"));
        assert_eq!(alpha.scrolled, Some(true));
        assert_eq!(alpha.agent, None);
        assert!(
            alpha.tail.is_empty(),
            "no sidecar means no preview, not a failure"
        );
        let beta = &info[1];
        assert!(beta.alive);
        assert_eq!(
            beta.cwd, None,
            "an unusable cwd is omitted independently of liveness"
        );
        assert_eq!(beta.mem_mb, Some(0.0));
        assert_eq!(beta.scrolled, Some(false));
        let gone = &info[2];
        assert!(!gone.alive);
        assert_eq!(
            (
                gone.idle_secs,
                gone.mem_mb,
                gone.fg.as_deref(),
                gone.scrolled
            ),
            (None, None, None, None)
        );
        assert!(gone.tail.is_empty() && gone.cwd.is_none());
    }

    /// The palette command validates before it talks to tmux, and every tmux
    /// probe in this module fails closed when the build has no sidecar:
    /// availability is false, styling is `TmuxMissing`, previews are empty.
    #[test]
    fn mode_style_and_availability_fail_closed_without_a_bundled_tmux() {
        assert_eq!(
            set_terminal_mode_style("red".into(), "#000000".into())
                .unwrap_err()
                .kind(),
            ErrorKind::Other
        );
        assert!(capture_tails(&[], 2).is_empty());
        if crate::tmux::tmux_bin().is_empty() {
            assert!(!tmux_available());
            assert_eq!(
                set_terminal_mode_style("#000000".into(), "#ffffff".into())
                    .unwrap_err()
                    .kind(),
                ErrorKind::TmuxMissing
            );
            let name = "deck-preview-unit".to_string();
            assert!(capture_tails(&[&name], 2).is_empty());
        }
    }

    /// FR-SI-03/03.1: `agent` and the finish rule's `finish_fg` come from
    /// the SAME pane — the session's Signal target (its active pane) — and
    /// `finish_fg` exists only for a single-pane session. `fg` keeps the
    /// representative (first) pane for the Board's other uses. Neither
    /// split topology may read as "no agent state, shell in front":
    /// - first-listed shell + active agent pane;
    /// - active shell pane + inactive live agent pane (the whole session,
    ///   agent included, would be killed by a retirement).
    #[test]
    fn retirement_evidence_is_same_pane_and_single_pane_only() {
        use crate::procinfo::ProcessInfo;
        let _store = crate::agent_status::STORE_TEST_LOCK.lock_or_recover();
        let _tracker = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        crate::agent_status::reset_for_tests();
        let process = |pid, ppid, tty, fg, start| ProcessInfo {
            pid,
            ppid,
            pgid: pid,
            tty,
            tty_pgid: fg,
            start_seconds: start,
            start_micros: 0,
        };
        // pane %3: a shell alone; pane %4: shell → agent (leader) → helper
        let table: crate::agent_status::ProcessTable = [
            process(300, 42, 7, 300, 1000),
            process(400, 42, 8, 410, 1000),
            process(410, 400, 8, 410, 2000),
            process(420, 410, 8, 410, 5000),
        ]
        .into_iter()
        .map(|info| (info.pid, info))
        .collect();
        let pane = |pane: &str, pid: u32, active: bool, fg: &str| PaneRow {
            server_pid: 42,
            session_id: "$1".into(),
            session_name: "deck-card-mixed".into(),
            window_id: "@1".into(),
            pane_id: pane.into(),
            pane_pid: pid,
            window_active: true,
            pane_active: active,
            command: fg.into(),
            ..PaneRow::default()
        };
        let line = format!(
            "{{\"v\":1,\"source\":\"claude-code\",\"state\":\"working\",\"socket\":\"{}\",\"server_pid\":42,\"pane\":\"%4\"}}",
            crate::tmux::socket()
        );
        let poll = |rows: Vec<PaneRow>| {
            let table = table.clone();
            poll_from_listing(
                vec!["deck-card-mixed".into()],
                vec![],
                false,
                Ok(rows),
                move || table,
            )
            .unwrap()
            .remove(0)
        };
        let finish = |info: &SessInfo| {
            // what `runFinishHolds` reads: no agent word AND a shell in front
            info.agent.is_none()
                && info
                    .finish_fg
                    .as_deref()
                    .is_some_and(|fg| crate::context::shell_process(Some(fg)))
        };

        // first-listed shell + ACTIVE agent pane (reporting)
        let agent_active = vec![
            pane("%3", 300, false, "zsh"),
            pane("%4", 400, true, "claude"),
        ];
        let origin = crate::agent_status::Origin {
            peer: Some(420),
            table: table.clone(),
        };
        assert_eq!(
            crate::agent_status::ingest(&line, &origin, || Some(agent_active.clone())),
            Ok(())
        );
        let info = poll(agent_active.clone());
        assert_eq!(info.agent, Some("working"), "the active pane's observation");
        assert_eq!(
            info.fg.as_deref(),
            Some("zsh"),
            "fg keeps the first pane for the Board"
        );
        assert_eq!(info.finish_fg, None, "two panes: no retirement evidence");
        assert!(!finish(&info));
        crate::agent_status::reset_for_tests();
        assert!(!finish(&poll(agent_active)), "nor without an observation");

        // the inverse: ACTIVE shell pane + inactive live agent pane
        let shell_active = vec![
            pane("%3", 300, true, "zsh"),
            pane("%4", 400, false, "claude"),
        ];
        assert_eq!(
            crate::agent_status::ingest(&line, &origin, || Some(shell_active.clone())),
            Ok(())
        );
        let info = poll(shell_active);
        assert_eq!(info.agent, None, "the active shell pane reports nothing");
        assert_eq!(
            info.finish_fg, None,
            "yet its shell is no retirement evidence"
        );
        assert!(
            !finish(&info),
            "the live agent in the other pane keeps the session"
        );

        // no unique target: no agent word and no evidence
        let unmarked = vec![
            pane("%3", 300, false, "zsh"),
            pane("%4", 400, false, "claude"),
        ];
        let info = poll(unmarked);
        assert_eq!((info.agent, info.finish_fg), (None, None));

        // a single-pane session keeps the existing fallback: the agent
        // program exited, a shell is in front → evidence present
        crate::agent_status::reset_for_tests();
        let single = vec![pane("%3", 300, true, "zsh")];
        let info = poll(single);
        assert_eq!(info.finish_fg.as_deref(), Some("zsh"));
        assert!(finish(&info), "single-pane shell: finish=close may retire");
        // and a single pane still running its agent is no evidence
        let running = vec![pane("%4", 400, true, "claude")];
        assert!(!finish(&poll(running)));
        crate::agent_status::reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    /// FR-SI-03 poll cost with tracked foreground generations, on a
    /// THROWAWAY tmux server (`deck-bench-signal-<pid>`): 5 / 20 / 50
    /// sessions whose pane program (`sleep`) reported one observation each,
    /// so every poll reconciles N generations. Opt-in timing, not a gate:
    /// `cargo test --bin deck-app poll_cost_with_tracked_generations -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn poll_cost_with_tracked_generations() {
        use std::process::Command;
        use std::time::Instant;
        let _store = crate::agent_status::STORE_TEST_LOCK.lock_or_recover();
        let _tracker = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let socket = format!("deck-bench-signal-{}", std::process::id());
        let bin = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/tmux-aarch64-apple-darwin");
        let run = |args: &[&str]| {
            let out = Command::new(&bin)
                .args(["-f", "/dev/null", "-L", &socket])
                .args(args)
                .output()
                .expect("tmux");
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        const ROUNDS: u32 = 20;
        let mut created = 0;
        eprintln!("sessions  scans/poll  processes() ms  reconcile ms  poll_from_listing ms  (avg of {ROUNDS})");
        for n in [5usize, 20, 50] {
            while created < n {
                run(&[
                    "new-session",
                    "-d",
                    "-s",
                    &format!("b{created}"),
                    "sleep",
                    "600",
                ]);
                created += 1;
            }
            let rows: Vec<PaneRow> = run(&["list-panes", "-a", "-F", crate::tmux::PANE_FORMAT])
                .lines()
                .filter_map(crate::tmux::parse_pane_row)
                .collect();
            assert_eq!(rows.len(), n);
            crate::agent_status::reset_for_tests();
            let table = crate::procinfo::processes();
            for row in &rows {
                // the pane program reports for itself: peer == pane process
                let line = format!(
                    "{{\"v\":1,\"source\":\"codex\",\"state\":\"working\",\"socket\":\"{}\",\"server_pid\":{},\"pane\":\"{}\"}}",
                    crate::tmux::socket(),
                    row.server_pid,
                    row.pane_id
                );
                let origin = crate::agent_status::Origin {
                    peer: Some(row.pane_pid),
                    table: table.clone(),
                };
                assert_eq!(
                    crate::agent_status::ingest(&line, &origin, || Some(rows.clone())),
                    Ok(())
                );
            }
            let names: Vec<String> = (0..n).map(|i| format!("b{i}")).collect();
            let t = Instant::now();
            for _ in 0..ROUNDS {
                std::hint::black_box(crate::procinfo::processes());
            }
            let scan = t.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);
            let t = Instant::now();
            for _ in 0..ROUNDS {
                crate::agent_status::reconcile(&rows, &table);
            }
            let reconcile = t.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);
            let t = Instant::now();
            let mut last = Vec::new();
            for _ in 0..ROUNDS {
                last = poll_from_listing(
                    names.clone(),
                    vec![],
                    false,
                    Ok(rows.clone()),
                    crate::procinfo::processes,
                )
                .unwrap();
            }
            let poll = t.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);
            assert!(
                last.iter().all(|info| info.agent == Some("working")),
                "every generation survived"
            );
            eprintln!(
                "{n:>8}  {:>10}  {scan:>14.2}  {reconcile:>12.3}  {poll:>20.2}",
                1
            );
        }
        // tmux leaves its socket file behind: remove exactly this one
        let path = run(&["display-message", "-p", "#{socket_path}"]);
        run(&["kill-server"]);
        let path = std::path::Path::new(path.trim());
        if path.file_name().and_then(|n| n.to_str()) == Some(socket.as_str()) {
            let _ = std::fs::remove_file(path);
        }
        crate::agent_status::reset_for_tests();
        crate::notify::retain(&std::collections::HashSet::new());
    }

    #[test]
    fn failed_listing_rejects_poll_instead_of_reporting_dead_sessions() {
        for kind in [
            ErrorKind::Tmux,
            ErrorKind::TmuxMissing,
            ErrorKind::Perm,
            ErrorKind::NoSession,
        ] {
            let result = poll_from_listing(
                vec!["live-session".into()],
                vec![],
                false,
                Err(DeckError::new(kind, "listing unavailable")),
                crate::procinfo::processes,
            );
            assert_eq!(result.unwrap_err().kind(), kind);
        }
    }

    /// Pins the plan `tmux_contract::shell_restore_bootstrap_becomes_tmux_
    /// history_without_executing_text` executes: the pane is created with no
    /// command, the bytes travel stdin → private buffer → pane tty inside the
    /// tmux server, and nothing on the path is a shell or a script.
    #[test]
    fn restore_start_uses_the_created_pane_tty_with_no_shell_or_argv_payload() {
        let args = restore_start_args("sess", "/tmp/dir", "deck-restore-7");
        let steps: Vec<Vec<&str>> = args
            .split(|a| a == ";")
            .map(|step| step.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(steps[0], ["start-server"]);
        assert_eq!(
            steps[1],
            ["load-buffer", "-b", "deck-restore-7", "-"],
            "bytes come from stdin"
        );
        assert_eq!(
            steps[2],
            [
                "new-session",
                "-d",
                "-P",
                "-F",
                crate::shell_state::RESTORE_TTY_FORMAT,
                "-s",
                "sess",
                "-c",
                "/tmp/dir"
            ],
            "new-session itself reports the created pane tty"
        );
        assert_eq!(steps.len(), 3);
        let emit = restore_emit_args("deck-restore-7", "/dev/ttys007");
        assert_eq!(
            emit,
            [
                "save-buffer",
                "-b",
                "deck-restore-7",
                "/dev/ttys007",
                ";",
                "delete-buffer",
                "-b",
                "deck-restore-7"
            ]
        );
        for forbidden in [
            "sh",
            "/bin/",
            "send-keys",
            "run-shell",
            "if-shell",
            "pipe-pane",
            "deck-app",
        ] {
            assert!(
                !args
                    .iter()
                    .chain(emit.iter())
                    .any(|a| a == forbidden || a.contains("/bin/")),
                "{forbidden} on the restore path"
            );
        }
    }

    #[test]
    fn restore_tty_accepts_one_real_device_and_rejects_ambiguous_output() {
        let tty = "/dev/tty".to_string();
        assert!(crate::procinfo::tty_device(&tty).is_some());
        assert_eq!(restore_tty(&format!("{tty}\n")).unwrap(), tty);
        for bad in ["", "/tmp/not-a-tty", "/dev/ttys001\n/dev/ttys002\n"] {
            assert!(restore_tty(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn card_preview_depth_matches_the_frontend_constant() {
        // pure.js CARD_PREVIEW_ROWS pins the same value; the two must agree
        // or fixed-height previews clip or pad.
        assert_eq!(CARD_PREVIEW_LINES, 6);
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
    fn killing_an_already_missing_session_is_idempotent_but_real_errors_survive() {
        for missing in [
            "tmux kill-session failed: can't find session: x",
            "tmux kill-session failed: no server running",
            "tmux kill-session failed: error connecting to socket (No such file or directory)",
            "tmux kill-session failed: no current target",
        ] {
            assert!(idempotent_kill_result(Err(DeckError::classified(missing))).is_ok());
        }
        for uncertain in [
            "tmux kill-session failed: permission denied",
            "tmux kill-session failed: connection timed out",
            "tmux kill-session failed: malformed response",
            "tmux kill-session failed: unrelated resource not found",
            "tmux list-panes failed: no current target",
        ] {
            assert!(idempotent_kill_result(Err(DeckError::classified(uncertain))).is_err());
        }
        assert!(idempotent_kill_result(Ok(String::new())).is_ok());
    }

    #[test]
    fn terminal_palette_command_accepts_only_literal_hex_colors() {
        for color in ["#000000", "#4fd6be", "#FFFFFF"] {
            assert!(validated_palette_color(color));
        }
        for color in [
            "red",
            "#fff",
            "#000000;run-shell",
            "#[fg=red]",
            "#１２３４５６",
        ] {
            assert!(!validated_palette_color(color), "accepted {color}");
        }
    }

    #[test]
    fn process_tree_memory_reports_each_requested_root() {
        let mut roots = HashMap::new();
        roots.insert("self".to_string(), std::process::id());
        roots.insert("missing".to_string(), u32::MAX);
        let memory = tree_mem(&crate::procinfo::processes(), &roots);
        assert_eq!(memory.len(), 2);
        assert!(memory["self"] > 0.0);
        assert_eq!(memory["missing"], 0.0);
    }
}
