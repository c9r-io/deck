//! Contract tests for the tmux behaviors deck depends on — each one encodes
//! a bug that shipped (v0.4.9–0.4.12) so it can never silently regress.
//!
//! They run the committed static tmux sidecar against a THROWAWAY socket
//! (`deck-test-*`), never the live `deck` socket.

use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

fn tmux_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin")
}

struct Server(String);

impl Server {
    fn new(tag: &str) -> Self {
        let s = Server(format!("deck-test-{tag}-{}", std::process::id()));
        s.run(&[
            "new-session",
            "-d",
            "-s",
            "t",
            "-x",
            "80",
            "-y",
            "12",
            "/bin/sh",
        ]);
        sleep(Duration::from_millis(400)); // let the shell print its prompt
        s
    }
    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .expect("tmux spawn");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn fmt(&self, f: &str) -> String {
        self.run(&["display-message", "-p", "-t", "t", f])
    }
    fn shell(&self, cmd: &str) {
        self.run(&["send-keys", "-t", "t", "-l", cmd]);
        self.run(&["send-keys", "-t", "t", "Enter"]);
        sleep(Duration::from_millis(600));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new(tmux_bin())
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

/// v0.4.12: resize reflow pushes blank lines into history, so "empty" shells
/// scrolled into void. deck clears history once after the first attach; the
/// empty-history guard in scroll_session then makes scrolling a true no-op.
#[test]
fn resize_junk_history_is_clearable_and_stays_zero() {
    let s = Server::new("hist");
    s.run(&["resize-window", "-t", "t", "-x", "120", "-y", "30"]);
    s.run(&["resize-window", "-t", "t", "-x", "60", "-y", "8"]);
    sleep(Duration::from_millis(200));
    s.run(&["clear-history", "-t", "t"]);
    assert_eq!(
        s.fmt("#{history_size}"),
        "0",
        "clear-history must zero the scrollback"
    );
    assert_eq!(
        s.fmt("#{pane_in_mode}"),
        "0",
        "clearing must not enter copy-mode"
    );
}

/// v0.4.11 scrolling model: scroll-up enters copy-mode positioned in history;
/// scroll-down past the bottom AUTO-EXITS (copy-mode -e). If -e ever stops
/// working, the terminal gets stuck in copy-mode and looks frozen.
#[test]
fn copy_mode_enters_on_scroll_up_and_auto_exits_at_bottom() {
    let s = Server::new("scroll");
    s.shell("i=0; while [ $i -lt 40 ]; do echo line$i; i=$((i+1)); done");
    let hist: i64 = s
        .fmt("#{history_size}")
        .parse()
        .expect("history_size numeric");
    assert!(
        hist > 0,
        "40 echoed lines on a 12-row pane must create history"
    );

    s.run(&["copy-mode", "-e", "-t", "t"]);
    s.run(&["send-keys", "-t", "t", "-X", "-N", "5", "scroll-up"]);
    assert_eq!(
        s.fmt("#{pane_in_mode}"),
        "1",
        "scroll-up must land in copy-mode"
    );

    s.run(&["send-keys", "-t", "t", "-X", "-N", "500", "scroll-down"]);
    assert_eq!(
        s.fmt("#{pane_in_mode}"),
        "0",
        "copy-mode -e must auto-exit at bottom"
    );
}

/// Scheduled prompts and pty_write inject via `send-keys -l`: the text must
/// arrive byte-for-byte — no tmux format expansion (#{...}), no key-name
/// parsing ("C-c"), no shell splitting on semicolons.
#[test]
fn send_keys_literal_is_byte_for_byte() {
    let s = Server::new("lit");
    s.shell("echo 'a;b #{x} C-c Enter'");
    let screen = s.run(&["capture-pane", "-p", "-t", "t"]);
    assert!(
        screen.contains("a;b #{x} C-c Enter"),
        "literal send-keys was mangled; screen:\n{screen}"
    );
}

/// The scheduler injects a prompt as ONE atomic tmux command: literal text
/// with a trailing CR in a single `send-keys -l`. This is what removes the
/// "text landed but Enter didn't" partial-send window — the command either
/// executes wholly (the shell runs the line) or is refused. The CR byte must
/// behave exactly like pressing Enter.
#[test]
fn single_send_keys_with_trailing_cr_executes_the_line() {
    let s = Server::new("atomic");
    s.run(&["send-keys", "-t", "t", "-l", "echo atomic-$((20+22))\r"]);
    sleep(Duration::from_millis(600));
    let screen = s.run(&["capture-pane", "-p", "-t", "t"]);
    assert!(
        screen.contains("atomic-42"),
        "trailing CR in a single -l send must execute the line; screen:\n{screen}"
    );
    // and a refused variant (dead target) really is refused, not half-run
    let out = Command::new(tmux_bin())
        .args(["-f", "/dev/null", "-L", &s.0])
        .args(["send-keys", "-t", "=no-such-session:", "-l", "echo nope\r"])
        .output()
        .expect("tmux spawn");
    assert!(
        !out.status.success(),
        "a bad target must fail the whole atomic injection"
    );
}

/// The board's tail preview + fg-process gate read these formats every poll;
/// a tmux upgrade that renames them would blank the whole board.
#[test]
fn poll_formats_exist() {
    let s = Server::new("fmt");
    assert!(!s.fmt("#{pane_pid}").is_empty(), "pane_pid");
    assert!(!s.fmt("#{window_activity}").is_empty(), "window_activity");
    assert_eq!(
        s.fmt("#{pane_in_mode}"),
        "0",
        "pane_in_mode (scrollback chip)"
    );
    let fg = s.fmt("#{pane_current_command}");
    // macOS /bin/sh is bash in sh-mode; the SHELL_FG gate matches both
    assert!(
        ["sh", "bash", "zsh", "dash"].contains(&fg.as_str()),
        "pane_current_command should report the fg shell, got {fg:?}"
    );
}

/// poll_sessions batches every visible card's preview into ONE tmux
/// invocation: `display-message -p <mark> ; capture-pane -p …` pairs.
/// This encodes the two tmux behaviors that design depends on:
/// 1. command batches run in order, output concatenated on stdout;
/// 2. a failing command mid-batch aborts the REST of the batch — which is
///    why poll_sessions only ever batches targets it just saw alive.
#[test]
fn batched_capture_markers_and_dead_target_abort() {
    let s = Server::new("batch");
    s.run(&[
        "new-session",
        "-d",
        "-s",
        "u",
        "-x",
        "80",
        "-y",
        "12",
        "/bin/sh",
    ]);
    sleep(Duration::from_millis(400));
    // exact `=name:` targets, always: with >1 session a BARE name target can
    // resolve to a different session entirely (observed with this very
    // sidecar: bare `-t t` delivered keys to session u) — the reason deck's
    // pane_target()/session_target() prefix every target with `=`.
    s.run(&["send-keys", "-t", "=t:", "-l", "echo tee-one"]);
    s.run(&["send-keys", "-t", "=t:", "Enter"]);
    s.run(&["send-keys", "-t", "=u:", "-l", "echo tee-two"]);
    s.run(&["send-keys", "-t", "=u:", "Enter"]);
    sleep(Duration::from_millis(600));

    const MARK: &str = "\u{1}deck-tail\u{1}";
    let both = s.run(&[
        "display-message",
        "-p",
        &format!("{MARK}t"),
        ";",
        "capture-pane",
        "-p",
        "-t",
        "=t:",
        "-S",
        "-30",
        ";",
        "display-message",
        "-p",
        &format!("{MARK}u"),
        ";",
        "capture-pane",
        "-p",
        "-t",
        "=u:",
        "-S",
        "-30",
    ]);
    let segs: Vec<&str> = both.split(MARK).filter(|s| !s.is_empty()).collect();
    let t_seg = segs
        .iter()
        .find(|s| s.starts_with("t\n"))
        .expect("t segment");
    let u_seg = segs
        .iter()
        .find(|s| s.starts_with("u\n"))
        .expect("u segment");
    assert!(t_seg.contains("tee-one"), "t capture in batch: {both:?}");
    assert!(u_seg.contains("tee-two"), "u capture in batch: {both:?}");
    assert!(!t_seg.contains("tee-two"), "segments must not bleed");

    // dead target mid-batch: everything after the failing command is lost
    let after_dead = s.run(&[
        "display-message",
        "-p",
        &format!("{MARK}gone"),
        ";",
        "capture-pane",
        "-p",
        "-t",
        "=gone:",
        "-S",
        "-30",
        ";",
        "display-message",
        "-p",
        &format!("{MARK}t"),
        ";",
        "capture-pane",
        "-p",
        "-t",
        "=t:",
        "-S",
        "-30",
    ]);
    assert!(
        !after_dead.contains("tee-one"),
        "tmux aborts a batch at the first failure — if this ever changes, \
         poll_sessions' alive-only filtering is merely redundant, not wrong: {after_dead:?}"
    );
}

/// The v0.4.16 all-gray-board bug: GUI-launched apps have no locale env, and
/// under the C locale tmux sanitizes control chars in command output — the
/// \t separators poll_sessions parses become '_'. deck pins LANG=en_US.UTF-8
/// on every tmux invocation (tmux.rs, pty.rs); this pins WHY.
#[test]
fn format_tabs_survive_only_with_utf8_locale() {
    let s = Server::new("locale");
    let run_env = |lang: Option<&str>| {
        let mut c = Command::new(tmux_bin());
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .args(["-f", "/dev/null", "-L", &s.0])
            .args(["display-message", "-p", "-t", "t", "a\tb"]);
        if let Some(l) = lang {
            c.env("LANG", l);
        }
        String::from_utf8_lossy(&c.output().expect("tmux spawn").stdout).into_owned()
    };
    assert!(
        run_env(Some("en_US.UTF-8")).contains("a\tb"),
        "UTF-8 locale must pass tabs through"
    );
    assert!(
        run_env(None).contains("a_b"),
        "C locale sanitizes tabs to '_' — if this ever stops failing without \
         LANG, the pinned env in tmux()/pty.rs is merely redundant"
    );
}
