// Headless smoke test for the PTY bridge: create a tmux session, attach to it
// through a PTY, type a command, and verify its output comes back through the
// byte stream. Run with: cargo run --example pty_smoke
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

fn main() {
    let name = "deck-pty-smoke";
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output();
    let ok = Command::new("tmux")
        .args(["new-session", "-d", "-s", name, "-x", "120", "-y", "30"])
        .status()
        .unwrap();
    assert!(ok.success(), "tmux new-session failed");

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut cmd = CommandBuilder::new("tmux");
    cmd.args(["attach-session", "-t", &format!("={name}")]);
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    // give tmux a moment to paint, then type a command
    std::thread::sleep(Duration::from_millis(600));
    writer.write_all(b"echo deck_smoke_$((6*7))\r").unwrap();
    writer.flush().unwrap();

    let mut collected = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
            collected.extend(chunk);
            if String::from_utf8_lossy(&collected).contains("deck_smoke_42") {
                found = true;
                break;
            }
        }
    }

    // resize while attached must not error
    pair.master
        .resize(PtySize {
            rows: 40,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let _ = child.kill();
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", name])
        .output();

    assert!(
        found,
        "expected command output via PTY stream; got {} bytes",
        collected.len()
    );
    println!("PTY smoke test OK: attach → write → stream → resize → detach");
}
