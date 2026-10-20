//! Bounded, content-free coordination for an intentional shell-service restart.
//! Exit keys are guarded by the observed tmux generation and foreground name.
//! A timeout before kill-server aborts: elapsed time never proves a saved resume.
//! Order: what may refuse the restart for the shells' sake (their snapshots)
//! runs BEFORE the first exit key; after it only identity and safety checks
//! may still refuse (a pane lost or added, another server, the re-entry
//! seal, the queue pause, the deadline). The exited agents' resume hints are
//! read best effort and an exited agent's pane may change foreground
//! (`tmux::unchanged_after_exit`): refusing there would only lose the agents.
//! The lifecycle supplies the pane reader so a verified, reachable empty
//! server can be checked without treating tmux's missing pane target as loss,
//! and the key sender of the same server handle.
//! A client that runs under its release's file name (tmux then shows a
//! version, and argv[0] is a path — every client Deck's own re-entry
//! started, and one started through a path or a wrapper) is exited as an
//! agent only by its running code: the re-entry transaction froze that
//! exact pane as a direct client of the product (`frozen`), or the pane's
//! non-shell foreground leader runs valid code of a re-entry adapter's
//! product right now (`running_agent`, one dynamic validation under the
//! prepare deadline, its generation checked before and after). Never by a
//! version-shaped name; a late or unreadable answer sends no key.
use std::time::{Duration, Instant};

use crate::applog::{applog, session_tag};
use crate::error::{DeckError, ErrorKind, RestartFailure};
use crate::session_runtime::{check_deadline, Deadline};
use crate::tmux::{same_pane, unchanged_rows, PaneRow};

pub(crate) const PREPARE_BUDGET: Duration = Duration::from_secs(3);
pub(crate) const TOTAL_BUDGET: Duration = Duration::from_secs(8);
const EXIT_BUDGET: Duration = Duration::from_secs(2);

fn error(code: &str) -> DeckError {
    DeckError::new(ErrorKind::Tmux, code)
}

/// Only these closed reasons may enter logs; all other IO failures keep their
/// existing redacted error category. Never log terminal text or raw messages.
pub(crate) fn failure_reason(error: &DeckError) -> &'static str {
    error
        .restart_failure()
        .map_or_else(|| error.code(), RestartFailure::log_reason)
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
/// Which agent the process `pid` born at `birth` is by its running code:
/// each re-entry adapter's product, asked in turn (`procinfo::runs_product`,
/// under the caller's deadline). `None` for anything else, and for an
/// answer that did not come in time.
fn running_agent(pid: u32, birth: (u64, u32)) -> Option<Agent> {
    use crate::agent_reentry::{claude, codex};
    [
        (claude::CAPABILITY.product, Agent::Claude),
        (codex::CAPABILITY.product, Agent::Codex),
    ]
    .into_iter()
    .find(|(product, _)| crate::procinfo::runs_product(pid, birth, product))
    .map(|(_, kind)| kind)
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
            return Err(DeckError::restart(RestartFailure::ImpactChanged));
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
            return Err(DeckError::restart(RestartFailure::AgentTimeout));
        }
        check_deadline()?;
        let current = list()?;
        for (row, _) in &targets {
            if !current.iter().any(|now| same_pane(row, now)) {
                return Err(DeckError::restart(RestartFailure::PaneLost));
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
            return Err(DeckError::restart(RestartFailure::AgentTimeout));
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
    frozen: &[PaneRow],
    list: &dyn Fn() -> Result<Vec<PaneRow>, DeckError>,
    send: &dyn Fn(&[String]) -> Result<String, DeckError>,
    progress: &dyn Fn(&str, usize, usize),
) -> Result<Prepared, DeckError> {
    check_deadline()?;
    // The exit keys below may end a shell (a second Ctrl+D landing after its
    // agent already quit). That is Deck's own keystroke, not the user's, so
    // no shell that ends from here on may authorize card retirement.
    crate::commands::forget_exit_identities();
    let started = Instant::now();
    let rows = list()?;
    if !unchanged_rows(reviewed, &rows) {
        return Err(DeckError::restart(RestartFailure::ImpactChanged));
    }
    let processes = crate::procinfo::processes();
    let mut targets = Vec::new();
    let mut by_code = 0usize;
    for row in &rows {
        let leader = crate::procinfo::tty_device(&row.tty)
            .and_then(|device| crate::procinfo::foreground_leader(&processes, device));
        let argv = leader
            .and_then(crate::procinfo::argv0)
            .and_then(|s| crate::context::sanitize_process(&s));
        // What the re-entry transaction froze for this exact pane and
        // foreground name — a client qualified by its running code — not a
        // guess from the name itself.
        let reentered = frozen
            .iter()
            .any(|known| same_pane(known, row) && known.command == row.command)
            .then_some(Agent::Claude);
        // Any other foreground that is not a shell: the product by its
        // running code, or nothing (a client Deck itself re-entered runs
        // under its release's file name, and so may one the user started
        // through a path or a wrapper).
        let running = || {
            if crate::context::shell_process(Some(&row.command)) {
                return None;
            }
            let info = processes.get(&leader?)?;
            let found = running_agent(info.pid, (info.start_seconds, info.start_micros));
            by_code += usize::from(found.is_some());
            found
        };
        if let Some(kind) = agent(&row.command, argv.as_deref())
            .or(reentered)
            .or_else(running)
        {
            // Only a sanitized executable may enter the tmux format expression.
            if crate::context::sanitize_process(&row.command).as_deref() != Some(&row.command) {
                return Err(error("tmux-restart-agent-unrecognized"));
            }
            targets.push((row.clone(), kind));
        }
    }
    let total = targets.len();
    applog(&format!(
        "[tmux-restart] classified panes={} agents={} by_code={} other_foreground={}",
        rows.len(),
        total,
        by_code,
        rows.iter()
            .filter(|r| !crate::context::shell_process(Some(&r.command)))
            .count()
            .saturating_sub(total)
    ));
    let exiting: Vec<PaneRow> = targets.iter().map(|(row, _)| row.clone()).collect();
    // Everything that may refuse the restart for the shells' sake runs
    // BEFORE the first exit key: an agent that exits cannot be brought
    // back by refusing afterwards.
    let epoch = if save_shells {
        let epoch = crate::shell_state::begin_restart_checkpoint();
        progress("saving", 0, rows.len().saturating_sub(total));
        crate::shell_state::checkpoint_before_restart(&rows, &exiting, epoch, list, send, progress)
            .map_err(|e| {
                applog(&format!(
                    "[tmux-restart] snapshot-failed code={} reason={} elapsed_ms={}",
                    e.code(),
                    crate::shell_state::restart_snapshot_reason(&e),
                    started.elapsed().as_millis()
                ));
                DeckError::restart(RestartFailure::SnapshotFailed)
            })?;
        Some(epoch)
    } else {
        None
    };
    exit_agents(targets, EXIT_BUDGET, list, send, progress)?;
    if total > 0 && !save_shells {
        std::thread::sleep(Duration::from_millis(100));
    }
    check_deadline()?;
    let final_rows = list()?;
    if rows.len() != final_rows.len()
        || rows
            .iter()
            .any(|row| !final_rows.iter().any(|now| same_pane(row, now)))
    {
        return Err(DeckError::restart(RestartFailure::ImpactChanged));
    }
    // After the exit only the resume hints are read, best effort: a pane
    // that cannot be read is skipped, never a refusal (`shell_state`).
    if let Some(epoch) = epoch.filter(|_| total > 0) {
        let exited: Vec<PaneRow> = final_rows
            .iter()
            .filter(|now| exiting.iter().any(|agent| same_pane(agent, now)))
            .cloned()
            .collect();
        crate::shell_state::checkpoint_exited_agents(&exited, &final_rows, epoch, list, send);
    }
    applog(&format!(
        "[tmux-restart] prepared agents={total} restore_enabled={save_shells} elapsed_ms={}",
        started.elapsed().as_millis()
    ));
    check_deadline()?;
    Ok(Prepared {
        rows: final_rows,
        exited: exiting,
    })
}

/// What `prepare` leaves for the replacement: the panes as read after the
/// exit, and the agents it exited, whose foreground is expected to differ
/// from the reviewed listing (`tmux::unchanged_after_exit`).
#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) rows: Vec<PaneRow>,
    pub(crate) exited: Vec<PaneRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The logged reason comes from the closed RestartFailure, never from the
    /// message text; the text itself is unchanged for the user.
    #[test]
    fn failure_reason_reads_the_closed_restart_failure() {
        use crate::error::RestartFailure::*;
        for (failure, message, reason) in [
            (AgentTimeout, "tmux-restart-agent-timeout", "agent-timeout"),
            (
                SnapshotFailed,
                "tmux-restart-snapshot-failed",
                "snapshot-failed",
            ),
            (Deadline, "tmux-restart-timeout", "deadline"),
            (DeliveryBusy, "tmux-restart-busy", "delivery-busy"),
            (PaneLost, "tmux-restart-pane-lost", "pane-lost"),
            (
                ImpactChanged,
                "tmux-server-impact-changed",
                "impact-changed",
            ),
        ] {
            let error = DeckError::restart(failure);
            assert_eq!(error.message(), message);
            assert_eq!(error.kind(), ErrorKind::Tmux);
            assert_eq!(failure_reason(&error), reason);
        }
        let text_only = DeckError::new(ErrorKind::Tmux, "tmux-restart-timeout");
        assert_eq!(
            failure_reason(&text_only),
            "tmux",
            "text is not a reason code"
        );
        assert_eq!(
            failure_reason(&DeckError::new(ErrorKind::Perm, "x")),
            "perm"
        );
    }
    use crate::session_runtime::bounded_output;
    use crate::tmux;
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
        // One budget for all three: a per-agent budget would take at least
        // three of them, so the bound below discriminates while leaving room
        // for coverage instrumentation and a loaded machine.
        let budget = Duration::from_millis(400);
        let start = Instant::now();
        let err = exit_agents(
            targets,
            budget,
            &|| server.rows(),
            &|args| server.run(&args.iter().map(String::as_str).collect::<Vec<_>>()),
            &|_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.message(), "tmux-restart-agent-timeout");
        assert!(
            start.elapsed() < budget * 5 / 2,
            "budget is shared, not per agent"
        );
        assert_eq!(server.rows().unwrap().len(), 3);
    }

    fn unique(name: &str) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn send_on<'a>(
        server: &'a Server,
        sent: &'a std::cell::RefCell<Vec<String>>,
    ) -> impl Fn(&[String]) -> Result<String, DeckError> + 'a {
        move |args: &[String]| {
            sent.borrow_mut().push(args.join(" "));
            server.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
        }
    }

    /// A shell snapshot that cannot be taken refuses the restart BEFORE any
    /// exit key: the agent is still running and can still be re-entered.
    #[test]
    fn a_shell_snapshot_failure_refuses_before_any_exit_key() {
        use crate::sync::LockRecover;
        // the snapshot epoch is process-wide; one restart at a time, as in production
        let _epoch = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let server = Server::new();
        let agent = unique("rx-agent");
        let plain = unique("rx-plain");
        server.fixture(&agent, 2);
        server
            .run(&["new-session", "-d", "-s", &plain, "/bin/sh"])
            .unwrap();
        let reviewed = server.rows().unwrap();
        let frozen: Vec<PaneRow> = reviewed
            .iter()
            .filter(|r| r.session_name == agent)
            .cloned()
            .collect();
        // the second listing (phase one's own check) sees the plain pane changed
        let reads = std::cell::Cell::new(0);
        let list = || {
            reads.set(reads.get() + 1);
            let mut rows = server.rows()?;
            if reads.get() == 2 {
                for row in rows.iter_mut().filter(|r| r.session_name == plain) {
                    row.command = "vim".into();
                }
            }
            Ok(rows)
        };
        let sent = std::cell::RefCell::new(Vec::new());
        let error = prepare(
            &reviewed,
            true,
            &frozen,
            &list,
            &send_on(&server, &sent),
            &|_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(error.message(), "tmux-restart-snapshot-failed");
        assert!(
            sent.borrow().iter().all(|args| !args.contains("C-d")),
            "no exit key was sent: {:?}",
            sent.borrow()
        );
        assert!(server
            .rows()
            .unwrap()
            .iter()
            .any(|r| r.session_name == agent && r.command == "dd"));
    }

    /// After the agent exited, its pane may run something of its own for a
    /// moment. That is neither a refusal nor a reason to keep an older
    /// snapshot: the hint is skipped and the stale file removed.
    #[test]
    fn an_exited_agent_pane_that_changes_foreground_is_skipped_not_refused() {
        use crate::sync::LockRecover;
        // the snapshot epoch is process-wide; one restart at a time, as in production
        let _epoch = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let server = Server::new();
        let agent = unique("rx-flicker");
        server.fixture(&agent, 2);
        crate::shell_state::seed_snapshot_for_test(&agent, "/tmp", "before the agent started");
        let reviewed = server.rows().unwrap();
        let frozen = reviewed.clone();
        // once the pane reads as a shell, every later read shows a helper
        let back = std::cell::Cell::new(false);
        let list = || {
            let mut rows = server.rows()?;
            for row in rows.iter_mut().filter(|r| r.session_name == agent) {
                if back.get() {
                    row.command = "git".into();
                } else if crate::context::shell_process(Some(&row.command)) {
                    back.set(true);
                }
            }
            Ok(rows)
        };
        let sent = std::cell::RefCell::new(Vec::new());
        let prepared = prepare(
            &reviewed,
            true,
            &frozen,
            &list,
            &send_on(&server, &sent),
            &|_, _, _| {},
        )
        .expect("the restart goes on");
        assert!(sent.borrow().iter().any(|args| args.contains("C-d")));
        assert_eq!(prepared.exited.len(), 1);
        assert!(
            crate::shell_state::load_snapshot(&agent).unwrap().is_none(),
            "the stale snapshot is gone"
        );
        // the final comparison accepts the exited pane's foreground changing
        // again (helper → shell), but not a changed pane identity
        let mut now = list().unwrap();
        for row in now.iter_mut().filter(|r| r.session_name == agent) {
            row.command = "sh".into();
        }
        assert!(!unchanged_rows(&prepared.rows, &now));
        assert!(crate::tmux::unchanged_after_exit(
            &prepared.rows,
            &now,
            &prepared.exited
        ));
        let mut other = now.clone();
        other[0].pane_pid += 1;
        assert!(!crate::tmux::unchanged_after_exit(
            &prepared.rows,
            &other,
            &prepared.exited
        ));
    }

    /// The ordinary path: the exited agent's resume hint is what is saved.
    #[test]
    fn an_exited_agents_resume_hint_is_saved_after_the_exit() {
        use crate::sync::LockRecover;
        // the snapshot epoch is process-wide; one restart at a time, as in production
        let _epoch = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let server = Server::new();
        let agent = unique("rx-hint");
        server.fixture(&agent, 2);
        let reviewed = server.rows().unwrap();
        let sent = std::cell::RefCell::new(Vec::new());
        prepare(
            &reviewed,
            true,
            &reviewed,
            &|| server.rows(),
            &send_on(&server, &sent),
            &|_, _, _| {},
        )
        .expect("prepared");
        let saved = crate::shell_state::load_snapshot(&agent)
            .unwrap()
            .expect("hint saved");
        assert!(saved.transcript.contains("RESUME-FIXTURE"));
    }

    #[test]
    fn unchanged_after_exit_relaxes_only_the_exited_panes_foreground() {
        let row = |pane: &str, command: &str| PaneRow {
            server_pid: 1,
            session_id: "$1".into(),
            session_name: pane.into(),
            pane_id: pane.into(),
            pane_pid: 9,
            command: command.into(),
            ..PaneRow::default()
        };
        let before = [row("%1", "zsh"), row("%2", "zsh")];
        let exited = [row("%1", "claude")];
        assert!(crate::tmux::unchanged_after_exit(
            &before,
            &[row("%1", "git"), row("%2", "zsh")],
            &exited
        ));
        assert!(!crate::tmux::unchanged_after_exit(
            &before,
            &[row("%1", "zsh"), row("%2", "vim")],
            &exited
        ));
        assert!(!crate::tmux::unchanged_after_exit(
            &before,
            &[row("%1", "zsh")],
            &exited
        ));
        assert!(!crate::tmux::unchanged_after_exit(
            &before,
            &[row("%1", "zsh"), row("%3", "zsh")],
            &exited
        ));
    }

    #[test]
    fn restart_snapshot_reasons_are_closed() {
        use crate::shell_state::restart_snapshot_reason as reason;
        for (message, expected) in [
            ("tmux-restart-output-unstable", "unstable"),
            ("tmux-restart-snapshot-cancelled", "cancelled"),
            ("tmux-restart-snapshot-target-changed", "target-changed"),
            ("tmux-restart-snapshot-invalid-cwd", "invalid-cwd"),
        ] {
            assert_eq!(
                reason(&DeckError::new(ErrorKind::Recovery, message)),
                expected
            );
        }
        assert_eq!(
            reason(&DeckError::restart(RestartFailure::Deadline)),
            "deadline"
        );
        assert_eq!(
            reason(&DeckError::new(ErrorKind::Perm, "/private/path")),
            "perm"
        );
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

    /// A foreground that is neither named as an agent, nor frozen, nor
    /// either product by its running code gets no exit key, even with the
    /// running-code question asked under a live deadline.
    #[test]
    fn an_unnamed_unfrozen_foreground_that_is_no_product_gets_no_key() {
        use crate::sync::LockRecover;
        let _epoch = crate::shell_state::TRACKER_TEST_LOCK.lock_or_recover();
        let _phase = crate::session_runtime::test_phase();
        let server = Server::new();
        let other = unique("rx-other");
        server.fixture(&other, 2);
        let reviewed = server.rows().unwrap();
        let sent = std::cell::RefCell::new(Vec::new());
        let prepared = prepare(
            &reviewed,
            false,
            &[],
            &|| server.rows(),
            &send_on(&server, &sent),
            &|_, _, _| {},
        )
        .expect("the restart goes on");
        assert!(prepared.exited.is_empty());
        assert!(
            sent.borrow().iter().all(|args| !args.contains("C-d")),
            "no exit key: {:?}",
            sent.borrow()
        );
        // the test process itself is no product either, and a dead
        // generation is never one
        let me = std::process::id();
        let birth = crate::procinfo::process_start(me).unwrap();
        assert_eq!(running_agent(me, birth), None);
        assert_eq!(running_agent(me, (birth.0 + 1, birth.1)), None);
    }

    /// `running_agent` against one REAL running client the caller names
    /// (`DECK_RESTART_AGENT_PROBE`, a pid) and the kind it must be
    /// (`DECK_RESTART_AGENT_EXPECT`: `claude`, `codex` or `none`). Read-only;
    /// prints closed words and milliseconds only.
    #[test]
    #[ignore = "requires an explicitly named real process; sends nothing"]
    fn running_agent_on_a_named_real_process() {
        let _phase = crate::session_runtime::test_phase();
        let pid: u32 = std::env::var("DECK_RESTART_AGENT_PROBE")
            .expect("DECK_RESTART_AGENT_PROBE")
            .parse()
            .expect("a pid");
        let expect = match std::env::var("DECK_RESTART_AGENT_EXPECT").as_deref() {
            Ok("claude") => Some(Agent::Claude),
            Ok("codex") => Some(Agent::Codex),
            Ok("none") => None,
            other => panic!("DECK_RESTART_AGENT_EXPECT: {other:?}"),
        };
        let birth = crate::procinfo::process_start(pid).expect("the named process exists");
        let started = Instant::now();
        let found = running_agent(pid, birth);
        println!(
            "running_agent={found:?} ms={}",
            started.elapsed().as_millis()
        );
        assert_eq!(found, expect);
    }

    #[test]
    fn recognizes_only_supported_foregrounds_including_versioned_launchers() {
        assert_eq!(agent("2.1.277", Some("claude")), Some(Agent::Claude));
        assert_eq!(agent("codex", None), Some(Agent::Codex));
        assert_eq!(agent("node", Some("node")), None);
        assert_eq!(agent("zsh", Some("zsh")), None);
    }
}
