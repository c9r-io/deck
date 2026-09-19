//! On-demand resume hints from the current shell's tmux history. Read a bounded
//! joined tail, return only tool/UUID pairs, and never persist or log content.
//! Exit headings establish provenance; arbitrary UUIDs and shell input do not.
//! Restored shell transcripts use the same path as live output.

use serde::Serialize;

use crate::error::DeckError;
use crate::tmux::{pane_row, pane_target, tmux, validate_session_name};

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ResumeHint {
    agent: String,
    id: String,
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

fn extract(text: &str) -> Vec<ResumeHint> {
    let mut hints = Vec::new();
    let mut remaining = 0;
    for line in text.lines() {
        let line = line.trim();
        if [
            "To continue this session, run:",
            "To resume this session, run:",
            "Resume this session with:",
        ]
        .contains(&line)
        {
            remaining = 3;
            continue;
        }
        if remaining == 0 {
            continue;
        }
        remaining -= 1;
        let parts: Vec<_> = line.split_whitespace().collect();
        if let [agent, resume, id] = parts.as_slice() {
            if matches!(
                (*agent, *resume),
                ("codex", "resume") | ("claude", "--resume") | ("claude", "-r")
            ) && uuid(id)
            {
                let hint = ResumeHint {
                    agent: agent.to_string(),
                    id: id.to_ascii_lowercase(),
                };
                hints.retain(|old| old != &hint);
                hints.push(hint);
            }
        }
        // Only blanks may separate the heading and command. An input prompt,
        // prose or code fence ends the block, even if a command follows it.
        if !line.is_empty() {
            remaining = 0;
        }
    }
    hints.into_iter().rev().take(8).collect()
}

#[tauri::command]
pub(crate) async fn terminal_resume_hints(name: String) -> Result<Vec<ResumeHint>, DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        validate_session_name(&name)?;
        let _activity = crate::session_runtime::activity_guard()?;
        let target = pane_target(&name);
        let before = pane_row(&target)?;
        if !crate::context::shell_process(Some(&before.command)) {
            return Ok(Vec::new());
        }
        // -J joins soft wraps; omitting -e strips terminal styling. Limit the
        // history read and the parser input independently (wide panes exist).
        let text = tmux(&[
            "capture-pane",
            "-p",
            "-J",
            "-t",
            &before.pane_id,
            "-S",
            "-1000",
        ])?;
        let after = pane_row(&target)?;
        if before.server_pid != after.server_pid
            || before.pane_id != after.pane_id
            || before.pane_pid != after.pane_pid
            || !crate::context::shell_process(Some(&after.command))
        {
            return Ok(Vec::new());
        }
        let start = text.len().saturating_sub(128 * 1024);
        // Drop a partial first line; never reinterpret a truncated prompt as
        // an exit heading or command.
        let tail = if start == 0 {
            text.as_str()
        } else {
            text.as_bytes()[start..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|i| &text[start + i + 1..])
                .unwrap_or("")
        };
        Ok(extract(tail))
    })
    .await
    .map_err(|_| {
        crate::error::DeckError::new(crate::error::ErrorKind::Other, "resume-hints-worker-failed")
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "01a0b76c-b577-7493-86ef-a1f6209b823e";
    const B: &str = "0f3ab19c-4d2e-4a71-9b8c-1d2e3f4a5b6c";

    #[test]
    fn live_and_restored_exit_hints_are_recent_and_deduplicated() {
        let text = format!("To continue this session, run:\n\n  codex resume {A}\n\nOr run codex resume\n---------------- deck restart ----------------\nResume this session with:\nclaude --resume {B}\nTo continue this session, run:\n\ncodex resume {A}\nuser % codex resume {B}");
        assert_eq!(
            extract(&text),
            vec![
                ResumeHint {
                    agent: "codex".into(),
                    id: A.into()
                },
                ResumeHint {
                    agent: "claude".into(),
                    id: B.into()
                },
            ]
        );
    }

    #[test]
    fn rejects_examples_input_invalid_ids_and_shell_suffixes() {
        for text in [
            format!("codex resume {A}"),
            format!("Example: To continue this session, run:\ncodex resume {A}"),
            format!("To continue this session, run:\n```\ncodex resume {A}\n```"),
            format!("To continue this session, run:\nuser % codex resume {A}"),
            format!("To continue this session, run:\ncodex resume {A}; echo hi"),
            "To continue this session, run:\ncodex resume not-a-uuid".into(),
            format!("To continue this session, run:\n\n\n\ncodex resume {A}"),
        ] {
            assert!(extract(&text).is_empty(), "{text}");
        }
        assert!(!uuid("01a0b76c_b577-7493-86ef-a1f6209b823e"));
    }
}
