//! channel_fixture — a harmless interactive `claude` stand-in for the
//! `channel-first-send` real-WKWebView smoke. Debug only: `app/run.sh` builds
//! it for that mode, copies it into the unique test app bundle as `claude`,
//! and signs it before LaunchServices opens that carrier.
//!
//! It accepts terminal input with bracketed paste enabled, records only
//! content-free receipt facts in `<smoke data>/channel-fixture/receipt.json`,
//! and never opens a network connection, starts a child, reports Agent
//! Signal, or touches the clipboard. The first fixed step must arrive after
//! Deck has spent at least four minutes hidden; without Signal the later
//! chain row must remain held. A ten-minute bound keeps an abandoned fixture
//! from living indefinitely.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EXPECTED_FIRST: &str = "CHANNEL-FIRST-SEND-STEP-1";
const EXPECTED_SECOND: &str = "CHANNEL-FIRST-SEND-STEP-2";
const LIFETIME: Duration = Duration::from_secs(600);

fn main() {
    let dir = match isolated() {
        Ok(dir) => dir,
        Err(why) => {
            eprintln!("channel_fixture: refused ({why})");
            std::process::exit(2);
        }
    };
    private_write(&dir.join("fixture.pid"), &std::process::id().to_string());
    private_write(&dir.join("READY"), "");

    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b[?2004hchannel fixture: ready\r\n");
    let _ = out.flush();

    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        let mut line = Vec::new();
        while input.read_until_byte(&mut line) {
            if send.send(std::mem::take(&mut line)).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + LIFETIME;
    let mut receipts = 0_u8;
    let mut first_matches = false;
    let mut second_seen = false;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        let Ok(line) = receive.recv_timeout(remaining) else {
            break;
        };
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_matches(['\r', '\n']);
        let text = text.strip_prefix("\x1b[200~").unwrap_or(text);
        let text = text.strip_suffix("\x1b[201~").unwrap_or(text);
        receipts = receipts.saturating_add(1);
        if receipts == 1 {
            first_matches = text == EXPECTED_FIRST;
        } else if text == EXPECTED_SECOND {
            second_seen = true;
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let receipt = format!(
            "{{\"receipts\":{receipts},\"firstMatches\":{first_matches},\"secondSeen\":{second_seen},\"lastAt\":{at}}}\n"
        );
        private_write_atomic(&dir, "receipt.json", &receipt);
    }
    let _ = write!(out, "\x1b[?2004l");
    let _ = out.flush();
}

trait ReadLineByte: Read {
    fn read_until_byte(&mut self, line: &mut Vec<u8>) -> bool {
        let mut byte = [0_u8; 1];
        loop {
            match self.read(&mut byte) {
                Ok(1) if matches!(byte[0], b'\r' | b'\n') => return true,
                Ok(1) => {
                    if line.len() < 1024 {
                        line.push(byte[0]);
                    }
                }
                _ => return false,
            }
        }
    }
}
impl<T: Read + ?Sized> ReadLineByte for T {}

fn isolated() -> Result<PathBuf, &'static str> {
    if std::env::args_os().len() != 1 {
        return Err("arguments are forbidden");
    }
    let exe = std::fs::canonicalize(std::env::current_exe().map_err(|_| "own path")?)
        .map_err(|_| "canonical own path")?;
    if exe.file_name().and_then(|v| v.to_str()) != Some("claude") {
        return Err("not named claude");
    }
    let bin = exe.parent().ok_or("no fixture bin")?;
    let macos = bin.parent().ok_or("no MacOS directory")?;
    let contents = macos.parent().ok_or("no Contents directory")?;
    let bundle = contents.parent().ok_or("no app bundle")?;
    if bin.file_name().and_then(|v| v.to_str()) != Some("channel-fixture-bin")
        || macos.file_name().and_then(|v| v.to_str()) != Some("MacOS")
        || contents.file_name().and_then(|v| v.to_str()) != Some("Contents")
        || !bundle
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|name| name.starts_with("deck-channel-smoke-") && name.ends_with(".app"))
    {
        return Err("not in channel smoke bundle");
    }
    let tmux = std::env::var("TMUX").map_err(|_| "not in tmux")?;
    if !tmux
        .split(',')
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .is_some_and(|name| name.starts_with("deck-smoke-channel-"))
    {
        return Err("not the channel smoke socket");
    }
    let status = PathBuf::from(std::env::var_os("DECK_STATUS_SOCK").ok_or("no status socket")?);
    let root = std::fs::canonicalize(status.parent().ok_or("bad status socket")?)
        .map_err(|_| "canonical data root")?;
    let dir = root.join("channel-fixture");
    let canonical_env = |name| {
        std::fs::canonicalize(PathBuf::from(std::env::var_os(name).ok_or("missing path")?))
            .map_err(|_| "canonical environment path")
    };
    let home = canonical_env("HOME")?;
    let zdotdir = canonical_env("ZDOTDIR")?;
    let tmp = canonical_env("TMPDIR")?;
    if !status.is_absolute()
        || !dir.is_dir()
        || home != root.join("home")
        || zdotdir != home
        || tmp != root.join("tmp")
    {
        return Err("environment is not isolated");
    }
    let expected_path = format!("{}:/usr/bin:/bin", bin.display());
    if std::env::var("PATH").ok().as_deref() != Some(expected_path.as_str()) {
        return Err("unsafe PATH");
    }
    Ok(dir)
}

fn private_write(path: &Path, text: &str) {
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

fn private_write_atomic(dir: &Path, name: &str, text: &str) {
    let staged = dir.join(format!(".{name}.tmp"));
    private_write(&staged, text);
    let _ = std::fs::rename(staged, dir.join(name));
}
