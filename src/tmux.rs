use anyhow::{bail, Result};
use std::collections::HashSet;
use std::process::Command;

pub fn available() -> bool {
    run(&["-V"]).is_ok()
}

pub fn inside_tmux() -> bool {
    std::env::var_os("TMUX").is_some()
}

/// Run tmux with output captured (never inherit stdio — stray stderr corrupts the TUI).
fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux").args(args).output()?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Exact-match target for session-level commands (kill-session, switch-client, attach).
pub fn session_target(name: &str) -> String {
    format!("={name}")
}

/// Exact-match target for pane-level commands (send-keys, capture-pane).
/// tmux parses `=name` alone as a pane lookup and fails; `=name:` selects
/// the session's active window/pane.
fn pane_target(name: &str) -> String {
    format!("={name}:")
}

/// Names of all live tmux sessions.
pub fn sessions() -> HashSet<String> {
    match run(&["list-sessions", "-F", "#{session_name}"]) {
        Ok(out) => out.lines().map(|s| s.to_string()).collect(),
        Err(_) => HashSet::new(), // no server running
    }
}

/// Create a detached session running a login shell in `dir`, then type `command` into it.
/// Typing (rather than exec'ing the command directly) keeps the shell alive after the
/// agent exits, so the card's session survives and scrollback stays inspectable.
pub fn new_session(name: &str, dir: &str, command: &str) -> Result<()> {
    run(&["new-session", "-d", "-s", name, "-c", dir])?;
    if !command.trim().is_empty() {
        run(&["send-keys", "-t", &pane_target(name), command, "Enter"])?;
    }
    Ok(())
}

pub fn kill_session(name: &str) -> Result<()> {
    run(&["kill-session", "-t", &session_target(name)])?;
    Ok(())
}

/// Last `lines` non-empty lines of the session's active pane.
pub fn capture_tail(name: &str, lines: usize) -> Vec<String> {
    match run(&["capture-pane", "-p", "-t", &pane_target(name), "-S", "-40"]) {
        Ok(text) => {
            let non_empty: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.to_string())
                .collect();
            let skip = non_empty.len().saturating_sub(lines);
            non_empty.into_iter().skip(skip).collect()
        }
        Err(_) => Vec::new(),
    }
}

/// Switch the current tmux client to `name` (only valid when running inside tmux).
pub fn switch_client(name: &str) -> Result<()> {
    run(&["switch-client", "-t", &session_target(name)])?;
    Ok(())
}
