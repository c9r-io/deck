//! Contract tests for the tmux behaviors deck depends on — each one encodes
//! a bug that shipped (v0.4.9–0.4.12) so it can never silently regress.
//!
//! They run the committed static tmux sidecar against a THROWAWAY socket
//! (`deck-test-*`), never the live `deck` socket.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

#[path = "../src/terminal_scroll.rs"]
mod terminal_scroll;
#[path = "../src/terminal_selection.rs"]
mod terminal_selection;
use terminal_selection::{cursor_steps_for_cell, snapshot_selection};

fn tmux_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/tmux-aarch64-apple-darwin")
}

struct Server(String);

impl Server {
    fn new(tag: &str) -> Self {
        let s = Server(format!("deck-test-{tag}-{}", std::process::id()));
        s.run(&[
            "start-server",
            ";",
            "set-option",
            "-g",
            "history-limit",
            "50000",
            ";",
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
        String::from_utf8(self.run_raw(args))
            .expect("tmux output utf8")
            .trim()
            .to_string()
    }
    fn run_raw(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .expect("tmux spawn");
        out.stdout
    }
    fn run_raw_checked(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .expect("tmux spawn");
        assert!(
            out.status.success(),
            "tmux {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
    fn run_owned(&self, args: &[String]) -> Result<String, String> {
        let out = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .expect("tmux spawn");
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        String::from_utf8(out.stdout).map_err(|_| "tmux output was not UTF-8".to_string())
    }
    fn fmt(&self, f: &str) -> String {
        self.run(&["display-message", "-p", "-t", "t", f])
    }
    fn shell(&self, cmd: &str) {
        self.run(&["send-keys", "-t", "t", "-l", cmd]);
        self.run(&["send-keys", "-t", "t", "Enter"]);
        sleep(Duration::from_millis(600));
    }

    fn write_pane_lines(&self, prefix: &str, start: usize, end: usize) {
        let tty = self.fmt("#{pane_tty}");
        let mut pane = OpenOptions::new()
            .write(true)
            .open(tty)
            .expect("open pane tty");
        for index in start..end {
            write!(pane, "{prefix}-{index:03}\r\n").expect("write pane fixture");
        }
        pane.flush().expect("flush pane fixture");
        let marker = format!("{prefix}-{:03}", end - 1);
        for _ in 0..100 {
            if self
                .run(&["capture-pane", "-p", "-t", "t"])
                .contains(&marker)
            {
                return;
            }
            sleep(Duration::from_millis(10));
        }
        panic!("pane did not render fixture marker {marker}");
    }

    fn fixture(tag: &str, width: u32, height: u32, command: &str) -> Self {
        let s = Server(format!("deck-test-{tag}-{}", std::process::id()));
        s.run(&[
            "start-server",
            ";",
            "set-option",
            "-g",
            "history-limit",
            "50000",
            ";",
            "new-session",
            "-d",
            "-s",
            "t",
            "-x",
            &width.to_string(),
            "-y",
            &height.to_string(),
            command,
        ]);
        sleep(Duration::from_millis(300));
        s
    }

    fn move_copy_cursor(&self, row: u32, col: u32) {
        self.run(&["send-keys", "-t", "t", "-X", "top-line"]);
        self.run(&["send-keys", "-t", "t", "-X", "start-of-line"]);
        if row > 0 {
            self.run(&[
                "send-keys",
                "-t",
                "t",
                "-X",
                "-N",
                &row.to_string(),
                "cursor-down",
            ]);
        }
        let scroll: i64 = self
            .fmt("#{scroll_position}")
            .parse()
            .expect("scroll position numeric");
        let coord = row as i64 - scroll;
        let captured = String::from_utf8(self.run_raw_checked(&[
            "capture-pane",
            "-p",
            "-J",
            "-S",
            &coord.to_string(),
            "-E",
            &coord.to_string(),
            "-t",
            "t",
        ]))
        .expect("fixture row utf8");
        let steps = cursor_steps_for_cell(captured.strip_suffix('\n').unwrap_or(&captured), col);
        if steps > 0 {
            self.run(&[
                "send-keys",
                "-t",
                "t",
                "-X",
                "-N",
                &steps.to_string(),
                "cursor-right",
            ]);
        }
    }

    fn select(&self, anchor: (u32, u32), active: (u32, u32)) {
        let _ = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(["send-keys", "-t", "t", "-X", "cancel"])
            .output();
        self.run(&["copy-mode", "-H", "-t", "t"]);
        self.move_copy_cursor(anchor.0, anchor.1);
        self.run(&["send-keys", "-t", "t", "-X", "begin-selection"]);
        self.move_copy_cursor(active.0, active.1);
        assert_eq!(self.fmt("#{selection_present}"), "1");
    }

    fn selection_points(&self) -> ((i64, u32), (i64, u32)) {
        let raw = self.fmt(
            "#{history_size}\t#{selection_start_y}\t#{selection_start_x}\t#{selection_end_y}\t#{selection_end_x}",
        );
        let fields: Vec<i64> = raw
            .split('\t')
            .map(|field| field.parse().expect("selection coordinate numeric"))
            .collect();
        assert_eq!(fields.len(), 5);
        let history = fields[0];
        (
            (fields[1] - history, fields[2] as u32),
            (fields[3] - history, fields[4] as u32),
        )
    }

    fn production_selection_snapshot(&self, prefix: &str) -> Vec<u8> {
        self.try_production_selection_snapshot(prefix)
            .expect("production selection snapshot")
    }

    fn try_production_selection_snapshot(&self, prefix: &str) -> Result<Vec<u8>, String> {
        snapshot_selection("t", prefix, |args| self.run_owned(args)).map(String::into_bytes)
    }

    fn tmux_selection_oracle(&self, label: &str) -> Vec<u8> {
        self.run(&[
            "send-keys",
            "-t",
            "t",
            "-X",
            "copy-selection-no-clear",
            "-C",
            label,
        ]);
        let name = self
            .run(&["list-buffers", "-F", "#{buffer_name}"])
            .lines()
            .find(|name| name.starts_with(label))
            .expect("selection oracle buffer")
            .to_string();
        let bytes = self.run_raw_checked(&["show-buffer", "-b", &name]);
        self.run(&["delete-buffer", "-b", &name]);
        bytes
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

/// Starting a selection while one is already present must clear it first:
/// tmux's begin-selection command is a toggle, so calling it directly would
/// remove the old selection and leave the next update with nothing active.
/// A zero-cell selection is also a valid intermediate state until movement.
#[test]
fn repeated_selection_clear_then_begin_keeps_the_new_anchor_active() {
    let s = Server::fixture(
        "selection-repeat",
        40,
        8,
        "printf 'zero\\none\\ntwo\\nthree\\nfour\\nfive\\n'; sleep 30",
    );
    s.run(&["copy-mode", "-H", "-t", "t"]);
    s.move_copy_cursor(1, 0);
    s.run(&["send-keys", "-t", "t", "-X", "begin-selection"]);
    s.move_copy_cursor(3, 0);
    assert_eq!(s.fmt("#{selection_present}"), "1");

    s.run(&["send-keys", "-t", "t", "-X", "clear-selection"]);
    s.move_copy_cursor(2, 0);
    s.run(&["send-keys", "-t", "t", "-X", "begin-selection"]);
    assert_eq!(
        s.fmt("#{selection_present}"),
        "0",
        "the anchor alone is not yet selected text"
    );
    s.move_copy_cursor(5, 0);
    assert_eq!(s.fmt("#{selection_present}"), "1");
    assert_eq!(s.selection_points(), ((2, 0), (5, 0)));
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

/// Production scrolling is a single tmux command list: it enters copy-mode
/// only when history exists, advances on every call, and reports auto-exit at
/// the live bottom without separate state-query subprocesses.
#[test]
fn production_scroll_batch_enters_advances_and_exits() {
    let empty = Server::fixture("scroll-batch-empty", 40, 8, "sleep 30");
    assert_eq!(empty.fmt("#{history_size}"), "0");
    let no_op = empty
        .run_owned(&terminal_scroll::args("t", -2))
        .expect("empty production scroll");
    assert_eq!(no_op.trim(), "0", "empty history remains a live no-op");

    let s = Server::new("scroll-batch");
    s.shell("i=0; while [ $i -lt 40 ]; do echo batch$i; i=$((i+1)); done");

    let first = s
        .run_owned(&terminal_scroll::args("t", -2))
        .expect("first production scroll");
    assert_eq!(first.trim(), "1");
    assert_eq!(s.fmt("#{scroll_position}"), "2");

    let second = s
        .run_owned(&terminal_scroll::args("t", -3))
        .expect("second production scroll");
    assert_eq!(second.trim(), "1");
    assert_eq!(s.fmt("#{scroll_position}"), "5");

    let live = s
        .run_owned(&terminal_scroll::args("t", 60))
        .expect("production scroll to bottom");
    assert_eq!(live.trim(), "0");
    assert_eq!(s.fmt("#{pane_in_mode}"), "0");
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

/// Cross-screen selection delegates anchor, endpoint, repaint and wrap
/// semantics to tmux copy-mode. This exercises 2,500 deterministic rows and
/// verifies that the selected text crosses many screens in both directions,
/// rejoins soft wraps, and preserves real hard/blank line boundaries.
#[test]
fn copy_mode_selection_crosses_2500_rows_with_exact_wrap_boundaries() {
    let s = Server::new("selection");
    s.shell(
        "python3 -c '[print((f\"R7-{i:04d}|中文|😀|é|\") + (\"x\"*180 if i == 120 else \"\")) for i in range(2500)]; print(\"BLANK-A\"); print(); print(\"BLANK-B\")'",
    );
    for _ in 0..30 {
        if s.run(&["capture-pane", "-p", "-t", "t"])
            .contains("BLANK-B")
        {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    let hist: usize = s.fmt("#{history_size}").parse().expect("history numeric");
    assert!(hist > 2_400, "fixture must retain all 2,500 logical rows");

    s.run(&["copy-mode", "-H", "-t", "t"]);
    s.run(&["send-keys", "-t", "t", "-X", "history-top"]);
    s.run(&["send-keys", "-t", "t", "-X", "start-of-line"]);
    s.run(&["send-keys", "-t", "t", "-X", "begin-selection"]);
    s.run(&["send-keys", "-t", "t", "-X", "history-bottom"]);
    s.run(&["send-keys", "-t", "t", "-X", "end-of-line"]);
    s.run(&[
        "send-keys",
        "-t",
        "t",
        "-X",
        "copy-selection-no-clear",
        "-C",
        "r7selection",
    ]);
    let name = s
        .run(&["list-buffers", "-F", "#{buffer_name}"])
        .lines()
        .find(|name| name.starts_with("r7selection"))
        .expect("selection buffer")
        .to_string();
    let copied = s.run(&["show-buffer", "-b", &name]);
    assert!(copied.contains("R7-0000|中文|😀|é|"));
    assert!(copied.contains("R7-2499|中文|😀|é|"));
    let long = format!("R7-0120|中文|😀|é|{}", "x".repeat(180));
    assert!(copied.contains(&long), "soft wrap must not add a newline");
    assert!(
        copied.contains("BLANK-A\n\nBLANK-B"),
        "hard blank line must survive"
    );
    assert!(
        copied.find("R7-0000").unwrap() < copied.find("R7-2499").unwrap(),
        "selection order must stay forward"
    );

    // Reverse the active endpoint back toward the anchor: selection shrinks
    // without flipping or duplicating the intermediate rows.
    s.run(&["send-keys", "-t", "t", "-X", "-N", "100", "cursor-up"]);
    assert_eq!(s.fmt("#{selection_present}"), "1");
    s.run(&["send-keys", "-t", "t", "-X", "cancel"]);
}

/// The production history limit is 50,000. Keep a contract beyond 20,000
/// physical rows so a future tmux/config change cannot silently restore the
/// old short-scrollback ceiling or clamp selection coordinates to a screen.
#[test]
fn copy_mode_selection_reaches_beyond_20000_rows() {
    let s = Server::new("selection20k");
    s.shell(
        "python3 -c '[print(f\"R7-DEEP-{i:05d}\") for i in range(20050)]; print(\"R7-DEEP-END\")'",
    );
    for _ in 0..100 {
        if s.run(&["capture-pane", "-p", "-t", "t"])
            .contains("R7-DEEP-END")
        {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    let hist: i64 = s.fmt("#{history_size}").parse().expect("history numeric");
    assert!(
        hist > 20_000,
        "fixture must cross the 20,000-row boundary: {hist}"
    );

    s.run(&["copy-mode", "-H", "-t", "t"]);
    s.run(&["send-keys", "-t", "t", "-X", "history-top"]);
    s.run(&["send-keys", "-t", "t", "-X", "start-of-line"]);
    s.run(&["send-keys", "-t", "t", "-X", "begin-selection"]);
    s.run(&["send-keys", "-t", "t", "-X", "history-bottom"]);
    s.run(&["send-keys", "-t", "t", "-X", "end-of-line"]);
    assert_eq!(s.fmt("#{selection_present}"), "1");
    let start: i64 = s
        .fmt("#{selection_start_y}")
        .parse()
        .expect("selection_start_y numeric");
    let end: i64 = s
        .fmt("#{selection_end_y}")
        .parse()
        .expect("selection_end_y numeric");
    assert!(
        (start - end).abs() > 20_000,
        "selection span must cross 20,000 rows: start={start}, end={end}"
    );
}

/// The release copy command snapshots tmux's native selection into a uniquely
/// named buffer, reads its exact bytes, and deletes it in one command batch.
/// Compare that production batch with a separately-read tmux buffer and fixed
/// literals so command plumbing cannot add a byte or clear the selection.
#[test]
fn production_selection_snapshot_matches_tmux_byte_for_byte() {
    let s = Server::fixture(
        "selection-bytes",
        80,
        8,
        "sh -c 'printf \"ABCDEFGHIJKLMNO\\nSECOND-LINE\\n\\nA中BéC👩‍💻D\\nTRAIL   X\\n\"; sleep 30'",
    );
    let cases = [
        ("ascii-forward", (0, 3), (0, 7), "DEFG"),
        ("ascii-reverse", (0, 7), (0, 3), "DEFG"),
        ("ascii-one", (0, 3), (0, 4), "D"),
        ("line-start", (0, 0), (0, 1), "A"),
        ("line-end", (0, 14), (1, 0), "O\n"),
        ("multi-forward", (0, 3), (1, 6), "DEFGHIJKLMNO\nSECOND"),
        ("multi-reverse", (1, 6), (0, 3), "DEFGHIJKLMNO\nSECOND"),
        ("blank-line", (1, 11), (3, 0), "\n\n"),
        // Requesting the second cell of 中 makes tmux snap the endpoint to
        // the next legal cursor boundary (column 3); extraction uses the
        // actual coordinates reported by that same pane.
        ("wide-second-end", (3, 0), (3, 2), "A"),
        ("wide-second-start", (3, 2), (3, 3), "中"),
        ("combining", (3, 4), (3, 5), "é"),
        ("zwj", (3, 6), (3, 8), "👩‍💻"),
        ("trailing-spaces", (4, 0), (4, 8), "TRAIL   "),
    ];

    for (label, anchor, active, literal) in cases {
        s.select(anchor, active);
        let prefix = format!("production-{label}-");
        let production = s.production_selection_snapshot(&prefix);
        let oracle = s.tmux_selection_oracle(label);
        assert_eq!(production, oracle, "tmux mismatch: {label}");
        assert_eq!(production, literal.as_bytes(), "literal mismatch: {label}");
        assert!(
            !s.run(&["list-buffers", "-F", "#{buffer_name}"])
                .lines()
                .any(|name| name.starts_with(&prefix)),
            "production snapshot buffer must be deleted: {label}"
        );
    }
}

#[test]
fn production_selection_snapshot_rejoins_only_soft_wraps() {
    let s = Server::fixture(
        "selection-wrap",
        10,
        5,
        "sh -c 'printf \"ABCDEFGHIJKLMNO\\nSECOND\\n\"; sleep 30'",
    );
    for (label, anchor, active) in [
        ("wrap-forward", (0, 3), (2, 3)),
        ("wrap-reverse", (2, 3), (0, 3)),
    ] {
        s.select(anchor, active);
        let prefix = format!("production-{label}-");
        let production = s.production_selection_snapshot(&prefix);
        assert_eq!(
            production, b"DEFGHIJKLMNO\nSEC",
            "literal mismatch: {label}"
        );
        assert_eq!(
            production,
            s.tmux_selection_oracle(label),
            "tmux mismatch: {label}"
        );
    }
}

/// v0.4.32 read history_size and absolute selection rows once, then issued up
/// to three capture-pane commands with derived relative coordinates. Force
/// output before every one of those captures: all stale coordinates drift,
/// while the production native snapshot remains the exact selected bytes.
#[test]
fn selection_snapshot_stays_exact_while_history_grows_between_captures() {
    let s = Server::fixture("selection-race", 80, 8, "sleep 30");
    s.write_pane_lines("PRE", 0, 80);
    s.select((3, 0), (5, 7));
    let (start, end) = s.selection_points();
    assert_eq!((start, end), ((3, 0), (5, 7)));

    let stale_ranges = [(start.0, start.0), (end.0, end.0), (start.0, end.0)];
    for (index, (capture_start, capture_end)) in stale_ranges.into_iter().enumerate() {
        s.write_pane_lines("POST", index * 20, (index + 1) * 20);
        let captured = s.run_raw_checked(&[
            "capture-pane",
            "-p",
            "-J",
            "-S",
            &capture_start.to_string(),
            "-E",
            &capture_end.to_string(),
            "-t",
            "t",
        ]);
        assert!(
            !captured.windows(b"PRE-076".len()).any(|w| w == b"PRE-076"),
            "stale capture {index} must demonstrate coordinate drift"
        );
    }

    let production = s.production_selection_snapshot("production-race-");
    let oracle = s.tmux_selection_oracle("race-oracle");
    assert_eq!(production, b"PRE-076\nPRE-077\nPRE-078");
    assert_eq!(production, oracle);
}

#[test]
fn vanished_selection_never_copies_or_deletes_an_unrelated_tmux_buffer() {
    let s = Server::fixture("selection-vanished", 80, 8, "sleep 30");
    s.run(&["set-buffer", "-b", "unrelated", "DO-NOT-COPY"]);
    s.run(&["copy-mode", "-H", "-t", "t"]);

    let error = s
        .try_production_selection_snapshot("production-vanished-")
        .unwrap_err();
    assert_eq!(error, "tmux did not create a terminal selection snapshot");
    assert_eq!(
        s.run_raw_checked(&["show-buffer", "-b", "unrelated"]),
        b"DO-NOT-COPY"
    );
}
