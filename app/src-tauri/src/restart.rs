//! Bounded, content-free coordination for an intentional shell-service restart.
//! Exit keys are guarded by the observed tmux generation and foreground name.
//! A timeout before kill-server aborts: elapsed time never proves a saved resume.
use std::time::{Duration, Instant};

use crate::applog::{applog, session_tag};
use crate::error::{DeckError, ErrorKind};
use crate::session_runtime::{check_deadline, Deadline};
use crate::tmux::{self, same_pane, unchanged_rows, PaneRow};

pub(crate) const PREPARE_BUDGET: Duration = Duration::from_secs(3);
pub(crate) const TOTAL_BUDGET: Duration = Duration::from_secs(8);
const EXIT_BUDGET: Duration = Duration::from_secs(2);

fn error(code: &str) -> DeckError {
    DeckError::new(ErrorKind::Tmux, code)
}

/// Only these closed reasons may enter logs; all other IO failures keep their
/// existing redacted error category. Never log terminal text or raw messages.
pub(crate) fn failure_reason(error: &DeckError) -> &'static str {
    match error.message() {
        "tmux-restart-agent-timeout" => "agent-timeout",
        "tmux-restart-snapshot-failed" => "snapshot-failed",
        "tmux-restart-timeout" => "deadline",
        "tmux-restart-busy" => "delivery-busy",
        "tmux-restart-pane-lost" => "pane-lost",
        "tmux-server-impact-changed" => "impact-changed",
        _ => error.code(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Agent {
    Claude,
    Codex,
}
fn agent(command: &str, argv: Option<&str>) -> Option<Agent> {
    match argv.unwrap_or(command) {
        "claude" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        _ => None,
    }
}
fn exit_keys(row: &PaneRow) -> Vec<String> {
    let actual = "#{pid}:#{session_id}:#{window_id}:#{pane_id}:#{pane_pid}:#{pane_current_command}";
    let expected = format!(
        "{}:{}:{}:{}:{}:{}",
        row.server_pid, row.session_id, row.window_id, row.pane_id, row.pane_pid, row.command
    );
    vec!["if-shell".into(), "-F".into(), "-t".into(), row.pane_id.clone(),
        format!("#{{==:{actual},{expected}}}"),
        // Leave copy mode before sending the key to the application.
        format!("if-shell -F -t {id} '#{{pane_in_mode}}' 'send-keys -X -t {id} cancel' ''; send-keys -t {id} C-d", id = row.pane_id),
        "display-message -p deck-restart-target-changed".into()]
}

fn exit_agents(
    mut targets: Vec<(PaneRow, Agent)>,
    budget: Duration,
    list: &dyn Fn() -> Result<Vec<PaneRow>, DeckError>,
    send: &dyn Fn(&[String]) -> Result<String, DeckError>,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<(), DeckError> {
    let started = Instant::now();
    let total = targets.len();
    progress("exiting", 0, total);
    let exit_end = Instant::now() + budget;
    let _exit_deadline = Deadline::until(exit_end);
    for (row, _) in &targets {
        let out = send(&exit_keys(row))?;
        if out.contains("deck-restart-target-changed") {
            return Err(error("tmux-server-impact-changed"));
        }
        applog(&format!(
            "[tmux-restart] exit-request {} elapsed_ms={}",
            session_tag(&row.session_name),
            started.elapsed().as_millis()
        ));
    }
    let mut second_sent = false;
    while !targets.is_empty() {
        if Instant::now() >= exit_end {
            applog(&format!(
                "[tmux-restart] exit-timeout remaining={} elapsed_ms={}",
                targets.len(),
                started.elapsed().as_millis()
            ));
            return Err(error("tmux-restart-agent-timeout"));
        }
        check_deadline()?;
        let current = list()?;
        for (row, _) in &targets {
            if !current.iter().any(|now| same_pane(row, now)) {
                return Err(error("tmux-restart-pane-lost"));
            }
        }
        targets.retain(|(row, _)| {
            !current
                .iter()
                .any(|now| same_pane(row, now) && crate::context::shell_process(Some(&now.command)))
        });
        progress("exiting", total - targets.len(), total);
        if targets.is_empty() {
            break;
        }
        if Instant::now() >= exit_end {
            applog(&format!(
                "[tmux-restart] exit-timeout remaining={} elapsed_ms={}",
                targets.len(),
                started.elapsed().as_millis()
            ));
            return Err(error("tmux-restart-agent-timeout"));
        }
        if !second_sent && started.elapsed() >= Duration::from_millis(100) {
            for (row, kind) in &targets {
                if *kind == Agent::Claude {
                    send(&exit_keys(row))?;
                }
            }
            second_sent = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    applog(&format!(
        "[tmux-restart] agents-exited count={total} elapsed_ms={}",
        started.elapsed().as_millis()
    ));
    Ok(())
}

/// All agents receive their first key before polling starts; every agent uses
/// the SAME exit deadline. Unknown applications receive no guessed keystrokes.
pub(crate) fn prepare(
    reviewed: &[PaneRow],
    save_shells: bool,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<Vec<PaneRow>, DeckError> {
    check_deadline()?;
    let started = Instant::now();
    let rows = tmux::list_panes()?;
    if !unchanged_rows(reviewed, &rows) {
        return Err(error("tmux-server-impact-changed"));
    }
    let processes = crate::procinfo::processes();
    let mut targets = Vec::new();
    for row in &rows {
        let argv = crate::procinfo::tty_device(&row.tty)
            .and_then(|device| crate::procinfo::foreground_leader(&processes, device))
            .and_then(crate::procinfo::argv0)
            .and_then(|s| crate::context::sanitize_process(&s));
        if let Some(kind) = agent(&row.command, argv.as_deref()) {
            // Only a sanitized executable may enter the tmux format expression.
            if crate::context::sanitize_process(&row.command).as_deref() != Some(&row.command) {
                return Err(error("tmux-restart-agent-unrecognized"));
            }
            targets.push((row.clone(), kind));
        }
    }
    let total = targets.len();
    applog(&format!(
        "[tmux-restart] classified panes={} agents={} other_foreground={}",
        rows.len(),
        total,
        rows.iter()
            .filter(|r| !crate::context::shell_process(Some(&r.command)))
            .count()
            .saturating_sub(total)
    ));
    exit_agents(
        targets,
        EXIT_BUDGET,
        &tmux::list_panes,
        &tmux::tmux_owned,
        progress,
    )?;
    if total > 0 && !save_shells {
        std::thread::sleep(Duration::from_millis(100));
    }
    check_deadline()?;
    let final_rows = tmux::list_panes()?;
    if rows.len() != final_rows.len()
        || rows
            .iter()
            .any(|row| !final_rows.iter().any(|now| same_pane(row, now)))
    {
        return Err(error("tmux-server-impact-changed"));
    }
    progress("saving", 0, final_rows.len());
    if save_shells {
        crate::shell_state::checkpoint_before_restart(&final_rows, progress).map_err(|e| {
            applog(&format!(
                "[tmux-restart] snapshot-failed code={} elapsed_ms={}",
                e.code(),
                started.elapsed().as_millis()
            ));
            error("tmux-restart-snapshot-failed")
        })?;
    }
    applog(&format!(
        "[tmux-restart] prepared agents={total} restore_enabled={save_shells} elapsed_ms={}",
        started.elapsed().as_millis()
    ));
    check_deadline()?;
    Ok(final_rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_runtime::bounded_output;
    use std::os::unix::fs::FileTypeExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Server(String);
    impl Server {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let server = Self(format!(
                "deck-test-exit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            server
                .run(&["start-server", ";", "set-option", "-g", "exit-empty", "off"])
                .unwrap();
            server
        }
        fn run(&self, args: &[&str]) -> Result<String, DeckError> {
            let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("binaries/tmux-aarch64-apple-darwin");
            let out = bounded_output(
                Command::new(bin)
                    .args(["-f", "/dev/null", "-L", &self.0])
                    .args(args),
                Instant::now() + Duration::from_secs(3),
            )?;
            if !out.status.success() {
                return Err(error(&String::from_utf8_lossy(&out.stderr)));
            }
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        fn rows(&self) -> Result<Vec<PaneRow>, DeckError> {
            Ok(self
                .run(&["list-panes", "-a", "-F", tmux::PANE_FORMAT])?
                .lines()
                .map(|s| tmux::parse_pane_row(s).unwrap())
                .collect())
        }
        fn socket_path(&self) -> Option<std::path::PathBuf> {
            self.run(&["display-message", "-p", "#{socket_path}"])
                .ok()
                .map(|path| std::path::PathBuf::from(path.trim()))
        }
        fn fixture(&self, name: &str, keys: usize) {
            // Raw-mode fake agent: consume N control bytes, print an exit hint,
            // then return to an ordinary shell. No real credentials/model calls.
            let cmd = format!("stty raw -echo; dd bs=1 count={keys} of=/dev/null 2>/dev/null; stty sane; printf \"\\nRESUME-FIXTURE\\n\"");
            self.run(&["new-session", "-d", "-s", name, "/bin/sh"])
                .unwrap();
            self.run(&["send-keys", "-t", name, "-l", &cmd]).unwrap();
            self.run(&["send-keys", "-t", name, "Enter"]).unwrap();
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                if self
                    .rows()
                    .unwrap()
                    .iter()
                    .any(|r| r.session_name == name && r.command == "dd")
                {
                    break;
                }
                assert!(Instant::now() < until, "fake agent became ready");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let socket = self.socket_path();
            let _ = self.run(&["kill-server"]);
            if let Some(path) = socket.filter(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(&self.0)
                    && std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_type().is_socket())
            }) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    #[test]
    fn real_tmux_exits_agents_together_leaves_copy_mode_and_never_sends_eof_to_shell() {
        let server = Server::new();
        server.fixture("codex", 1);
        server.fixture("claude", 2);
        server.run(&["copy-mode", "-t", "claude:"]).unwrap();
        let before = server.rows().unwrap();
        let targets = before
            .iter()
            .map(|r| {
                (
                    r.clone(),
                    if r.session_name == "claude" {
                        Agent::Claude
                    } else {
                        Agent::Codex
                    },
                )
            })
            .collect();
        let observations = std::cell::RefCell::new(Vec::new());
        exit_agents(
            targets,
            Duration::from_secs(2),
            &|| server.rows(),
            &|args| server.run(&args.iter().map(String::as_str).collect::<Vec<_>>()),
            &|_, done, total| observations.borrow_mut().push((done, total)),
        )
        .unwrap();
        assert_eq!(observations.borrow().last(), Some(&(2, 2)));
        for row in &before {
            let tail = server
                .run(&["capture-pane", "-p", "-t", &row.pane_id])
                .unwrap();
            assert!(
                tail.contains("RESUME-FIXTURE"),
                "exit hint is in tmux history"
            );
            let late = server
                .run(
                    &exit_keys(row)
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            assert!(late.contains("deck-restart-target-changed"));
        }
        assert!(server
            .rows()
            .unwrap()
            .iter()
            .all(|r| crate::context::shell_process(Some(&r.command))));
    }

    #[test]
    fn all_stalled_agents_share_one_timeout_and_their_panes_survive() {
        let server = Server::new();
        for name in ["one", "two", "three"] {
            server.fixture(name, 99);
        }
        let targets = server
            .rows()
            .unwrap()
            .into_iter()
            .map(|r| (r, Agent::Codex))
            .collect();
        let start = Instant::now();
        let err = exit_agents(
            targets,
            Duration::from_millis(150),
            &|| server.rows(),
            &|args| server.run(&args.iter().map(String::as_str).collect::<Vec<_>>()),
            &|_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.message(), "tmux-restart-agent-timeout");
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "budget is shared, not per agent"
        );
        assert_eq!(server.rows().unwrap().len(), 3);
    }

    #[test]
    fn replacement_or_rename_is_not_the_reviewed_pane() {
        let row = PaneRow {
            server_pid: 12,
            session_id: "$1".into(),
            session_name: "card".into(),
            pane_id: "%1".into(),
            pane_pid: 34,
            command: "sh".into(),
            ..PaneRow::default()
        };
        for changed in [
            PaneRow {
                server_pid: 13,
                ..row.clone()
            },
            PaneRow {
                session_name: "new-card".into(),
                ..row.clone()
            },
            PaneRow {
                command: "codex".into(),
                ..row.clone()
            },
        ] {
            assert!(!unchanged_rows(std::slice::from_ref(&row), &[changed]));
        }
    }

    #[test]
    fn recognizes_only_supported_foregrounds_including_versioned_launchers() {
        assert_eq!(agent("2.1.277", Some("claude")), Some(Agent::Claude));
        assert_eq!(agent("codex", None), Some(Agent::Codex));
        assert_eq!(agent("node", Some("node")), None);
        assert_eq!(agent("zsh", Some("zsh")), None);
    }
}
