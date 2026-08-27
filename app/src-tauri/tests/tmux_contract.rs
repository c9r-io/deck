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

/// The board's tail preview + fg-process gate read these formats every poll;
/// a tmux upgrade that renames them would blank the whole board.
#[test]
fn poll_formats_exist() {
    let s = Server::new("fmt");
    assert!(!s.fmt("#{pane_pid}").is_empty(), "pane_pid");
    assert!(!s.fmt("#{window_activity}").is_empty(), "window_activity");
    let fg = s.fmt("#{pane_current_command}");
    // macOS /bin/sh is bash in sh-mode; the SHELL_FG gate matches both
    assert!(
        ["sh", "bash", "zsh", "dash"].contains(&fg.as_str()),
        "pane_current_command should report the fg shell, got {fg:?}"
    );
}
