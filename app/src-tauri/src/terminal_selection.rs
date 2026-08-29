//! tmux copy-mode selection helpers.
//!
//! Cursor placement starts from terminal cells, while copying delegates the
//! byte snapshot to tmux itself. In particular, copying must not translate
//! absolute selection rows into `capture-pane` coordinates: pane history may
//! grow between those operations and silently move the coordinates.

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
}
