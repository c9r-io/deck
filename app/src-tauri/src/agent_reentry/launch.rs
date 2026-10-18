//! The one terminal-input site of re-entry: a single, guarded, literal
//! launch request into a shell this operation created.
//!
//! # Contract
//! - The text is rebuilt from structured fields (`command_line`): the frozen
//!   absolute executable as one single-quoted shell word, then argument
//!   words from the adapter's closed set. No environment assignment, no
//!   prompt, no saved card command, no replayed argv. It travels on stdin
//!   into a private tmux buffer, never in process arguments or logs.
//! - The launch CONTEXT is a separate question from the input edge. Before
//!   the request this module establishes a bounded set of facts and no more:
//!   the baseline Deck hands its server (`supported_before_restart`), the
//!   one home variable tmux gives the pane (`baseline_home`, read by name,
//!   four distinct answers), and the input edge below. What the new shell's
//!   startup files export after that is NOT read and is not claimed: the
//!   request is typed in that accepted window. After the launch the new
//!   client is checked against what this operation asked for
//!   (`actual_context`, a closed answer from `procinfo::check_launch`); a
//!   mismatch means the entry is not confirmed — never that the start was
//!   undone.
//! - At the paste and again at the Enter the kernel is asked for the exact
//!   shell generation in the foreground (`edge_exact`), in addition to
//!   tmux's atomic pane/name/paste-mode guard. A same-named child, a reused
//!   pid or a program started meanwhile fails it: no paste, or no Enter.
//!   The two layers are not one atomic step; the remaining window is the
//!   few milliseconds between the kernel read and tmux's command.
//! - Input goes only to a clean shell of THIS operation whose input edge is
//!   positively verified (`wait_ready`): the exact new pane generation, not
//!   dead, not in copy mode, the pane's own process in the foreground, that
//!   process executing the user's login shell — which must be the system's
//!   own build of the supported shell — in the expected working directory,
//!   the pane's tmux environment handing over the expected home, and the
//!   line editor's bracketed-paste mode on. A
//!   live shell process, quiet time or any Signal is not that evidence. A
//!   shell that never shows it is refused; nothing is typed to find out, and
//!   no key is ever sent to clear, confirm or dismiss anything.
//! - Paste and Enter are two atomically guarded tmux commands on the same
//!   target fence (`prompt_delivery`). A refused paste proves no byte
//!   reached the pane (`Refused`). Anything after that — an indeterminate
//!   transport, a refused Enter with the text possibly in the editor — is
//!   `Unknown`: no second Enter, no Ctrl+C, no retry.
//! - `send_once` is called at most once per entry, after the caller consumed
//!   that entry's launch slot. It runs under the restart's exclusive
//!   activity guard and takes the guard as its witness.

use std::path::Path;
use std::time::{Duration, Instant};

use super::Env;
use crate::context::RawProbe;
use crate::error::{DeckError, ErrorKind};
use crate::procinfo::DirRef;
use crate::prompt_delivery::{self, LiteralOutcome, LiteralRequest};
use crate::tmux::PaneRow;

/// A shell this operation created, as listed right after its creation.
pub(crate) struct Target<'a> {
    pub(crate) row: &'a PaneRow,
    pub(crate) shell_birth: (u64, u32),
    pub(crate) cwd: &'a DirRef,
    /// The entry's adapter's baseline: what Deck handed its server carries
    /// the expected home and no name that adapter's grammar refuses.
    pub(crate) baseline: bool,
}

/// The file name of the one login shell whose line editor the launch sink
/// is written against (the system's own build of it), for every adapter.
const LOGIN_SHELL: &str = "zsh";

/// The single command line for `executable` and closed argument `words`:
/// one line, the path as one single-quoted word. `None` for a path that
/// cannot be quoted on one line or a word outside `[A-Za-z0-9-]`.
pub(crate) fn command_line(executable: &Path, words: &[String]) -> Option<String> {
    let path = executable.to_str().filter(|_| executable.is_absolute())?;
    if path.chars().any(char::is_control) {
        return None;
    }
    let mut line = format!("'{}'", path.replace('\'', r"'\''"));
    for word in words {
        if word.is_empty() || !word.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return None;
        }
        line.push(' ');
        line.push_str(word);
    }
    Some(line)
}

const READY_FORMAT: &str = "#{pid}\t#{session_id}\t#{window_id}\t#{pane_id}\t#{pane_pid}\t#{pane_dead}\t#{pane_in_mode}\t#{bracket_paste_flag}\t#{pane_current_command}";

/// The supported shell's name, when the user's login shell is that shell.
fn shell_name(env: &Env) -> Option<&'static str> {
    (env.login_shell.file_name().and_then(|name| name.to_str()) == Some(LOGIN_SHELL))
        .then_some(LOGIN_SHELL)
}

/// What is known about the launch before the restart, for every adapter:
/// the login shell is the system's own build of the supported shell. The
/// other condition, the baseline Deck hands over, is each adapter's own
/// (`Adapter::baseline`) and is read beside this. Shown in the preview and
/// required at freeze, so a setup that is known to be unsupported is never
/// discovered after the exit. It says nothing about the shell's startup
/// files: those run later and are the accepted window (module header).
pub(crate) fn supported_before_restart(env: &Env) -> bool {
    shell_name(env).is_some() && crate::procinfo::system_file(env.login_shell)
}

/// What one tmux environment scope says about one variable. The four
/// answers are different facts and are never folded into one another.
#[derive(Debug, PartialEq, Eq)]
enum Variable {
    Set(String),
    /// The scope marks it removed (`-NAME`): a pane gets no such variable,
    /// whatever an outer scope says.
    Removed,
    /// tmux positively answered that the scope does not mention it.
    Absent,
    /// The read failed or timed out, or answered in a shape this reader
    /// does not know (another line, more than one line): no fact.
    Unavailable,
}

/// One answer of `show-environment <scope> NAME` as a `Variable`. Only
/// tmux's own "unknown variable" for exactly this name is `Absent`; every
/// other failure — a timeout, an unreachable server, a missing session —
/// and every unexpected output is `Unavailable`.
fn variable_of(answer: Result<String, DeckError>, name: &str) -> Variable {
    let out = match answer {
        Ok(out) => out,
        Err(error)
            if error.message()
                == format!("tmux show-environment failed: unknown variable: {name}") =>
        {
            return Variable::Absent
        }
        Err(_) => return Variable::Unavailable,
    };
    let mut lines = out.lines();
    let (Some(line), None) = (lines.next(), lines.next()) else {
        return Variable::Unavailable;
    };
    if line == format!("-{name}") {
        Variable::Removed
    } else if let Some(value) = line.strip_prefix(&format!("{name}=")) {
        Variable::Set(value.to_string())
    } else {
        Variable::Unavailable
    }
}

/// One variable of the tmux environment, read by name. Never the whole
/// environment: the query names the variable and tmux answers for it alone.
fn tmux_variable(env: &Env, scope: &[&str], name: &str) -> Variable {
    let mut args = vec!["show-environment"];
    args.extend_from_slice(scope);
    args.push(name);
    variable_of((env.tmux)(&args), name)
}

/// The home tmux hands to a new pane of `session`, by tmux's own
/// inheritance rule: the session scope decides when it says anything, and
/// the global scope is consulted ONLY when the session scope positively
/// does not mention the variable. A removal does not fall back; neither
/// does a failed, timed-out or unreadable answer — those refuse the launch
/// before anything is typed. Exactly one variable is read, by name; no
/// other variable and no value but HOME's enters this module.
///
/// This is the baseline, not the launch context: what the shell's startup
/// files export afterwards is not visible here. tmux prints a value's
/// control characters in an escaped form, so two different values can print
/// alike; the post-launch check reads the client's own launch environment
/// from the kernel and does not depend on this spelling.
pub(super) fn baseline_home(env: &Env, session: &str, baseline: bool) -> bool {
    let target = crate::tmux::session_target(session);
    let home = match tmux_variable(env, &["-t", &target], "HOME") {
        Variable::Set(local) => Some(local),
        Variable::Absent => match tmux_variable(env, &["-g"], "HOME") {
            Variable::Set(global) => Some(global),
            Variable::Removed | Variable::Absent | Variable::Unavailable => None,
        },
        Variable::Removed | Variable::Unavailable => None,
    };
    baseline && home.as_deref().map(Path::new) == Some(env.home)
}

/// What the kernel says about the target's input edge at this instant: the
/// pane's own process generation holds the terminal's foreground (not a
/// child, not a later process under the same pid, not a program of the same
/// name), it executes the user's login shell, that file is the system's,
/// and its working directory is the frozen one.
fn edge_exact(env: &Env, target: &Target) -> bool {
    let row = target.row;
    crate::agent_status::live_generation(row.pane_pid).is_some_and(|foreground| {
        (foreground.pid(), foreground.birth()) == (row.pane_pid, target.shell_birth)
    }) && crate::procinfo::shell_facts(row.pane_pid, target.shell_birth, env.login_shell)
        .is_some_and(|facts| facts.image && facts.system && facts.cwd.as_ref() == Some(target.cwd))
}

/// One read-only look at the target's input edge.
fn ready(env: &Env, target: &Target) -> bool {
    let row = target.row;
    let Some(shell) = shell_name(env) else {
        return false;
    };
    let expected = format!(
        "{}\t{}\t{}\t{}\t{}\t0\t0\t1\t{shell}",
        row.server_pid, row.session_id, row.window_id, row.pane_id, row.pane_pid
    );
    let edge = (env.tmux)(&["display-message", "-p", "-t", &row.pane_id, READY_FORMAT]);
    edge.ok().as_deref().map(str::trim_end) == Some(expected.as_str())
        && edge_exact(env, target)
        && baseline_home(env, &row.session_name, target.baseline)
}

/// Wait, reading only, until the target's input edge is positively verified
/// or `end` passes.
pub(crate) fn wait_ready(env: &Env, target: &Target, end: Instant) -> bool {
    loop {
        if ready(env, target) {
            return true;
        }
        if Instant::now() + Duration::from_millis(50) >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The new client against what this operation asked for, once it exists:
/// the exact generation is bound to the frozen executable (its running image
/// validated against that file's code identity), was started with
/// exactly the request words this operation built, in a launch environment
/// with the expected home and no refused name, and is in the frozen
/// directory. The comparison happens inside procinfo; this module hands in
/// its own expectations and gets one closed answer back.
///
/// It is evidence of how that process was STARTED and where it is when
/// looked at (see `procinfo::check_launch` for which fact is of which
/// instant). It is not evidence of the configuration the client loaded, of
/// its effective permission, or that a startup file ran nothing else
/// before it. A startup file that changed the home, a config root or a
/// provider variable shows here, and such a client is never reported as a
/// confirmed entry — it has still been started.
pub(crate) fn actual_context(
    env: &Env,
    client: (u32, (u64, u32)),
    executable: &crate::procinfo::ExecutableRef,
    words: &[String],
    cwd: &DirRef,
    grammar: &crate::procinfo::LaunchGrammar,
) -> crate::procinfo::LaunchCheck {
    crate::procinfo::check_launch(
        client.0,
        client.1,
        &crate::procinfo::LaunchExpectation {
            executable,
            words,
            home: env.home,
            grammar,
            cwd,
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    /// A guard refused the paste: no byte reached the pane.
    Refused,
    /// Pasted and submitted with a separate guarded Enter.
    Submitted,
    /// The transport may have had an effect. Never retried.
    Unknown,
}

const REFUSED: &str = "deck-context-refused\n";

/// The transport of one launch. Before the paste and again before the Enter
/// it asks the kernel for the target's exact input edge and answers the
/// primitive's own refusal word when that no longer holds — so the paste
/// primitive sees a refused paste (nothing typed) or a refused Enter (no
/// second key), exactly as for tmux's own guard, which stays the second
/// layer inside the same command. The two layers are not one atomic step:
/// between this kernel read and tmux running the command a few milliseconds
/// remain in which the foreground could change; tmux's pane/name/paste-mode
/// guard covers that window by name and mode only, not by pid and birth.
struct EnvTransport<'a> {
    env: &'a Env<'a>,
    target: &'a Target<'a>,
}

impl prompt_delivery::Transport for EnvTransport<'_> {
    fn probe(&self, _session: &str) -> Result<RawProbe, DeckError> {
        // the name tmux compares atomically, from the row this operation
        // listed; the exact generation is asked for at each input below
        Ok(crate::context::probe_from_row(self.target.row))
    }
    fn run(&self, args: &[String]) -> Result<String, DeckError> {
        // The primitive runs two commands through here: the guarded Enter
        // and the cleanup of its private buffer. Everything that is not
        // that cleanup is treated as input and checked.
        let guarded_key = args.first().is_some_and(|verb| verb != "delete-buffer");
        if guarded_key && !edge_exact(self.env, self.target) {
            return Ok(REFUSED.to_string());
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        (self.env.tmux)(&args)
    }
    fn run_with_stdin(&self, args: &[String], input: &[u8]) -> Result<String, DeckError> {
        if !edge_exact(self.env, self.target) {
            return Ok(REFUSED.to_string());
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        (self.env.tmux_stdin)(&args, input)
    }
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The entry's one transport attempt. The caller has consumed the launch
/// slot; this function is never called twice for an entry.
pub(crate) fn send_once(
    env: &Env,
    target: &Target,
    entry: usize,
    line: &str,
    held: &std::sync::RwLockWriteGuard<'static, ()>,
) -> Sent {
    let Some(shell) = shell_name(env) else {
        return Sent::Refused;
    };
    let pane = crate::context::probe_from_row(target.row).identity;
    let delivery = format!("reentry-{}-{entry}", target.row.server_pid);
    let outcome = prompt_delivery::deliver_exclusive(
        LiteralRequest {
            session: &target.row.session_name,
            pane: &pane,
            expected_process: Some(shell),
            delivery: &delivery,
            text: line,
            submit: true,
            require_bracketed: true,
            require_paste_mode: true,
        },
        &EnvTransport { env, target },
        held,
    );
    match outcome {
        Ok(LiteralOutcome::Submitted) => Sent::Submitted,
        Err(error) if error.kind() == ErrorKind::ContextChanged => Sent::Refused,
        Ok(LiteralOutcome::Inserted | LiteralOutcome::EnterRefused) | Err(_) => Sent::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";

    fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| word.to_string()).collect()
    }

    #[test]
    fn the_line_is_one_quoted_absolute_path_and_closed_words() {
        let exe = Path::new("/Users/x/.local/share/claude/versions/2.1.294");
        assert_eq!(
            command_line(exe, &words(&["--resume", ID])).unwrap(),
            format!("'/Users/x/.local/share/claude/versions/2.1.294' --resume {ID}")
        );
        assert_eq!(
            command_line(exe, &words(&["--dangerously-skip-permissions", "--resume", ID])).unwrap(),
            format!(
                "'/Users/x/.local/share/claude/versions/2.1.294' --dangerously-skip-permissions --resume {ID}"
            )
        );
        // a path with spaces, quotes, dollars and backticks stays ONE word
        let odd = Path::new("/Users/o'brien/my $HOME `x`/2.1.294");
        assert_eq!(
            command_line(odd, &words(&["--resume", ID])).unwrap(),
            format!(r"'/Users/o'\''brien/my $HOME `x`/2.1.294' --resume {ID}")
        );
    }

    /// The four answers of one variable query stay four answers. tmux's own
    /// spellings are the ones the bundled tmux gives (the integration tests
    /// run the same reader against a real server).
    #[test]
    fn a_variable_query_is_set_removed_absent_or_unavailable_and_nothing_in_between() {
        let failed = |text: &str| Err(DeckError::new(ErrorKind::Tmux, text.to_string()));
        assert_eq!(
            variable_of(Ok("HOME=/Users/u\n".into()), "HOME"),
            Variable::Set("/Users/u".into())
        );
        assert_eq!(
            variable_of(Ok("HOME=\n".into()), "HOME"),
            Variable::Set(String::new())
        );
        assert_eq!(variable_of(Ok("-HOME\n".into()), "HOME"), Variable::Removed);
        assert_eq!(
            variable_of(
                failed("tmux show-environment failed: unknown variable: HOME"),
                "HOME"
            ),
            Variable::Absent
        );
        for answer in [
            // another variable's absence, a missing session, a dead server
            failed("tmux show-environment failed: unknown variable: HOMEX"),
            failed("tmux show-environment failed: no such session: =a"),
            failed("tmux show-environment failed: no server running on /tmp/x"),
            failed("tmux not runnable: timed out"),
            Err(DeckError::restart(crate::error::RestartFailure::Deadline)),
            // an answer in a shape this reader does not know
            Ok(String::new()),
            Ok("\n".into()),
            Ok("HOME\n".into()),
            Ok("HOMEX=/Users/u\n".into()),
            Ok("PATH=/usr/bin\n".into()),
            Ok("-HOMEX\n".into()),
            Ok(" HOME=/Users/u\n".into()),
            // more than one line: ambiguous, never the first line
            Ok("HOME=/Users/u\nHOME=/Users/other\n".into()),
            Ok("HOME=/Users/u\n-HOME\n".into()),
            Ok("HOME=/Users/u\n\n".into()),
        ] {
            let shown = format!("{answer:?}");
            assert_eq!(
                variable_of(answer, "HOME"),
                Variable::Unavailable,
                "{shown}"
            );
        }
    }

    #[test]
    fn nothing_outside_the_closed_shape_can_become_a_line() {
        let exe = Path::new("/opt/claude/2.1.294");
        for bad in [
            Path::new("claude"),
            Path::new("./claude"),
            Path::new("/opt/cl\naude"),
            Path::new("/opt/cl\u{1b}aude"),
        ] {
            assert!(
                command_line(bad, &words(&["--resume", ID])).is_none(),
                "{bad:?}"
            );
        }
        for bad in [
            "",
            "a b",
            "x;y",
            "$(id)",
            "KEY=value",
            "--model=opus",
            "continue\n",
            "'quoted'",
        ] {
            assert!(
                command_line(exe, &words(&["--resume", bad])).is_none(),
                "{bad:?}"
            );
        }
    }
}
