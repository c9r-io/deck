//! Pure tmux command-list construction for frame-paced terminal scrolling.

/// Build one tmux invocation that conditionally enters copy-mode, scrolls,
/// and prints the resulting mode. `target` is produced only after deck's
/// strict session-name validation, so it is safe inside nested tmux commands.
pub(crate) fn args(target: &str, lines: i32) -> Vec<String> {
    let mut args = Vec::new();
    if lines < 0 {
        let n = lines.unsigned_abs().clamp(1, 60);
        let scroll = format!("send-keys -t {target} -X -N {n} scroll-up");
        let enter_and_scroll = format!(
            "if-shell -F -t {target} '#{{>:#{{history_size}},0}}' \
             'copy-mode -e -t {target} ; {scroll}' ''"
        );
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
    if !args.is_empty() {
        args.push(";".into());
    }
    args.extend([
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        "#{pane_in_mode}".into(),
    ]);
    args
}

#[cfg(test)]
mod tests {
    use super::args;

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
}
