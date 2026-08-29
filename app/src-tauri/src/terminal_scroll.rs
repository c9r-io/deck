//! Pure tmux command-list construction for frame-paced terminal scrolling.

pub(crate) const CURSOR_ROW_OPTION: &str = "@deck-scroll-cursor-row";

/// Build one tmux invocation that conditionally enters copy-mode, scrolls,
/// and prints the resulting mode. `target` is produced only after deck's
/// strict session-name validation, so it is safe inside nested tmux commands.
pub(crate) fn args(target: &str, lines: i32) -> Vec<String> {
    build_args(target, lines, false)
}

/// Ordinary wheel scrolling enters tmux copy-mode too, but unlike a real
/// copy-mode interaction the user is not moving a copy cursor. tmux's
/// `scroll-up` keeps that cursor on a fixed viewport row while the content
/// underneath it moves, which detaches the visible cursor from an agent's
/// input composer. Preserve the live cursor's content row while it remains
/// visible and clamp it at the viewport edge after it scrolls out of view.
pub(crate) fn cursor_following_args(target: &str, lines: i32) -> Vec<String> {
    build_args(target, lines, true)
}

fn build_args(target: &str, lines: i32, follow_live_cursor: bool) -> Vec<String> {
    let mut args = Vec::new();
    if follow_live_cursor && lines != 0 {
        // A pane may already be in copy-mode after an app restart or a tmux
        // key binding. Adopt its current live cursor only when Deck has no
        // anchor yet; ordinary Deck-entered scrollback sets this atomically
        // just before `copy-mode` below.
        args.extend([
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            target.into(),
            format!("#{{&&:#{{pane_in_mode}},#{{==:#{{{CURSOR_ROW_OPTION}}},}}}}"),
            format!("set-option -p -F -t {target} {CURSOR_ROW_OPTION} '#{{cursor_y}}'"),
            String::new(),
            ";".into(),
        ]);
    }
    if lines < 0 {
        let n = lines.unsigned_abs().clamp(1, 60);
        let scroll = format!("send-keys -t {target} -X -N {n} scroll-up");
        let enter = if follow_live_cursor {
            format!(
                "set-option -p -F -t {target} {CURSOR_ROW_OPTION} '#{{cursor_y}}' ; \
                 copy-mode -e -t {target} ; {scroll}"
            )
        } else {
            format!("copy-mode -e -t {target} ; {scroll}")
        };
        let enter_and_scroll =
            format!("if-shell -F -t {target} '#{{>:#{{history_size}},0}}' \"{enter}\" ''");
        args.extend([
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            target.into(),
            "#{pane_in_mode}".into(),
            scroll,
            enter_and_scroll,
        ]);
    } else if lines > 0 {
        let n = lines.clamp(1, 60);
        args.extend([
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            target.into(),
            "#{pane_in_mode}".into(),
            format!("send-keys -t {target} -X -N {n} scroll-down"),
            String::new(),
        ]);
    }
    if follow_live_cursor && lines != 0 {
        push_cursor_follow(&mut args, target);
        args.extend([
            ";".into(),
            "if-shell".into(),
            "-F".into(),
            "-t".into(),
            target.into(),
            "#{pane_in_mode}".into(),
            String::new(),
            format!("set-option -p -u -t {target} {CURSOR_ROW_OPTION}"),
        ]);
    }
    if !args.is_empty() {
        args.push(";".into());
    }
    let report = if follow_live_cursor {
        let content_row = format!("#{{e|+:#{{{CURSOR_ROW_OPTION}}},#{{scroll_position}}}}");
        format!(
            "#{{pane_in_mode}}\t#{{?#{{&&:#{{pane_in_mode}},#{{e|>=:{content_row},#{{pane_height}}}}}},0,1}}"
        )
    } else {
        "#{pane_in_mode}".into()
    };
    args.extend([
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        report,
    ]);
    args
}

/// Reposition copy-mode's cursor to the live cursor's original content row.
/// `scroll_position` is how far that row has moved down from the live frame.
/// Both motions are bounded to the current viewport so following the cursor
/// can never undo the user's scroll by moving past an edge.
fn push_cursor_follow(args: &mut Vec<String>, target: &str) {
    if !args.is_empty() {
        args.push(";".into());
    }
    let content_row = format!("#{{e|+:#{{{CURSOR_ROW_OPTION}}},#{{scroll_position}}}}");
    let last_row = "#{e|-:#{pane_height},1}";
    let target_row =
        format!("#{{?#{{e|<:{content_row},#{{pane_height}}}},{content_row},{last_row}}}");
    let down_condition =
        format!("#{{&&:#{{pane_in_mode}},#{{e|>:{target_row},#{{copy_cursor_y}}}}}}");
    let down_count = format!("#{{e|-:{target_row},#{{copy_cursor_y}}}}");
    let up_condition =
        format!("#{{&&:#{{pane_in_mode}},#{{e|<:{target_row},#{{copy_cursor_y}}}}}}");
    let up_count = format!("#{{e|-:#{{copy_cursor_y}},{target_row}}}");

    args.extend([
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        target.into(),
        down_condition,
        format!("send-keys -t {target} -X -N '{down_count}' cursor-down"),
        String::new(),
        ";".into(),
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        target.into(),
        up_condition,
        format!("send-keys -t {target} -X -N '{up_count}' cursor-up"),
        String::new(),
    ]);
}

#[cfg(test)]
mod tests {
    use super::{args, cursor_following_args, CURSOR_ROW_OPTION};

    #[test]
    fn scroll_is_one_bounded_tmux_command_list() {
        let up = args("=deck-card:", -999);
        assert_eq!(up[0], "if-shell");
        assert!(up.iter().any(|arg| arg.contains("-N 60 scroll-up")));
        assert!(up.iter().any(|arg| arg.contains("#{>:#{history_size},0}")));
        assert_eq!(up.iter().filter(|arg| arg.as_str() == ";").count(), 1);
        assert_eq!(up.last().map(String::as_str), Some("#{pane_in_mode}"));

        let down = args("=deck-card:", 999);
        assert_eq!(down[0], "if-shell");
        assert!(down.iter().any(|arg| arg.contains("-N 60 scroll-down")));
        assert_eq!(down.iter().filter(|arg| arg.as_str() == ";").count(), 1);

        let idle = args("=deck-card:", 0);
        assert_eq!(idle[0], "display-message");
        assert!(!idle.iter().any(|arg| arg == ";"));
    }

    #[test]
    fn ordinary_scroll_keeps_the_live_cursor_on_its_content_row() {
        let up = cursor_following_args("=deck-card:", -3);
        let joined = up.join(" ");
        assert!(joined.contains(&format!(
            "set-option -p -F -t =deck-card: {CURSOR_ROW_OPTION} '#{{cursor_y}}'"
        )));
        assert!(joined.contains("#{scroll_position}"));
        assert!(joined.contains("#{pane_height}"));
        assert!(joined.contains("#{copy_cursor_y}"));
        assert!(joined.contains("cursor-down"));
        assert!(joined.contains("cursor-up"));
        assert!(up.last().is_some_and(|report| report.contains('\t')));

        // Selection scrolling deliberately retains tmux's copy cursor: moving
        // it here would mutate the user's selection endpoint.
        let selection = args("=deck-card:", -3).join(" ");
        assert!(!selection.contains(CURSOR_ROW_OPTION));
        assert!(!selection.contains("cursor-down"));
        assert!(!selection.contains("cursor-up"));
    }
}
