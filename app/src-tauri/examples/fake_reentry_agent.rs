//! A harmless stand-in for a direct native agent client, for the re-entry
//! integration tests only. It is never bundled and never runs a model.
//!
//! It behaves like the certified topology: a foreground program started by a
//! pane's shell that reads raw keys, runs the REAL bundled lifecycle helper
//! as its direct child (in a fresh session, as the native client does) with a
//! native-shaped hook payload, and exits at its input after the second
//! Ctrl+D. Every launch appends one line — its argv after argv[0] — to
//! `$HOME/launches.log`, so a test can count real launches and read their
//! exact arguments.
//!
//! Behaviour comes from `fake-agent.plan` in the working directory
//! (`key=value` lines): `session=<uuid>` (the native id of a fresh start),
//! `switch=<uuid>,<uuid>` (ids switched through with `/clear`-style
//! end/start pairs right after start), `exit=normal|no-end|other|stall|
//! linger|suspend|conflict:<uuid>`, `resume=ok|wrong:<uuid>|startup|silent|
//! leave|vanish|stop|chdir|reexec:<arguments>`. `stop` reports and then
//! stops itself (alive, no longer the foreground); `chdir` leaves the working
//! directory before reporting; `reexec:` replaces this process's image with
//! the same program and other arguments (same pid, another launch image)
//! and then reports the session it was ORIGINALLY asked to resume. The
//! helper's path is the one line of `$HOME/helper-path`. `also=<program>`
//! makes a fresh start run that program once as one more hook, with a status
//! hook's arguments: a stand-in for the Signal hook a user's own settings
//! put beside the lifecycle pair. `ui=<directory>` makes it draw screens for
//! a test of an operator's reading of the pane (`Ui`), and `keys=<file>`
//! makes it write down every byte it is sent.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

fn plan() -> std::collections::HashMap<String, String> {
    std::fs::read_to_string("fake-agent.plan")
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Run the lifecycle helper the way the native client runs a hook: a direct
/// child in its own session without a terminal, the payload on stdin, and
/// wait for it. The payload carries extra fields a helper must never read.
fn hook(event: &str, session: &str, word: &str) {
    let Ok(helper) = std::fs::read_to_string(home().join("helper-path")) else {
        return;
    };
    let key = if event == "session-start" {
        "source"
    } else {
        "reason"
    };
    let payload = format!(
        r#"{{"session_id":"{session}","{key}":"{word}","transcript_path":"/SENTINEL/transcript","cwd":"/SENTINEL/cwd","prompt":"SENTINEL-PROMPT","model":"SENTINEL-MODEL","permission_mode":"bypassPermissions"}}"#
    );
    run_hook(helper.trim(), event, &payload);
}

/// One hook program, run as the native client runs any hook of any family.
fn run_hook(program: &str, word: &str, payload: &str) {
    let mut command = Command::new(program);
    command
        .args(["claude-code", word])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid(2) is async-signal-safe and only detaches the child.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Ok(mut child) = command.spawn() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.as_bytes());
        }
        let _ = child.wait();
    }
}

/// Drawn screens, for a test of how an operator reads a pane: with
/// `ui=<directory>` in the plan this stand-in shows
/// `<directory>/<kind>-<n>.screen` from n = 0, where `<kind>` is `fresh`,
/// `resume` or `resume-bypass` by its own arguments. `{input}` in a screen
/// is what was typed since that screen was shown. Enter, Down and Shift+Tab
/// move on to the next screen when there is one; a first line `@after <ms>`
/// makes a screen move on by itself when no key arrives for that long.
struct Ui {
    dir: std::path::PathBuf,
    kind: &'static str,
    at: usize,
    typed: String,
}

impl Ui {
    fn file(&self, at: usize) -> std::path::PathBuf {
        self.dir.join(format!("{}-{at}.screen", self.kind))
    }

    /// (milliseconds after which this screen moves on by itself, its text)
    fn screen(&self) -> (Option<i32>, String) {
        let text = std::fs::read_to_string(self.file(self.at)).unwrap_or_default();
        match text
            .strip_prefix("@after ")
            .and_then(|rest| rest.split_once('\n'))
        {
            Some((ms, rest)) => (ms.trim().parse().ok(), rest.to_string()),
            None => (None, text),
        }
    }

    fn draw(&self) {
        let text = self
            .screen()
            .1
            .replace("{input}", &self.typed)
            .replace('\n', "\r\n");
        let mut out = std::io::stdout();
        let _ = write!(out, "\x1b[2J\x1b[H{text}");
        let _ = out.flush();
    }

    fn next(&mut self) {
        if self.file(self.at + 1).exists() {
            self.at += 1;
        }
        self.typed.clear();
        self.draw();
    }
}

fn raw_terminal() -> Option<libc::termios> {
    // SAFETY: plain termios calls on this process's own stdin.
    unsafe {
        let mut saved = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(0, &mut saved) != 0 {
            return None;
        }
        let mut raw = saved;
        libc::cfmakeraw(&mut raw);
        libc::tcsetattr(0, libc::TCSANOW, &raw);
        Some(saved)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let plan = plan();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join("launches.log"))
    {
        let _ = writeln!(log, "{}", args.join(" "));
    }
    let resumed = args
        .iter()
        .position(|arg| arg == "--resume")
        .and_then(|at| args.get(at + 1))
        .cloned();
    let fresh = plan
        .get("session")
        .cloned()
        .unwrap_or_else(|| "00000000-0000-4000-8000-000000000000".into());
    const REEXEC: &str = "FAKE_REENTRY_ASKED";
    let asked = std::env::var(REEXEC).ok();
    if let (Some(id), Some(other), None) = (
        resumed.as_ref(),
        plan.get("resume").and_then(|r| r.strip_prefix("reexec:")),
        asked.as_ref(),
    ) {
        if let Ok(me) = std::env::current_exe() {
            // returns only when the exec failed
            let _ = Command::new(me)
                .args(other.split(' '))
                .env(REEXEC, id)
                .exec();
        }
    }
    let mut session = asked.or_else(|| resumed.clone()).unwrap_or(fresh);
    let saved = raw_terminal();
    let restore = || {
        if let Some(saved) = saved {
            // SAFETY: restores the attributes read above.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, &saved) };
        }
    };
    match (resumed.is_some(), plan.get("resume").map(String::as_str)) {
        (false, _) => hook("session-start", &session, "startup"),
        (true, Some("silent")) => {}
        (true, Some("startup")) => hook("session-start", &session, "startup"),
        (true, Some(other)) if other.starts_with("wrong:") => {
            hook("session-start", &other["wrong:".len()..], "resume")
        }
        // enters and is gone at once, its end never reported
        (true, Some("vanish")) => {
            hook("session-start", &session, "resume");
            restore();
            return;
        }
        // enters, then stops itself: the shell takes the foreground back
        // while this exact process stays alive
        (true, Some("stop")) => {
            hook("session-start", &session, "resume");
            restore();
            // SAFETY: stops this process; nothing else is touched.
            unsafe { libc::raise(libc::SIGSTOP) };
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        (true, Some("chdir")) => {
            let _ = std::env::set_current_dir("/");
            hook("session-start", &session, "resume");
        }
        (true, Some("leave")) => {
            hook("session-start", &session, "resume");
            hook("session-end", &session, "other");
            restore();
            return;
        }
        (true, _) => hook("session-start", &session, "resume"),
    }
    if resumed.is_none() {
        for next in plan
            .get("switch")
            .map(|ids| ids.split(',').map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default()
        {
            hook("session-end", &session, "clear");
            session = next;
            hook("session-start", &session, "clear");
        }
        // A hook of ANOTHER family beside the lifecycle pair, as a user's own
        // settings may add one: run once, with a status hook's arguments.
        if let Some(program) = plan.get("also") {
            run_hook(
                program,
                "working",
                r#"{"hook_event_name":"UserPromptSubmit","prompt":"SENTINEL-PROMPT"}"#,
            );
        }
    }
    let mut ui = plan.get("ui").map(|dir| Ui {
        dir: std::path::PathBuf::from(dir),
        kind: match (
            resumed.is_some(),
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions"),
        ) {
            (true, true) => "resume-bypass",
            (true, false) => "resume",
            (false, _) => "fresh",
        },
        at: 0,
        typed: String::new(),
    });
    if let Some(ui) = &ui {
        ui.draw();
    }
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join("ready.log"))
    {
        let _ = writeln!(log, "{}", std::process::id());
    }
    let exit = plan.get("exit").cloned().unwrap_or_else(|| "normal".into());
    let mut stdin = std::io::stdin();
    let mut eofs = 0;
    let mut byte = [0u8; 1];
    // where a key sequence stands: 0 none, 1 after ESC, 2 after ESC [
    let mut escape = 0u8;
    // `keys=<file>`: every byte this client is sent, as it arrives
    let mut keys = plan.get("keys").and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    loop {
        if let Some(ms) = ui.as_ref().and_then(|ui| ui.screen().0) {
            let mut key = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for this process's own stdin.
            if unsafe { libc::poll(&mut key, 1, ms) } == 0 {
                if let Some(ui) = ui.as_mut() {
                    ui.next();
                }
                continue;
            }
        }
        if !stdin.read(&mut byte).is_ok_and(|n| n == 1) {
            break;
        }
        if let Some(keys) = keys.as_mut() {
            let _ = keys.write_all(&byte);
        }
        if byte[0] == 4 {
            eofs += 1;
        }
        if eofs >= 2 && exit != "stall" {
            break;
        }
        if let Some(ui) = ui.as_mut() {
            escape = match (escape, byte[0]) {
                (0, 0x1b) => 1,
                (1, b'[') => 2,
                // Down, Shift+Tab, Enter
                (2, b'B' | b'Z') | (0, b'\r') => {
                    ui.next();
                    0
                }
                (0, key) if (0x20..0x7f).contains(&key) => {
                    ui.typed.push(key as char);
                    ui.draw();
                    0
                }
                _ => 0,
            };
        }
    }
    match exit.as_str() {
        "no-end" => {}
        // reports its final session, then stops itself: the shell takes the
        // foreground back while this exact process stays alive
        "suspend" => {
            hook("session-end", &session, "prompt_input_exit");
            restore();
            // SAFETY: stops this process; nothing else is touched.
            unsafe { libc::raise(libc::SIGSTOP) };
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        // reports its final session and then does not go away
        "linger" => {
            hook("session-end", &session, "prompt_input_exit");
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        "other" => hook("session-end", &session, "other"),
        conflict if conflict.starts_with("conflict:") => {
            hook(
                "session-end",
                &conflict["conflict:".len()..],
                "prompt_input_exit",
            );
            hook("session-end", &session, "prompt_input_exit");
        }
        _ => hook("session-end", &session, "prompt_input_exit"),
    }
    restore();
}
