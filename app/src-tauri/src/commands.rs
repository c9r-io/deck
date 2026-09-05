//! The remaining Tauri command surface: event self-test, tmux mode style,
//! editor discovery, session start/kill, clipboard write and the one poll
//! command that feeds card status/memory/preview.
//!
//! # Contract
//! One poll command (`poll_sessions`) returns liveness + `#{window_activity}` recency +
//! process-tree RSS (pane_pid → ps tree walk) + the last six non-empty pane rows
//! for fixed-height, bottom-aligned card previews. Frontend polls every 2.5s and
//! diffs into granular UI events (status/mem/output) — never full re-renders on
//! output.

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
    expand_tilde, pane_target, session_target, tmux, tmux_bin, tmux_with_stdin,
    validate_session_name,
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
    Command::new(tmux_bin())
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
/// The restored-shell start as ONE tmux server sequence: keep an empty server
/// alive, load the sanitized transcript from stdin into a private buffer,
/// create the pane the ORDINARY way (tmux's own login shell, no command),
/// have the SERVER write the buffer to the new pane's tty, then discard it.
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
        "-s",
        name,
        "-c",
        dir,
        ";",
        "save-buffer",
        "-b",
        buffer,
        crate::shell_state::RESTORE_TTY_FORMAT,
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
        // One sequence: the pane is created exactly like a clean shell
        // (tmux's own login shell, no command), then the SERVER writes the
        // private buffer to that pane's tty and discards it. The write
        // follows the fork immediately, so it lands before the shell's
        // first prompt; a slow rc file can only reorder text, never run it.
        let args = restore_start_args(&name, &dir, &bootstrap.buffer);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let restored_start = tmux_with_stdin(&args, &bootstrap.output);
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
    start?;
    let created_ms = t0.elapsed().as_millis();
    if !cmd.trim().is_empty() {
        tmux(&["send-keys", "-t", &pane_target(&name), &cmd, "Enter"])?;
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
pub(crate) fn kill_session(name: String) -> Result<(), DeckError> {
    validate_session_name(&name)?;
    idempotent_kill_result(tmux(&["kill-session", "-t", &session_target(&name)]))?;
    // Closing a card is also a privacy deletion: its transcript, backup and
    // quarantined recovery copies must not outlive the card.
    crate::shell_state::clear_snapshot(&name)
}

pub(crate) fn idempotent_kill_result(result: Result<String, DeckError>) -> Result<(), DeckError> {
    match result {
        Ok(_) => Ok(()),
        // Closing an already-gone session is the successful end state. This
        // also covers an empty deck tmux server ("no server running").
        Err(e) if matches!(e.kind(), ErrorKind::NoSession | ErrorKind::Missing) => Ok(()),
        Err(e) => Err(e),
    }
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
    /// live pane cwd; the frontend persists changes into the card so even a
    /// disabled transcript checkpoint still restarts in the right directory
    cwd: Option<String>,
    /// pane is in tmux copy-mode: the VISIBLE frame is frozen scrollback,
    /// not live output — the UI must say so (a silently frozen agent TUI
    /// reads as a hung session)
    scrolled: Option<bool>,
    /// closed agent-hook state word ("working" | "needs-input" |
    /// "turn-done"), if an agent module reported one (agent_status.rs)
    agent: Option<&'static str>,
}

pub(crate) fn tree_mem(roots: &HashMap<String, u32>) -> HashMap<String, f64> {
    crate::procinfo::tree_memory(roots)
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

/// One pane-listing line → (session, pane pid, activity epoch, in copy-mode,
/// fg command). Every tmux session has at least one pane, so this listing
/// doubles as the liveness set — no separate `list-sessions` round-trip.
pub(crate) fn parse_panes(text: &str) -> HashMap<String, (u32, u64, bool, String, String)> {
    let mut panes: HashMap<String, (u32, u64, bool, String, String)> = HashMap::new();
    for line in text.lines() {
        let mut it = line.split('\t');
        if let (Some(s), Some(pid), Some(act), Some(mode), Some(fg), Some(cwd)) = (
            it.next(),
            it.next(),
            it.next(),
            it.next(),
            it.next(),
            it.next(),
        ) {
            if let (Ok(pid), Ok(act)) = (pid.parse(), act.parse()) {
                if !cwd.is_empty() && !cwd.chars().any(char::is_control) {
                    panes.entry(s.to_string()).or_insert((
                        pid,
                        act,
                        mode == "1",
                        fg.to_string(),
                        cwd.to_string(),
                    ));
                }
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
pub(crate) fn poll_sessions(
    names: Vec<String>,
    tail_for: Vec<String>,
    checkpoint_shells: bool,
) -> Vec<SessInfo> {
    // one listing supplies liveness + activity + pid + fg for every session
    let listing = tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}\t#{pane_pid}\t#{window_activity}\t#{pane_in_mode}\t#{pane_current_command}\t#{pane_current_path}",
    ]);
    // a failing listing silently reads as "everything is dead" — log the
    // failure and the recovery, once per transition (tmux errors carry no
    // user content)
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
    let panes = parse_panes(&listing.unwrap_or_default());
    // agent-hook state lives exactly as long as the foreground process that
    // reported it — clear entries whose pane moved on before they render
    crate::agent_status::reconcile(&panes);

    let roots: HashMap<String, u32> = names
        .iter()
        .filter_map(|n| panes.get(n).map(|(pid, _, _, _, _)| (n.clone(), *pid)))
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
    let mut tails = capture_tails(&want_tails, CARD_PREVIEW_LINES);
    let now = now_epoch();

    // Snapshot work is throttled and runs off-thread; this call only selects
    // the small fair batch.  No pane content enters logs or the poll payload.
    crate::shell_state::schedule_checkpoints(
        panes
            .iter()
            .map(|(session, (_, activity, _, foreground, cwd))| {
                crate::shell_state::ShellObservation {
                    session: session.clone(),
                    activity: *activity,
                    cwd: cwd.clone(),
                    foreground: foreground.clone(),
                }
            })
            .collect(),
        checkpoint_shells,
    );

    names
        .into_iter()
        .map(|name| {
            let pane = panes.get(&name);
            SessInfo {
                alive: pane.is_some(),
                idle_secs: pane.map(|(_, act, _, _, _)| now.saturating_sub(*act)),
                mem_mb: mem.get(&name).copied(),
                tail: tails.remove(&name).unwrap_or_default(),
                fg: pane.map(|(_, _, _, fg, _)| fg.clone()),
                cwd: pane.map(|(_, _, _, _, cwd)| cwd.clone()),
                scrolled: pane.map(|(_, _, m, _, _)| *m),
                agent: pane.and_then(|_| crate::agent_status::current(&name)),
                name,
            }
        })
        .collect()
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

    #[test]
    fn parse_panes_basic_and_malformed() {
        let text = "alpha\t100\t1700000000\t0\tzsh\t/tmp/a\nbeta\t200\t1700000005\t1\tclaude\t/tmp/b\njunk-line\nempty\t\t\t\t\t\n";
        let p = parse_panes(text);
        assert_eq!(p.len(), 2);
        assert_eq!(
            p["alpha"],
            (100, 1700000000, false, "zsh".into(), "/tmp/a".into())
        );
        assert_eq!(
            p["beta"],
            (200, 1700000005, true, "claude".into(), "/tmp/b".into()),
            "copy-mode pane reported as scrolled"
        );
    }

    #[test]
    fn parse_panes_first_pane_wins() {
        // multi-pane session: the first listed pane is the representative one
        let text = "s\t10\t111\t0\tzsh\t/tmp/one\ns\t20\t222\t1\tvim\t/tmp/two\n";
        assert_eq!(
            parse_panes(text)["s"],
            (10, 111, false, "zsh".into(), "/tmp/one".into())
        );
    }

    /// Pins the plan `tmux_contract::shell_restore_bootstrap_becomes_tmux_
    /// history_without_executing_text` executes: the pane is created with no
    /// command, the bytes travel stdin → private buffer → pane tty inside the
    /// tmux server, and nothing on the path is a shell or a script.
    #[test]
    fn restore_start_is_one_tmux_sequence_with_no_shell_and_no_argv_payload() {
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
            ["new-session", "-d", "-s", "sess", "-c", "/tmp/dir"],
            "the pane is created exactly like a clean shell: no command argument"
        );
        assert_eq!(
            steps[3],
            [
                "save-buffer",
                "-b",
                "deck-restore-7",
                crate::shell_state::RESTORE_TTY_FORMAT
            ]
        );
        assert_eq!(steps[4], ["delete-buffer", "-b", "deck-restore-7"]);
        assert_eq!(steps.len(), 5);
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
                !args.iter().any(|a| a == forbidden || a.contains("/bin/")),
                "{forbidden} on the restore path"
            );
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
        ] {
            assert!(idempotent_kill_result(Err(DeckError::classified(missing))).is_ok());
        }
        let real = idempotent_kill_result(Err(DeckError::classified(
            "tmux kill-session failed: permission denied",
        )));
        assert!(real.is_err());
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
        let memory = tree_mem(&roots);
        assert_eq!(memory.len(), 2);
        assert!(memory["self"] > 0.0);
        assert_eq!(memory["missing"], 0.0);
    }
}
