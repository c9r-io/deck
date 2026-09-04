//! Automatic, metadata-only context protection for scheduled prompt delivery.
//!
//! Exact tmux pane identity is always required. A sanitized foreground
//! executable is additionally required only when one was captured
//! automatically. Hooks, terminal contents, output activity and agent-specific
//! readiness states are deliberately outside this module.

use serde::{Deserialize, Serialize};

use crate::tmux::{pane_target, tmux};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContextStatus {
    Ready,
    ForegroundDifferent,
    SessionReplaced,
    Unavailable,
    Starting,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContextCode {
    /// Only written by deck <= 0.5.8 (explicit rebind). Kept so an existing
    /// queue.json round-trips instead of degrading to `Unknown`.
    TargetChecked,
    ProcessMatched,
    CompatibilityTarget,
    IdentityChanged,
    ForegroundDifferent,
    SessionMissing,
    StartupFailed,
    ProbeFailed,
    StartupTimeout,
    CancelledOrRevised,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PaneIdentity {
    #[serde(default)]
    pub(crate) server_pid: u32,
    pub(crate) session_id: String,
    pub(crate) window_id: String,
    pub(crate) pane_id: String,
    pub(crate) pane_pid: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ContextCheck {
    pub(crate) status: ContextStatus,
    pub(crate) code: ContextCode,
    pub(crate) checked_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RawProbe {
    pub(crate) identity: PaneIdentity,
    /// tmux's `pane_current_command`: the kernel's name of the foreground
    /// process, i.e. the basename of the file that was exec'd. For a
    /// launcher symlink such as Claude Code's `claude →
    /// versions/2.1.259` this is the VERSION, not the command.
    pub(crate) foreground: Option<String>,
    /// The same foreground process as `ps` names it (argv[0] basename),
    /// read from the pane tty's foreground process group. `claude` for the
    /// case above. Either name may match the expected process.
    pub(crate) foreground_argv: Option<String>,
}

impl RawProbe {
    /// The most recognizable name for the foreground process, for display
    /// and for capturing an expectation from a live pane.
    pub(crate) fn foreground_name(&self) -> Option<String> {
        self.foreground_argv
            .clone()
            .or_else(|| self.foreground.clone())
    }
    pub(crate) fn foreground_is(&self, expected: &str) -> bool {
        self.foreground.as_deref() == Some(expected)
            || self.foreground_argv.as_deref() == Some(expected)
    }
}

/// The group leader of the tty's foreground process group (what `ps -t`
/// marks `+`), by its argv[0] basename — read through libproc/sysctl, never
/// by spawning `ps`.
fn foreground_from_tty(tty: &str) -> Option<String> {
    let name = tty.strip_prefix("/dev/")?;
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let device = crate::procinfo::tty_device(tty)?;
    let table = crate::procinfo::processes();
    let leader = crate::procinfo::foreground_leader(&table, device)?;
    sanitize_process(&crate::procinfo::argv0(leader)?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProbeResult {
    pub(crate) status: ContextStatus,
    pub(crate) code: ContextCode,
    pub(crate) identity: Option<PaneIdentity>,
    pub(crate) current_process: Option<String>,
}

impl ProbeResult {
    pub(crate) fn blocked(status: ContextStatus, code: ContextCode) -> Self {
        Self {
            status,
            code,
            identity: None,
            current_process: None,
        }
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.status == ContextStatus::Ready
    }
}

fn strip_simple_quotes(raw: &str) -> &str {
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if matches!(
            (bytes[0], bytes[raw.len() - 1]),
            (b'\'', b'\'') | (b'"', b'"')
        ) {
            return &raw[1..raw.len() - 1];
        }
    }
    raw
}

pub(crate) fn sanitize_process(raw: &str) -> Option<String> {
    let raw = strip_simple_quotes(raw.trim()).trim_start_matches('-');
    let basename = raw.rsplit('/').next()?.to_string();
    (!basename.is_empty()
        && basename.len() <= 64
        && basename
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')))
    .then_some(basename)
}

fn assignment_prefix(raw: &str) -> bool {
    let Some((name, _)) = raw.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

/// Conservatively derive only the executable basename. Arguments and paths
/// are never persisted. Assignment prefixes, `env`, common `env` switches and
/// absolute executable paths are supported without attempting shell parsing.
pub(crate) fn expected_from_command(command: &str) -> Option<String> {
    let mut parts = command.split_whitespace().peekable();
    let mut after_env = false;
    while let Some(raw) = parts.next() {
        let part = strip_simple_quotes(raw);
        if assignment_prefix(part) {
            continue;
        }
        let candidate = sanitize_process(part)?;
        if !after_env && candidate == "env" {
            after_env = true;
            continue;
        }
        if after_env && matches!(part, "-i" | "--ignore-environment" | "-0" | "--null") {
            continue;
        }
        if after_env && (part == "-u" || part == "--unset") {
            let _ = parts.next();
            continue;
        }
        if matches!(
            candidate.as_str(),
            "cd" | "source"
                | "."
                | "alias"
                | "export"
                | "set"
                | "unset"
                | "while"
                | "until"
                | "for"
                | "if"
                | "case"
                | "function"
                | "exec"
                | "command"
                | "builtin"
                | "eval"
        ) || shell_process(Some(&candidate))
        {
            return None;
        }
        return Some(candidate);
    }
    None
}

pub(crate) fn shell_process(foreground: Option<&str>) -> bool {
    matches!(
        foreground.map(str::to_ascii_lowercase).as_deref(),
        Some("zsh" | "bash" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "csh")
    )
}

/// Parse one tmux metadata line. Malformed or unsanitized foreground data is
/// represented as no process; it is never persisted verbatim.
pub(crate) fn parse_raw_probe(raw: &str) -> Option<RawProbe> {
    let mut fields = raw.trim_end_matches(['\r', '\n']).split('\t');
    let server_pid = fields.next()?.parse().ok()?;
    let session_id = fields.next()?.to_string();
    let window_id = fields.next()?.to_string();
    let pane_id = fields.next()?.to_string();
    let pane_pid = fields.next()?.parse().ok()?;
    let foreground = sanitize_process(fields.next().unwrap_or_default());
    if fields.next().is_some()
        || !session_id.starts_with('$')
        || !window_id.starts_with('@')
        || !pane_id.starts_with('%')
        || pane_pid == 0
        || server_pid == 0
    {
        return None;
    }
    Some(RawProbe {
        identity: PaneIdentity {
            server_pid,
            session_id,
            window_id,
            pane_id,
            pane_pid,
        },
        foreground,
        foreground_argv: None,
    })
}

/// One metadata-only tmux read. No pane capture, prompt text, argument, path or
/// user-configured hook participates in the decision.
pub(crate) fn raw_probe(session: &str) -> Result<RawProbe, String> {
    let raw = tmux(&[
        "display-message",
        "-p",
        "-t",
        &pane_target(session),
        "#{pid}\t#{session_id}\t#{window_id}\t#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_tty}",
    ])?;
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    let (meta, tty) = trimmed.rsplit_once('\t').unwrap_or((trimmed, ""));
    let mut probe = parse_raw_probe(meta)
        .ok_or_else(|| "tmux returned malformed context metadata".to_string())?;
    probe.foreground_argv = foreground_from_tty(tty);
    Ok(probe)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CreationContext {
    pub(crate) binding: Option<PaneIdentity>,
    pub(crate) expected_process: Option<String>,
}

pub(crate) fn creation_context(session: &str, command: &str) -> CreationContext {
    creation_from_probe(command, raw_probe(session).ok())
}

fn creation_from_probe(command: &str, raw: Option<RawProbe>) -> CreationContext {
    let explicit = expected_from_command(command);
    match raw {
        Some(raw) => {
            let name = raw.foreground_name();
            CreationContext {
                binding: Some(raw.identity),
                expected_process: explicit
                    .or_else(|| (!shell_process(name.as_deref())).then_some(name).flatten()),
            }
        }
        None => CreationContext {
            binding: None,
            expected_process: explicit,
        },
    }
}

/// Pure automatic evaluation. Identity is checked first and cannot be
/// overridden. With no expected process, a valid identity is sufficient.
pub(crate) fn evaluate(
    raw: &RawProbe,
    expected_identity: Option<&PaneIdentity>,
    expected_process: Option<&str>,
) -> ProbeResult {
    if expected_identity.is_some_and(|expected| expected != &raw.identity) {
        return ProbeResult {
            status: ContextStatus::SessionReplaced,
            code: ContextCode::IdentityChanged,
            identity: Some(raw.identity.clone()),
            current_process: raw.foreground_name(),
        };
    }
    let (status, code) = match expected_process {
        Some(expected) if raw.foreground_is(expected) => {
            (ContextStatus::Ready, ContextCode::ProcessMatched)
        }
        Some(_) => (
            ContextStatus::ForegroundDifferent,
            ContextCode::ForegroundDifferent,
        ),
        None => (ContextStatus::Ready, ContextCode::CompatibilityTarget),
    };
    ProbeResult {
        status,
        code,
        identity: Some(raw.identity.clone()),
        current_process: raw.foreground_name(),
    }
}

pub(crate) fn probe(
    session: &str,
    expected_identity: Option<&PaneIdentity>,
    expected_process: Option<&str>,
) -> ProbeResult {
    match raw_probe(session) {
        Ok(raw) => evaluate(&raw, expected_identity, expected_process),
        Err(e) => ProbeResult::blocked(
            ContextStatus::Unavailable,
            if crate::storage::err_code(&e) == "no-session" {
                ContextCode::SessionMissing
            } else {
                ContextCode::ProbeFailed
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(fg: &str) -> RawProbe {
        parse_raw_probe(&format!("99\t$1\t@2\t%3\t44\t{fg}\n")).unwrap()
    }

    #[test]
    fn foreground_argv_names_are_sanitized_basenames() {
        assert_eq!(sanitize_process("-zsh"), Some("zsh".into()));
        assert_eq!(
            sanitize_process("/Users/x/.local/bin/claude"),
            Some("claude".into())
        );
        assert_eq!(
            sanitize_process("/opt/My App/bin/thing"),
            Some("thing".into()),
            "basename only"
        );
        assert_eq!(
            sanitize_process("/x/bad name"),
            None,
            "unsanitizable name is no name"
        );
        assert_eq!(foreground_from_tty("/dev/../etc/passwd"), None);
        assert_eq!(foreground_from_tty("not-a-tty"), None);
    }

    #[test]
    fn a_versioned_launcher_binary_matches_by_its_argv_name() {
        // Claude Code: `claude` is a symlink to versions/2.1.259; tmux reports
        // the version, ps reports the launcher name.
        let mut probe = raw("2.1.259");
        probe.foreground_argv = Some("claude".into());
        let result = evaluate(&probe, None, Some("claude"));
        assert_eq!(result.status, ContextStatus::Ready);
        assert_eq!(result.code, ContextCode::ProcessMatched);
        assert_eq!(result.current_process.as_deref(), Some("claude"));
        let other = evaluate(&probe, None, Some("codex"));
        assert_eq!(other.status, ContextStatus::ForegroundDifferent);
        // without the argv view the version alone still does not match
        let bare = raw("2.1.259");
        assert_eq!(
            evaluate(&bare, None, Some("claude")).status,
            ContextStatus::ForegroundDifferent
        );
        // a live pane with no explicit command captures the recognizable name
        let ctx = creation_from_probe("", Some(probe));
        assert_eq!(ctx.expected_process.as_deref(), Some("claude"));
    }

    #[test]
    fn no_expected_process_accepts_shell_and_hookless_agent() {
        assert!(evaluate(&raw("zsh"), None, None).is_ready());
        assert!(evaluate(&raw("codex"), None, None).is_ready());
    }

    #[test]
    fn creation_captures_a_live_non_shell_but_not_a_pre_agent_shell() {
        let running = creation_from_probe("", Some(raw("codex")));
        assert_eq!(running.expected_process.as_deref(), Some("codex"));
        let before_agent = creation_from_probe("", Some(raw("zsh")));
        assert!(before_agent.expected_process.is_none());
        assert!(evaluate(&raw("codex"), before_agent.binding.as_ref(), None).is_ready());
    }

    #[test]
    fn expected_process_waits_for_an_exact_sanitized_basename() {
        let different = evaluate(&raw("zsh"), None, Some("codex"));
        assert_eq!(different.status, ContextStatus::ForegroundDifferent);
        let matched = evaluate(&raw("codex"), None, Some("codex"));
        assert_eq!(matched.code, ContextCode::ProcessMatched);
    }

    #[test]
    fn identity_change_wins_for_every_process_mode() {
        let mut expected = raw("codex").identity;
        expected.pane_id = "%99".into();
        for process in [None, Some("codex")] {
            let result = evaluate(&raw("codex"), Some(&expected), process);
            assert_eq!(result.status, ContextStatus::SessionReplaced);
        }
    }

    #[test]
    fn command_extraction_supports_prefixes_env_and_paths() {
        assert_eq!(
            expected_from_command("codex --full-auto"),
            Some("codex".into())
        );
        assert_eq!(
            expected_from_command("FOO=1 /opt/bin/Claude --x"),
            Some("Claude".into())
        );
        assert_eq!(
            expected_from_command("env FOO=1 /usr/local/bin/codex -a"),
            Some("codex".into())
        );
        assert_eq!(
            expected_from_command("env -i FOO=1 python3.12 app.py"),
            Some("python3.12".into())
        );
        assert_eq!(expected_from_command(""), None);
        assert_eq!(expected_from_command("FOO=1"), None);
        assert_eq!(expected_from_command("while true; do date; done"), None);
        assert_eq!(expected_from_command("/bin/zsh -lc codex"), None);
    }

    #[test]
    fn probe_format_has_no_hook_fields_and_rejects_extra_metadata() {
        assert!(parse_raw_probe("99\t$1\t@2\t%3\t44\tcodex\n").is_some());
        assert!(parse_raw_probe("99\t$1\t@2\t%3\t44\tcodex\tready\n").is_none());
    }
}
