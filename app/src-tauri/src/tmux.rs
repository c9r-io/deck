//! tmux backend: sidecar discovery, the private `deck` server, config, raw
//! command execution, and the ONE pane row (`PaneRow` / `PANE_FORMAT`,
//! `list_panes`, `pane_row`) every probe in deck reads panes through. Every
//! production listing is framed by a per-query random nonce (`PaneQuery`)
//! and fails whole when an untrusted pane path splits or forges a row.
//! Everything deck knows about tmux lives here. The spawn sites stay in
//! `tmux`, `tmux_owned`, `tmux_with_stdin`, `tmux_batch` and `connect_with`;
//! the processing behind them (`captured_output`, `command_with_stdin`,
//! `list_panes_with`, `pane_row_with`, `init_deck_server_with`) takes the
//! command or runner as an argument so an isolated bundled tmux exercises
//! the production logic without touching the deck/deck-dev sockets.
//!
//! # Contract
//! tmux ships INSIDE the app: a statically linked binary (see
//! `binaries/build-tmux.sh`, committed as `binaries/tmux-aarch64-apple-darwin`,
//! bundled+signed via tauri `externalBin`). `tmux_bin()` resolves ONLY that
//! sidecar, next to this build's own executable, and never falls back: no
//! Homebrew/MacPorts probe and no PATH lookup, because `/usr/local/bin` is
//! user-writable on many Macs and a PATH entry is not deck's to trust — the
//! one tmux deck executes is the one it signed and shipped. `tmux_program()`
//! is the single gate every spawn goes through (here, `commands.rs`,
//! `pty.rs`, `tmux_lifecycle.rs`; `tests/edr_quiet.rs` allowlists exactly
//! those). A build without its sidecar has no tmux at all (`tmux_kind()`
//! says `missing`, every spawn fails `TmuxMissing`); `app/run.sh` copies the
//! sidecar into the dev bundle for exactly this reason. deck talks to its
//! OWN server (`-L deck`
//! socket) — never version-clashes with a user tmux, and deck sessions don't
//! appear in the user's `tmux ls`. Production debug: `tmux -L deck ls`;
//! source bundles use `tmux -L deck-dev ls`.
//!
//! The server runs with `set-clipboard off` (`tmux_conf_text`,
//! `init_deck_server`; `tests/tmux_contract.rs` pins it): tmux neither
//! accepts an application's OSC 52 request as a paste buffer nor answers
//! its `?` query, and it never forwards its own copy-mode buffers to the
//! attached client as OSC 52. Any program in a pane could otherwise rewrite
//! the macOS clipboard (paste-jacking: `cat` of a hostile file, a commit
//! message, tool output) or read what another card's program pushed. deck's
//! own copy never used OSC 52: ⌘C snapshots the tmux selection into a named
//! buffer and writes it through `/usr/bin/pbcopy` (`terminal_selection.rs`,
//! `commands::write_clipboard`); `off` rather than `external` because
//! `external` would still base64 every selection snapshot through the PTY
//! into the webview for nothing.

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStderr, ChildStdout, Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};

use crate::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;

// ---------- tmux helpers ----------------------------------------------------

/// Absolute path to the ONE tmux deck may execute: the statically linked
/// sidecar next to this build's own executable, signed inside the same
/// bundle. There is deliberately no second candidate — a Homebrew/MacPorts
/// probe or a PATH lookup would let anything with write access to
/// `/usr/local/bin` (user-owned on many Macs) or to a PATH entry choose the
/// binary deck runs as the user, and every deck session descends from it.
/// Empty when this build has no sidecar; `tmux_program` turns that into
/// `TmuxMissing` instead of letting an empty program reach a spawn.
pub(crate) fn tmux_bin() -> &'static str {
    static BIN: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let sidecar = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("tmux")))
            .filter(|path| path.is_file());
        match sidecar {
            // log the CATEGORY, never the absolute path (it can embed
            // the .app location / user directories and ends up in exports)
            Some(path) => {
                applog("[tmux] using the bundled sidecar");
                path.display().to_string()
            }
            None => {
                applog("[tmux] this build has no bundled sidecar");
                String::new()
            }
        }
    })
}

/// The sidecar, or `TmuxMissing` — the one gate every spawn goes through, so
/// a build without its sidecar reports the same error everywhere instead of
/// handing an empty program to `Command`/`CommandBuilder`.
pub(crate) fn tmux_program() -> Result<&'static str, DeckError> {
    let bin = tmux_bin();
    if bin.is_empty() {
        return Err(DeckError::new(
            ErrorKind::TmuxMissing,
            "this build has no bundled tmux",
        ));
    }
    Ok(bin)
}

/// Path-free tmux availability for logs/exports: deck either runs its own
/// sidecar or has no tmux.
pub(crate) fn tmux_kind() -> &'static str {
    if tmux_bin().is_empty() {
        "missing"
    } else {
        "sidecar"
    }
}

/// deck runs its own tmux server (socket "deck"): the bundled binary never
/// clashes with a user-installed tmux of a different version, and deck's
/// sessions stay out of the user's personal `tmux ls`.
pub(crate) fn socket() -> &'static str {
    static SOCKET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SOCKET.get_or_init(|| {
        if crate::launch_args::debug_arg("--smoke-data-dir").is_some() {
            return crate::launch_args::debug_arg("--smoke-tmux-socket")
                .filter(|s| {
                    s.starts_with("deck-smoke")
                        && !s.is_empty()
                        && s.len() <= 48
                        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
                .unwrap_or_else(|| "deck-smoke".into());
        }
        if cfg!(debug_assertions) {
            "deck-dev".into()
        } else {
            "deck".into()
        }
    })
}

/// Server config file: options set via `tmux set -g` die with the server
/// process (a session-less server exits immediately, so boot-time `set`
/// calls raced server restarts and the status bar kept coming back).
/// A -f config is applied at every server spawn, deterministically.
pub(crate) fn tmux_conf() -> String {
    static CONF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CONF.get_or_init(|| {
        let deck_dir = crate::datadir::deck_dir();
        let path = deck_dir.join("tmux.conf");
        if let Some(dir) = path.parent() {
            let _ = crate::datadir::create_private_dir(dir);
        }
        let _ = crate::datadir::write_private(&path, tmux_conf_text(&deck_dir).as_bytes());
        path.display().to_string()
    })
    .clone()
}

/// The server defaults every deck tmux server starts with. Intermediate
/// copy-mode selection frames are styled `none` so tmux's cursor-bound
/// highlight never flashes under Deck's own selection overlay; only the
/// copy cursor position is styled.
pub(crate) fn tmux_conf_text(deck_dir: &std::path::Path) -> String {
    format!(
        "# generated by deck — server defaults\n\
         set -g status off\n\
         set -g set-clipboard off\n\
         set -g history-limit 50000\n\
         set -g mode-style 'none'\n\
         set -g copy-mode-selection-style 'none'\n\
         set -g copy-mode-position-style 'reverse'\n\
         set -g copy-mode-position-format ''\n\
         set-environment -g COLORTERM truecolor\n\
         {}",
        status_sock_env_line(deck_dir)
    )
}

/// Run tmux (on the deck server) with output captured — stray stderr must
/// never reach a terminal.
///
/// LANG is pinned to a UTF-8 locale: GUI-launched apps get NO locale env,
/// and under the C locale tmux sanitizes every control character in command
/// output — including the \t field separators poll_sessions parses — to
/// '_', and mangles non-ASCII pane content in capture-pane. (Shipped as the
/// v0.4.16 "board all gray / no separators" bug: dev builds launched from a
/// terminal inherited the shell's LANG and never reproduced it.) pty.rs
/// sets the same for the attach client.
pub(crate) fn tmux(args: &[&str]) -> Result<String, DeckError> {
    let conf = tmux_conf();
    captured_output(
        Command::new(tmux_program()?).args(["-f", &conf, "-L", socket()]),
        args,
    )
}

/// The capture shared by `tmux` and `tmux_owned`: one bounded run under the
/// caller's deadline, a timeout or not-runnable spawn classified, a failed
/// status turned into the classified `tmux <verb> failed: <stderr>` error,
/// and stdout returned as text. The caller owns the spawn site; isolated
/// tests supply their own tmux socket through `command`.
pub(super) fn captured_output<S: AsRef<std::ffi::OsStr>>(
    command: &mut Command,
    args: &[S],
) -> Result<String, DeckError> {
    let out = crate::session_runtime::command_output(command.args(args).env("LANG", "en_US.UTF-8"))
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::TimedOut {
                DeckError::restart(crate::error::RestartFailure::Deadline)
            } else {
                DeckError::new(ErrorKind::TmuxMissing, format!("tmux not runnable: {e}"))
            }
        })?;
    if !out.status.success() {
        return Err(DeckError::classified(format!(
            "tmux {} failed: {}",
            args.first()
                .map(|arg| arg.as_ref().to_string_lossy())
                .unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run tmux with bounded caller-owned bytes on stdin. Prompt delivery and shell
/// recovery use this so private text never appears in argv, an environment
/// variable, or a temporary file. `start-server ; load-buffer ... - ;
/// new-session ...` may be submitted as one batch, which also keeps a newly
/// spawned zero-session server alive until the restored pane exists.
pub(crate) fn tmux_with_stdin(args: &[&str], input: &[u8]) -> Result<String, DeckError> {
    let conf = tmux_conf();
    let mut command = Command::new(tmux_program()?);
    command.args(["-f", &conf, "-L", socket()]);
    command_with_stdin(&mut command, args, input)
}

/// Shared pipe implementation; isolated tests supply their own tmux socket.
pub(super) fn command_with_stdin(
    command: &mut Command,
    args: &[&str],
    input: &[u8],
) -> Result<String, DeckError> {
    let mut child = command
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| DeckError::new(ErrorKind::TmuxMissing, format!("tmux not runnable: {e}")))?;
    if crate::session_runtime::deadline_active() {
        let result = bounded_stdin_output(&mut child, input);
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let out = result?;
        if !out.status.success() {
            return Err(DeckError::classified(format!(
                "tmux {} failed: {}",
                args.first().unwrap_or(&""),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let write_result = child
        .stdin
        .as_mut()
        .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux stdin unavailable"))
        .and_then(|stdin| {
            stdin
                .write_all(input)
                .map_err(|e| DeckError::classified(format!("tmux stdin failed: {e}")))
        });
    drop(child.stdin.take());
    if let Err(error) = write_result {
        let _ = child.wait();
        return Err(error);
    }
    let out = child
        .wait_with_output()
        .map_err(|e| DeckError::classified(format!("tmux wait failed: {e}")))?;
    if !out.status.success() {
        return Err(DeckError::classified(format!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn bounded_stdin_output(
    child: &mut std::process::Child,
    input: &[u8],
) -> Result<Output, DeckError> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux stdin unavailable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux stdout unavailable"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux stderr unavailable"))?;
    for fd in [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(DeckError::new(ErrorKind::Tmux, "tmux pipe unavailable"));
        }
    }
    let mut written = 0usize;
    let mut stdin = Some(stdin);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut exited = None;
    loop {
        crate::session_runtime::check_deadline()?;
        if written < input.len() {
            match stdin.as_mut().unwrap().write(&input[written..]) {
                Ok(0) => {
                    return Err(DeckError::new(ErrorKind::Tmux, "tmux stdin closed"));
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(DeckError::new(ErrorKind::Tmux, "tmux stdin failed")),
            }
        }
        if written == input.len() {
            stdin.take();
        }
        let mut caught_up = true;
        for (pipe, bytes) in [
            (&mut stdout as &mut dyn Read, &mut out),
            (&mut stderr as &mut dyn Read, &mut err),
        ] {
            let mut buffer = [0u8; 8192];
            let mut drained = false;
            for _ in 0..64 {
                match pipe.read(&mut buffer) {
                    Ok(0) => {
                        drained = true;
                        break;
                    }
                    Ok(count) => {
                        bytes.extend_from_slice(&buffer[..count]);
                        if bytes.len() > 8 * 1024 * 1024 {
                            return Err(DeckError::new(ErrorKind::Tmux, "tmux output limit"));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        drained = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(DeckError::new(ErrorKind::Tmux, "tmux output failed")),
                }
            }
            caught_up &= drained;
        }
        if let Some(status) = exited.filter(|_| caught_up) {
            return Ok(Output {
                status,
                stdout: out,
                stderr: err,
            });
        }
        exited = child
            .try_wait()
            .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux wait failed"))?;
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Owned-argument variant used for bounded command batches whose numeric
/// cursor movements are assembled at runtime. Arguments still bypass a
/// shell; `;` is an explicit tmux command separator, never shell syntax.
pub(crate) fn tmux_owned(args: &[String]) -> Result<String, DeckError> {
    let conf = tmux_conf();
    captured_output(
        Command::new(tmux_program()?).args(["-f", &conf, "-L", socket()]),
        args,
    )
}

/// Run a `;`-separated tmux command batch, returning stdout even when one
/// command in the batch fails (e.g. a session died between listing and
/// capture). Callers parse per-command markers, so partial output is useful
/// and a hard error would throw away every other command's result.
pub(crate) fn tmux_batch(args: &[String]) -> String {
    let Ok(tmux_sidecar) = tmux_program() else {
        return String::new();
    };
    let conf = tmux_conf();
    Command::new(tmux_sidecar)
        .args(["-f", &conf, "-L", socket()])
        .args(args)
        .env("LANG", "en_US.UTF-8") // see tmux(): C locale mangles output
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// One tmux.conf line exporting this instance's status-socket path into the
/// server environment (see agent_status.rs). A path tmux's double-quoted
/// string could misread is skipped — the helper then uses its production
/// default, which is only wrong for exotic isolated data dirs.
pub(crate) fn status_sock_env_line(deck_dir: &std::path::Path) -> String {
    let sock = deck_dir.join("status.sock").display().to_string();
    if sock.contains(['"', '\\', '\n', '#', '$', '`']) {
        return String::new();
    }
    format!("set-environment -g DECK_STATUS_SOCK \"{sock}\"\n")
}

/// Escape a string for use inside a tmux format/message argument:
/// `#` starts a format expansion in `display-message -p`.
pub(crate) fn fmt_escape(s: &str) -> String {
    s.replace('#', "##")
}

/// Session names reach tmux CLI arguments, format strings, and log lines, so
/// the accepted alphabet is the intersection of what tmux allows (no `:` `.`)
/// and what can never read as a flag (no leading `-`) or a format expansion
/// (no `#`). deck itself only generates `deck-<a-z0-9-slug>-<id>`.
pub(crate) fn validate_session_name(name: &str) -> Result<(), DeckError> {
    if name.is_empty() || name.len() > 64 {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "session name must be 1–64 characters",
        ));
    }
    if name.starts_with('-') {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "session name must not start with '-'",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '@'))
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "session name may only contain letters, digits, _ - @",
        ));
    }
    Ok(())
}

/// Exact-match session target. Pane-level commands need the trailing colon.
pub(crate) fn session_target(name: &str) -> String {
    format!("={name}")
}
pub(crate) fn pane_target(name: &str) -> String {
    format!("={name}:")
}

pub(crate) fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.display(), rest);
        }
    }
    path.to_string()
}

// ---------- session lifecycle ------------------------------------------------

/// Apply the server defaults to an ALREADY RUNNING server that a previous
/// deck build started (`-f tmux.conf` covers every server this build
/// spawns). Called once per boot by `tmux_lifecycle::reconcile_on_boot`
/// when it reuses a compatible server — never per session: each `set` is
/// one tmux exec, and an endpoint-security agent taxes every exec.
pub(crate) fn init_deck_server() {
    init_deck_server_with(&tmux, &crate::datadir::deck_dir());
}

/// The reuse defaults themselves, against the server `run` reaches (an
/// isolated test server, or deck's own through `tmux`).
pub(crate) fn init_deck_server_with(
    run: &dyn Fn(&[&str]) -> Result<String, DeckError>,
    deck_dir: &std::path::Path,
) {
    let _ = run(&["set-environment", "-g", "COLORTERM", "truecolor"]);
    // Panes tell the status helper (agent_status.rs) which instance's socket
    // to write to, so an isolated/smoke instance receives its own events.
    let sock = deck_dir.join("status.sock");
    let _ = run(&[
        "set-environment",
        "-g",
        "DECK_STATUS_SOCK",
        &sock.display().to_string(),
    ]);
    let _ = run(&["set", "-g", "status", "off"]);
    let _ = run(&["set", "-g", "mouse", "off"]);
    let _ = run(&["set", "-g", "set-clipboard", "off"]);
    let _ = run(&["set", "-g", "history-limit", "50000"]);
    // Deck paints selection geometry in one DOM layer after each settled
    // backend update. Hiding tmux's transient selection prevents its
    // top-line/cursor motion steps from flashing large history regions.
    // Keep mode-style empty as the compatibility fallback while the bundled
    // tmux exposes separate selection and position styles.
    let _ = run(&["set", "-g", "mode-style", "none"]);
    let _ = run(&["set", "-g", "copy-mode-selection-style", "none"]);
    let _ = run(&["set", "-g", "copy-mode-position-style", "reverse"]);
    let _ = run(&["set", "-g", "copy-mode-position-format", ""]);
}

// ---------- pane rows -------------------------------------------------------

/// The ONE pane row every deck probe reads. `PANE_FORMAT` is the superset of
/// the fields the poll (`commands.rs`), the agent-status resolver, the
/// scheduler tick, the context probe and the lifecycle probe need;
/// `list_panes()` reads every pane on the server and `pane_row(target)` one
/// pane, both through a `PaneQuery`. Adding a field is one format entry
/// plus one struct field. `path` is last because a directory name may
/// contain a tab; every other field is tmux-generated and tab-free.
///
/// A directory name may also contain a newline, and tmux prints
/// `#{pane_current_path}` verbatim in both the one-shot and the control-mode
/// listing (verified against the bundled tmux). An untrusted path could
/// therefore split one pane into two lines, forge a whole pane row, or put a
/// `%end` record inside a control frame. Every production read wraps the
/// format in a fresh random nonce (first and last field) and rejects the
/// whole listing when any line is not exactly framed, so a split, forged or
/// truncated row fails the read instead of being half-trusted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PaneRow {
    pub(crate) server_pid: u32,
    /// `$N`
    pub(crate) session_id: String,
    pub(crate) session_name: String,
    /// `@N`
    pub(crate) window_id: String,
    /// `%N`
    pub(crate) pane_id: String,
    pub(crate) pane_pid: u32,
    pub(crate) window_activity: u64,
    pub(crate) in_mode: bool,
    /// `#{pane_current_command}` verbatim; callers sanitize.
    pub(crate) command: String,
    pub(crate) tty: String,
    /// `#{pane_current_path}` verbatim; callers validate.
    pub(crate) path: String,
}

/// Compare the observed pane identities, with or without foreground state.
pub(crate) fn unchanged_rows(before: &[PaneRow], after: &[PaneRow]) -> bool {
    before.len() == after.len()
        && before.iter().all(|row| {
            after
                .iter()
                .any(|now| same_pane(row, now) && row.command == now.command)
        })
}
pub(crate) fn same_pane(a: &PaneRow, b: &PaneRow) -> bool {
    a.server_pid == b.server_pid
        && a.session_name == b.session_name
        && a.session_id == b.session_id
        && a.window_id == b.window_id
        && a.pane_id == b.pane_id
        && a.pane_pid == b.pane_pid
}
pub(crate) const PANE_FORMAT: &str = "#{pid}\t#{session_id}\t#{session_name}\t#{window_id}\t#{pane_id}\t#{pane_pid}\t#{window_activity}\t#{pane_in_mode}\t#{pane_current_command}\t#{pane_tty}\t#{pane_current_path}";

fn tmux_id(value: &str, prefix: char) -> bool {
    value
        .strip_prefix(prefix)
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// Strict: exactly the fields of `PANE_FORMAT`, tmux ids in their `$ @ %`
/// shapes and non-zero pids. Anything else is `None` — a row is never
/// half-read. (Session names are NOT validated here: a foreign session on
/// the socket must not blank the listing; callers that store a name check
/// it with `validate_session_name`.)
pub(crate) fn parse_pane_row(line: &str) -> Option<PaneRow> {
    let mut fields = line.trim_end_matches(['\r', '\n']).splitn(11, '\t');
    let server_pid: u32 = fields.next()?.parse().ok()?;
    let session_id = fields.next()?;
    let session_name = fields.next()?;
    let window_id = fields.next()?;
    let pane_id = fields.next()?;
    let pane_pid: u32 = fields.next()?.parse().ok()?;
    let window_activity: u64 = fields.next()?.parse().ok()?;
    let in_mode = match fields.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    let command = fields.next()?;
    let tty = fields.next()?;
    let path = fields.next()?;
    if server_pid == 0
        || pane_pid == 0
        || !tmux_id(session_id, '$')
        || !tmux_id(window_id, '@')
        || !tmux_id(pane_id, '%')
    {
        return None;
    }
    Some(PaneRow {
        server_pid,
        session_id: session_id.into(),
        session_name: session_name.into(),
        window_id: window_id.into(),
        pane_id: pane_id.into(),
        pane_pid,
        window_activity,
        in_mode,
        command: command.into(),
        tty: tty.into(),
        path: path.into(),
    })
}

fn malformed_row() -> DeckError {
    DeckError::new(ErrorKind::Tmux, "tmux returned a malformed pane row")
}

/// One pane listing's framing: a fresh 128-bit random nonce is the first
/// and the last field of every row. A pane path cannot know the nonce, so a
/// newline inside it leaves an unframed line and the whole listing is
/// rejected; the one-shot and control-mode reads share this one parser and
/// therefore fail identically.
pub(crate) struct PaneQuery {
    nonce: String,
    format: String,
}

impl PaneQuery {
    pub(crate) fn new() -> Result<Self, DeckError> {
        let mut bytes = [0u8; 16];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| DeckError::new(ErrorKind::Tmux, "pane query nonce unavailable"))?;
        Ok(Self::with_nonce(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }

    fn with_nonce(nonce: String) -> Self {
        let format = format!("{nonce}\t{PANE_FORMAT}\t{nonce}");
        Self { nonce, format }
    }

    /// The `-F` format for this one query; hex nonce only, so it is safe
    /// inside the single-quoted control-mode command as well.
    pub(crate) fn format(&self) -> &str {
        &self.format
    }

    fn row(&self, line: &str) -> Option<PaneRow> {
        let body = line
            .strip_prefix(self.nonce.as_str())?
            .strip_prefix('\t')?
            .strip_suffix(self.nonce.as_str())?
            .strip_suffix('\t')?;
        parse_pane_row(body)
    }

    /// Every row of one listing, or an error when ANY line is not exactly
    /// framed. There is no partial result.
    pub(crate) fn rows(&self, raw: &str) -> Result<Vec<PaneRow>, DeckError> {
        raw.lines()
            .map(|line| self.row(line).ok_or_else(malformed_row))
            .collect()
    }
}

/// Every pane on deck's server, in tmux's listing order (a session's first
/// pane comes first). One malformed or unframed line fails the whole read.
pub(crate) fn list_panes() -> Result<Vec<PaneRow>, DeckError> {
    list_panes_with(&tmux)
}

/// The same read against whichever server `run` reaches (the lifecycle
/// probe and isolated tests pass their own runner).
pub(crate) fn list_panes_with(
    run: &dyn Fn(&[&str]) -> Result<String, DeckError>,
) -> Result<Vec<PaneRow>, DeckError> {
    let query = PaneQuery::new()?;
    query.rows(&run(&["list-panes", "-a", "-F", query.format()])?)
}

// ---------- persistent query channel ----------------------------------------
//
// The client's argv, and why it is NOT read-only (`-r`), live in
// `tmux_clients.rs`: while no pane is attached it is tmux's ambient target
// client for every one-shot command. `tests/tmux_contract.rs` attaches that
// same argv in its client-topology matrix.

const CONTROL_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const CONTROL_QUERY_BUDGET: Duration = Duration::from_millis(1500);
const CONTROL_RETRY_DELAY: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedControlClient {
    client_pid: u32,
    server_pid: u32,
    session: String,
}

static OWNED_CONTROL_CLIENT: Mutex<Option<OwnedControlClient>> = Mutex::new(None);

/// Identity used only to remove Deck's own query control client from the
/// lifecycle impact count. Callers must still verify it against list-clients;
/// a remembered PID alone is never authority after a process exits/reuses it.
pub(crate) fn owned_control_client() -> Option<(u32, u32, String)> {
    OWNED_CONTROL_CLIENT
        .lock_or_recover()
        .clone()
        .map(|owned| (owned.client_pid, owned.server_pid, owned.session))
}

#[derive(Debug, Eq, PartialEq)]
enum ControlFeed {
    Pending,
    Complete(String),
    Failed,
    Exited,
}

#[derive(Default)]
struct ControlParser {
    command_id: Option<u64>,
    output: String,
}

fn control_marker(line: &str, marker: &str) -> Option<u64> {
    let mut fields = line.split_whitespace();
    (fields.next()? == marker)
        .then_some(())
        .and_then(|_| fields.next())
        .and_then(|_| fields.next())
        .and_then(|id| id.parse().ok())
}

impl ControlParser {
    fn feed(&mut self, line: &str) -> Result<ControlFeed, DeckError> {
        if let Some(id) = control_marker(line, "%begin") {
            if self.command_id.replace(id).is_some() {
                return Err(DeckError::new(ErrorKind::Tmux, "tmux control nested frame"));
            }
            self.output.clear();
            return Ok(ControlFeed::Pending);
        }
        if let Some(id) = control_marker(line, "%end") {
            if self.command_id.take() != Some(id) {
                return Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux control frame mismatch",
                ));
            }
            return Ok(ControlFeed::Complete(std::mem::take(&mut self.output)));
        }
        if let Some(id) = control_marker(line, "%error") {
            if self.command_id.take() != Some(id) {
                return Err(DeckError::new(
                    ErrorKind::Tmux,
                    "tmux control frame mismatch",
                ));
            }
            self.output.clear();
            return Ok(ControlFeed::Failed);
        }
        if line == "%exit" || line.starts_with("%exit ") {
            self.command_id = None;
            self.output.clear();
            return Ok(ControlFeed::Exited);
        }
        // tmux never emits a notification inside an output block, so a `%`
        // line inside a frame is command output that imitates the protocol
        // (for example a pane path containing a newline): reject the frame.
        // Outside a frame, `%` records are asynchronous notifications.
        if line.starts_with('%') && self.command_id.is_some() {
            self.command_id = None;
            self.output.clear();
            return Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux control record inside a frame",
            ));
        }
        if self.command_id.is_none() {
            return Ok(ControlFeed::Pending);
        }
        if self.output.len().saturating_add(line.len() + 1) > CONTROL_OUTPUT_LIMIT {
            return Err(DeckError::new(ErrorKind::Tmux, "tmux control output limit"));
        }
        self.output.push_str(line);
        self.output.push('\n');
        Ok(ControlFeed::Pending)
    }
}

fn take_control_line(buffer: &mut Vec<u8>) -> Result<Option<String>, DeckError> {
    let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
        return Ok(None);
    };
    let bytes: Vec<u8> = buffer.drain(..=end).collect();
    let line = std::str::from_utf8(&bytes[..bytes.len() - 1])
        .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux control invalid utf8"))?;
    Ok(Some(line.strip_suffix('\r').unwrap_or(line).to_owned()))
}

struct TmuxQueryChannel {
    child: Child,
    stdout: ChildStdout,
    stderr: ChildStderr,
    bytes: Vec<u8>,
    parser: ControlParser,
    server_pid: u32,
    session: String,
}

fn nonblocking(fd: i32) -> Result<(), DeckError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux control pipe unavailable",
        ));
    }
    Ok(())
}

impl TmuxQueryChannel {
    fn connect(rows: &[PaneRow]) -> Result<Self, DeckError> {
        let conf = tmux_conf();
        let program = tmux_program()?;
        reap_orphaned_query_clients(program, &conf, socket());
        Self::connect_with(program, &conf, socket(), rows)
    }

    fn connect_with(
        program: &str,
        conf: &str,
        socket_name: &str,
        rows: &[PaneRow],
    ) -> Result<Self, DeckError> {
        let first = rows
            .iter()
            .min_by(|a, b| a.session_name.cmp(&b.session_name))
            .ok_or_else(|| DeckError::new(ErrorKind::NoSession, "no sessions"))?;
        let server_pid = first.server_pid;
        if rows.iter().any(|row| row.server_pid != server_pid) {
            return Err(DeckError::new(ErrorKind::Tmux, "tmux server changed"));
        }
        let session = first.session_name.clone();
        let mut child = Command::new(program)
            .args(["-f", conf, "-L", socket_name])
            .args(crate::tmux_clients::query_client_args(&session_target(
                &session,
            )))
            .env("LANG", "en_US.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                DeckError::new(
                    ErrorKind::TmuxMissing,
                    format!("tmux control not runnable: {error}"),
                )
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux control stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux control stderr unavailable"))?;
        nonblocking(stdout.as_raw_fd())?;
        nonblocking(stderr.as_raw_fd())?;
        let mut channel = Self {
            child,
            stdout,
            stderr,
            bytes: Vec::new(),
            parser: ControlParser::default(),
            server_pid,
            session,
        };
        // The attach command itself is the first framed response. Consume it
        // before accepting the first fixed query so command frames cannot mix.
        channel.read_frame(Instant::now() + CONTROL_QUERY_BUDGET)?;
        let owned = OwnedControlClient {
            client_pid: channel.child.id(),
            server_pid,
            session: channel.session.clone(),
        };
        *OWNED_CONTROL_CLIENT.lock_or_recover() = Some(owned);
        Ok(channel)
    }

    fn drain_stderr(&mut self) -> Result<(), DeckError> {
        let mut total = 0usize;
        let mut bytes = [0u8; 4096];
        loop {
            match self.stderr.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(count) => {
                    total += count;
                    if total > 64 * 1024 {
                        return Err(DeckError::new(ErrorKind::Tmux, "tmux control stderr limit"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    return Err(DeckError::new(
                        ErrorKind::Tmux,
                        "tmux control stderr failed",
                    ))
                }
            }
        }
    }

    fn next_line(&mut self, deadline: Instant) -> Result<String, DeckError> {
        loop {
            if let Some(line) = take_control_line(&mut self.bytes)? {
                return Ok(line);
            }
            if Instant::now() >= deadline {
                return Err(DeckError::new(ErrorKind::Tmux, "tmux control timeout"));
            }
            let mut chunk = [0u8; 8192];
            match self.stdout.read(&mut chunk) {
                Ok(0) => {
                    return Err(DeckError::new(ErrorKind::Tmux, "tmux control closed"));
                }
                Ok(count) => {
                    self.bytes.extend_from_slice(&chunk[..count]);
                    if self.bytes.len() > CONTROL_OUTPUT_LIMIT {
                        return Err(DeckError::new(ErrorKind::Tmux, "tmux control output limit"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.drain_stderr()?;
                    if self
                        .child
                        .try_wait()
                        .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux control wait failed"))?
                        .is_some()
                    {
                        return Err(DeckError::new(ErrorKind::Tmux, "tmux control exited"));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    return Err(DeckError::new(
                        ErrorKind::Tmux,
                        "tmux control output failed",
                    ))
                }
            }
        }
    }

    fn read_frame(&mut self, deadline: Instant) -> Result<String, DeckError> {
        loop {
            let line = self.next_line(deadline)?;
            match self.parser.feed(&line)? {
                ControlFeed::Pending => {}
                ControlFeed::Complete(output) => return Ok(output),
                ControlFeed::Failed => {
                    return Err(DeckError::new(ErrorKind::Tmux, "tmux control query failed"))
                }
                ControlFeed::Exited => {
                    return Err(DeckError::new(ErrorKind::Tmux, "tmux control exited"))
                }
            }
        }
    }

    fn list_panes(&mut self) -> Result<Vec<PaneRow>, DeckError> {
        let query = PaneQuery::new()?;
        let command = format!("list-panes -a -F '{}'\n", query.format());
        self.child
            .stdin
            .as_mut()
            .ok_or_else(|| DeckError::new(ErrorKind::Tmux, "tmux control stdin unavailable"))?
            .write_all(command.as_bytes())
            .map_err(|_| DeckError::new(ErrorKind::Tmux, "tmux control stdin failed"))?;
        let raw = self.read_frame(Instant::now() + CONTROL_QUERY_BUDGET)?;
        let rows = query.rows(&raw)?;
        if rows.is_empty() || rows.iter().any(|row| row.server_pid != self.server_pid) {
            return Err(DeckError::new(
                ErrorKind::Tmux,
                "tmux control generation changed",
            ));
        }
        Ok(rows)
    }

    fn stop(&mut self) {
        let _ = self.child.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for TmuxQueryChannel {
    fn drop(&mut self) {
        self.stop();
        {
            let mut slot = OWNED_CONTROL_CLIENT.lock_or_recover();
            if slot
                .as_ref()
                .is_some_and(|owned| owned.client_pid == self.child.id())
            {
                *slot = None;
            }
        }
    }
}

#[derive(Default)]
struct QueryState {
    channel: Option<TmuxQueryChannel>,
    retry_after: Option<Instant>,
}

static QUERY_STATE: Mutex<QueryState> = Mutex::new(QueryState {
    channel: None,
    retry_after: None,
});

/// The Board-only high-frequency read path. Discovery/failure uses the
/// existing one-shot implementation as an oracle, then stable polling stays
/// on one control client. No user value is ever parsed as a control
/// command.
pub(crate) fn query_list_panes() -> Result<Vec<PaneRow>, DeckError> {
    let mut state = QUERY_STATE.lock_or_recover();
    if let Some(channel) = state.channel.as_mut() {
        match channel.list_panes() {
            Ok(rows) => return Ok(rows),
            Err(error) => {
                applog(&format!("[tmux-control] query reset ({})", error.code()));
                state.channel.take();
                state.retry_after = Some(Instant::now() + CONTROL_RETRY_DELAY);
                // Exactly one one-shot oracle read accompanies a failed
                // generation. Cooldown polls fail closed instead of exec-looping.
                return list_panes();
            }
        }
    }
    if state
        .retry_after
        .is_some_and(|retry| Instant::now() < retry)
    {
        return Err(DeckError::new(ErrorKind::Tmux, "tmux control recovering"));
    }
    state.retry_after = None;
    let rows = list_panes()?;
    if rows.is_empty() {
        return Ok(rows);
    }
    match TmuxQueryChannel::connect(&rows) {
        Ok(channel) => {
            applog("[tmux-control] read channel connected");
            state.channel = Some(channel);
        }
        Err(error) => {
            applog(&format!(
                "[tmux-control] connect deferred ({})",
                error.code()
            ));
            state.retry_after = Some(Instant::now() + CONTROL_RETRY_DELAY);
        }
    }
    Ok(rows)
}

/// Smoke evidence only (`smoke_faults::smoke_query_channel`): whether the
/// persistent query client is attached right now.
pub(crate) fn query_channel_connected() -> bool {
    QUERY_STATE.lock_or_recover().channel.is_some()
}

/// A query client whose Deck died without `stop` (crash, SIGKILL) outlives
/// it: once the server holds notification output it can no longer deliver
/// (the parent's pipe is gone), the client waits forever for an exit that
/// never comes, still attached to this server and no longer shown by
/// `list-clients`. Before connecting a new one, SIGTERM every orphan (ppid 1,
/// this uid) running exactly this bundle's tmux with this socket's query
/// argv; `kill(2)` spawns nothing and only such a process can match.
fn reap_orphaned_query_clients(program: &str, conf: &str, socket_name: &str) {
    let query = crate::tmux_clients::query_client_args("=");
    let mut prefix = vec![program, "-f", conf, "-L", socket_name];
    prefix.extend_from_slice(&query[..query.len() - 1]);
    let orphans = crate::procinfo::orphans_running(program, &prefix, "=");
    for pid in &orphans {
        // SAFETY: kill(2) on a pid just matched above; no memory involved.
        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGTERM) };
    }
    if !orphans.is_empty() {
        applog(&format!(
            "[tmux-control] reaped {} orphaned query client(s)",
            orphans.len()
        ));
    }
}

/// SIGTERM, SIGINT and SIGHUP take the normal quit path (`RunEvent::Exit` →
/// `stop_query_channel`) instead of the default instant death, which could
/// orphan the query client (see `reap_orphaned_query_clients`). The handler
/// only writes one byte to a self-pipe; a thread blocked in read(2) — no
/// timer — asks the app to exit and, should the event loop not get there,
/// stops the channel itself and exits after a bounded wait.
pub(crate) fn exit_on_termination_signals(app: tauri::AppHandle) {
    use std::sync::atomic::{AtomicI32, Ordering};
    static WAKE: AtomicI32 = AtomicI32::new(-1);
    extern "C" fn on_signal(_: libc::c_int) {
        let fd = WAKE.load(Ordering::Relaxed);
        if fd >= 0 {
            // SAFETY: write(2) is async-signal-safe; a lost byte only means
            // an earlier signal is already pending.
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe(2) fills the two-element array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    for fd in fds {
        // SAFETY: fcntl on a descriptor this function just created.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    WAKE.store(fds[1], Ordering::Relaxed);
    let watcher = std::thread::Builder::new()
        .name("deck-exit-signal".into())
        .spawn(move || {
            let mut byte = 0u8;
            // SAFETY: one-byte read into a local; retried only on EINTR.
            while unsafe { libc::read(fds[0], (&mut byte as *mut u8).cast(), 1) } < 0
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
            {
            }
            applog("[exit] termination signal");
            app.exit(0);
            std::thread::sleep(Duration::from_secs(3));
            stop_query_channel();
            std::process::exit(0);
        });
    if watcher.is_err() {
        return;
    }
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only calls write(2) on the pipe above.
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
}

/// Restart and process exit call this after excluding active poll operations.
pub(crate) fn stop_query_channel() {
    let mut state = QUERY_STATE.lock_or_recover();
    state.channel.take();
    state.retry_after = None;
}

/// One pane, by tmux target (`pane_target(session)` for a card's pane).
pub(crate) fn pane_row(target: &str) -> Result<PaneRow, DeckError> {
    pane_row_with(&tmux, target)
}

/// One pane through `run` (see `list_panes_with`).
pub(crate) fn pane_row_with(
    run: &dyn Fn(&[&str]) -> Result<String, DeckError>,
    target: &str,
) -> Result<PaneRow, DeckError> {
    let query = PaneQuery::new()?;
    let raw = run(&["display-message", "-p", "-t", target, query.format()])?;
    let mut rows = query.rows(&raw)?;
    match (rows.pop(), rows.is_empty()) {
        (Some(row), true) => Ok(row),
        _ => Err(malformed_row()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn row(fields: &[&str]) -> String {
        fields.join("\t")
    }

    #[test]
    fn pane_rows_parse_every_field_and_keep_tabs_in_the_path() {
        let line = row(&[
            "99",
            "$1",
            "deck-card-ab12",
            "@2",
            "%3",
            "44",
            "1700000005",
            "1",
            "claude",
            "/dev/ttys004",
            "/tmp/a\tb",
        ]);
        let parsed = parse_pane_row(&format!("{line}\n")).unwrap();
        assert_eq!(
            parsed,
            PaneRow {
                server_pid: 99,
                session_id: "$1".into(),
                session_name: "deck-card-ab12".into(),
                window_id: "@2".into(),
                pane_id: "%3".into(),
                pane_pid: 44,
                window_activity: 1700000005,
                in_mode: true,
                command: "claude".into(),
                tty: "/dev/ttys004".into(),
                path: "/tmp/a\tb".into(),
            }
        );
        assert_eq!(
            PANE_FORMAT.split('\t').count(),
            11,
            "one field per struct member"
        );
    }

    #[test]
    fn pane_rows_are_all_or_nothing() {
        let good = [
            "99",
            "$1",
            "s",
            "@2",
            "%3",
            "44",
            "1",
            "0",
            "zsh",
            "/dev/ttys0",
            "/",
        ];
        assert!(parse_pane_row(&row(&good)).is_some());
        for (i, bad) in [
            (0, "0"),     // server pid 0
            (0, "x"),     // non-numeric
            (1, "1"),     // session id without $
            (3, "2"),     // window id without @
            (4, "%"),     // pane id without digits
            (5, "0"),     // pane pid 0
            (6, "later"), // activity non-numeric
            (7, "2"),     // in_mode outside 0/1
        ] {
            let mut fields = good;
            fields[i] = bad;
            assert!(
                parse_pane_row(&row(&fields)).is_none(),
                "field {i} = {bad:?}"
            );
        }
        assert!(parse_pane_row(&row(&good[..10])).is_none(), "missing field");
        assert!(parse_pane_row("junk-line").is_none());
        assert!(parse_pane_row("").is_none());
    }

    #[test]
    fn session_names_accept_the_deck_alphabet() {
        for ok in ["deck-my-card-ab12", "elemental-tcg-02", "a", "A_b@2"] {
            assert!(validate_session_name(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn session_names_reject_flags_formats_and_separators() {
        for bad in [
            "",             // empty
            "-starts-dash", // reads as a flag
            "has space",
            "has:colon", // tmux target separator
            "has.dot",   // tmux target separator
            "has#hash",  // tmux format expansion
            "has;semi",
            "日本語", // outside the generated alphabet
            &"x".repeat(65),
        ] {
            assert!(validate_session_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn server_defaults_keep_selection_frames_invisible_and_status_off() {
        let conf = tmux_conf_text(std::path::Path::new("/tmp/deck"));
        for line in [
            "set -g status off",
            "set -g set-clipboard off",
            "set -g history-limit 50000",
            "set -g mode-style 'none'",
            "set -g copy-mode-selection-style 'none'",
            "set -g copy-mode-position-style 'reverse'",
            "set-environment -g DECK_STATUS_SOCK \"/tmp/deck/status.sock\"",
        ] {
            assert!(
                conf.lines().any(|l| l == line),
                "missing server default: {line}"
            );
        }
    }

    #[test]
    fn status_sock_env_line_is_quoted_or_omitted() {
        assert_eq!(
            status_sock_env_line(std::path::Path::new("/tmp/deck test")),
            "set-environment -g DECK_STATUS_SOCK \"/tmp/deck test/status.sock\"\n"
        );
        // a path tmux's double-quoted string could misread is omitted, not mangled
        assert_eq!(
            status_sock_env_line(std::path::Path::new("/tmp/de\"ck")),
            ""
        );
        assert_eq!(status_sock_env_line(std::path::Path::new("/tmp/de$ck")), "");
    }

    #[test]
    fn fmt_escape_doubles_hashes() {
        assert_eq!(fmt_escape("a#b##c"), "a##b####c");
        assert_eq!(fmt_escape("plain"), "plain");
    }

    #[test]
    fn production_and_debug_socket_names_are_reserved() {
        assert_ne!("deck", "deck-dev");
        assert!("deck-smoke-123".starts_with("deck-smoke"));
    }

    #[test]
    fn targets_socket_and_paths_are_classified_without_guessing() {
        assert_eq!(session_target("deck-card-ab12"), "=deck-card-ab12");
        assert_eq!(pane_target("deck-card-ab12"), "=deck-card-ab12:");
        assert_eq!(expand_tilde("/tmp/project"), "/tmp/project");
        assert!(expand_tilde("~/project").ends_with("/project"));
        assert_eq!(socket(), "deck-dev");
    }

    /// The security property: the only binary deck can ever execute as tmux
    /// is the sidecar next to its own executable. No Homebrew/MacPorts
    /// candidate, no PATH lookup, and no empty program reaching a spawn.
    #[test]
    fn the_only_tmux_candidate_is_this_build_s_own_sidecar() {
        let sidecar = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("tmux")));
        let binary = tmux_bin();
        if binary.is_empty() {
            // a test binary has no sidecar beside it — that is "no tmux",
            // never a fallback to whatever the machine happens to have
            assert_eq!(tmux_kind(), "missing");
            let error = tmux_program().expect_err("no sidecar must not resolve to a program");
            assert_eq!(error.kind(), ErrorKind::TmuxMissing);
            assert!(tmux_batch(&["list-sessions".to_string()]).is_empty());
            assert_eq!(tmux(&["-V"]).unwrap_err().kind(), ErrorKind::TmuxMissing);
        } else {
            assert_eq!(
                Some(std::path::PathBuf::from(binary)),
                sidecar,
                "tmux_bin resolved something other than this build's sidecar"
            );
            assert_eq!(tmux_kind(), "sidecar");
            assert_eq!(tmux_program().unwrap(), binary);
        }
    }

    #[test]
    fn a_rejected_session_name_is_an_invalid_argument() {
        for bad in ["", "-x", "a b", &"x".repeat(65)] {
            assert_eq!(
                validate_session_name(bad).unwrap_err().kind(),
                ErrorKind::Invalid,
                "{bad:?}"
            );
        }
        assert!(validate_session_name("deck-web-1@2").is_ok());
    }

    #[test]
    fn stdin_transport_inherits_deadline_and_preserves_literal_bytes() {
        let input = b"literal\nbytes\twithout-shell-expansion";
        let mut cat = Command::new("/bin/sh");
        let _deadline = crate::session_runtime::Deadline::until(
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert_eq!(
            command_with_stdin(&mut cat, &["-c", "cat"], input).unwrap(),
            String::from_utf8_lossy(input)
        );
        drop(_deadline);

        let begin = std::time::Instant::now();
        let mut stalled = Command::new("/bin/sh");
        let _deadline =
            crate::session_runtime::Deadline::until(begin + std::time::Duration::from_millis(60));
        assert!(command_with_stdin(&mut stalled, &["-c", "sleep 2"], b"").is_err());
        assert!(begin.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn control_parser_correlates_frames_and_ignores_notifications() {
        let mut parser = ControlParser::default();
        assert_eq!(
            parser.feed("%begin 1700000000 41 0").unwrap(),
            ControlFeed::Pending
        );
        assert_eq!(parser.feed("99\t$1\talpha").unwrap(), ControlFeed::Pending);
        assert_eq!(
            parser.feed("%end 1700000000 41 0").unwrap(),
            ControlFeed::Complete("99\t$1\talpha\n".into())
        );
        // Outside a frame, `%` records are notifications and are skipped.
        assert_eq!(
            parser.feed("%session-changed $1 alpha").unwrap(),
            ControlFeed::Pending
        );

        assert_eq!(
            parser.feed("%begin 1700000001 42 0").unwrap(),
            ControlFeed::Pending
        );
        assert_eq!(
            parser.feed("%error 1700000001 42 0").unwrap(),
            ControlFeed::Failed
        );
        assert_eq!(parser.feed("%exit reason").unwrap(), ControlFeed::Exited);
    }

    #[test]
    fn control_parser_rejects_ambiguous_or_oversized_frames() {
        let mut nested = ControlParser::default();
        nested.feed("%begin 1 7 0").unwrap();
        assert!(nested.feed("%begin 1 8 0").is_err());

        let mut mismatched = ControlParser::default();
        mismatched.feed("%begin 1 7 0").unwrap();
        assert!(mismatched.feed("%end 1 8 0").is_err());

        let mut oversized = ControlParser::default();
        oversized.feed("%begin 1 7 0").unwrap();
        assert!(oversized.feed(&"x".repeat(CONTROL_OUTPUT_LIMIT)).is_err());
    }

    const TEST_NONCE: &str = "0123456789abcdef0123456789abcdef";

    fn framed(fields: &[&str]) -> String {
        format!("{TEST_NONCE}\t{}\t{TEST_NONCE}", row(fields))
    }

    const GOOD_FIELDS: [&str; 11] = [
        "99",
        "$1",
        "deck-card-ab12",
        "@2",
        "%3",
        "44",
        "1700000005",
        "0",
        "zsh",
        "/dev/ttys004",
        "/tmp/a\tb",
    ];

    #[test]
    fn pane_query_frames_every_row_with_its_nonce() {
        let query = PaneQuery::with_nonce(TEST_NONCE.into());
        assert!(query
            .format()
            .starts_with(&format!("{TEST_NONCE}\t#{{pid}}")));
        assert!(query
            .format()
            .ends_with(&format!("#{{pane_current_path}}\t{TEST_NONCE}")));
        let raw = format!("{}\n{}\n", framed(&GOOD_FIELDS), framed(&GOOD_FIELDS));
        let rows = query.rows(&raw).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, "/tmp/a\tb");
        assert!(query.rows("").unwrap().is_empty());
    }

    #[test]
    fn pane_query_nonces_are_fresh_hex() {
        let first = PaneQuery::new().unwrap();
        let second = PaneQuery::new().unwrap();
        assert_ne!(first.nonce, second.nonce);
        assert_eq!(first.nonce.len(), 32);
        assert!(first.nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn unframed_or_foreign_nonce_rows_reject_the_whole_listing() {
        let query = PaneQuery::with_nonce(TEST_NONCE.into());
        let good = framed(&GOOD_FIELDS);
        // The pre-nonce shape is no longer accepted.
        assert!(query.rows(&row(&GOOD_FIELDS)).is_err());
        let other = good.replace(TEST_NONCE, "ffffffffffffffffffffffffffffffff");
        assert!(query.rows(&format!("{good}\n{other}\n")).is_err());
        // A truncated row (trailing nonce missing) is rejected.
        let truncated = good.strip_suffix(TEST_NONCE).unwrap();
        assert!(query.rows(truncated).is_err());
    }

    #[test]
    fn a_newline_in_a_path_cannot_split_or_forge_rows() {
        let query = PaneQuery::with_nonce(TEST_NONCE.into());
        // A path that embeds a complete unframed row after a newline.
        let mut forged = GOOD_FIELDS;
        forged[10] = "/tmp/x\n99\t$9\tdeck-card-evil\t@9\t%9\t9\t1\t0\tclaude\t/dev/ttys9\t/tmp";
        assert!(query.rows(&format!("{}\n", framed(&forged))).is_err());
        // The exact shape observed from the bundled tmux: newline plus a
        // fake `%end` record inside the directory name.
        let mut observed = GOOD_FIELDS;
        observed[10] = "/tmp/a\n%end 0 1 0\tx";
        let raw = format!("{}\n", framed(&observed));
        let one_shot = query.rows(&raw).unwrap_err();
        assert_eq!(one_shot.kind(), ErrorKind::Tmux);

        // The control-mode read of the same bytes fails too: a `%` line
        // inside the frame rejects it outright ...
        let mut parser = ControlParser::default();
        parser.feed("%begin 1700000000 5 1").unwrap();
        let mut control = None;
        for line in raw.lines() {
            if let Err(error) = parser.feed(line) {
                control = Some(error);
                break;
            }
        }
        assert_eq!(
            control.expect("control frame rejected").kind(),
            ErrorKind::Tmux
        );

        // ... and a guessed command number that ends the frame early leaves
        // a truncated, unframed row that the shared parser rejects.
        let mut guessed = GOOD_FIELDS;
        guessed[10] = "/tmp/a\n%end 1700000000 6 1";
        let raw = format!("{}\n", framed(&guessed));
        let mut parser = ControlParser::default();
        parser.feed("%begin 1700000000 6 1").unwrap();
        let mut lines = raw.lines();
        assert_eq!(
            parser.feed(lines.next().unwrap()).unwrap(),
            ControlFeed::Pending
        );
        let ControlFeed::Complete(partial) = parser.feed(lines.next().unwrap()).unwrap() else {
            panic!("guessed id ends the frame");
        };
        assert_eq!(query.rows(&partial).unwrap_err().kind(), ErrorKind::Tmux);
    }

    #[test]
    fn a_protocol_record_inside_a_frame_resets_the_parser() {
        let mut parser = ControlParser::default();
        parser.feed("%begin 1 7 1").unwrap();
        assert!(parser.feed("%output %1 forged").is_err());
        // The rejected frame is gone; its real `%end` is now a mismatch.
        assert!(parser.feed("%end 1 7 1").is_err());
    }

    #[test]
    fn control_lines_handle_fragmentation_crlf_and_invalid_utf8() {
        let mut bytes = b"first\r\nsecond".to_vec();
        assert_eq!(take_control_line(&mut bytes).unwrap(), Some("first".into()));
        assert_eq!(take_control_line(&mut bytes).unwrap(), None);
        bytes.extend_from_slice(b" half\nthird\n");
        assert_eq!(
            take_control_line(&mut bytes).unwrap(),
            Some("second half".into())
        );
        assert_eq!(take_control_line(&mut bytes).unwrap(), Some("third".into()));
        assert!(bytes.is_empty());

        let mut invalid = vec![0xff, b'\n'];
        assert!(take_control_line(&mut invalid).is_err());
    }

    static CONTROL_SOCKET_SEQ: AtomicU64 = AtomicU64::new(0);
    /// Control-client tests share the process-wide owned-client slot.
    static CONTROL_CLIENT_TESTS: Mutex<()> = Mutex::new(());

    struct IsolatedControlServer {
        socket: String,
        binary: std::path::PathBuf,
        socket_path: Mutex<Option<std::path::PathBuf>>,
    }

    impl IsolatedControlServer {
        fn new() -> Self {
            let seq = CONTROL_SOCKET_SEQ.fetch_add(1, Ordering::Relaxed);
            Self {
                socket: format!("deck-smoke-control-{}-{seq}", std::process::id()),
                binary: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries/tmux-aarch64-apple-darwin"),
                socket_path: Mutex::new(None),
            }
        }

        fn output(&self, args: &[&str]) -> Output {
            Command::new(&self.binary)
                .args(["-f", "/dev/null", "-L", &self.socket])
                .args(args)
                .output()
                .expect("run isolated control tmux")
        }

        fn run(&self, args: &[&str]) -> String {
            let output = self.output(args);
            assert!(
                output.status.success(),
                "tmux {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut saved = self.socket_path.lock().unwrap_or_else(|e| e.into_inner());
            if saved.is_none() {
                let location = self.output(&["display-message", "-p", "#{socket_path}"]);
                if location.status.success() {
                    *saved = Some(std::path::PathBuf::from(
                        String::from_utf8_lossy(&location.stdout).trim(),
                    ));
                }
            }
            String::from_utf8(output.stdout)
                .expect("tmux output utf8")
                .trim_end()
                .to_owned()
        }

        fn socket_path(&self) -> Option<std::path::PathBuf> {
            if let Some(path) = self
                .socket_path
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                return Some(path);
            }
            let output = self.output(&["display-message", "-p", "#{socket_path}"]);
            output
                .status
                .success()
                .then(|| std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        }

        /// Exactly what `tmux()` does after its spawn, on this socket.
        fn tmux(&self, args: &[&str]) -> Result<String, DeckError> {
            captured_output(
                Command::new(&self.binary).args(["-f", "/dev/null", "-L", &self.socket]),
                args,
            )
        }

        fn new_session(&self, name: &str, program: &str) {
            self.run(&["new-session", "-d", "-s", name, program, "30"]);
        }
    }

    impl Drop for IsolatedControlServer {
        fn drop(&mut self) {
            let socket = self.socket_path();
            let _ = self.output(&["kill-server"]);
            if let Some(path) = socket.filter(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(&self.socket)
                    && std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_type().is_socket())
            }) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    #[test]
    fn persistent_control_matches_one_shot_and_is_writable_no_output() {
        let _serial = CONTROL_CLIENT_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let server = IsolatedControlServer::new();
        assert!(server.binary.is_file(), "bundled tmux test binary missing");
        server.run(&[
            "new-session",
            "-d",
            "-s",
            "alpha",
            "-x",
            "80",
            "-y",
            "24",
            "/bin/sleep",
            "30",
        ]);
        let raw = server.run(&["list-panes", "-a", "-F", PANE_FORMAT]);
        let expected: Vec<_> = raw
            .lines()
            .map(|line| parse_pane_row(line).unwrap())
            .collect();

        let mut channel = TmuxQueryChannel::connect_with(
            server.binary.to_str().unwrap(),
            "/dev/null",
            &server.socket,
            &expected,
        )
        .expect("connect persistent control client");
        assert_eq!(
            owned_control_client(),
            Some((channel.child.id(), expected[0].server_pid, "alpha".into()))
        );
        let actual = channel.list_panes().expect("persistent list-panes");
        assert_eq!(actual, expected);

        let clients = server.run(&[
            "list-clients",
            "-F",
            "#{client_pid}\t#{client_control_mode}\t#{client_flags}\t#{session_name}",
        ]);
        let owned = clients
            .lines()
            .find(|line| line.starts_with(&format!("{}\t", channel.child.id())))
            .expect("owned client is listed");
        assert!(owned.contains("\t1\t"), "not a control client: {owned}");
        assert!(
            !owned.contains("read-only"),
            "a read-only query client refuses send-keys: {owned}"
        );
        assert!(
            owned.contains("no-output"),
            "output not suppressed: {owned}"
        );
        assert!(owned.contains("ignore-size"), "size not ignored: {owned}");
        assert_eq!(
            server.run(&[
                "display-message",
                "-p",
                "-t",
                "alpha:",
                "#{window_width}x#{window_height}"
            ]),
            "80x24"
        );

        server.run(&["kill-session", "-t", "=alpha"]);
        assert!(channel.list_panes().is_err());
        drop(channel);
        assert_eq!(owned_control_client(), None);
    }

    #[test]
    fn a_newline_path_fails_one_shot_and_control_reads_identically() {
        let _serial = CONTROL_CLIENT_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let server = IsolatedControlServer::new();
        assert!(server.binary.is_file(), "bundled tmux test binary missing");
        let root = std::env::temp_dir().join(format!(
            "deck-smoke-r4-{}-{}",
            std::process::id(),
            CONTROL_SOCKET_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        struct RemoveOnDrop(std::path::PathBuf);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = RemoveOnDrop(root.clone());
        let hostile = root.join("a\n%end 0 1 0\tx");
        std::fs::create_dir(&hostile).unwrap();
        server.run(&["new-session", "-d", "-s", "alpha", "/bin/sleep", "30"]);
        let query = PaneQuery::new().unwrap();
        let clean = query
            .rows(&server.run(&["list-panes", "-a", "-F", query.format()]))
            .unwrap();
        let mut channel = TmuxQueryChannel::connect_with(
            server.binary.to_str().unwrap(),
            "/dev/null",
            &server.socket,
            &clean,
        )
        .expect("connect persistent control client");
        assert_eq!(channel.list_panes().unwrap(), clean);

        server.run(&[
            "new-session",
            "-d",
            "-s",
            "hostile",
            "-c",
            hostile.to_str().unwrap(),
            "/bin/sleep",
            "30",
        ]);
        let query = PaneQuery::new().unwrap();
        let raw = server.run(&["list-panes", "-a", "-F", query.format()]);
        assert!(
            raw.contains("\n%end 0 1 0"),
            "tmux printed the path verbatim"
        );
        assert_eq!(query.rows(&raw).unwrap_err().kind(), ErrorKind::Tmux);
        assert_eq!(channel.list_panes().unwrap_err().kind(), ErrorKind::Tmux);
        drop(channel);
    }

    /// The capture behind `tmux` / `tmux_owned`: stdout on success, tmux's
    /// stderr classified on a failed status, the caller's deadline turned
    /// into the restart timeout, and a program that cannot start reported
    /// as missing tmux — for borrowed and owned argument lists alike.
    #[test]
    fn captured_output_classifies_success_failure_timeout_and_a_missing_program() {
        let server = IsolatedControlServer::new();
        server.new_session("alpha", "/bin/sleep");
        let pid = server
            .tmux(&["display-message", "-p", "#{pid}"])
            .expect("server pid");
        assert!(pid.trim().parse::<u32>().is_ok(), "{pid:?}");
        let owned: Vec<String> = ["display-message", "-p", "#{session_name}"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            captured_output(
                Command::new(&server.binary).args(["-f", "/dev/null", "-L", &server.socket]),
                &owned,
            )
            .unwrap(),
            "alpha\n"
        );

        let missing = server
            .tmux(&["has-session", "-t", "=nope"])
            .expect_err("unknown session fails");
        assert_eq!(missing.kind(), ErrorKind::NoSession);
        assert!(
            missing.message().starts_with("tmux has-session failed: "),
            "{}",
            missing.message()
        );

        let begin = Instant::now();
        let _deadline = crate::session_runtime::Deadline::until(begin + Duration::from_millis(60));
        let stalled = captured_output(&mut Command::new("/bin/sh"), &["-c", "sleep 2"])
            .expect_err("the deadline bounds the capture");
        assert_eq!(stalled.message(), "tmux-restart-timeout");
        assert_eq!(stalled.kind(), ErrorKind::Tmux);
        assert!(begin.elapsed() < Duration::from_secs(1));
        drop(_deadline);

        let unrunnable = captured_output(&mut Command::new("/nonexistent/deck-test/tmux"), &["-V"])
            .expect_err("a program that cannot start");
        assert_eq!(unrunnable.kind(), ErrorKind::TmuxMissing);
        assert!(unrunnable.message().starts_with("tmux not runnable: "));
    }

    /// `list_panes_with` / `pane_row_with` are the production reads on an
    /// injected runner: real rows from the bundled tmux, a missing target
    /// reported by tmux, and a listing that is not exactly one framed row
    /// rejected as malformed.
    #[test]
    fn list_panes_and_pane_row_read_real_rows_through_the_runner() {
        let server = IsolatedControlServer::new();
        server.new_session("alpha", "/bin/sleep");
        server.new_session("beta", "/bin/sleep");
        let run = |args: &[&str]| server.tmux(args);
        let rows = list_panes_with(&run).expect("list panes");
        let mut names: Vec<_> = rows.iter().map(|row| row.session_name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["alpha", "beta"]);
        assert!(rows.iter().all(|row| row.command == "sleep"), "{rows:?}");
        assert!(rows.iter().all(|row| row.server_pid == rows[0].server_pid));

        let alpha = pane_row_with(&run, "=alpha:").expect("one pane");
        assert_eq!(
            rows.iter().find(|row| row.session_name == "alpha"),
            Some(&alpha)
        );
        // tmux answers a missing `display-message -p` target with empty
        // fields (exit 0); the read rejects them rather than half-trusting.
        let missing = pane_row_with(&run, "=missing:").unwrap_err();
        assert_eq!(missing.kind(), ErrorKind::Tmux);
        assert_eq!(missing.message(), "tmux returned a malformed pane row");

        // A runner that answers with every pane is not one pane.
        let listing = |args: &[&str]| {
            let mut args = args.to_vec();
            if args[0] == "display-message" {
                let format = args[4];
                args = vec!["list-panes", "-a", "-F", format];
            }
            server.tmux(&args)
        };
        let two = pane_row_with(&listing, "=alpha:").unwrap_err();
        assert_eq!(two.kind(), ErrorKind::Tmux);
        assert_eq!(two.message(), "tmux returned a malformed pane row");
        // An unframed listing fails the whole read.
        let unframed = |_: &[&str]| Ok(String::from("junk\n"));
        assert_eq!(
            list_panes_with(&unframed).unwrap_err().kind(),
            ErrorKind::Tmux
        );
        // The runner's own error propagates untouched.
        let broken = |_: &[&str]| Err(DeckError::new(ErrorKind::Tmux, "tmux control recovering"));
        assert_eq!(
            list_panes_with(&broken).unwrap_err().message(),
            "tmux control recovering"
        );
    }

    /// Reusing a compatible server applies the same defaults `tmux.conf`
    /// gives a freshly spawned one (a `-f /dev/null` server starts with
    /// tmux's stock values).
    #[test]
    fn init_deck_server_applies_the_reuse_defaults_to_a_running_server() {
        let server = IsolatedControlServer::new();
        server.new_session("alpha", "/bin/sleep");
        assert_eq!(server.run(&["show-options", "-gv", "status"]), "on");
        let run = |args: &[&str]| server.tmux(args);
        init_deck_server_with(&run, std::path::Path::new("/tmp/deck-test-init"));
        for (option, value) in [
            ("status", "off"),
            ("mouse", "off"),
            ("history-limit", "50000"),
            ("mode-style", "none"),
            ("copy-mode-selection-style", "none"),
            ("copy-mode-position-style", "reverse"),
            ("copy-mode-position-format", ""),
        ] {
            assert_eq!(
                server.run(&["show-options", "-gv", option]),
                value,
                "{option}"
            );
        }
        let env = server.run(&["show-environment", "-g"]);
        assert!(
            env.lines().any(|line| line == "COLORTERM=truecolor"),
            "{env}"
        );
        assert!(
            env.lines()
                .any(|line| line == "DECK_STATUS_SOCK=/tmp/deck-test-init/status.sock"),
            "{env}"
        );
    }

    #[test]
    fn nonblocking_sets_the_flag_and_rejects_a_closed_descriptor() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: pipe(2) fills the two-element array.
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        // SAFETY: descriptors this test just created.
        let before = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
        assert_eq!(before & libc::O_NONBLOCK, 0);
        nonblocking(fds[0]).expect("pipe end becomes nonblocking");
        // SAFETY: as above.
        let after = unsafe { libc::fcntl(fds[0], libc::F_GETFL) };
        assert_ne!(after & libc::O_NONBLOCK, 0);
        for fd in fds {
            // SAFETY: closing descriptors this test owns.
            unsafe { libc::close(fd) };
        }
        let closed = nonblocking(fds[0]).expect_err("a closed descriptor");
        assert_eq!(closed.message(), "tmux control pipe unavailable");
        assert_eq!(nonblocking(-1).unwrap_err().kind(), ErrorKind::Tmux);
    }

    /// The Board read path serves rows from an installed channel, refuses
    /// to exec during the recovery cooldown, and `stop_query_channel`
    /// releases the client and its owned identity.
    #[test]
    fn query_state_serves_an_installed_channel_and_stops_it() {
        let _serial = CONTROL_CLIENT_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let server = IsolatedControlServer::new();
        server.new_session("alpha", "/bin/sleep");
        let run = |args: &[&str]| server.tmux(args);
        let expected = list_panes_with(&run).unwrap();
        let mut channel = TmuxQueryChannel::connect_with(
            server.binary.to_str().unwrap(),
            "/dev/null",
            &server.socket,
            &expected,
        )
        .expect("connect persistent control client");
        assert_eq!(channel.list_panes().unwrap(), expected);
        let client_pid = channel.child.id();

        stop_query_channel();
        assert!(!query_channel_connected());
        QUERY_STATE.lock_or_recover().channel = Some(channel);
        assert!(query_channel_connected());
        assert_eq!(query_list_panes().expect("rows from the channel"), expected);
        let clients = server.run(&["list-clients", "-F", "#{client_pid}"]);
        assert!(clients.lines().any(|line| line == client_pid.to_string()));

        stop_query_channel();
        assert!(!query_channel_connected());
        assert_eq!(owned_control_client(), None);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline
            && server
                .run(&["list-clients", "-F", "#{client_pid}"])
                .lines()
                .any(|line| line == client_pid.to_string())
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !server
                .run(&["list-clients", "-F", "#{client_pid}"])
                .lines()
                .any(|line| line == client_pid.to_string()),
            "the stopped client detached"
        );

        QUERY_STATE.lock_or_recover().retry_after = Some(Instant::now() + CONTROL_RETRY_DELAY);
        let cooling = query_list_panes().expect_err("cooldown fails closed");
        assert_eq!(cooling.message(), "tmux control recovering");
        assert_eq!(cooling.kind(), ErrorKind::Tmux);
        stop_query_channel();
        assert!(QUERY_STATE.lock_or_recover().retry_after.is_none());
    }

    /// Reaping targets only parentless query clients of this socket: a live
    /// channel whose Deck is still running keeps serving afterwards.
    #[test]
    fn reaping_orphans_leaves_a_live_query_client_attached() {
        let _serial = CONTROL_CLIENT_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let server = IsolatedControlServer::new();
        server.new_session("alpha", "/bin/sleep");
        let run = |args: &[&str]| server.tmux(args);
        let rows = list_panes_with(&run).unwrap();
        let program = server.binary.to_str().unwrap();
        let mut channel =
            TmuxQueryChannel::connect_with(program, "/dev/null", &server.socket, &rows)
                .expect("connect persistent control client");
        reap_orphaned_query_clients(program, "/dev/null", &server.socket);
        assert_eq!(channel.list_panes().unwrap(), rows);
        let clients = server.run(&["list-clients", "-F", "#{client_pid}"]);
        assert!(
            clients
                .lines()
                .any(|line| line == channel.child.id().to_string()),
            "{clients}"
        );
        drop(channel);
    }
}
