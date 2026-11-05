//! Pure tmux command-list construction for frame-paced terminal scrolling
//! and the forwarded click.
//!
//! Ordinary wheel input is negotiated at the pane at dispatch time: copy-mode
//! keeps viewport ownership, a live mouse-reporting application receives its
//! negotiated protocol, and every other pane uses Deck's history path. A plain
//! click (`click_args`) is negotiated the same way with no history branch: an
//! application outside copy-mode that asked for button reports receives one
//! left press and its release at the clicked cell, and every other pane
//! receives nothing. A cell a legacy encoding cannot name is never sent
//! clamped to another cell. Mouse frames are fixed hexadecimal argv, never
//! literal user-controlled strings.

pub(crate) const CURSOR_ROW_OPTION: &str = "@deck-scroll-cursor-row";

/// Build one tmux invocation that conditionally enters copy-mode, scrolls,
/// and prints the resulting mode. `target` is produced only after deck's
/// strict session-name validation, so it is safe inside nested tmux commands.
#[cfg(test)]
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

/// A zero-based pointer cell supplied by the frontend. The wheel clamps it
/// against the pane geometry read while terminal's resize/selection
/// operation lock is held; a click on a cell outside that geometry is
/// refused instead (`terminal::click_session`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PointerCell {
    pub(crate) column: u32,
    pub(crate) row: u32,
}

impl PointerCell {
    pub(crate) fn clamped(column: u32, row: u32, pane_width: u32, pane_height: u32) -> Self {
        Self {
            column: column.min(pane_width.saturating_sub(1)),
            row: row.min(pane_height.saturating_sub(1)),
        }
    }
}

/// Route an ordinary physical wheel atomically inside tmux. `cell` is an
/// explicit opt-in: callers without both coordinates retain history-only
/// behavior. The route condition is evaluated by the server immediately
/// before the bytes are sent; if the pane no longer reports mouse tracking,
/// that execution takes the history branch. An application that exits without
/// resetting its modes leaves the same terminal state as any other client.
pub(crate) fn negotiated_args(target: &str, lines: i32, cell: PointerCell) -> Vec<String> {
    if lines == 0 {
        return build_args(target, lines, true);
    }
    let history = command_string(build_args(target, lines, true));
    let sgr = mouse_command(target, lines, cell, MouseEncoding::Sgr);
    let utf8 = mouse_command(target, lines, cell, MouseEncoding::Utf8);
    let x10 = mouse_command(target, lines, cell, MouseEncoding::X10);
    let legacy = nested_if(target, "#{mouse_utf8_flag}", &utf8, &x10);
    let negotiated = nested_if(target, "#{mouse_sgr_flag}", &sgr, &legacy);
    let capture = "#{||:#{mouse_any_flag},#{mouse_button_flag},#{mouse_standard_flag}}";
    let live = nested_if(target, capture, &negotiated, &history);
    vec![
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        target.into(),
        "#{pane_in_mode}".into(),
        history.clone(),
        live,
        ";".into(),
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        scroll_report(),
    ]
}

/// The pane state a click is decided on, read before the route and printed
/// again at the end of `click_args`: copy-mode, whether the application
/// asked for button reports, its two encoding flags, and the pane size.
pub(crate) const CLICK_PROBE: &str = "#{pane_in_mode}\t#{||:#{mouse_any_flag},#{mouse_button_flag},#{mouse_standard_flag}}\t#{mouse_sgr_flag}\t#{mouse_utf8_flag}\t#{pane_width}\t#{pane_height}";

/// The route condition: "1" for an application outside copy-mode that asked
/// for button reports, else "0".
const CLICK_ROUTE: &str =
    "#{?pane_in_mode,0,#{||:#{mouse_any_flag},#{mouse_button_flag},#{mouse_standard_flag}}}";

/// Whether the negotiated encoding can name `cell`: SGR any cell, UTF-8
/// cells up to 2014, X10 cells up to 222 (one byte of `cell + 33`).
pub(crate) fn click_encodable(sgr: bool, utf8: bool, cell: PointerCell) -> bool {
    let limit = if sgr {
        u32::MAX
    } else if utf8 {
        2014
    } else {
        222
    };
    cell.column <= limit && cell.row <= limit
}

/// Route one plain click atomically inside tmux. The condition is evaluated
/// by the server immediately before the bytes are sent, as the wheel's is: a
/// pane in copy-mode, an application that no longer reports the mouse, or an
/// encoding that cannot name the cell takes nothing. The list ends with
/// `CLICK_PROBE`; nothing else runs inside one list, so the caller reads the
/// state the route was decided on.
pub(crate) fn click_args(target: &str, cell: PointerCell) -> Vec<String> {
    let sgr = click_command(target, cell, MouseEncoding::Sgr);
    let utf8 = click_command(target, cell, MouseEncoding::Utf8);
    let x10 = click_command(target, cell, MouseEncoding::X10);
    let legacy = nested_if(target, "#{mouse_utf8_flag}", &utf8, &x10);
    let negotiated = nested_if(target, "#{mouse_sgr_flag}", &sgr, &legacy);
    vec![
        "if-shell".into(),
        "-F".into(),
        "-t".into(),
        target.into(),
        CLICK_ROUTE.into(),
        negotiated,
        ";".into(),
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        CLICK_PROBE.into(),
    ]
}

/// One left press and its release at `cell`, or no command when the
/// encoding cannot name the cell.
fn click_command(target: &str, cell: PointerCell, encoding: MouseEncoding) -> String {
    let (sgr, utf8) = match encoding {
        MouseEncoding::Sgr => (true, false),
        MouseEncoding::Utf8 => (false, true),
        MouseEncoding::X10 => (false, false),
    };
    if !click_encodable(sgr, utf8, cell) {
        return String::new();
    }
    let mut frame = mouse_frame(0, false, cell, encoding);
    frame.extend(mouse_frame(0, true, cell, encoding));
    let hex: Vec<String> = frame.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("send-keys -t {target} -H {}", hex.join(" "))
}

#[derive(Clone, Copy)]
enum MouseEncoding {
    Sgr,
    Utf8,
    X10,
}

fn command_string(mut args: Vec<String>) -> String {
    // A nested route reports once after the outer conditional.
    args.truncate(args.len().saturating_sub(5));
    if args.last().is_some_and(|arg| arg == ";") {
        args.pop();
    }
    args.iter()
        .map(|arg| {
            if arg == ";" {
                ";".to_string()
            } else {
                tmux_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn nested_if(target: &str, condition: &str, yes: &str, no: &str) -> String {
    [
        "if-shell".to_string(),
        "-F".to_string(),
        "-t".to_string(),
        target.to_string(),
        condition.to_string(),
        yes.to_string(),
        no.to_string(),
    ]
    .iter()
    .map(|arg| tmux_quote(arg))
    .collect::<Vec<_>>()
    .join(" ")
}

fn tmux_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn scroll_report() -> String {
    let content_row = format!("#{{e|+:#{{{CURSOR_ROW_OPTION}}},#{{scroll_position}}}}");
    format!(
        "#{{pane_in_mode}}\t#{{?#{{&&:#{{pane_in_mode}},#{{e|>=:{content_row},#{{pane_height}}}}}},0,1}}"
    )
}

fn mouse_command(target: &str, lines: i32, cell: PointerCell, encoding: MouseEncoding) -> String {
    let button = if lines < 0 { 64 } else { 65 };
    let count = lines.unsigned_abs().clamp(1, 60);
    let frame = mouse_frame(button, false, cell, encoding);
    let hex: Vec<String> = frame
        .iter()
        .cycle()
        .take(frame.len() * count as usize)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("send-keys -t {target} -H {}", hex.join(" "))
}

/// One mouse report at `cell` in an encoding. `button` is the protocol's
/// button number (0 the left button, 64/65 the wheel). A release keeps the
/// button in SGR (`m`); the legacy encodings cannot name it and send 3. The
/// legacy coordinates are clamped to what one report can carry, which only
/// the wheel relies on (`click_encodable` keeps clicks inside it).
fn mouse_frame(button: u8, release: bool, cell: PointerCell, encoding: MouseEncoding) -> Vec<u8> {
    let legacy_button = 32 + if release { 3 } else { button };
    match encoding {
        MouseEncoding::Sgr => format!(
            "\x1b[<{button};{};{}{}",
            cell.column.saturating_add(1),
            cell.row.saturating_add(1),
            if release { 'm' } else { 'M' }
        )
        .into_bytes(),
        MouseEncoding::Utf8 => {
            let mut bytes = vec![0x1b, b'[', b'M', legacy_button];
            push_utf8_codepoint(&mut bytes, cell.column.min(2014).saturating_add(33));
            push_utf8_codepoint(&mut bytes, cell.row.min(2014).saturating_add(33));
            bytes
        }
        MouseEncoding::X10 => vec![
            0x1b,
            b'[',
            b'M',
            legacy_button,
            cell.column.min(222) as u8 + 33,
            cell.row.min(222) as u8 + 33,
        ],
    }
}

fn push_utf8_codepoint(bytes: &mut Vec<u8>, value: u32) {
    if let Some(ch) = char::from_u32(value) {
        let mut encoded = [0; 4];
        bytes.extend_from_slice(ch.encode_utf8(&mut encoded).as_bytes());
    }
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
        scroll_report()
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
    use super::{
        args, click_args, click_encodable, cursor_following_args, negotiated_args, PointerCell,
        CLICK_PROBE, CURSOR_ROW_OPTION,
    };

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

        // Drag-time selection movement deliberately retains tmux's copy
        // cursor because it is still the mutable endpoint. Completed ranges
        // use cursor_following_args after their endpoints have been frozen.
        let selection = args("=deck-card:", -3).join(" ");
        assert!(!selection.contains(CURSOR_ROW_OPTION));
        assert!(!selection.contains("cursor-down"));
        assert!(!selection.contains("cursor-up"));
    }

    #[test]
    fn negotiated_wheel_is_bounded_hex_and_keeps_history_fallbacks() {
        let args = negotiated_args("=deck-card:", -999, PointerCell { column: 4, row: 7 });
        let joined = args.join(" ");
        assert!(joined.contains("#{pane_in_mode}"));
        assert!(joined.contains("#{mouse_any_flag}"));
        assert!(joined.contains("#{mouse_sgr_flag}"));
        assert!(joined.contains("#{mouse_utf8_flag}"));
        assert!(joined.contains("#{mouse_standard_flag}"));
        assert!(joined.contains("send-keys -t =deck-card: -H"));
        assert!(!joined.contains("send-keys -l"));
        assert!(joined.matches("1b 5b 3c 36 34 3b 35 3b 38 4d").count() >= 60);
        assert!(joined.matches("1b 5b 4d 60 25 28").count() >= 60);
        assert!(joined.contains("scroll-up"));

        let idle = negotiated_args("=deck-card:", 0, PointerCell { column: 0, row: 0 });
        assert!(!idle.join(" ").contains("send-keys -H"));
        assert_eq!(
            PointerCell::clamped(u32::MAX, u32::MAX, 80, 24),
            PointerCell {
                column: 79,
                row: 23
            }
        );
    }

    #[test]
    fn a_click_is_one_press_and_its_release_in_each_encoding() {
        let args = click_args("=deck-card:", PointerCell { column: 4, row: 7 });
        assert_eq!(args[0], "if-shell");
        assert_eq!(args[1], "-F");
        assert!(args[4].contains("#{?pane_in_mode,0,"));
        assert!(args[4].contains("#{mouse_any_flag}"));
        assert_eq!(args.iter().filter(|arg| arg.as_str() == ";").count(), 1);
        assert_eq!(args.last().map(String::as_str), Some(CLICK_PROBE));
        let joined = args.join(" ");
        // SGR: `ESC[<0;5;8M` then `ESC[<0;5;8m`, once each
        assert_eq!(
            joined
                .matches("1b 5b 3c 30 3b 35 3b 38 4d 1b 5b 3c 30 3b 35 3b 38 6d")
                .count(),
            1
        );
        // UTF-8 and X10: press button 0 (0x20), release button 3 (0x23)
        assert_eq!(
            joined
                .matches("1b 5b 4d 20 25 28 1b 5b 4d 23 25 28")
                .count(),
            2
        );
        assert!(!joined.contains("send-keys -l"));
        assert!(!joined.contains("scroll"));
        assert!(!joined.contains("copy-mode"));

        // Past 222 X10 has no byte for the cell: that branch sends nothing,
        // UTF-8 still names it, and SGR names any cell.
        let wide = click_args(
            "=deck-card:",
            PointerCell {
                column: 300,
                row: 7,
            },
        )
        .join(" ");
        assert!(wide.contains("1b 5b 4d 20 c5 8d 28 1b 5b 4d 23 c5 8d 28"));
        assert_eq!(wide.matches("send-keys").count(), 2);
        assert!(click_encodable(
            true,
            false,
            PointerCell {
                column: 5000,
                row: 5000
            }
        ));
        assert!(click_encodable(
            false,
            true,
            PointerCell {
                column: 2014,
                row: 0
            }
        ));
        assert!(!click_encodable(
            false,
            true,
            PointerCell {
                column: 2015,
                row: 0
            }
        ));
        assert!(click_encodable(
            false,
            false,
            PointerCell {
                column: 222,
                row: 222
            }
        ));
        assert!(!click_encodable(
            false,
            false,
            PointerCell {
                column: 0,
                row: 223
            }
        ));
        let far = click_args(
            "=deck-card:",
            PointerCell {
                column: 3000,
                row: 7,
            },
        )
        .join(" ");
        assert_eq!(far.matches("send-keys").count(), 1, "SGR alone can name it");
    }
}
