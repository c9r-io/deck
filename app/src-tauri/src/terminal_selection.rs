//! tmux copy-mode selection helpers.
//!
//! Cursor placement starts from terminal cells, while copying delegates the
//! byte snapshot to tmux itself. In particular, copying must not translate
//! absolute selection rows into `capture-pane` coordinates: pane history may
//! grow between those operations and silently move the coordinates.
//!
//! # Contract
//! A selection endpoint is an absolute content row of the copy-mode SNAPSHOT
//! (tmux clones the screen on entry; `#{history_size}` is the live pane's,
//! `capture-pane` reads the live screen — `terminal.rs` `SelectionPoint`
//! holds the coordinate rules). The copy cursor can only
//! be walked over VISIBLE rows (`copy_cursor_moves`), so an endpoint whose
//! row has left the frame — every cross-screen drag's anchor — is reached by
//! moving the copy-mode viewport with `goto-line` first (`endpoint_frame`),
//! never by clamping it to the frame's edge: the clamp shipped in 0.6.1 and
//! turned every cross-screen selection into one screen (governance 07).
//! `materialize_args` builds the whole placement as ONE tmux command list
//! (`clear-selection`, viewport, anchor walk, `begin-selection`, viewport,
//! active walk) so pane output cannot move the frame between the two walks;
//! its leading `display-message` reports the history the list ran with. tmux
//! pins the anchor to content, so the list may leave the viewport on the
//! active endpoint's frame. `goto-line` neither exits `copy-mode -e` nor
//! moves the cursor's visible row, but copy-mode caps it at the history size
//! seen when copy-mode was entered (bundled tmux 3.7c): the caller verifies
//! the rows tmux reports against the rows it asked for and refuses the
//! selection rather than returning a shortened one.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// tmux's `cursor-right -N` counts graphemes, while pointer geometry is in
/// terminal cells. Convert the requested cell to a grapheme step count so a
/// wide character before the pointer does not shift every later endpoint.
/// A request inside a wide grapheme snaps to that grapheme's start.
pub(crate) fn cursor_steps_for_cell(text: &str, cell: u32) -> u32 {
    let mut width = 0u32;
    let mut steps = 0u32;
    for grapheme in text.graphemes(true) {
        let next = width.saturating_add(UnicodeWidthStr::width(grapheme) as u32);
        if cell < next {
            return steps;
        }
        width = next;
        steps = steps.saturating_add(1);
    }
    steps
}

/// The `copy-mode` moves that land the cursor exactly on visible cell
/// (`row`, `col`), given the visible frame's rows down to `row`.
///
/// Only three tmux copy-mode motions place a cursor predictably: `top-line`
/// (exactly row 0, column 0), `cursor-down` (row-exact, column unreliable)
/// and `cursor-right` (cell-exact from a known column). `start-of-line`,
/// `end-of-line` and `back-to-indentation` walk to the ends of the WRAPPED
/// logical line and so leave the visible row entirely; `cursor-left` lands on
/// the trailing column of a wide grapheme and wraps up out of the row at
/// column 0. None of them may be used here.
///
/// `cursor-down` keeps the column at 0 only once tmux has recorded a desired
/// column for the walk, which it does the first time it steps off a line that
/// is not empty. Until then it snaps the cursor to the end of the line it
/// lands on, and the following `cursor-right` moves then wrap onto later
/// rows. That is the whole bug this plan exists to avoid: a shell pane
/// carries text on the first visible row and selected correctly, while a
/// full-screen agent UI (blank rows at the top of its frame) placed both
/// endpoints on rows the pointer never touched.
///
/// So descend to the last empty row above the first row that carries text,
/// step once to the right — an empty line has no cells, so `cursor-right`
/// wraps to column 0 of the next row — and only then descend the rest. Every
/// `cursor-down` after that starts from a non-empty line and keeps column 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopyCursorMoves {
    /// `cursor-down` count issued from the top line.
    pub(crate) descend: u32,
    /// A single `cursor-right` wrapping out of an empty row into the next one.
    pub(crate) wrap: bool,
    /// `cursor-down` count issued after that wrap.
    pub(crate) descend_after_wrap: u32,
    /// `cursor-right` count that reaches the requested column.
    pub(crate) steps: u32,
}

pub(crate) fn copy_cursor_moves(rows: &[String], row: u32, col: u32) -> CopyCursorMoves {
    let index = (row as usize).min(rows.len().saturating_sub(1));
    let steps = cursor_steps_for_cell(rows.get(index).map_or("", String::as_str), col);
    let first_text = rows
        .get(..=index)
        .and_then(|frame| frame.iter().position(|line| !line.is_empty()));
    match first_text {
        Some(first) if first > 0 => CopyCursorMoves {
            descend: first as u32 - 1,
            wrap: true,
            descend_after_wrap: row - first as u32,
            steps,
        },
        _ => CopyCursorMoves {
            descend: row,
            wrap: false,
            descend_after_wrap: 0,
            steps,
        },
    }
}

/// Split a `capture-pane` frame into rows 0..=`through_row`.
///
/// `capture-pane` drops trailing blanks exactly the way tmux's own line length
/// does, so an empty row here is a row tmux also measures as zero-length —
/// which is what makes both halves of `copy_cursor_moves` sound: the blank
/// row it wraps out of really has no cells, and a step count taken from a row
/// can never run past that row's end onto the next one.
pub(crate) fn frame_rows(captured: &str, through_row: u32) -> Vec<String> {
    let mut rows: Vec<String> = captured
        .strip_suffix('\n')
        .unwrap_or(captured)
        .split('\n')
        .map(str::to_string)
        .collect();
    // A frame that is entirely empty captures as a single empty line.
    rows.resize(through_row as usize + 1, String::new());
    rows
}

/// Where an endpoint is placed: the copy-mode viewport (`scroll_position`)
/// that shows its content row, and the visible row it occupies there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EndpointFrame {
    pub(crate) scroll_position: u32,
    pub(crate) row: u32,
    /// The endpoint lies outside the current frame: the plan reaches it with
    /// `goto-line` before walking to its row.
    pub(crate) offscreen: bool,
}

/// The frame an endpoint at snapshot content row `absolute_row` is placed
/// in (`snapshot_history` is the copy-mode snapshot's history size, the
/// origin of that row). Inside the current frame it keeps that frame;
/// otherwise the viewport is moved so the row sits on the first visible line
/// (or, for a row below history, on its line of the live frame), which is
/// the cheapest walk from `top-line`.
pub(crate) fn endpoint_frame(
    snapshot_history: u32,
    scroll_position: u32,
    pane_rows: u32,
    absolute_row: u32,
) -> EndpointFrame {
    let last = pane_rows.saturating_sub(1);
    let top = snapshot_history.saturating_sub(scroll_position);
    if absolute_row >= top && absolute_row - top <= last {
        return EndpointFrame {
            scroll_position,
            row: absolute_row - top,
            offscreen: false,
        };
    }
    let scroll_position = snapshot_history.saturating_sub(absolute_row);
    let row = absolute_row
        .saturating_sub(snapshot_history - scroll_position)
        .min(last);
    EndpointFrame {
        scroll_position,
        row,
        offscreen: true,
    }
}

pub(crate) fn push_tmux_command(batch: &mut Vec<String>, command: &[String]) {
    if !batch.is_empty() {
        batch.push(";".into());
    }
    batch.extend(command.iter().cloned());
}

fn push_copy_command(batch: &mut Vec<String>, target: &str, action: &[&str]) {
    let mut command = vec![
        "send-keys".to_string(),
        "-t".into(),
        target.into(),
        "-X".into(),
    ];
    command.extend(action.iter().map(|a| (*a).to_string()));
    push_tmux_command(batch, &command);
}

fn push_copy_motion(batch: &mut Vec<String>, target: &str, count: u32, action: &str) {
    match count {
        0 => {}
        1 => push_copy_command(batch, target, &[action]),
        _ => push_copy_command(batch, target, &["-N", &count.to_string(), action]),
    }
}

/// Place tmux's copy cursor on the visible cell (`row`, `col`) of the frame
/// `rows` describes. The move plan and the tmux motions it may use are
/// documented on `copy_cursor_moves`. Returns the plan it issued: its
/// repetition count is what a selection update actually costs, because tmux
/// redraws the changed selection after EVERY repetition.
// The tmux contract tests include this module by path and use the pure
// builders only; the production caller is `terminal.rs`.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn push_copy_cursor(
    batch: &mut Vec<String>,
    target: &str,
    rows: &[String],
    row: u32,
    col: u32,
) -> CopyCursorMoves {
    let moves = copy_cursor_moves(rows, row, col);
    push_copy_cursor_moves(batch, target, moves);
    moves
}

fn push_copy_cursor_moves(batch: &mut Vec<String>, target: &str, moves: CopyCursorMoves) {
    push_copy_motion(batch, target, 1, "top-line");
    push_copy_motion(batch, target, moves.descend, "cursor-down");
    if moves.wrap {
        push_copy_motion(batch, target, 1, "cursor-right");
    }
    push_copy_motion(batch, target, moves.descend_after_wrap, "cursor-down");
    push_copy_motion(batch, target, moves.steps, "cursor-right");
}

/// One endpoint of a materialized selection: its frame and the walk that
/// reaches its cell inside that frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EndpointPlacement {
    pub(crate) frame: EndpointFrame,
    pub(crate) moves: CopyCursorMoves,
}

/// The ONE tmux command list that builds the real selection from two content
/// endpoints (see the module contract). `current_scroll` is the viewport the
/// pane shows now, so a viewport move is issued only when a frame differs
/// from the one before it.
pub(crate) fn materialize_args(
    target: &str,
    current_scroll: u32,
    anchor: EndpointPlacement,
    active: EndpointPlacement,
) -> Vec<String> {
    let mut batch = vec![
        "display-message".to_string(),
        "-p".into(),
        "-t".into(),
        target.into(),
        "#{history_size}".into(),
    ];
    // begin-selection is a toggle, so a previous attempt's selection must go
    // before this one starts.
    push_copy_command(&mut batch, target, &["clear-selection"]);
    if anchor.frame.scroll_position != current_scroll {
        push_copy_command(
            &mut batch,
            target,
            &["goto-line", &anchor.frame.scroll_position.to_string()],
        );
    }
    push_copy_cursor_moves(&mut batch, target, anchor.moves);
    push_copy_command(&mut batch, target, &["begin-selection"]);
    if active.frame.scroll_position != anchor.frame.scroll_position {
        push_copy_command(
            &mut batch,
            target,
            &["goto-line", &active.frame.scroll_position.to_string()],
        );
    }
    push_copy_cursor_moves(&mut batch, target, active.moves);
    batch
}

/// Snapshot the current selection into a uniquely-prefixed tmux paste buffer,
/// read its exact bytes, and delete it. The first tmux command freezes the
/// selection atomically in the server event loop; later pane output cannot
/// change that buffer.
///
/// tmux appends a server-global numeric suffix to `prefix`, so the buffer list
/// is used to resolve the exact name. A missing buffer means the selection
/// disappeared before the snapshot command; it must never fall back to some
/// unrelated top buffer.
pub(crate) fn snapshot_selection<F>(
    target: &str,
    prefix: &str,
    mut run_tmux: F,
) -> Result<String, String>
where
    F: FnMut(&[String]) -> Result<String, String>,
{
    run_tmux(&[
        "send-keys".into(),
        "-t".into(),
        target.into(),
        "-X".into(),
        "copy-selection-no-clear".into(),
        "-C".into(),
        prefix.into(),
    ])?;
    let names = run_tmux(&["list-buffers".into(), "-F".into(), "#{buffer_name}".into()])?;
    let mut matches = names.lines().filter(|name| name.starts_with(prefix));
    let buffer = matches
        .next()
        .ok_or_else(|| "tmux did not create a terminal selection snapshot".to_string())?;
    if matches.next().is_some() {
        return Err("tmux returned ambiguous terminal selection snapshots".into());
    }

    let shown = run_tmux(&["show-buffer".into(), "-b".into(), buffer.into()]);
    let deleted = run_tmux(&["delete-buffer".into(), "-b".into(), buffer.into()]);
    match (shown, deleted) {
        (Ok(text), Ok(_)) => Ok(text),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_steps_snap_wide_combining_and_zwj_cells_to_graphemes() {
        let text = "a中e\u{301}👩‍💻z";
        assert_eq!(cursor_steps_for_cell(text, 1), 1);
        assert_eq!(cursor_steps_for_cell(text, 2), 1);
        assert_eq!(cursor_steps_for_cell(text, 3), 2);
        assert_eq!(cursor_steps_for_cell(text, 4), 3);
        assert_eq!(cursor_steps_for_cell(text, 5), 3);
        assert_eq!(cursor_steps_for_cell(text, 6), 4);
    }

    fn frame(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    #[test]
    fn text_on_the_first_visible_row_descends_straight_to_the_cell() {
        let rows = frame(&["shell prompt", "", "line three"]);
        assert_eq!(
            copy_cursor_moves(&rows, 2, 5),
            CopyCursorMoves {
                descend: 2,
                wrap: false,
                descend_after_wrap: 0,
                steps: 5,
            }
        );
    }

    #[test]
    fn blank_rows_above_the_text_are_left_by_wrapping_not_by_a_column_move() {
        // The agent-UI frame: two empty rows, then the row the pointer is on.
        // Descending blindly would snap the column to the end of a line and
        // wrap the following cursor-right onto a row the pointer never hit.
        let rows = frame(&["", "", "改好了，一个词的改动：PR #2。", "", "英文 README"]);
        assert_eq!(
            copy_cursor_moves(&rows, 2, 0),
            CopyCursorMoves {
                descend: 1,
                wrap: true,
                descend_after_wrap: 0,
                steps: 0,
            }
        );
        assert_eq!(
            copy_cursor_moves(&rows, 4, 8),
            CopyCursorMoves {
                descend: 1,
                wrap: true,
                descend_after_wrap: 2,
                // "英文 " is five columns wide but three graphemes.
                steps: 6,
            }
        );
    }

    #[test]
    fn a_single_blank_row_above_the_text_wraps_without_descending_first() {
        let rows = frame(&["", "line two"]);
        assert_eq!(
            copy_cursor_moves(&rows, 1, 4),
            CopyCursorMoves {
                descend: 0,
                wrap: true,
                descend_after_wrap: 0,
                steps: 4,
            }
        );
    }

    #[test]
    fn an_entirely_blank_frame_needs_no_wrap_because_every_row_ends_at_column_zero() {
        let rows = frame(&["", "", ""]);
        assert_eq!(
            copy_cursor_moves(&rows, 2, 7),
            CopyCursorMoves {
                descend: 2,
                wrap: false,
                descend_after_wrap: 0,
                steps: 0,
            }
        );
    }

    #[test]
    fn steps_come_from_the_target_row_so_wide_characters_above_it_never_shift_it() {
        let rows = frame(&["", "中文中文中文", "ascii row here"]);
        assert_eq!(copy_cursor_moves(&rows, 1, 4).steps, 2);
        assert_eq!(copy_cursor_moves(&rows, 2, 4).steps, 4);
    }

    #[test]
    fn a_short_frame_never_panics_and_stays_on_the_no_wrap_path() {
        assert_eq!(
            copy_cursor_moves(&[], 3, 9),
            CopyCursorMoves {
                descend: 3,
                wrap: false,
                descend_after_wrap: 0,
                steps: 0,
            }
        );
    }

    #[test]
    fn frame_rows_keep_blank_rows_and_pad_a_short_capture() {
        assert_eq!(frame_rows("one\ntwo\n", 1), frame(&["one", "two"]));
        // The last visible row being blank must stay a row, not vanish with
        // the capture's terminating newline.
        assert_eq!(frame_rows("one\n\n", 1), frame(&["one", ""]));
        // An entirely empty frame captures as one empty line.
        assert_eq!(frame_rows("\n", 2), frame(&["", "", ""]));
        assert_eq!(frame_rows("", 1), frame(&["", ""]));
    }

    #[test]
    fn a_padded_short_capture_never_invents_a_wrap() {
        // Padding is blank, so the plan stays on the no-wrap path rather than
        // wrapping out of a row it never actually saw.
        let rows = frame_rows("", 3);
        assert_eq!(
            copy_cursor_moves(&rows, 3, 5),
            CopyCursorMoves {
                descend: 3,
                wrap: false,
                descend_after_wrap: 0,
                steps: 0,
            }
        );
    }

    #[test]
    fn selection_snapshot_resolves_reads_and_deletes_its_own_buffer() {
        let mut commands = Vec::new();
        let text = snapshot_selection("=deck-card:", "deck-copy-abc-", |args| {
            commands.push(args.to_vec());
            match args.first().map(String::as_str) {
                Some("list-buffers") => Ok("other0\ndeck-copy-abc-42\n".into()),
                Some("show-buffer") => Ok("exact text  \n".into()),
                _ => Ok(String::new()),
            }
        })
        .unwrap();
        assert_eq!(text, "exact text  \n");
        assert_eq!(commands[0][4], "copy-selection-no-clear");
        assert_eq!(commands[0][5], "-C");
        assert_eq!(commands[2], ["show-buffer", "-b", "deck-copy-abc-42"]);
        assert_eq!(commands[3], ["delete-buffer", "-b", "deck-copy-abc-42"]);
    }

    #[test]
    fn vanished_selection_never_falls_back_to_an_unrelated_buffer() {
        let mut calls = 0;
        let error = snapshot_selection("=deck-card:", "deck-copy-missing-", |args| {
            calls += 1;
            if args.first().map(String::as_str) == Some("list-buffers") {
                Ok("unrelated0\n".into())
            } else {
                Ok(String::new())
            }
        })
        .unwrap_err();
        assert_eq!(error, "tmux did not create a terminal selection snapshot");
        assert_eq!(calls, 2);
    }

    /// The conversion the drift fix rests on: a visible row means a different
    /// content row once the pane has printed, so an endpoint is kept as
    /// content and converted back against the CURRENT frame.
    #[test]
    fn an_endpoint_inside_the_frame_keeps_the_current_viewport() {
        // history 100, live frame: content row 105 is visible row 5.
        assert_eq!(
            endpoint_frame(100, 0, 24, 105),
            EndpointFrame {
                scroll_position: 0,
                row: 5,
                offscreen: false
            }
        );
        // Five lines printed: the same text is now five rows higher.
        assert_eq!(endpoint_frame(105, 0, 24, 105).row, 0);
        assert_eq!(endpoint_frame(103, 0, 24, 105).row, 2);
        // A pane scrolled up into history reads the same way.
        assert_eq!(
            endpoint_frame(120, 20, 24, 105),
            EndpointFrame {
                scroll_position: 20,
                row: 5,
                offscreen: false
            }
        );
        // Exactly the last visible row is still inside the frame.
        assert_eq!(endpoint_frame(82, 0, 24, 105).row, 23);
        assert!(!endpoint_frame(82, 0, 24, 105).offscreen);
    }

    /// The 0.6.1 clamp is gone: an endpoint outside the frame names the
    /// viewport that shows it instead of the edge the overlay clips to.
    #[test]
    fn an_endpoint_outside_the_frame_names_the_viewport_that_shows_it() {
        // Scrolled off the top by one line: goto-line 1 puts it on row 0.
        assert_eq!(
            endpoint_frame(106, 0, 24, 105),
            EndpointFrame {
                scroll_position: 1,
                row: 0,
                offscreen: true
            }
        );
        // The smoke's upward drag: anchor on the live frame's last row,
        // the viewport now 130 rows up into history.
        assert_eq!(
            endpoint_frame(2600, 130, 24, 2623),
            EndpointFrame {
                scroll_position: 0,
                row: 23,
                offscreen: true
            }
        );
        // Below the frame but still in history: row 0 of its own viewport.
        assert_eq!(
            endpoint_frame(200, 120, 24, 105),
            EndpointFrame {
                scroll_position: 95,
                row: 0,
                offscreen: true
            }
        );
        // Below history: the live frame, clamped to its last row.
        assert_eq!(endpoint_frame(100, 60, 24, 130).scroll_position, 0);
        assert_eq!(endpoint_frame(100, 60, 24, 130).row, 23);
    }

    fn placement(scroll_position: u32, row: u32, offscreen: bool, steps: u32) -> EndpointPlacement {
        EndpointPlacement {
            frame: EndpointFrame {
                scroll_position,
                row,
                offscreen,
            },
            moves: CopyCursorMoves {
                descend: row,
                wrap: false,
                descend_after_wrap: 0,
                steps,
            },
        }
    }

    fn joined(batch: &[String]) -> Vec<String> {
        batch.join(" ").split(" ; ").map(str::to_string).collect()
    }

    #[test]
    fn materialize_list_moves_the_viewport_only_between_differing_frames() {
        // Both endpoints in the current frame: no goto-line at all.
        let same = joined(&materialize_args(
            "=t:",
            7,
            placement(7, 2, false, 3),
            placement(7, 5, false, 0),
        ));
        assert_eq!(same[0], "display-message -p -t =t: #{history_size}");
        assert_eq!(same[1], "send-keys -t =t: -X clear-selection");
        assert!(!same.iter().any(|c| c.contains("goto-line")));
        assert_eq!(
            same[2..],
            [
                "send-keys -t =t: -X top-line",
                "send-keys -t =t: -X -N 2 cursor-down",
                "send-keys -t =t: -X -N 3 cursor-right",
                "send-keys -t =t: -X begin-selection",
                "send-keys -t =t: -X top-line",
                "send-keys -t =t: -X -N 5 cursor-down",
            ]
        );

        // The upward drag: anchor back on the live frame, active on the
        // current history frame; the list ends on the active frame.
        let up = joined(&materialize_args(
            "=t:",
            130,
            placement(0, 23, true, 7),
            placement(130, 0, false, 7),
        ));
        let gotos: Vec<&String> = up.iter().filter(|c| c.contains("goto-line")).collect();
        assert_eq!(
            gotos,
            [
                "send-keys -t =t: -X goto-line 0",
                "send-keys -t =t: -X goto-line 130"
            ]
        );
        let begin = up
            .iter()
            .position(|c| c.ends_with("begin-selection"))
            .unwrap();
        assert!(up[..begin].iter().any(|c| c.ends_with("goto-line 0")));
        assert!(up[begin..].iter().any(|c| c.ends_with("goto-line 130")));

        // Two endpoints on the same history frame that is not the current
        // one: one viewport move before the anchor, none before the active.
        let deep = joined(&materialize_args(
            "=t:",
            0,
            placement(400, 0, true, 0),
            placement(400, 9, true, 0),
        ));
        assert_eq!(deep.iter().filter(|c| c.contains("goto-line")).count(), 1);
    }
}
