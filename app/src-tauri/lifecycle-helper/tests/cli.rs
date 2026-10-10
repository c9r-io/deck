//! The built `deck-lifecycle-helper` against a private status socket: the
//! exact line, the connection held until the host closes it, the total
//! deadline, the bounded stdin, and the silent exit 0 in every case.
//!
//! Each test owns a fresh private directory that is both the helper's HOME
//! and the socket's parent, and the helper runs with a cleared environment,
//! so nothing here can reach a real `~/.deck`.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const HELPER: &str = env!("CARGO_BIN_EXE_deck-lifecycle-helper");
const ID: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
const TMUX: &str = "/private/tmp/tmux-501/deck-dev,4242,0";

/// A private directory holding `status.sock`, removed on drop.
struct Private(PathBuf);

impl Private {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "dlh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut dir = std::env::temp_dir().join(&name);
        // sun_path holds about 104 bytes; a long TMPDIR cannot carry a socket
        if dir.join("status.sock").as_os_str().len() > 100 {
            dir = Path::new("/tmp").join(&name);
        }
        std::fs::create_dir(&dir).unwrap();
        Private(dir)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("status.sock")
    }

    fn listen(&self) -> UnixListener {
        let listener = UnixListener::bind(self.socket()).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The helper in a deck pane's environment and nothing else.
fn helper(home: &Private, args: &[&str]) -> Command {
    let mut command = Command::new(HELPER);
    command
        .args(args)
        .env_clear()
        .env("HOME", &home.0)
        .env("TMUX", TMUX)
        .env("TMUX_PANE", "%7")
        .env("DECK_STATUS_SOCK", home.socket())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Start the helper, write `payload` and close its stdin.
fn run(mut command: Command, payload: &str) -> Child {
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(payload.as_bytes()).unwrap();
    drop(stdin);
    child
}

/// The helper's connection, or None when none arrives within `wait`.
fn accept(listener: &UnixListener, wait: Duration) -> Option<UnixStream> {
    let deadline = Instant::now() + wait;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).unwrap();
                return Some(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(e) => panic!("accept: {e}"),
        }
    }
}

/// Everything the helper wrote before it closed its write half.
fn read_line(stream: &mut UnixStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut line = String::new();
    stream.read_to_string(&mut line).unwrap();
    line
}

/// Wait for the helper to exit by itself; a helper that does not is killed
/// and fails the test.
fn finish(mut child: Child, wait: Duration) -> Output {
    let deadline = Instant::now() + wait;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the helper did not exit within {wait:?}");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    child.wait_with_output().unwrap()
}

fn assert_silent_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

fn start_payload() -> String {
    format!(
        r#"{{"hook_event_name":"SessionStart","session_id":"{ID}","transcript_path":"/Users/SENTINEL/t.jsonl","cwd":"/Users/SENTINEL","model":"SENTINEL","source":"resume"}}"#
    )
}

fn start_line() -> String {
    format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-start\",\"nativeSessionId\":\"{ID}\",\"startSource\":\"resume\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}}\n")
}

#[test]
fn session_start_sends_the_exact_line_and_stays_until_the_host_closes() {
    let home = Private::new();
    let listener = home.listen();
    let mut child = run(
        helper(&home, &["claude-code", "session-start"]),
        &start_payload(),
    );
    let mut stream = accept(&listener, Duration::from_secs(3)).expect("a connection");
    assert_eq!(read_line(&mut stream), start_line());
    // the host has the line and has not closed: the helper is still there
    std::thread::sleep(Duration::from_millis(150));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the helper left before the host closed the connection"
    );
    let closed = Instant::now();
    drop(stream);
    let output = finish(child, Duration::from_secs(3));
    let took = closed.elapsed();
    assert!(
        took < Duration::from_millis(250),
        "exit follows the close, not the deadline: {took:?}"
    );
    assert_silent_success(&output);
    assert!(
        accept(&listener, Duration::from_millis(50)).is_none(),
        "one connection only"
    );
}

#[test]
fn session_end_sends_the_exact_line() {
    let home = Private::new();
    let listener = home.listen();
    let payload = format!(
        r#"{{"reason":"prompt_input_exit","session_id":"{ID}","permission_mode":"SENTINEL","cwd":"/Users/SENTINEL"}}"#
    );
    let child = run(helper(&home, &["claude-code", "session-end"]), &payload);
    let mut stream = accept(&listener, Duration::from_secs(3)).expect("a connection");
    assert_eq!(
        read_line(&mut stream),
        format!("{{\"protocol\":\"agent-lifecycle\",\"v\":1,\"source\":\"claude-code\",\"event\":\"session-end\",\"nativeSessionId\":\"{ID}\",\"endReason\":\"prompt-input-exit\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}}\n")
    );
    drop(stream);
    assert_silent_success(&finish(child, Duration::from_secs(3)));
}

#[test]
fn a_host_that_never_closes_is_left_at_the_deadline() {
    let home = Private::new();
    let listener = home.listen();
    let started = Instant::now();
    let child = run(
        helper(&home, &["claude-code", "session-start"]),
        &start_payload(),
    );
    let mut stream = accept(&listener, Duration::from_secs(3)).expect("a connection");
    let connected = Instant::now();
    assert_eq!(read_line(&mut stream), start_line());
    // the stream is held open for the whole wait
    let output = finish(child, Duration::from_secs(3));
    let held = connected.elapsed();
    let total = started.elapsed();
    assert!(
        held >= Duration::from_millis(300),
        "the helper waits for the host: {held:?}"
    );
    assert!(
        total < Duration::from_millis(1000),
        "one 500 ms deadline bounds the exchange: {total:?}"
    );
    assert_silent_success(&output);
    drop(stream);
}

#[test]
fn a_stdin_that_is_never_closed_sends_nothing_and_still_exits_zero() {
    let home = Private::new();
    let listener = home.listen();
    let started = Instant::now();
    let mut child = helper(&home, &["claude-code", "session-start"])
        .spawn()
        .unwrap();
    // a complete, valid payload — but the writer keeps the pipe open
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(start_payload().as_bytes()).unwrap();
    stdin.flush().unwrap();
    let output = finish(child, Duration::from_secs(3));
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(250), "{took:?}");
    assert!(took < Duration::from_millis(1000), "{took:?}");
    assert_silent_success(&output);
    assert!(
        accept(&listener, Duration::from_millis(100)).is_none(),
        "no message without EOF on stdin"
    );
    drop(stdin);
}

#[test]
fn an_unknown_argv_does_nothing() {
    let home = Private::new();
    let listener = home.listen();
    for args in [
        &[][..],
        &["claude-code"],
        &["codex", "session-start"],
        &["claude-code", "turn-done"],
        &["claude-code", "session-start", "extra"],
    ] {
        let child = run(helper(&home, args), &start_payload());
        assert_silent_success(&finish(child, Duration::from_secs(3)));
        assert!(
            accept(&listener, Duration::from_millis(50)).is_none(),
            "{args:?}"
        );
    }
}

#[test]
fn a_refused_payload_does_nothing() {
    let home = Private::new();
    let listener = home.listen();
    for payload in [
        String::new(),
        "not json".to_string(),
        format!(r#"{{"session_id":"{ID}"}}"#),
        format!(
            r#"{{"session_id":"{}","source":"startup"}}"#,
            ID.to_uppercase()
        ),
        format!(r#"{{"session_id":"{ID}","source":"startup","session_id":"{ID}"}}"#),
        format!(r#"{{"session_id":"{ID}","source":"startup"}} {{}}"#),
    ] {
        let child = run(helper(&home, &["claude-code", "session-start"]), &payload);
        assert_silent_success(&finish(child, Duration::from_secs(3)));
        assert!(
            accept(&listener, Duration::from_millis(50)).is_none(),
            "{payload:?}"
        );
    }
}

#[test]
fn outside_a_deck_tmux_pane_nothing_is_sent() {
    let home = Private::new();
    let listener = home.listen();
    let cases: [fn(&mut Command); 4] = [
        |c| {
            c.env_remove("TMUX");
        },
        |c| {
            c.env_remove("TMUX_PANE");
        },
        |c| {
            c.env("TMUX", "/private/tmp/tmux-501/default,4242,0");
        },
        |c| {
            c.env("TMUX_PANE", "7");
        },
    ];
    for (index, adjust) in cases.iter().enumerate() {
        let mut command = helper(&home, &["claude-code", "session-start"]);
        adjust(&mut command);
        let child = run(command, &start_payload());
        assert_silent_success(&finish(child, Duration::from_secs(3)));
        assert!(
            accept(&listener, Duration::from_millis(50)).is_none(),
            "case {index}"
        );
    }
}

#[test]
fn an_unusable_socket_setting_falls_back_inside_home_and_exits_zero() {
    // No listener anywhere: an invalid DECK_STATUS_SOCK routes to
    // <HOME>/.deck/status.sock, which does not exist in the private HOME.
    let home = Private::new();
    let mut command = helper(&home, &["claude-code", "session-start"]);
    command.env("DECK_STATUS_SOCK", home.0.join("other.sock"));
    let started = Instant::now();
    let child = run(command, &start_payload());
    assert_silent_success(&finish(child, Duration::from_secs(3)));
    assert!(started.elapsed() < Duration::from_millis(500));
    // and the helper created nothing
    assert_eq!(std::fs::read_dir(&home.0).unwrap().count(), 0);

    // With a listener at the fallback path, that is where the line goes.
    std::fs::create_dir(home.0.join(".deck")).unwrap();
    let listener = UnixListener::bind(home.0.join(".deck/status.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut command = helper(&home, &["claude-code", "session-start"]);
    command.env_remove("DECK_STATUS_SOCK");
    let child = run(command, &start_payload());
    let mut stream = accept(&listener, Duration::from_secs(3)).expect("a connection");
    assert_eq!(read_line(&mut stream), start_line());
    drop(stream);
    assert_silent_success(&finish(child, Duration::from_secs(3)));
}
