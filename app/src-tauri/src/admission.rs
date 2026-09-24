//! The ONE remote-agent admission policy, shared by the Slack channel
//! monitor, the Slack badge rules, the Phone Connector and the scheduler's
//! external rows. Nothing here reads settings, a socket or the Board: a
//! caller hands in a saved target command or a message body and gets the
//! decision back. `tests/external_admission.rs` pins every path that puts
//! non-owner text into a queue or pane to these two functions.

/// The ONE remote-agent admission policy shared by Slack and Connector.
/// Accept a bare agent or simple, unquoted arguments. The restricted alphabet
/// cannot introduce shell expansion, redirection, pipelines or a second
/// command when the saved command is sent to the pane's shell.
pub(crate) fn channel_agent_command(cmd: &str) -> Option<&'static str> {
    let mut words = cmd.split(' ');
    let agent = match words.next()? {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        _ => None,
    }?;
    words
        .all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_./:=+,@".contains(&byte))
        })
        .then_some(agent)
}

/// Invisible characters that can hide instructions from the person who
/// inspects a staged note, or visually reorder it: bidi embeddings,
/// overrides and isolates, directional marks, zero-width space, word joiner
/// and invisible operators, BOM, and Unicode tag characters. ZWJ/ZWNJ are
/// language and emoji structure, so a single joiner between two visible
/// characters is kept; runs of joiners and joiners next to whitespace or a
/// text edge are removed. Other format characters (e.g. soft hyphen) stay.
fn invisible(c: char) -> bool {
    matches!(c,
        '\u{200B}' | '\u{200E}' | '\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}'
        | '\u{E0000}'..='\u{E007F}')
}

fn joiner(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}')
}

pub(crate) fn strip_invisible(text: &str) -> String {
    let chars: Vec<char> = text.chars().filter(|c| !invisible(*c)).collect();
    let visible = |c: Option<&char>| c.is_some_and(|c| !joiner(*c) && !c.is_whitespace());
    chars
        .iter()
        .enumerate()
        .filter(|(i, c)| {
            !joiner(**c) || (*i > 0 && visible(chars.get(i - 1)) && visible(chars.get(i + 1)))
        })
        .map(|(_, c)| *c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_commands_admit_simple_arguments_without_shell_syntax() {
        assert_eq!(channel_agent_command("claude"), Some("claude"));
        assert_eq!(channel_agent_command("codex"), Some("codex"));
        for command in [
            "codex --yolo",
            "codex -c approval_policy=never",
            "claude --dangerously-skip-permissions",
            "claude --permission-mode bypassPermissions",
        ] {
            assert_eq!(
                channel_agent_command(command),
                Some(if command.starts_with("codex") {
                    "codex"
                } else {
                    "claude"
                })
            );
        }
        for command in [
            "",
            " claude",
            "claude ",
            "claude  --version",
            "Claude",
            "env claude",
            "/tmp/x/claude",
            "claude;zsh",
            "codex $(true)",
            "claude && sh",
            "claude --model='x'",
            "codex\t--yolo",
        ] {
            assert_eq!(channel_agent_command(command), None, "{command:?}");
        }
    }

    #[test]
    fn invisible_characters_are_stripped_but_a_lone_joiner_survives() {
        // A lone joiner between two visible characters is language/emoji
        // structure and survives; a run of joiners (a hidden bit channel)
        // or a joiner at an edge does not.
        assert_eq!(strip_invisible("می\u{200C}خواهم"), "می\u{200C}خواهم");
        assert_eq!(strip_invisible("a\u{200C}\u{200D}\u{200C}b"), "ab");
        assert_eq!(strip_invisible("\u{200D}a b\u{200C} c"), "a b c");
        assert_eq!(strip_invisible("x\u{00AD}y"), "x\u{00AD}y");
        assert_eq!(
            strip_invisible("INC-42\u{202E}\u{2066}\u{200B}\u{FEFF}\u{E0041} ok"),
            "INC-42 ok"
        );
    }
}
