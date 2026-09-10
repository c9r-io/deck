//! Shared byte-literal prompt injection. The scheduler owns its ledger;
//! immediate input owns its draft. Both pin identity/foreground and separate
//! paste from Enter. Errors after transport begins can be ambiguous: callers
//! must not blindly retry. Interactive multi-line input requires paste mode.
use crate::context::{self, PaneIdentity};
use crate::error::{DeckError, ErrorKind};
use crate::tmux::{tmux, tmux_owned};
use serde::Serialize;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum LiteralOutcome {
    Inserted,
    Submitted,
    EnterRefused,
}

pub(crate) struct LiteralRequest<'a> {
    pub session: &'a str,
    pub pane: &'a PaneIdentity,
    pub expected_process: Option<&'a str>,
    pub delivery: &'a str,
    pub text: &'a str,
    pub submit: bool,
    pub require_bracketed: bool,
}

pub(crate) fn deliver(request: LiteralRequest<'_>) -> Result<LiteralOutcome, DeckError> {
    let LiteralRequest {
        session,
        pane,
        expected_process,
        delivery,
        text,
        submit,
        require_bracketed,
    } = request;
    if !delivery
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(DeckError::new(
            ErrorKind::Other,
            "delivery identity is invalid",
        ));
    }
    let line = text.to_string();
    let buffer = format!("deck-send-{delivery}");
    // Pane/session ids can be reused after the entire tmux server exits. Put
    // the bytes in a uniquely named tmux buffer, then compare the FULL
    // generation and paste them in the same server command queue. `-F`
    // evaluates synchronously (no shell); the inner commands contain only
    // deck-generated ids, never user text. paste-buffer is byte-literal and
    // `-p` wraps the bytes in bracketed-paste marks only for an application
    // that asked for them, so an agent TUI sees one paste, not keystrokes.
    // Enter follows as a SEPARATE key after a short pause: a CR inside the
    // same burst is treated by agent inputs (Claude Code, Codex) as a
    // pasted newline and the prompt sits unsent in the input box.
    let actual = "#{pid}:#{session_id}:#{window_id}:#{pane_id}:#{pane_pid}";
    let expected = format!(
        "{}:{}:{}:{}:{}",
        pane.server_pid, pane.session_id, pane.window_id, pane.pane_id, pane.pane_pid
    );
    let identity_condition = format!("#{{==:{actual},{expected}}}");
    // tmux can only compare its own `pane_current_command` atomically. When
    // the expected process was recognized through its argv name (a launcher
    // symlink to a versioned binary — tmux says `2.1.259`, ps says `claude`),
    // pin the paste to the exact tmux name observed for that very process,
    // so the atomic check still means "the verified process is still here".
    let condition_process = expected_process.map(|expected| match context::raw_probe(session) {
        Ok(raw)
            if raw.foreground.as_deref() != Some(expected)
                && raw.foreground_argv.as_deref() == Some(expected) =>
        {
            raw.foreground.unwrap_or_else(|| expected.to_string())
        }
        _ => expected.to_string(),
    });
    let condition = match condition_process.as_deref() {
        Some(process) => {
            format!("#{{&&:{identity_condition},#{{==:#{{pane_current_command}},{process}}}}}")
        }
        None => identity_condition,
    };
    let condition = if require_bracketed && text.contains('\n') {
        format!("#{{&&:{condition},#{{==:#{{bracketed_paste_flag}},1}}}}")
    } else {
        condition
    };
    let condition = if require_bracketed {
        format!("#{{&&:{condition},#{{==:#{{pane_in_mode}},0}}}}")
    } else {
        condition
    };
    let yes = format!("paste-buffer -p -b {buffer} -d -t {}", pane.pane_id);
    let no = format!("delete-buffer -b {buffer}; display-message -p deck-context-refused");
    let out = tmux_owned(&[
        "set-buffer".into(),
        "-b".into(),
        buffer.clone(),
        line,
        ";".into(),
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        pane.pane_id.clone(),
        condition.clone(),
        yes,
        no,
    ]);
    let refused = |stdout: &String| {
        stdout
            .lines()
            .any(|line| line.trim() == "deck-context-refused")
    };
    if !out.as_ref().is_ok_and(|stdout| !refused(stdout)) {
        // A vanished target can abort the command queue before its refusal
        // branch deletes the private buffer. Never leave prompt bytes behind
        // in tmux after a refused/indeterminate injection.
        let _ = tmux(&["delete-buffer", "-b", &buffer]);
        return Err(DeckError::new(
            if out.as_ref().is_ok_and(refused) {
                ErrorKind::ContextChanged
            } else {
                ErrorKind::Other
            },
            if out.as_ref().is_ok_and(refused) {
                "target-changed"
            } else {
                "delivery-unknown"
            },
        ));
    }
    if !submit {
        return Ok(LiteralOutcome::Inserted);
    }
    std::thread::sleep(Duration::from_millis(600));
    let enter = format!("send-keys -t {} Enter", pane.pane_id);
    let out = tmux_owned(&[
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        pane.pane_id.clone(),
        condition,
        enter,
        "display-message -p deck-context-refused".into(),
    ]);
    if !out.as_ref().is_ok_and(|stdout| !refused(stdout)) {
        // The text is already in the pane; the user sees it and can submit
        // it. Counting this as sent keeps the audit honest about the bytes
        // that landed, and the log names the one thing that did not.
        return Ok(LiteralOutcome::EnterRefused);
    }
    Ok(LiteralOutcome::Submitted)
}
