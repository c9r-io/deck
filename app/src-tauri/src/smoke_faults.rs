//! Closed, debug-only fault injection for the packaged WKWebView gate.
//! Hooks arm only when both smoke arguments select an isolated data root;
//! normal and release launches cannot trigger them.
//! Clipboard metrics read UTF-8 regardless of the launching shell's locale;
//! the diagnostic must measure the same bytes the webview copied.

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const KINDS: &[&str] = &[
    "board-save",
    "settings-save",
    "queue-save",
    "channel-inbox-save",
    "channel-network",
    "channel-scope",
    "connector-after-accept",
    // local notifications (local_notify.rs): the receipt lost after the
    // publish was stored; a claim stored but not offered to the
    // dispatcher; a local item's ack refused after its card exists
    "local-no-receipt",
    "local-hold-offer",
    "local-ack-refused",
    "queue-cancel",
    "tmux-after-stop",
    "tmux-after-socket",
    "tmux-before-start",
    "tmux-after-metadata",
    STALL_ATTACH_SPAWN,
    STALL_START,
    STALL_KILL,
    STALL_CLIPBOARD,
];

/// Stall points (2026-11-27): the next armed hit of the attach worker's
/// spawn, a session start, a session kill or a clipboard write waits until
/// the smoke sets the same kind's count to 0 (or 120 s pass), standing in
/// for a process creation the kernel holds.
pub(crate) const STALL_ATTACH_SPAWN: &str = "stall-attach-spawn";
pub(crate) const STALL_START: &str = "stall-start";
pub(crate) const STALL_KILL: &str = "stall-kill";
pub(crate) const STALL_CLIPBOARD: &str = "stall-clipboard";
pub(crate) const ATTACH_SPAWN: &str = STALL_ATTACH_SPAWN;

/// Release epochs per stall kind: a stall waits until its kind's epoch
/// moves on.
static RELEASES: LazyLock<(Mutex<HashMap<&'static str, u64>>, std::sync::Condvar)> =
    LazyLock::new(|| (Mutex::new(HashMap::new()), std::sync::Condvar::new()));

/// Wait here if a stall of `kind` is armed (debug smoke only; a no-op in
/// every other launch).
pub(crate) fn stall_point(kind: &str) {
    if !take(kind) {
        return;
    }
    let (epochs, released) = &*RELEASES;
    let mut held = epochs.lock_or_recover();
    let start = *held.get(kind).unwrap_or(&0);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while *held.get(kind).unwrap_or(&0) == start {
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        held = crate::sync::wait_timeout_or_recover(released, held, left);
    }
    drop(held);
    crate::applog::applog(&format!(
        "[smoke-fault] released kind={}",
        canonical(kind).unwrap_or("?")
    ));
}
static COUNTS: LazyLock<Mutex<HashMap<&'static str, u8>>> = LazyLock::new(|| {
    let mut counts = HashMap::new();
    if let Some(kind) = crate::launch_args::debug_arg("--smoke-fault").and_then(|v| canonical(&v)) {
        counts.insert(kind, 1);
    }
    Mutex::new(counts)
});

pub(crate) fn enabled() -> bool {
    cfg!(debug_assertions)
        && crate::launch_args::debug_arg("--smoke-wkwebview").is_some()
        && crate::launch_args::debug_arg("--smoke-data-dir")
            .is_some_and(|root| std::path::Path::new(&root).is_absolute())
}

fn canonical(kind: &str) -> Option<&'static str> {
    KINDS.iter().copied().find(|candidate| *candidate == kind)
}

pub(crate) fn take(kind: &str) -> bool {
    if !enabled() {
        return false;
    }
    let Some(kind) = canonical(kind) else {
        return false;
    };
    let mut counts = COUNTS.lock_or_recover();
    let count = counts.entry(kind).or_default();
    if *count == 0 {
        return false;
    }
    *count -= 1;
    // Debug smoke only: a closed kind name proves an injected fault fired.
    crate::applog::applog(&format!("[smoke-fault] fired kind={kind}"));
    true
}

#[derive(Debug, Serialize)]
pub(crate) struct SmokeFaultState {
    kind: String,
    remaining: u8,
}

#[derive(Debug, Serialize)]
pub(crate) struct SmokeClipboardMetrics {
    bytes: usize,
    newlines: usize,
    hash: String,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

#[tauri::command]
pub(crate) fn smoke_fault_set(kind: String, count: u8) -> Result<SmokeFaultState, DeckError> {
    if !enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke fault hooks are unavailable",
        ));
    }
    let kind = canonical(&kind).ok_or(DeckError::new(ErrorKind::Other, "unknown smoke fault"))?;
    if count > 8 {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke fault count must be between 0 and 8",
        ));
    }
    COUNTS.lock_or_recover().insert(kind, count);
    if count == 0 {
        // disarming a stall kind also releases every hit still held by it
        let (epochs, released) = &*RELEASES;
        *epochs.lock_or_recover().entry(kind).or_default() += 1;
        released.notify_all();
    }
    Ok(SmokeFaultState {
        kind: kind.to_string(),
        remaining: count,
    })
}

fn clipboard_reader() -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/pbpaste");
    // LC_ALL also overrides an inherited LC_CTYPE=C. LANG alone does not.
    command.env("LC_ALL", "en_US.UTF-8");
    command
}

#[tauri::command]
pub(crate) fn smoke_clipboard_metrics() -> Result<SmokeClipboardMetrics, DeckError> {
    if !enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke clipboard metrics are unavailable",
        ));
    }
    let out = clipboard_reader()
        .output()
        .map_err(|_| DeckError::new(ErrorKind::Other, "clipboard reader unavailable"))?;
    if !out.status.success() {
        return Err(DeckError::new(ErrorKind::Other, "clipboard reader failed"));
    }
    Ok(SmokeClipboardMetrics {
        bytes: out.stdout.len(),
        newlines: out.stdout.iter().filter(|byte| **byte == b'\n').count(),
        hash: format!("{:016x}", fnv1a64(&out.stdout)),
    })
}

/// Whether the Board's persistent query client is attached: the
/// command-without-pane smoke must prove it ran in that topology.
#[tauri::command]
pub(crate) fn smoke_query_channel() -> Result<bool, DeckError> {
    if !enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke query channel state is unavailable",
        ));
    }
    Ok(crate::tmux::query_channel_connected())
}

/// The `signal-finish` smoke's deterministic fake agent
/// (`examples/signal_fixture.rs`, built by `app/run.sh` for that mode only
/// and never bundled): where it and this bundle's real status helper are,
/// and what its closed sentinels say. Sentinels live in the isolated smoke
/// data directory; nothing here spawns a process.
#[derive(Debug, Serialize)]
pub(crate) struct SmokeSignalFixture {
    fixture: String,
    helper: String,
    dir: String,
    started: bool,
    completed: bool,
    fixture_alive: bool,
    child_alive: bool,
}

fn fixture_process_alive(dir: &std::path::Path, name: &str) -> bool {
    fixture_named_process_alive(dir, name, "signal_fixture")
}

fn fixture_named_process_alive(dir: &std::path::Path, name: &str, executable: &str) -> bool {
    std::fs::read_to_string(dir.join(name))
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .is_some_and(|pid| {
            crate::procinfo::process_start(pid).is_some()
                && crate::procinfo::argv0(pid)
                    .is_some_and(|argv0| argv0.rsplit('/').next() == Some(executable))
        })
}

#[tauri::command]
pub(crate) fn smoke_signal_fixture() -> Result<SmokeSignalFixture, DeckError> {
    let unavailable = || DeckError::new(ErrorKind::Other, "signal fixture is unavailable");
    if !enabled() {
        return Err(unavailable());
    }
    let exe = std::env::current_exe().map_err(|_| unavailable())?;
    let macos = exe.parent().ok_or_else(unavailable)?;
    let helper = macos.join("deck-status-helper");
    // target/debug/deck-smoke.app/Contents/MacOS → target/debug/examples
    let fixture = macos
        .ancestors()
        .nth(3)
        .ok_or_else(unavailable)?
        .join("examples/signal_fixture");
    if !helper.is_file() || !fixture.is_file() {
        return Err(unavailable());
    }
    let dir = crate::datadir::deck_dir().join("signal-fixture");
    crate::datadir::create_private_dir(&dir)?;
    let text = |path: std::path::PathBuf| path.to_str().map(str::to_owned).ok_or_else(unavailable);
    Ok(SmokeSignalFixture {
        started: dir.join("STARTED").is_file(),
        completed: dir.join("COMPLETED").is_file(),
        fixture_alive: fixture_process_alive(&dir, "fixture.pid"),
        child_alive: fixture_process_alive(&dir, "child.pid"),
        fixture: text(fixture)?,
        helper: text(helper)?,
        dir: text(dir)?,
    })
}

/// What the `pi-live` smoke's page needs from its operator
/// (`scripts/pi-live-verify.py`): the command the operator wrote for the
/// run — it names the operator's own Pi and fixture provider inside the
/// experiment root — its working directory and the step text, and how often
/// the fixture provider was asked and Pi settled, counted from the fixture's
/// own event log, plus the models the operator lists for the worker type
/// (`models`, one per line, optional). Nothing else of that log is read
/// back, and this command starts nothing.
#[derive(Debug, Serialize)]
pub(crate) struct SmokePiFixture {
    command: String,
    dir: String,
    marker: String,
    models: Vec<String>,
    requests: u32,
    settled: u32,
}

#[tauri::command]
pub(crate) fn smoke_pi_fixture() -> Result<SmokePiFixture, DeckError> {
    let unavailable = || DeckError::new(ErrorKind::Other, "pi fixture is unavailable");
    if !enabled() {
        return Err(unavailable());
    }
    pi_fixture_in(&crate::datadir::deck_dir().join("pi-fixture")).ok_or_else(unavailable)
}

/// `smoke_pi_fixture` over the operator's directory.
fn pi_fixture_in(dir: &std::path::Path) -> Option<SmokePiFixture> {
    // one line the operator wrote: bounded, and nothing a terminal could misread
    let line = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| {
                !text.is_empty() && text.len() <= 2048 && !text.chars().any(char::is_control)
            })
    };
    let log = std::fs::read_to_string(dir.join("source.jsonl")).unwrap_or_default();
    let count = |event: &str| {
        let wanted = format!("\"event\":\"{event}\"");
        log.lines().filter(|line| line.contains(&wanted)).count() as u32
    };
    Some(SmokePiFixture {
        command: line("command")?,
        dir: line("dir")?,
        marker: line("marker")?,
        // one model per line: each bounded, and nothing a terminal could misread
        models: std::fs::read_to_string(dir.join("models"))
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|m| !m.is_empty() && m.len() <= 100 && !m.chars().any(char::is_control))
            .take(20)
            .map(str::to_owned)
            .collect(),
        requests: count("fixture_provider_request"),
        settled: count("agent_settled"),
    })
}

/// Closed, content-free state from the `channel-first-send` smoke's fake
/// `claude`. `app/run.sh` places the example in the signed carrier's closed
/// PATH, outside the hardened data tree; this command only reads its private
/// sentinels and never starts a process.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SmokeChannelFixture {
    available: bool,
    environment_private: bool,
    credentials_cleared: bool,
    keychain_blocked: bool,
    ready: bool,
    alive: bool,
    receipts: u8,
    first_matches: bool,
    second_seen: bool,
    last_at: u64,
    persisted_items: usize,
    persisted_deliveries: usize,
    persisted_overrides: usize,
}

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChannelReceipt {
    receipts: u8,
    first_matches: bool,
    second_seen: bool,
    last_at: u64,
}

/// Native-only completion oracle for the background channel smoke. Counts
/// only this isolated fixture's `claude` row and channel-first-send delivery;
/// the last count audits the missing-interaction override actually used by
/// this fixture; one delivered channel head must record one dependency.
pub(crate) type ChannelFixtureOracle = (u8, bool, bool, u64, usize, usize, usize);

pub(crate) fn channel_fixture_oracle() -> Result<ChannelFixtureOracle, DeckError> {
    let unavailable = || DeckError::new(ErrorKind::Other, "channel fixture is unavailable");
    if !enabled()
        || crate::launch_args::debug_arg("--smoke-wkwebview").as_deref()
            != Some("channel-first-send")
    {
        return Err(unavailable());
    }
    let root = crate::datadir::deck_dir();
    let receipt = std::fs::read(root.join("channel-fixture/receipt.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ChannelReceipt>(&bytes).ok())
        .unwrap_or_default();
    let queue = std::fs::read(root.join("queue.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let data = queue
        .as_ref()
        .and_then(|v| v.get("data"))
        .or(queue.as_ref());
    let persisted_items = data
        .and_then(|v| v.get("items"))
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter(|v| v.get("cmd").and_then(serde_json::Value::as_str) == Some("claude"))
                .count()
        })
        .unwrap_or(0);
    let deliveries = data
        .and_then(|v| v.get("deliveries"))
        .and_then(serde_json::Value::as_array);
    let persisted_deliveries = deliveries
        .map(|values| {
            values
                .iter()
                .filter(|v| {
                    v.get("channel_first_send")
                        .is_some_and(serde_json::Value::is_object)
                })
                .count()
        })
        .unwrap_or(0);
    let persisted_overrides = deliveries
        .map(|values| {
            values
                .iter()
                .filter(|v| {
                    v.get("readiness_overridden")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                })
                .count()
        })
        .unwrap_or(0);
    Ok((
        receipt.receipts,
        receipt.first_matches,
        receipt.second_seen,
        receipt.last_at,
        persisted_items,
        persisted_deliveries,
        persisted_overrides,
    ))
}

#[tauri::command]
pub(crate) fn smoke_channel_fixture() -> Result<SmokeChannelFixture, DeckError> {
    let unavailable = || DeckError::new(ErrorKind::Other, "channel fixture is unavailable");
    if !enabled()
        || crate::launch_args::debug_arg("--smoke-wkwebview").as_deref()
            != Some("channel-first-send")
    {
        return Err(unavailable());
    }
    let root = crate::datadir::deck_dir();
    crate::datadir::write_private(
        &root.join("channel-app.pid"),
        std::process::id().to_string().as_bytes(),
    )?;
    let fixture = crate::launch_args::channel_fixture_bin().map(|bin| bin.join("claude"));
    let dir = root.join("channel-fixture");
    let (
        receipts,
        first_matches,
        second_seen,
        last_at,
        persisted_items,
        persisted_deliveries,
        persisted_overrides,
    ) = channel_fixture_oracle()?;
    let environment_private = std::env::var_os("HOME").is_some_and(|v| v == root.join("home"))
        && std::env::var_os("ZDOTDIR").is_some_and(|v| v == root.join("home"))
        && std::env::var_os("TMPDIR").is_some_and(|v| v == root.join("tmp"))
        && fixture.as_ref().is_some_and(|fixture| {
            std::env::var("PATH").ok().is_some_and(|value| {
                value == format!("{}:/usr/bin:/bin", fixture.parent().unwrap().display())
            })
        });
    let credentials_cleared = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "CODEX_API_KEY",
        "OPENAI_ORG_ID",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GOOGLE_API_KEY",
        "GEMINI_API_KEY",
    ]
    .iter()
    .all(|name| std::env::var_os(name).is_none());
    Ok(SmokeChannelFixture {
        available: fixture.as_ref().is_some_and(|path| path.is_file()),
        environment_private,
        credentials_cleared,
        // keychain.rs refuses system Keychain I/O under this same gate.
        keychain_blocked: enabled(),
        ready: dir.join("READY").is_file(),
        alive: fixture_named_process_alive(&dir, "fixture.pid", "claude"),
        receipts,
        first_matches,
        second_seen,
        last_at,
        persisted_items,
        persisted_deliveries,
        persisted_overrides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_protocol_is_a_closed_enum_with_a_small_count() {
        assert_eq!(canonical("board-save"), Some("board-save"));
        assert_eq!(canonical("settings-save"), Some("settings-save"));
        assert_eq!(canonical("channel-network"), Some("channel-network"));
        assert_eq!(canonical("channel-scope"), Some("channel-scope"));
        assert_eq!(
            canonical("connector-after-accept"),
            Some("connector-after-accept")
        );
        assert_eq!(canonical("tmux-after-stop"), Some("tmux-after-stop"));
        assert_eq!(
            canonical("tmux-after-metadata"),
            Some("tmux-after-metadata")
        );
        assert_eq!(canonical("shell"), None);
        assert_eq!(canonical("/tmp/file"), None);
        assert_eq!(canonical("queue-save; rm"), None);
    }

    #[test]
    fn clipboard_hash_is_stable_and_byte_oriented() {
        assert_eq!(fnv1a64(b"DEFG"), 0xbab10472a66bfe51);
        assert_ne!(fnv1a64("中".as_bytes()), fnv1a64(b"?"));
    }

    #[test]
    fn clipboard_reader_overrides_missing_and_non_utf8_parent_locales() {
        // Inspect the child override without reading the user's pasteboard
        // or changing this test process's environment. LC_ALL wins over
        // either LC_CTYPE or LANG inherited from a C-locale shell.
        let command = clipboard_reader();
        assert_eq!(command.get_program(), "/usr/bin/pbpaste");
        assert!(command.get_envs().any(|(name, value)| {
            name == "LC_ALL" && value == Some(std::ffi::OsStr::new("en_US.UTF-8"))
        }));
    }

    #[test]
    fn normal_test_process_cannot_arm_or_consume_packaged_smoke_hooks() {
        assert!(!enabled());
        for kind in KINDS {
            assert!(!take(kind));
            assert_eq!(
                smoke_fault_set((*kind).to_string(), 1).unwrap_err(),
                "smoke fault hooks are unavailable"
            );
        }
        assert!(!take("unknown"));
        assert_eq!(
            smoke_clipboard_metrics().unwrap_err(),
            "smoke clipboard metrics are unavailable"
        );
        assert_eq!(
            smoke_signal_fixture().unwrap_err(),
            "signal fixture is unavailable"
        );
        assert_eq!(smoke_pi_fixture().unwrap_err(), "pi fixture is unavailable");
    }

    #[test]
    fn the_pi_fixture_is_three_operator_lines_and_two_counts() {
        let dir = std::env::temp_dir().join(format!("deck-pi-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(pi_fixture_in(&dir).is_none(), "nothing the operator wrote");
        std::fs::write(dir.join("command"), "pi --no-tools -e /tmp/x/fixture.ts\n").unwrap();
        std::fs::write(dir.join("dir"), "/tmp/x/work\n").unwrap();
        std::fs::write(dir.join("marker"), "deck-pi-live-step-1\n").unwrap();
        let fixture = pi_fixture_in(&dir).expect("the operator's three lines");
        assert_eq!(fixture.command, "pi --no-tools -e /tmp/x/fixture.ts");
        assert_eq!(
            (fixture.dir.as_str(), fixture.marker.as_str()),
            ("/tmp/x/work", "deck-pi-live-step-1")
        );
        assert_eq!((fixture.requests, fixture.settled), (0, 0), "no log yet");
        assert!(fixture.models.is_empty(), "no models listed");
        std::fs::write(dir.join("models"), "minimax/MiniMax-M3\n\nopenai/gpt-5.6\n").unwrap();
        assert_eq!(
            pi_fixture_in(&dir).unwrap().models,
            ["minimax/MiniMax-M3", "openai/gpt-5.6"]
        );
        std::fs::write(
            dir.join("source.jsonl"),
            "{\"event\":\"session_start\",\"pid\":7,\"id\":\"private\"}\n\
             {\"event\":\"fixture_provider_request\",\"pid\":7}\n\
             {\"event\":\"agent_settled\",\"pid\":7,\"aborted\":false}\n\
             {\"event\":\"fixture_provider_request\",\"pid\":7}\n",
        )
        .unwrap();
        let fixture = pi_fixture_in(&dir).unwrap();
        assert_eq!((fixture.requests, fixture.settled), (2, 1));
        // only the six fields leave: nothing of the log's content
        let sent = serde_json::to_value(&fixture).unwrap();
        assert_eq!(sent.as_object().unwrap().len(), 6);
        assert!(!sent.to_string().contains("private"));
        // a line a terminal could misread, or an oversized one, is no fixture
        for bad in ["pi\u{1b}[2J", &"p".repeat(2049), "  \n"] {
            std::fs::write(dir.join("command"), bad).unwrap();
            assert!(pi_fixture_in(&dir).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// What the `ipc-stall` smoke judges after a stall is released: for
/// `session`, its pane (tty) clients on the smoke server and whether Deck
/// holds an attachment / an attach in flight for it; and in all, the
/// sessions on the server, attach and start/kill workers not yet returned,
/// and reapers that have not reaped their client.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SmokeAttachState {
    pane_clients: usize,
    attached: bool,
    pending: bool,
    attach_workers: usize,
    session_workers: usize,
    reaping: usize,
    sessions: usize,
}

#[tauri::command]
pub(crate) fn smoke_attach_state(
    app: tauri::AppHandle,
    session: String,
) -> Result<SmokeAttachState, DeckError> {
    use tauri::Manager;
    if !enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke attach state is unavailable",
        ));
    }
    crate::tmux::validate_session_name(&session)?;
    let _bounded = crate::session_runtime::Deadline::until(
        std::time::Instant::now() + std::time::Duration::from_secs(2),
    );
    let target = crate::tmux::session_target(&session);
    let pane_clients = crate::tmux::tmux(&["list-clients", "-t", &target, "-F", "#{client_flags}"])
        .map(|out| {
            out.lines()
                .filter(|flags| !flags.contains("control-mode"))
                .count()
        })
        .unwrap_or(0);
    let sessions = crate::tmux::tmux(&["list-sessions", "-F", "s"])
        .map(|out| out.lines().count())
        .unwrap_or(0);
    let (attached, pending, attach_workers, reaping) =
        crate::pty::smoke_counts(&app.state::<crate::pty::PtyState>(), &session);
    Ok(SmokeAttachState {
        pane_clients,
        attached,
        pending,
        attach_workers,
        session_workers: crate::commands::session_workers(),
        reaping,
        sessions,
    })
}

/// The smoke server's pid, read before it was stopped (a stopped server
/// answers nothing).
static STOPPED_SERVER: Mutex<Option<i32>> = Mutex::new(None);

/// Stop (`SIGSTOP`) or continue (`SIGCONT`) the smoke's OWN tmux server,
/// standing in for a tmux that does not answer. The pid is read from the
/// server itself on its smoke socket (`deck-smoke…`, never `deck` or
/// `deck-dev`) and remembered, so the continue signals exactly that pid.
#[tauri::command]
pub(crate) fn smoke_tmux_server_signal(stop: bool) -> Result<i32, DeckError> {
    if !enabled() || !crate::tmux::socket().starts_with("deck-smoke") {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke server signal is unavailable",
        ));
    }
    let mut stopped = STOPPED_SERVER.lock_or_recover();
    let pid = if stop {
        let _bounded = crate::session_runtime::Deadline::until(
            std::time::Instant::now() + std::time::Duration::from_secs(2),
        );
        let out = crate::tmux::tmux(&["display-message", "-p", "#{pid} #{socket_path}"])?;
        let mut words = out.split_whitespace();
        let pid: i32 = words
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or(DeckError::new(
                ErrorKind::Other,
                "smoke server pid unreadable",
            ))?;
        let socket = words.next().unwrap_or_default();
        if !socket.ends_with(&format!("/{}", crate::tmux::socket())) || pid <= 1 {
            return Err(DeckError::new(
                ErrorKind::Other,
                "smoke server is not the smoke socket's",
            ));
        }
        *stopped = Some(pid);
        pid
    } else {
        stopped.take().ok_or(DeckError::new(
            ErrorKind::Other,
            "no smoke server was stopped",
        ))?
    };
    // SAFETY: a signal to the smoke server's own verified pid.
    let rc = unsafe { libc::kill(pid, if stop { libc::SIGSTOP } else { libc::SIGCONT }) };
    if rc != 0 {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke server signal failed",
        ));
    }
    crate::applog::applog(&format!(
        "[smoke-fault] tmux server {}",
        if stop { "stopped" } else { "continued" }
    ));
    Ok(pid)
}
