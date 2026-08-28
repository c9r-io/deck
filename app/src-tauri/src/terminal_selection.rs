//! Byte-exact extraction of a tmux copy-mode selection.
//!
//! tmux reports both endpoints as cell boundaries. The active endpoint is
//! exclusive and the two endpoints retain gesture direction, so extraction
//! must normalize them before slicing. `capture-pane -J` supplies logical
//! text while preserving real trailing spaces and joining only soft wraps.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SelectionPoint {
    pub(crate) row: i64,
    pub(crate) col: u32,
}

/// Convert a terminal cell boundary to a UTF-8 byte boundary. tmux never
/// places its copy cursor inside a wide grapheme, but snapping such a value
/// to the grapheme start keeps a stale or mouse-derived second-cell position
/// from splitting UTF-8 or a ZWJ/combining cluster.
pub(crate) fn byte_boundary_for_cell(text: &str, cell: u32) -> usize {
    let mut width = 0u32;
    for (offset, grapheme) in text.grapheme_indices(true) {
        let next = width.saturating_add(UnicodeWidthStr::width(grapheme) as u32);
        if cell < next {
            return offset;
        }
        width = next;
    }
    text.len()
}

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

fn without_capture_terminator(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

/// Extract the half-open range between the two tmux selection endpoints.
///
/// `capture(start, end)` must call the production form
/// `capture-pane -p -J -S start -E end`. It deliberately returns raw stdout:
/// the final newline belongs to the capture protocol and is removed once,
/// while hard blank lines and selected trailing spaces remain data.
pub(crate) fn extract_terminal_selection<F>(
    first: SelectionPoint,
    second: SelectionPoint,
    mut capture: F,
) -> Result<String, String>
where
    F: FnMut(i64, i64) -> Result<String, String>,
{
    let (start, end) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };

    let first_row_raw = capture(start.row, start.row)?;
    let first_row = without_capture_terminator(&first_row_raw);
    let start_byte = byte_boundary_for_cell(first_row, start.col);

    if start.row == end.row {
        let end_byte = byte_boundary_for_cell(first_row, end.col).max(start_byte);
        return Ok(first_row[start_byte..end_byte].to_string());
    }

    let last_row_raw = capture(end.row, end.row)?;
    let last_row = without_capture_terminator(&last_row_raw);
    let end_byte = byte_boundary_for_cell(last_row, end.col);
    let full_raw = capture(start.row, end.row)?;
    let full = without_capture_terminator(&full_raw);

    let first_prefix = &first_row[..start_byte];
    let selected = full
        .strip_prefix(first_prefix)
        .ok_or_else(|| "terminal selection changed while it was captured".to_string())?;
    let last_suffix = &last_row[end_byte..];
    let selected = selected
        .strip_suffix(last_suffix)
        .ok_or_else(|| "terminal selection changed while it was captured".to_string())?;
    Ok(selected.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_boundaries_never_split_wide_combining_or_zwj_graphemes() {
        let text = "a中e\u{301}👩‍💻z";
        assert_eq!(byte_boundary_for_cell(text, 1), 1);
        assert_eq!(byte_boundary_for_cell(text, 2), 1);
        assert_eq!(&text[byte_boundary_for_cell(text, 3)..], "e\u{301}👩‍💻z");
        assert_eq!(&text[byte_boundary_for_cell(text, 4)..], "👩‍💻z");
        assert_eq!(
            byte_boundary_for_cell(text, 5),
            byte_boundary_for_cell(text, 4)
        );
        assert_eq!(&text[byte_boundary_for_cell(text, 6)..], "z");
        assert_eq!(cursor_steps_for_cell(text, 1), 1);
        assert_eq!(cursor_steps_for_cell(text, 2), 1);
        assert_eq!(cursor_steps_for_cell(text, 3), 2);
        assert_eq!(cursor_steps_for_cell(text, 4), 3);
        assert_eq!(cursor_steps_for_cell(text, 5), 3);
        assert_eq!(cursor_steps_for_cell(text, 6), 4);
    }

    #[test]
    fn exclusive_end_and_reverse_direction_share_one_half_open_range() {
        let capture = |start, end| match (start, end) {
            (0, 0) => Ok("ABCDEFGHIJKLMNO\n".into()),
            _ => Err("unexpected capture".into()),
        };
        let a = SelectionPoint { row: 0, col: 3 };
        let b = SelectionPoint { row: 0, col: 7 };
        assert_eq!(extract_terminal_selection(a, b, capture).unwrap(), "DEFG");
        assert_eq!(extract_terminal_selection(b, a, capture).unwrap(), "DEFG");
    }

    #[test]
    fn multirow_extraction_preserves_hard_blank_lines_and_trailing_spaces() {
        let capture = |start, end| match (start, end) {
            (0, 0) => Ok("ABCD   \n".into()),
            (2, 2) => Ok("XYZ\n".into()),
            (0, 2) => Ok("ABCD   \n\nXYZ\n".into()),
            _ => Err("unexpected capture".into()),
        };
        let text = extract_terminal_selection(
            SelectionPoint { row: 0, col: 2 },
            SelectionPoint { row: 2, col: 2 },
            capture,
        )
        .unwrap();
        assert_eq!(text, "CD   \n\nXY");
    }
}
