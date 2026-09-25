//! signal_fixture — the deterministic fake agent of the `signal-finish`
//! WKWebView smoke (Signal Integrity FR-SI-01). Debug only: `app/run.sh`
//! builds it for that mode alone, it is never bundled, and it refuses to run
//! outside an isolated smoke (its tmux socket must be `deck-smoke*` and its
//! sentinel directory must sit beside that instance's `status.sock`).
//!
//! It replays the Claude Code shape that made a "close the card" automation
//! kill live work: a prompt arrives → `working` → a background command
//! starts → `turn-done` while that command still runs → the command
//! finishes → the agent resumes by itself (`working`, `turn-done`) → the
//! program exits. Every status report goes through the smoke bundle's real
//! `deck-status-helper` (`<helper> claude-code <word>`; the bundle
//! `run.sh` builds beside this example, `target/debug/deck-smoke.app`), so
//! deck sees the exact wire, pane identity and process ancestry an agent
//! hook produces. It takes no arguments: the automation's command is just
//! this path (a rule command is capped at 200 characters).
//!
//! Protocol v2 (FR-SI-04): each hook call pipes a Claude Code-shaped
//! payload whose `prompt_id` is this interaction's id (the first prompt,
//! then the resumed one), plus decoy text fields and a fake id inside the
//! prompt; only the real id may reach deck (`v=2` in its app.log).
//!
//! Content-free: the delivered prompt is read and discarded, the sentinels
//! (`STARTED`, `COMPLETED`, `fixture.pid`, `child.pid`, 0600) hold nothing but
//! a pid or an empty file, and the pane shows one fixed line. No network, no
//! model. The background child exits without `COMPLETED` the moment its
//! parent is gone, so a retired session leaves `COMPLETED` absent.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long the background command runs after `turn-done`: several times
/// the finish rule's three-poll confirmation, so a close that trusts the
/// word lands while the work is still running.
const BACKGROUND: Duration = Duration::from_secs(12);
/// The agent stays in front after its resumed turn so the smoke can see the
/// completed work with the card still on the Board.
const LINGER: Duration = Duration::from_secs(3);
/// Bounded waits: a fixture never outlives a broken smoke by long.
const PROMPT_WAIT: Duration = Duration::from_secs(120);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--background") {
        let (Some(dir), Some(parent)) = (args.get(2), args.get(3)) else {
            std::process::exit(2);
        };
        background(Path::new(dir), parent.parse().unwrap_or(0));
        return;
    }
    let (helper, dir) = match isolated() {
        Ok(found) => found,
        Err(why) => {
            eprintln!("signal_fixture: refused ({why})");
            std::process::exit(2);
        }
    };
    let helper = helper.to_string_lossy().into_owned();
    let helper = helper.as_str();
    sentinel(&dir.join("fixture.pid"), &std::process::id().to_string());

    // an agent-like terminal: bracketed paste on, so deck's process-bound
    // delivery (which requires it) pastes the run's prompt here
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b[?2004hsignal fixture: waiting for a prompt\r\n");
    let _ = out.flush();
    if !prompt_arrived() {
        std::process::exit(3);
    }

    // Each interaction carries its own source id, as Claude Code's
    // `prompt_id` does (runtime-proven): the first prompt, and the resumed
    // one after the background work. Protocol v2 end to end.
    let first = interaction_id(1);
    let resumed = interaction_id(2);
    report(helper, "working", &first);
    let Ok(mut child) = Command::new(std::env::current_exe().expect("own path"))
        .arg("--background")
        .arg(&dir)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        std::process::exit(4);
    };
    sentinel(&dir.join("child.pid"), &child.id().to_string());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.join("STARTED").is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    // the interaction boundary, while the background command still runs
    report(helper, "turn-done", &first);
    let _ = child.wait();
    // the agent resumes on its own once its background work is done
    report(helper, "working", &resumed);
    std::thread::sleep(Duration::from_millis(500));
    report(helper, "turn-done", &resumed);
    std::thread::sleep(LINGER);
    let _ = write!(out, "\x1b[?2004l");
    let _ = out.flush();
}

/// The background command: `STARTED` now, `COMPLETED` after `BACKGROUND`
/// unless its parent (the agent) died first.
fn background(dir: &Path, parent: u32) {
    sentinel(&dir.join("STARTED"), "");
    let deadline = Instant::now() + BACKGROUND;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        if unsafe { libc::getppid() } as u32 != parent {
            return;
        }
    }
    sentinel(&dir.join("COMPLETED"), "");
}

/// Only inside an isolated smoke: a `deck-smoke*` tmux socket, the smoke
/// bundle's helper beside this build, and the existing sentinel directory
/// `<data dir>/signal-fixture` of the instance whose status socket this pane
/// reports to. Returns (helper, sentinel dir).
fn isolated() -> Result<(PathBuf, PathBuf), &'static str> {
    let tmux = std::env::var("TMUX").map_err(|_| "not in tmux")?;
    let socket = tmux.split(',').next().unwrap_or("");
    if !socket
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with("deck-smoke"))
    {
        return Err("not a deck-smoke tmux socket");
    }
    let status = std::env::var("DECK_STATUS_SOCK").map_err(|_| "no status socket")?;
    let status = Path::new(&status);
    let dir = status
        .parent()
        .ok_or("bad status socket")?
        .join("signal-fixture");
    if !status.is_absolute() || !dir.is_dir() {
        return Err("no sentinel directory beside the status socket");
    }
    // target/debug/examples/signal_fixture → target/debug/deck-smoke.app
    let exe = std::env::current_exe().map_err(|_| "own path")?;
    let helper = exe
        .parent()
        .and_then(Path::parent)
        .ok_or("own path")?
        .join("deck-smoke.app/Contents/MacOS/deck-status-helper");
    if !helper.is_file() {
        return Err("no smoke bundle helper");
    }
    Ok((helper, dir))
}

/// Read the delivered prompt up to its Enter and discard it.
fn prompt_arrived() -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let mut stdin = std::io::stdin().lock();
        while let Ok(1) = stdin.read(&mut byte) {
            if matches!(byte[0], b'\n' | b'\r') {
                let _ = tx.send(());
                return;
            }
        }
    });
    rx.recv_timeout(PROMPT_WAIT).is_ok()
}

/// A lowercase UUID-shaped id, distinct per interaction and per run.
fn interaction_id(n: u32) -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!(
        "{pid:08x}-{:04x}-4{:03x}-8{:03x}-{nanos:012x}",
        n & 0xffff,
        n & 0xfff,
        (pid >> 12) & 0xfff
    )
}

/// The hook call: the real helper, with a Claude Code-shaped payload on
/// stdin. Only `prompt_id` may leave the helper; the decoy text fields and
/// a fake id inside the prompt must not (fixed fixture text, no content).
fn report(helper: &str, word: &str, interaction: &str) {
    let payload = format!(
        r#"{{"hook_event_name":"fixture","session_id":"00000000-0000-4000-8000-000000000000","prompt":"fixture prompt \"prompt_id\":\"ffffffff-ffff-4fff-8fff-ffffffffffff\"","cwd":"/nonexistent/fixture","prompt_id":"{interaction}"}}"#
    );
    let Ok(mut child) = Command::new(helper)
        .args(["claude-code", word])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload.as_bytes());
    }
    let _ = child.wait();
}

fn sentinel(path: &Path, text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
    {
        let _ = file.write_all(text.as_bytes());
    }
}
