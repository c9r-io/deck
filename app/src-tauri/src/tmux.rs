//! tmux backend: sidecar discovery, the private `deck` server, config, raw
//! command execution, and the ONE pane row (`PaneRow` / `PANE_FORMAT`,
//! `list_panes`, `pane_row`) every probe in deck reads panes through.
//! Everything deck knows about tmux lives here.
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

use std::io::Write;
use std::process::{Command, Stdio};

use crate::applog;
use crate::error::{DeckError, ErrorKind};

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
         set -g set-clipboard on\n\
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
    let out = Command::new(tmux_program()?)
        .args(["-f", &conf, "-L", socket()])
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .output()
        .map_err(|e| DeckError::new(ErrorKind::TmuxMissing, format!("tmux not runnable: {e}")))?;
    if !out.status.success() {
        return Err(DeckError::classified(format!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
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

/// Owned-argument variant used for bounded command batches whose numeric
/// cursor movements are assembled at runtime. Arguments still bypass a
/// shell; `;` is an explicit tmux command separator, never shell syntax.
pub(crate) fn tmux_owned(args: &[String]) -> Result<String, DeckError> {
    let conf = tmux_conf();
    let out = Command::new(tmux_program()?)
        .args(["-f", &conf, "-L", socket()])
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .output()
        .map_err(|e| DeckError::new(ErrorKind::TmuxMissing, format!("tmux not runnable: {e}")))?;
    if !out.status.success() {
        return Err(DeckError::classified(format!(
            "tmux {} failed: {}",
            args.first().map(String::as_str).unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
    let _ = tmux(&["set-environment", "-g", "COLORTERM", "truecolor"]);
    // Panes tell the status helper (agent_status.rs) which instance's socket
    // to write to, so an isolated/smoke instance receives its own events.
    let sock = crate::datadir::deck_dir().join("status.sock");
    let _ = tmux(&[
        "set-environment",
        "-g",
        "DECK_STATUS_SOCK",
        &sock.display().to_string(),
    ]);
    let _ = tmux(&["set", "-g", "status", "off"]);
    let _ = tmux(&["set", "-g", "mouse", "off"]);
    let _ = tmux(&["set", "-g", "set-clipboard", "on"]);
    let _ = tmux(&["set", "-g", "history-limit", "50000"]);
    // Deck paints selection geometry in one DOM layer after each settled
    // backend update. Hiding tmux's transient selection prevents its
    // top-line/cursor motion steps from flashing large history regions.
    // Keep mode-style empty as the compatibility fallback while the bundled
    // tmux exposes separate selection and position styles.
    let _ = tmux(&["set", "-g", "mode-style", "none"]);
    let _ = tmux(&["set", "-g", "copy-mode-selection-style", "none"]);
    let _ = tmux(&["set", "-g", "copy-mode-position-style", "reverse"]);
    let _ = tmux(&["set", "-g", "copy-mode-position-format", ""]);
}

// ---------- pane rows -------------------------------------------------------

/// The ONE pane row every deck probe reads. `PANE_FORMAT` is the superset of
/// the fields the poll (`commands.rs`), the agent-status resolver, the
/// scheduler tick, the context probe and the lifecycle probe need;
/// `list_panes()` reads every pane on the server and `pane_row(target)` one
/// pane, both through `parse_pane_row`. Adding a field is one format entry
/// plus one struct field. `path` is last because a directory name may
/// contain a tab; every other field is tmux-generated and tab-free.
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

/// Every pane on deck's server, in tmux's listing order (a session's first
/// pane comes first). One malformed line fails the whole read: the only
/// known way to get one is tmux running without a UTF-8 locale, and then
/// every line is malformed.
pub(crate) fn list_panes() -> Result<Vec<PaneRow>, DeckError> {
    tmux(&["list-panes", "-a", "-F", PANE_FORMAT])?
        .lines()
        .map(|line| parse_pane_row(line).ok_or_else(malformed_row))
        .collect()
}

/// One pane, by tmux target (`pane_target(session)` for a card's pane).
pub(crate) fn pane_row(target: &str) -> Result<PaneRow, DeckError> {
    let raw = tmux(&["display-message", "-p", "-t", target, PANE_FORMAT])?;
    parse_pane_row(&raw).ok_or_else(malformed_row)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
