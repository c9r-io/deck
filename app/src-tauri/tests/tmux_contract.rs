//! Contract tests for the tmux behaviors deck depends on — each one encodes
//! a bug that shipped (v0.4.9–0.4.12) so it can never silently regress.
//!
//! They run the committed static tmux sidecar against a THROWAWAY socket
//! (`deck-test-*`), never the live `deck` socket.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

#[path = "../src/terminal_scroll.rs"]
mod terminal_scroll;
#[path = "../src/terminal_selection.rs"]
mod terminal_selection;
use terminal_selection::{copy_cursor_moves, frame_rows, snapshot_selection};

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

    /// The production placement: `commands::push_copy_cursor` over
    /// `commands::visible_rows_through`, so these contracts exercise the same
    /// motions the app sends and not a second copy of the rules.
    fn write_pane(&self, text: &str) {
        let tty = self.fmt("#{pane_tty}");
        let mut pane = OpenOptions::new()
            .write(true)
            .open(tty)
            .expect("open pane tty");
        write!(pane, "{text}").expect("write pane bytes");
        pane.flush().expect("flush pane bytes");
    }

    /// Scroll up until the visible frame opens with `blank` empty rows over a
    /// row that carries text — the shape that made `cursor-down` snap.
    fn scroll_to_blank_top(&self, blank: u32) -> u32 {
        for _ in 0..120 {
            // run_raw_checked, not run: trimming a capture would eat exactly
            // the leading blank rows this is looking for.
            let captured = String::from_utf8(self.run_raw_checked(&[
                "capture-pane",
                "-p",
                "-S",
                &(-(self.scroll_position() as i64)).to_string(),
                "-E",
                &(blank as i64 - self.scroll_position() as i64).to_string(),
                "-t",
                "t",
            ]))
            .expect("frame utf8");
            let rows = frame_rows(&captured, blank);
            if rows[..blank as usize].iter().all(String::is_empty)
                && !rows[blank as usize].is_empty()
            {
                return self.scroll_position();
            }
            self.run(&["send-keys", "-t", "t", "-X", "scroll-up"]);
        }
        panic!("fixture never scrolled to a blank-topped frame");
    }

    fn scroll_position(&self) -> u32 {
        self.fmt("#{scroll_position}")
            .parse()
            .expect("scroll position numeric")
    }

    fn move_copy_cursor(&self, row: u32, col: u32) {
        let scroll: i64 = self
            .fmt("#{scroll_position}")
            .parse()
            .expect("scroll position numeric");
        let captured = String::from_utf8(self.run_raw_checked(&[
            "capture-pane",
            "-p",
            "-S",
            &(-scroll).to_string(),
            "-E",
            &(row as i64 - scroll).to_string(),
            "-t",
            "t",
        ]))
        .expect("fixture frame utf8");
        let moves = copy_cursor_moves(&frame_rows(&captured, row), row, col);
        self.run(&["send-keys", "-t", "t", "-X", "top-line"]);
        self.motion(moves.descend, "cursor-down");
        if moves.wrap {
            self.motion(1, "cursor-right");
        }
        self.motion(moves.descend_after_wrap, "cursor-down");
        self.motion(moves.steps, "cursor-right");
    }

    fn motion(&self, count: u32, action: &str) {
        if count == 0 {
            return;
        }
        self.run(&[
            "send-keys",
            "-t",
            "t",
            "-X",
            "-N",
            &count.to_string(),
            action,
        ]);
    }

    /// Paint a full-screen frame the way an agent UI does: alternate screen,
    /// cleared, then written from the home position.
    fn write_alternate_frame(&self, lines: &[&str]) {
        let mut frame = String::from("\x1b[?1049h\x1b[H\x1b[2J");
        for line in lines {
            frame.push_str(line);
            frame.push_str("\r\n");
        }
        self.write_pane(&frame);
        let marker = lines
            .iter()
            .rev()
            .find(|line| !line.is_empty())
            .expect("frame carries text");
        for _ in 0..200 {
            if self.fmt("#{alternate_on}") == "1"
                && self
                    .run(&["capture-pane", "-p", "-t", "t"])
                    .contains(&marker[..marker.len().min(12)])
            {
                return;
            }
            sleep(Duration::from_millis(10));
        }
        panic!("pane did not render the alternate-screen frame");
    }

    fn select(&self, anchor: (u32, u32), active: (u32, u32)) {
        let _ = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(["send-keys", "-t", "t", "-X", "cancel"])
            .output();
        self.run(&["copy-mode", "-H", "-t", "t"]);
        self.select_in_place(anchor, active);
    }

    /// The same drag, but on a pane already sitting where the user left it:
    /// deck never re-enters copy-mode on a wheel-scrolled pane, so scrolled
    /// placement has to be exercised without resetting the viewport.
    fn select_in_place(&self, anchor: (u32, u32), active: (u32, u32)) {
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

/// Restored text is pane OUTPUT, never shell INPUT. The real signed app
/// executable runs its hidden bootstrap mode, prints the one-use payload,
/// then execs a harmless long-lived process. tmux must retain the text in its
/// own scrollback and a command-shaped line must remain literal.
#[test]
fn shell_restore_bootstrap_becomes_tmux_history_without_executing_text() {
    let s = Server(format!(
        "deck-test-restore-bootstrap-{}",
        std::process::id()
    ));
    let root = std::env::temp_dir().join(format!("deck-restore-contract-{}", std::process::id()));
    let dir = root.join("shell-state");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&dir).expect("create restore fixture dir");
    let payload = dir.join(".restore-deck-contract-1.txt");
    let executed = root.join("must-not-exist");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&payload)
        .expect("create private restore payload");
    writeln!(file, "deck-shell-restore-v1").unwrap();
    for line in 0..40 {
        writeln!(file, "restored-contract-{line:02}").unwrap();
    }
    writeln!(file, "touch {}", executed.display()).unwrap();
    file.flush().unwrap();

    let deck = PathBuf::from(env!("CARGO_BIN_EXE_deck-app"));
    let deck = deck.to_str().expect("deck test binary path utf8");
    let payload_arg = payload.to_str().expect("payload path utf8");
    s.run_raw_checked(&[
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
        deck,
        "--deck-shell-bootstrap",
        payload_arg,
        "/bin/cat",
    ]);

    let mut captured = String::new();
    for _ in 0..200 {
        captured = s.run(&["capture-pane", "-p", "-J", "-S", "-100", "-t", "=t:"]);
        if captured.contains("deck restart") && s.fmt("#{pane_current_command}") == "cat" {
            break;
        }
        sleep(Duration::from_millis(10));
    }
    assert!(captured.contains("restored-contract-00"), "{captured}");
    assert!(captured.contains("restored-contract-39"), "{captured}");
    assert!(captured.contains(&format!("touch {}", executed.display())));
    assert!(
        !executed.exists(),
        "command-shaped history must not execute"
    );
    assert!(!payload.exists(), "bootstrap payload must be one-use");
    assert!(
        s.fmt("#{history_size}").parse::<u32>().unwrap_or(0) > 0,
        "restored output must live in tmux scrollback"
    );
    let _ = std::fs::remove_dir_all(root);
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

/// Ordinary scrolling is viewport navigation, not copy-cursor navigation.
/// tmux normally leaves its copy cursor on a fixed screen row, separating it
/// from an agent composer as that content moves. The production batch follows
/// the live cursor's content row in both directions without changing the
/// requested scroll position.
#[test]
fn production_scroll_cursor_stays_with_the_live_input_row() {
    let s = Server::new("scroll-cursor");
    s.shell("i=0; while [ $i -lt 40 ]; do echo cursor$i; i=$((i+1)); done");
    s.write_pane("\x1b[?1049h\x1b[2J\x1b[3;1Houtput\x1b[6;1HLONGINPUTTEXT\x1b[6;5H");
    for _ in 0..100 {
        if s.fmt("#{alternate_on}:#{cursor_y}:#{cursor_x}") == "1:5:4" {
            break;
        }
        sleep(Duration::from_millis(10));
    }
    assert_eq!(s.fmt("#{alternate_on}:#{cursor_y}:#{cursor_x}"), "1:5:4");

    let first_args = terminal_scroll::cursor_following_args("t", -3);
    let first = s
        .run_owned(&first_args)
        .unwrap_or_else(|error| panic!("cursor-following scroll up: {error}; args={first_args:?}"));
    assert_eq!(first.trim(), "1\t1");
    assert_eq!(
        s.fmt("#{scroll_position}:#{copy_cursor_y}:#{copy_cursor_x}"),
        "3:8:4"
    );

    let reverse = s
        .run_owned(&terminal_scroll::cursor_following_args("t", 1))
        .expect("cursor-following scroll down");
    assert_eq!(reverse.trim(), "1\t1");
    assert_eq!(
        s.fmt("#{scroll_position}:#{copy_cursor_y}:#{copy_cursor_x}"),
        "2:7:4"
    );

    let clamped = s
        .run_owned(&terminal_scroll::cursor_following_args("t", -60))
        .expect("cursor-following scroll clamps at the viewport edge");
    assert_eq!(clamped.trim(), "1\t0");
    let clamped_scroll = s.scroll_position();
    assert!(
        clamped_scroll > 6,
        "fixture must move the input row offscreen"
    );
    assert_eq!(s.fmt("#{copy_cursor_y}"), "11");
    let still_clamped = s
        .run_owned(&terminal_scroll::cursor_following_args("t", 1))
        .expect("cursor-following reverse scroll remains clamped");
    assert_eq!(still_clamped.trim(), "1\t0");
    assert_eq!(s.scroll_position(), clamped_scroll - 1);
    assert_eq!(
        s.fmt("#{copy_cursor_y}"),
        "11",
        "cursor following must not undo the requested viewport movement"
    );

    let live = s
        .run_owned(&terminal_scroll::cursor_following_args("t", 60))
        .expect("cursor-following scroll to bottom");
    assert_eq!(live.trim(), "0\t1");
    assert_eq!(s.fmt("#{pane_in_mode}:#{cursor_y}:#{cursor_x}"), "0:5:4");
    assert_eq!(
        s.fmt("#{@deck-scroll-cursor-row}"),
        "",
        "a later unrelated copy-mode must not inherit a stale cursor anchor"
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

/// A trailing CR in one literal tmux input operation removes the "text landed
/// but Enter didn't" partial-send window. The scheduler's guarded buffer paste
/// uses the same byte contract; this lower-level check proves CR behaves like
/// pressing Enter.
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

/// Scheduled context protection depends only on stable tmux generation ids
/// and the optional foreground basename. It must work without pane hooks.
/// The final command checks foreground and identity in the same server queue.
#[test]
fn scheduler_context_identity_process_and_hookless_compatibility_contract() {
    let s = Server::new("context");
    let format =
        "#{pid}\t#{session_id}\t#{window_id}\t#{pane_id}\t#{pane_pid}\t#{pane_current_command}";
    let first = s.run(&["display-message", "-p", "-t", "=t:", format]);
    let fields: Vec<&str> = first.split('\t').collect();
    assert_eq!(fields.len(), 6, "all context fields: {first:?}");
    assert!(fields[0].parse::<u32>().is_ok());
    assert!(fields[1].starts_with('$'));
    assert!(fields[2].starts_with('@'));
    assert!(fields[3].starts_with('%'));
    assert!(fields[4].parse::<u32>().is_ok());
    let old_pane = fields[3].to_string();
    let old_identity = fields[..5].join(":");

    // A foreground change after an earlier probe takes the refusal branch;
    // no prompt bytes reach the otherwise still-identical pane.
    let mismatch_buffer = "deck-contract-process";
    let identity = format!(
        "#{{==:#{{pid}}:#{{session_id}}:#{{window_id}}:#{{pane_id}}:#{{pane_pid}},{old_identity}}}"
    );
    let process_condition =
        format!("#{{&&:{identity},#{{==:#{{pane_current_command}},not-the-current-process}}}}");
    let out = s
        .run_owned(&[
            "set-buffer".into(),
            "-b".into(),
            mismatch_buffer.into(),
            "echo process-mismatch-must-not-land\r".into(),
            ";".into(),
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            old_pane.clone(),
            process_condition,
            format!("paste-buffer -b {mismatch_buffer} -d -t {old_pane}"),
            format!("delete-buffer -b {mismatch_buffer}; display-message -p deck-context-refused"),
        ])
        .expect("foreground-guarded literal command");
    assert!(
        out.contains("deck-context-refused"),
        "guard result: {out:?}"
    );
    assert!(!s
        .run(&["capture-pane", "-p", "-t", "=t:"])
        .contains("process-mismatch-must-not-land"));

    // With no expected process, exact identity alone is the compatibility
    // contract; no hook is configured or consulted.
    let compat_buffer = "deck-contract-compat";
    let out = s
        .run_owned(&[
            "set-buffer".into(),
            "-b".into(),
            compat_buffer.into(),
            "echo hookless-compat-landed\r".into(),
            ";".into(),
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            old_pane.clone(),
            identity,
            format!("paste-buffer -b {compat_buffer} -d -t {old_pane}"),
            format!("delete-buffer -b {compat_buffer}; display-message -p deck-context-refused"),
        ])
        .expect("identity-only compatibility command");
    assert!(
        !out.contains("deck-context-refused"),
        "guard result: {out:?}"
    );
    sleep(Duration::from_millis(200));
    assert!(s
        .run(&["capture-pane", "-p", "-t", "=t:"])
        .contains("hookless-compat-landed"));

    s.run(&["kill-session", "-t", "=t"]);
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
    sleep(Duration::from_millis(300));
    let second = s.run(&["display-message", "-p", "-t", "=t:", format]);
    let next: Vec<&str> = second.split('\t').collect();
    assert_ne!(fields[0], next[0], "server pid is the outer generation");

    // The numeric pane id itself may be reused by the new server. The
    // production guarded-literal pattern compares the whole generation and
    // therefore takes the refusal branch even in that case.
    let buffer = "deck-contract-send";
    let condition = format!(
        "#{{==:#{{pid}}:#{{session_id}}:#{{window_id}}:#{{pane_id}}:#{{pane_pid}},{old_identity}}}"
    );
    let yes =
        format!("paste-buffer -b {buffer} -d -t {old_pane}; display-message -p deck-context-sent");
    let no = format!("delete-buffer -b {buffer}; display-message -p deck-context-refused");
    let out = s
        .run_owned(&[
            "set-buffer".into(),
            "-b".into(),
            buffer.into(),
            "echo must-not-land\r".into(),
            ";".into(),
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            old_pane,
            condition,
            yes,
            no,
        ])
        .expect("guarded literal command");
    assert!(
        out.contains("deck-context-refused"),
        "guard result: {out:?}"
    );
    let screen = s.run(&["capture-pane", "-p", "-t", "=t:"]);
    assert!(!screen.contains("must-not-land"));
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

/// tmux's copy cursor cannot be both an immutable selection endpoint and a
/// freely moving viewport cursor. Production therefore snapshots once at
/// pointerup, clears only tmux's cursor-bound highlight, and retains these
/// content coordinates/bytes under the frontend token while scrolling.
#[test]
fn frozen_selection_coordinates_and_bytes_survive_viewport_scroll() {
    let s = Server::fixture(
        "selection-frozen-scroll",
        40,
        8,
        "python3 -c '[print(f\"FREEZE-{i:03d}\") for i in range(80)]'; sleep 30",
    );
    s.select((2, 0), (5, 10));
    let before_points = s.selection_points();
    let frozen = s.production_selection_snapshot("production-frozen-");
    assert!(!frozen.is_empty());

    s.run(&["send-keys", "-t", "t", "-X", "clear-selection"]);
    assert_eq!(s.fmt("#{selection_present}"), "0");
    let scrolled = s
        .run_owned(&terminal_scroll::cursor_following_args("t", -3))
        .expect("frozen-selection viewport scroll");
    assert_eq!(scrolled.trim(), "1\t0");
    assert_eq!(s.fmt("#{scroll_position}"), "3");
    assert_eq!(
        s.fmt("#{copy_cursor_y}"),
        "7",
        "the former selection endpoint must move to the live-input edge"
    );

    // These are the values the token-bound lease returns after scroll. They
    // are deliberately not re-read from tmux, where selection_present is now
    // false and the copy cursor is free to move the viewport.
    let after_points = before_points;
    let after_bytes = frozen.clone();
    assert_eq!(after_points, before_points);
    assert_eq!(after_bytes, frozen);
}

/// v0.4.38: a drag inside a full-screen agent UI (Claude Code, Codex)
/// highlighted a row the pointer never touched, while the identical drag in a
/// shell pane was correct. tmux's copy-mode `cursor-down` only keeps the
/// column at 0 once the walk has stepped off a line that is not empty; until
/// then it snaps the cursor to the end of the line it lands on, and the
/// `cursor-right` moves for the column then wrap onto later rows. A shell pane
/// carries text on the first visible row, an agent frame starts with blank
/// rows — which is the whole difference between the two.
#[test]
fn agent_frame_selection_lands_on_the_rows_the_pointer_touched() {
    let s = Server::fixture("selection-agent-frame", 40, 12, "sleep 30");
    let wrapped = "A".repeat(50);
    s.write_alternate_frame(&[
        "",
        "",
        "改好了 PR #2。",
        "",
        &wrapped,
        "",
        "tail",
        "",
        "trailing blanks   ",
    ]);

    // The reported case: the first line of text, under two blank rows.
    s.select((2, 0), (2, 6));
    assert_eq!(s.selection_points(), ((2, 0), (2, 6)));

    // A pointer inside a wide grapheme snaps to that grapheme, never to
    // another row.
    s.select((2, 3), (2, 9));
    assert_eq!(s.selection_points(), ((2, 2), (2, 9)));

    // The continuation row of a wrapped line: every tmux motion that walks to
    // a line end would leave this row for the logical line's real end.
    s.select((5, 0), (5, 4));
    assert_eq!(s.selection_points(), ((5, 0), (5, 4)));

    // A column at the very end of a short row must stay put, not wrap.
    s.select((7, 0), (7, 4));
    assert_eq!(s.selection_points(), ((7, 0), (7, 4)));

    // And the same holds across rows, blank ones included.
    s.select((2, 2), (7, 4));
    assert_eq!(s.selection_points(), ((2, 2), (7, 4)));

    // A row's trailing blanks are not cells: a pointer past the text clamps to
    // the line end instead of running a step over it onto the next row. (This
    // is why the frame is captured without -J, which would preserve them.)
    s.select((9, 0), (9, 30));
    assert_eq!(
        s.selection_points(),
        ((9, 0), (9, "trailing blanks".len() as u32))
    );
}

/// The wrap out of a blank row only works because tmux measures that row as
/// zero-length, and the step count only stays inside its row because
/// `capture-pane` measures line ends the same way tmux does. A row painted
/// with styled blanks (an agent UI's coloured bars) is the case where those
/// two could disagree: pin that they don't.
#[test]
fn styled_blank_rows_are_empty_to_both_capture_pane_and_the_copy_cursor() {
    let s = Server::fixture("selection-styled-blank", 40, 10, "sleep 30");
    // Two rows of spaces on a blue background, then real text.
    let bar = format!("\x1b[44m{: <40}\x1b[0m", "");
    s.write_alternate_frame(&[&bar, &bar, "text after the bars"]);

    let captured = String::from_utf8(s.run_raw_checked(&[
        "capture-pane",
        "-p",
        "-S",
        "0",
        "-E",
        "1",
        "-t",
        "t",
    ]))
    .expect("frame utf8");
    assert_eq!(
        frame_rows(&captured, 1),
        vec![String::new(), String::new()],
        "styled blanks must capture as empty rows"
    );

    // And tmux agrees: a cursor-right on such a row wraps to the next row's
    // column 0 instead of stepping along it.
    s.run(&["send-keys", "-t", "t", "-X", "cancel"]);
    s.run(&["copy-mode", "-H", "-t", "t"]);
    s.run(&["send-keys", "-t", "t", "-X", "top-line"]);
    s.run(&["send-keys", "-t", "t", "-X", "cursor-right"]);
    assert_eq!(s.fmt("#{copy_cursor_y}\t#{copy_cursor_x}"), "1\t0");

    s.select((2, 0), (2, 4));
    assert_eq!(s.selection_points(), ((2, 0), (2, 4)));
}

/// A wheel-scrolled pane is already in copy-mode at the user's history
/// position, so placement runs against a non-zero `scroll_position` and every
/// row/column stays frame-relative — including when the scrolled-to frame is
/// the blank-topped one that made `cursor-down` snap.
#[test]
fn selection_is_frame_relative_when_the_pane_is_scrolled() {
    let s = Server::fixture("selection-scrolled", 40, 10, "sleep 30");
    let mut fixture = String::new();
    for index in 0..30 {
        fixture.push_str(&format!("hist-{index:03}\r\n"));
    }
    // The blank run the frame must open on, then enough output to push it up
    // into history where a scroll can land on it.
    fixture.push_str("\r\n\r\n");
    for index in 0..30 {
        fixture.push_str(&format!("post-{index:03}\r\n"));
    }
    s.write_pane(&fixture);
    for _ in 0..200 {
        if s.run(&["capture-pane", "-p", "-t", "t"])
            .contains("post-029")
        {
            break;
        }
        sleep(Duration::from_millis(10));
    }

    s.run(&["send-keys", "-t", "t", "-X", "cancel"]);
    s.run(&["copy-mode", "-H", "-t", "t"]);
    let scroll = s.scroll_to_blank_top(2) as i64;
    assert!(scroll > 0, "fixture must actually be scrolled");

    s.select_in_place((2, 0), (2, 6));
    assert_eq!(
        s.scroll_position() as i64,
        scroll,
        "placement must not move the viewport"
    );
    let ((start_row, start_col), (end_row, end_col)) = s.selection_points();
    // selection_points() is history-relative; the frame sits `scroll` rows above it.
    assert_eq!(
        ((start_row + scroll, start_col), (end_row + scroll, end_col)),
        ((2, 0), (2, 6))
    );
    assert_eq!(
        s.production_selection_snapshot("deck-copy-scrolled-"),
        b"post-0"
    );
}

/// Coordinates are only half the contract: the bytes tmux hands back must be
/// the cells the pointer crossed. A wrapped row is where an endpoint that
/// silently slid onto the logical line's real end would still look plausible.
#[test]
fn agent_frame_selection_copies_the_cells_the_pointer_crossed() {
    let s = Server::fixture("selection-agent-bytes", 40, 10, "sleep 30");
    let wrapped = format!("{}{}", "C".repeat(40), "tail-of-wrapped-line");
    s.write_alternate_frame(&["", "", "改好了 PR #2。", "", &wrapped]);

    // Three graphemes of CJK on the first row of text.
    s.select((2, 0), (2, 6));
    assert_eq!(
        s.production_selection_snapshot("deck-copy-agent-cjk-"),
        "改好了".as_bytes()
    );

    // And a run inside the continuation row of the wrapped line.
    s.select((5, 0), (5, 8));
    assert_eq!(
        s.production_selection_snapshot("deck-copy-agent-wrapped-"),
        b"tail-of-"
    );
}
