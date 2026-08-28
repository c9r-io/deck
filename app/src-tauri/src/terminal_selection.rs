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
