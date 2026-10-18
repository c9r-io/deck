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
//! and then reports the session it was ORIGINALLY asked to resume;
//! `resume=exec:<program>` replaces it with ANOTHER program — another build
//! at another path — under the same pid, which then reports that session;
//! `source=<word>` names the source word its hooks are run with (default
//! `claude-code`; `codex` for a stand-in of that client), `exit-print=hint`
//! makes it print the Codex-shaped exit hint for its final session to the
//! pane before it leaves (and `exit-print=hint:<uuid>` one that names
//! another id), `exit-also=<uuid>,<uuid>` makes it report the end of those
//! other sessions too as it leaves (a client that switched threads and
//! reports every loaded one at its exit, as Codex does), and a request of
//! the shape `resume [flags] <uuid>` is read as a resume of that id beside
//! `--resume <uuid>`;
//! `exit=exec:<program>` does the same at the exit, and the other program
//! reports the final session and leaves; `resume=both:<uuid>` reports a
//! resume of that other id and then of the asked one; `resume=late:<ms>`
//! reports the asked one that many milliseconds after it started;
//! `resume=on:<file>` reports the asked one only once that file exists, so a
//! test decides the instant. `then=<uuid>:<file>` makes a client that was
//! resumed report a resume of that OTHER id as well, once that file exists:
//! a later, conflicting answer. Each of the two appends a line to
//! `$HOME/reported.log` when its hook has returned. The
//! helper's path is the one line of `$HOME/helper-path`.
//!
//! What a hook is handed and how it is run can be changed too, to stand for
//! a native client whose contract moved: `end-payload=` / `start-payload=`
//! with `no-id` (no session id), `renamed` (the id under another key),
//! `typed` (the id as a number) or `word` (a reason or source this build
//! does not know); `final-mode=<word>` puts that permission mode into every
//! payload and `final-mode=absent` leaves the field out; `hooks=wrap-end` /
//! `hooks=wrap-start` run that hook through one more process, so the helper
//! is the client's grandchild. `also=<program>`
//! makes a fresh start run that program once as one more hook, with a status
//! hook's arguments: a stand-in for the Signal hook a user's own settings
//! put beside the lifecycle pair. `ui=<directory>` makes it draw screens for
//! a test of an operator's reading of the pane (`Ui`), and `keys=<file>`
//! makes it write down every byte it is sent.
//!
//! This file uses nothing but the standard library (`sys` below declares
//! the few C calls it makes): the re-entry tests compile it by itself with
//! `rustc` (`tests.rs`, `standin_program`), because cargo builds an example
//! as a program only when a run names no target. Cargo still builds, formats
//! and lints it as this package's example.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

/// The C calls of this program, with macOS's own layouts and values
/// (`<sys/termios.h>`, `<poll.h>`, `<signal.h>`).
mod sys {
    use std::os::raw::{c_int, c_short, c_uint, c_ulong};

    pub const TCSANOW: c_int = 0;
    pub const POLLIN: c_short = 0x0001;
    pub const SIGSTOP: c_int = 17;

    /// `struct termios`: four flag words, twenty control characters, two speeds.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Termios {
        flags: [c_ulong; 4],
        control: [u8; 20],
        speeds: [c_ulong; 2],
    }

    impl Termios {
        pub const ZERO: Self = Self {
            flags: [0; 4],
            control: [0; 20],
            speeds: [0; 2],
        };
    }

    /// `struct pollfd`.
    #[repr(C)]
    pub struct PollFd {
        pub fd: c_int,
        pub events: c_short,
        pub revents: c_short,
    }

    extern "C" {
        pub fn setsid() -> c_int;
        pub fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
        pub fn tcsetattr(fd: c_int, actions: c_int, termios: *const Termios) -> c_int;
        pub fn cfmakeraw(termios: *mut Termios);
        pub fn raise(signal: c_int) -> c_int;
        pub fn poll(fds: *mut PollFd, count: c_uint, timeout: c_int) -> c_int;
    }
}

/// The sixteen bytes after the colon are this file's build tag. A test may
/// overwrite them in a COPY of the built file before signing that copy:
/// another build of the same stand-in — other content, another digest and
/// code directory, the same behaviour.
#[used]
static BUILD_TAG: [u8; 46] = *b"DECK-FAKE-REENTRY-AGENT-BUILD:0000000000000000";

/// Set for the program an exiting client replaced itself with: the session
/// that client was in.
const EXITING: &str = "FAKE_REENTRY_EXITING";

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

/// Set for the one more process of `hooks=wrap-…`: the hook program it runs.
const WRAPPING: &str = "FAKE_REENTRY_WRAPPING";

/// Wait until a test creates `file`.
fn wait_for(file: &str) {
    while !std::path::Path::new(file).exists() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Write down that a report a test asked for has been made: its hook ran
/// and returned.
fn reported(what: &str) {
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join("reported.log"))
    {
        let _ = writeln!(log, "{what}");
    }
}

/// Run the lifecycle helper the way the native client runs a hook: a direct
/// child in its own session without a terminal, the payload on stdin, and
/// wait for it. The payload carries extra fields a helper must never read.
/// The plan may change what the payload says about the session and the
/// event, and put one more process between the client and the helper.
fn hook(event: &str, session: &str, word: &str) {
    let Ok(helper) = std::fs::read_to_string(home().join("helper-path")) else {
        return;
    };
    let plan = plan();
    let start = event == "session-start";
    let key = if start { "source" } else { "reason" };
    let variant = plan
        .get(if start {
            "start-payload"
        } else {
            "end-payload"
        })
        .map(String::as_str)
        .unwrap_or_default();
    let id = match variant {
        "no-id" => String::new(),
        "renamed" => format!(r#""sessionId":"{session}","#),
        "typed" => r#""session_id":12345,"#.to_string(),
        _ => format!(r#""session_id":"{session}","#),
    };
    let word = if variant == "word" {
        "a_word_of_a_later_release"
    } else {
        word
    };
    let mode = match plan.get("final-mode").map(String::as_str) {
        Some("absent") => String::new(),
        Some(mode) => format!(r#","permission_mode":"{mode}""#),
        None => r#","permission_mode":"bypassPermissions""#.to_string(),
    };
    let payload = format!(
        r#"{{{id}"{key}":"{word}","transcript_path":"/SENTINEL/transcript","cwd":"/SENTINEL/cwd","prompt":"SENTINEL-PROMPT","model":"SENTINEL-MODEL"{mode}}}"#
    );
    let wrapped = plan.get("hooks").map(String::as_str)
        == Some(if start { "wrap-start" } else { "wrap-end" });
    match std::env::current_exe() {
        // one more process: this program again, which runs the helper as
        // ITS child — the helper is then the client's grandchild
        Ok(me) if wrapped => run_hook_as(
            Command::new(me).env(WRAPPING, helper.trim()),
            event,
            &payload,
        ),
        _ => run_hook(helper.trim(), event, &payload),
    }
}

/// One hook program, run as the native client runs any hook of any family.
fn run_hook(program: &str, word: &str, payload: &str) {
    run_hook_as(&mut Command::new(program), word, payload)
}

fn run_hook_as(command: &mut Command, word: &str, payload: &str) {
    let source = plan()
        .get("source")
        .cloned()
        .unwrap_or_else(|| "claude-code".into());
    command
        .args([source.as_str(), word])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid(2) is async-signal-safe and only detaches the child.
    unsafe {
        command.pre_exec(|| {
            sys::setsid();
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

fn raw_terminal() -> Option<sys::Termios> {
    // SAFETY: plain termios calls on this process's own stdin.
    unsafe {
        let mut saved = sys::Termios::ZERO;
        if sys::tcgetattr(0, &mut saved) != 0 {
            return None;
        }
        let mut raw = saved;
        sys::cfmakeraw(&mut raw);
        sys::tcsetattr(0, sys::TCSANOW, &raw);
        Some(saved)
    }
}

fn main() {
    std::hint::black_box(&BUILD_TAG);
    // the other program of `exit=exec:`: report the final session and leave
    if let Ok(session) = std::env::var(EXITING) {
        return hook("session-end", &session, "prompt_input_exit");
    }
    // the one more process of `hooks=wrap-…`: run the hook program as a
    // child of THIS process, hand it the payload, wait and leave
    if let Ok(program) = std::env::var(WRAPPING) {
        let _ = Command::new(program)
            .args(std::env::args().skip(1))
            .env_remove(WRAPPING)
            .status();
        return;
    }
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
        .cloned()
        .or_else(|| {
            // Codex's shape: the `resume` word, then flags, then the id
            args.iter()
                .position(|arg| arg == "resume")
                .and_then(|at| args[at + 1..].iter().find(|arg| !arg.starts_with('-')))
                .cloned()
        });
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
    if let (Some(id), Some(program), None) = (
        resumed.as_ref(),
        plan.get("resume").and_then(|r| r.strip_prefix("exec:")),
        asked.as_ref(),
    ) {
        // the SAME request, another program: returns only when the exec failed
        let _ = Command::new(program).args(&args).env(REEXEC, id).exec();
    }
    let mut session = asked.or_else(|| resumed.clone()).unwrap_or(fresh);
    let saved = raw_terminal();
    let restore = || {
        if let Some(saved) = saved {
            // SAFETY: restores the attributes read above.
            unsafe { sys::tcsetattr(0, sys::TCSANOW, &saved) };
        }
    };
    match (resumed.is_some(), plan.get("resume").map(String::as_str)) {
        (false, _) => hook("session-start", &session, "startup"),
        (true, Some("silent")) => {}
        (true, Some("startup")) => hook("session-start", &session, "startup"),
        (true, Some(other)) if other.starts_with("wrong:") => {
            hook("session-start", &other["wrong:".len()..], "resume")
        }
        // a resume of another id, and then of the asked one: two answers
        (true, Some(other)) if other.starts_with("both:") => {
            hook("session-start", &other["both:".len()..], "resume");
            hook("session-start", &session, "resume");
        }
        // the right answer, that many milliseconds too late
        (true, Some(other)) if other.starts_with("late:") => {
            let wait = other["late:".len()..].parse().unwrap_or(0);
            std::thread::sleep(std::time::Duration::from_millis(wait));
            hook("session-start", &session, "resume");
        }
        // the right answer, at the instant a test chooses
        (true, Some(other)) if other.starts_with("on:") => {
            wait_for(&other["on:".len()..]);
            hook("session-start", &session, "resume");
            reported("on");
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
            unsafe { sys::raise(sys::SIGSTOP) };
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
    // one more answer, later and about another session: when a test says so
    if let (true, Some((other, file))) = (
        resumed.is_some(),
        plan.get("then").and_then(|then| then.split_once(':')),
    ) {
        let (other, file) = (other.to_string(), file.to_string());
        std::thread::spawn(move || {
            wait_for(&file);
            hook("session-start", &other, "resume");
            reported("then");
        });
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
            let mut key = sys::PollFd {
                fd: 0,
                events: sys::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for this process's own stdin.
            if unsafe { sys::poll(&mut key, 1, ms) } == 0 {
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
    // what a Codex-like client prints to its pane as it leaves: the exit
    // hint naming its final session (or another id, when the plan says so)
    if let Some(print) = plan.get("exit-print") {
        let named = match print.strip_prefix("hint:") {
            Some(other) => other.to_string(),
            None => session.clone(),
        };
        restore();
        let mut out = std::io::stdout();
        let _ = write!(
            out,
            "\r\nTo continue this session, run:\r\n\r\n  codex resume {named}\r\n\r\nOr run codex resume and select the session.\r\n"
        );
        let _ = out.flush();
    }
    // the other sessions this client still holds, each ended as it leaves
    for other in plan
        .get("exit-also")
        .map(|ids| ids.split(',').map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        hook("session-end", &other, "other");
    }
    match exit.as_str() {
        "no-end" => {}
        // leaves as another program under the same pid, which reports the
        // final session: returns only when the exec failed
        other if other.starts_with("exec:") => {
            restore();
            let _ = Command::new(&other["exec:".len()..])
                .env(EXITING, &session)
                .exec();
        }
        // reports its final session, then stops itself: the shell takes the
        // foreground back while this exact process stays alive
        "suspend" => {
            hook("session-end", &session, "prompt_input_exit");
            restore();
            // SAFETY: stops this process; nothing else is touched.
            unsafe { sys::raise(sys::SIGSTOP) };
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
