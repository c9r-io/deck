//! The built `deck-status-helper` as an agent hook runs it: the exact line on
//! a private status socket, the connection held until the host closes it,
//! and the silent exit 0 in every case — outside tmux, inside another tmux,
//! with no deck listening, with a payload that names no interaction.
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

const HELPER: &str = env!("CARGO_BIN_EXE_deck-status-helper");
const ID: &str = "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000";
const TMUX: &str = "/private/tmp/tmux-501/deck-dev,4242,0";

/// A private directory, removed on drop.
struct Private(PathBuf);

impl Private {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "dsh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let mut dir = std::env::temp_dir().join(&name);
        // sun_path holds about 104 bytes; a long TMPDIR cannot carry a socket
        if dir.join(".deck/status.sock").as_os_str().len() > 100 {
            dir = Path::new("/tmp").join(&name);
        }
        std::fs::create_dir(&dir).unwrap();
        Private(dir)
    }

    fn listen(&self, socket: &Path) -> UnixListener {
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }
}

impl Drop for Private {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The helper in the given environment and nothing else. Under
/// `cargo llvm-cov` one more variable goes along: it tells the instrumented
/// helper where its counters belong. Without it they are written into this
/// crate's directory and the report never sees `main` run.
fn helper(home: &Private, args: &[&str], environment: &[(&str, &str)]) -> Command {
    let mut command = Command::new(HELPER);
    command
        .args(args)
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|file| ("LLVM_PROFILE_FILE", file)))
        .env("HOME", &home.0)
        .envs(environment.iter().copied())
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

/// One launch: the helper's arguments and its environment.
type Case<'a> = (&'a [&'a str], Vec<(&'a str, &'a str)>);

fn assert_silent_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

#[test]
fn one_line_reaches_the_owning_decks_socket_and_the_helper_waits_for_the_close() {
    let home = Private::new();
    let socket = home.0.join("status.sock");
    let listener = home.listen(&socket);
    let child = run(
        helper(
            &home,
            &["claude-code", "needs-input"],
            &[
                ("TMUX", TMUX),
                ("TMUX_PANE", "%7"),
                ("DECK_STATUS_SOCK", socket.to_str().unwrap()),
            ],
        ),
        &format!(
            r#"{{"prompt_id":"{ID}","message":"SENTINEL","tool_input":{{"prompt_id":"nested"}}}}"#
        ),
    );
    let mut stream = accept(&listener, Duration::from_secs(3)).expect("the helper connects");
    let line = read_line(&mut stream);
    assert_eq!(
        line,
        format!("{{\"v\":2,\"source\":\"claude-code\",\"state\":\"needs-input\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\",\"interaction\":\"{ID}\"}}\n")
    );
    assert!(
        !line.contains("SENTINEL"),
        "nothing of the payload but the id leaves"
    );
    // deck binds the event to the pane while the helper is still alive
    std::thread::sleep(Duration::from_millis(150));
    let mut child = child;
    assert!(
        child.try_wait().unwrap().is_none(),
        "the helper exited before the host closed"
    );
    drop(stream);
    assert_silent_success(&finish(child, Duration::from_secs(3)));
}

#[test]
fn a_payload_without_a_usable_id_falls_back_to_the_first_wire_version() {
    let home = Private::new();
    let socket = home.0.join("status.sock");
    let listener = home.listen(&socket);
    for payload in [
        "",
        "not json",
        r#"{"prompt_id":"NOT-A-UUID"}"#,
        &format!(r#"{{"prompt_id":"{ID}","prompt_id":"{ID}"}}"#),
        &format!(r#"{{"turn_id":"{ID}"}}"#),
    ] {
        let child = run(
            helper(
                &home,
                &["claude-code", "working"],
                &[
                    ("TMUX", TMUX),
                    ("TMUX_PANE", "%7"),
                    ("DECK_STATUS_SOCK", socket.to_str().unwrap()),
                ],
            ),
            payload,
        );
        let mut stream = accept(&listener, Duration::from_secs(3)).expect("the helper connects");
        assert_eq!(
            read_line(&mut stream),
            "{\"v\":1,\"source\":\"claude-code\",\"state\":\"working\",\"socket\":\"deck-dev\",\"server_pid\":4242,\"pane\":\"%7\"}\n",
            "payload {payload:?}"
        );
        drop(stream);
        assert_silent_success(&finish(child, Duration::from_secs(3)));
    }
}

#[test]
fn the_default_socket_is_under_the_helpers_home() {
    let home = Private::new();
    let listener = home.listen(&home.0.join(".deck/status.sock"));
    // no DECK_STATUS_SOCK, and one in the wrong shape: both mean the default
    for extra in [None, Some(("DECK_STATUS_SOCK", "relative/status.sock"))] {
        let mut environment = vec![("TMUX", TMUX), ("TMUX_PANE", "%7")];
        environment.extend(extra);
        let child = run(helper(&home, &["codex", "turn-done"], &environment), "{}");
        let mut stream = accept(&listener, Duration::from_secs(3)).expect("the helper connects");
        assert!(read_line(&mut stream)
            .starts_with("{\"v\":1,\"source\":\"codex\",\"state\":\"turn-done\""));
        drop(stream);
        assert_silent_success(&finish(child, Duration::from_secs(3)));
    }
}

#[test]
fn outside_a_deck_pane_the_helper_says_nothing_and_succeeds() {
    let home = Private::new();
    let socket = home.0.join("status.sock");
    let listener = home.listen(&socket);
    let sock = ("DECK_STATUS_SOCK", socket.to_str().unwrap());
    let cases: [Case; 6] = [
        // not inside tmux at all
        (&["claude-code", "working"], vec![sock]),
        // somebody else's tmux
        (
            &["claude-code", "working"],
            vec![
                ("TMUX", "/private/tmp/tmux-501/default,4242,0"),
                ("TMUX_PANE", "%7"),
                sock,
            ],
        ),
        // a pane id that is not one
        (
            &["claude-code", "working"],
            vec![("TMUX", TMUX), ("TMUX_PANE", "7"), sock],
        ),
        // words outside the closed alphabet
        (
            &["Claude Code", "working"],
            vec![("TMUX", TMUX), ("TMUX_PANE", "%7"), sock],
        ),
        // too few arguments
        (
            &["claude-code"],
            vec![("TMUX", TMUX), ("TMUX_PANE", "%7"), sock],
        ),
        (&[], vec![("TMUX", TMUX), ("TMUX_PANE", "%7"), sock]),
    ];
    for (args, environment) in cases {
        let child = run(helper(&home, args, &environment), "{}");
        assert_silent_success(&finish(child, Duration::from_secs(3)));
        assert!(
            accept(&listener, Duration::from_millis(100)).is_none(),
            "args {args:?} connected"
        );
    }
}

#[test]
fn no_deck_listening_is_a_silent_success() {
    let home = Private::new();
    let child = run(
        helper(
            &home,
            &["claude-code", "turn-done"],
            &[("TMUX", TMUX), ("TMUX_PANE", "%7")],
        ),
        "{}",
    );
    assert_silent_success(&finish(child, Duration::from_secs(3)));
}

#[test]
fn a_host_that_never_closes_is_left_after_the_read_timeout() {
    let home = Private::new();
    let socket = home.0.join("status.sock");
    let listener = home.listen(&socket);
    let started = Instant::now();
    let child = run(
        helper(
            &home,
            &["claude-code", "turn-done"],
            &[
                ("TMUX", TMUX),
                ("TMUX_PANE", "%7"),
                ("DECK_STATUS_SOCK", socket.to_str().unwrap()),
            ],
        ),
        "{}",
    );
    // held open and never read from or closed
    let _stream = accept(&listener, Duration::from_secs(3)).expect("the helper connects");
    let output = finish(child, Duration::from_secs(6));
    assert_silent_success(&output);
    assert!(
        started.elapsed() >= Duration::from_millis(1_800),
        "the helper left before its two-second bound: {:?}",
        started.elapsed()
    );
}
