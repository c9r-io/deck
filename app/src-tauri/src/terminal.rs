//! Terminal commands over the attached tmux pane: wheel scrolling, history
//! clearing, the token-bound selection lease state machine (start/update/
//! finish/copy/scroll/cancel) and pane metrics. Pure helpers live in
//! `terminal_selection.rs` / `terminal_scroll.rs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync::LockRecover;
use crate::tmux::{pane_target, tmux, tmux_owned, validate_session_name};

/// Wheel scrolling is deck-driven: xterm keeps LOCAL selection (mouse mode
/// stays off) and deck translates wheel deltas into tmux copy-mode motion.
/// Returns copy-mode and live-cursor visibility AFTER the scroll, so the UI
/// can update both without waiting for the next poll.
#[derive(Debug, Serialize)]
pub(crate) struct TerminalScrollResult {
    active: bool,
    cursor_visible: bool,
}

fn parse_terminal_scroll_result(raw: &str) -> Result<TerminalScrollResult, String> {
    let mut fields = raw.trim_end().split('\t');
    let active = fields.next().ok_or("scroll-status-invalid")? == "1";
    let cursor_visible = fields.next().ok_or("scroll-status-invalid")? == "1";
    if fields.next().is_some() {
        return Err("scroll-status-invalid".into());
    }
    Ok(TerminalScrollResult {
        active,
        cursor_visible,
    })
}

#[tauri::command]
pub(crate) fn scroll_session(name: String, lines: i32) -> Result<TerminalScrollResult, String> {
    validate_session_name(&name)?;
    let t = pane_target(&name);
    // State test, optional copy-mode entry, movement and post-state report all
    // execute in one tmux server command list. This removes two to three
    // process/IPC round trips from every display-frame scroll update.
    let after = tmux_owned(&crate::terminal_scroll::cursor_following_args(&t, lines))?;
    parse_terminal_scroll_result(&after)
}

/// Leave copy-mode and return to the live view (typing, the scrollback
/// chip, or wheel-to-bottom all end here). A pane that is not in copy-mode
/// is a no-op — tmux's error for that case is deliberately swallowed.
#[tauri::command]
pub(crate) fn scroll_bottom(name: String) -> Result<(), String> {
    validate_session_name(&name)?;
    let target = pane_target(&name);
    let _ = tmux(&["send-keys", "-t", &target, "-X", "cancel"]);
    let _ = tmux(&[
        "set-option",
        "-p",
        "-u",
        "-t",
        &target,
        crate::terminal_scroll::CURSOR_ROW_OPTION,
    ]);
    Ok(())
}

/// Fresh shells accumulate junk history from the attach-time resize
/// reflow (blank lines pushed into scrollback), which made "empty" shells
/// scrollable. Called once for sessions deck itself just started.
#[tauri::command]
pub(crate) fn clear_history(name: String) {
    if validate_session_name(&name).is_err() {
        return;
    }
    let t = pane_target(&name);
    let _ = tmux(&["clear-history", "-t", &t]);
}

/// tmux copy-mode is the sole byte and cross-screen geometry authority. The
/// frontend paints its settled coordinates over xterm; no second scrollback
/// document or private xterm API is involved.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalSelectionStatus {
    active: bool,
    cursor_visible: bool,
    selection_present: bool,
    history_rows: u32,
    history_limit: u32,
    pane_rows: u32,
    pane_cols: u32,
    scroll_position: u32,
    cursor_row: u32,
    cursor_col: u32,
    absolute_row: u64,
    at_top: bool,
    at_bottom: bool,
    history_at_limit: bool,
    selection_start_row: u32,
    selection_start_col: u32,
    selection_end_row: u32,
    selection_end_col: u32,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct TerminalSelectionGrid {
    cols: u32,
    rows: u32,
}

#[derive(Clone, Debug)]
enum TerminalSelectionLease {
    Cancelled {
        token: u64,
    },
    Dragging {
        token: u64,
    },
    Frozen {
        token: u64,
        text: String,
        bytes: u64,
        history_limit: u32,
        selection_start_row: u32,
        selection_start_col: u32,
        selection_end_row: u32,
        selection_end_col: u32,
    },
}

impl TerminalSelectionLease {
    fn token(&self) -> u64 {
        match self {
            Self::Cancelled { token } | Self::Dragging { token } | Self::Frozen { token, .. } => {
                *token
            }
        }
    }
}

fn terminal_selection_leases() -> &'static Mutex<HashMap<String, TerminalSelectionLease>> {
    static LEASES: OnceLock<Mutex<HashMap<String, TerminalSelectionLease>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn terminal_selection_operation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn selection_token_matches(name: &str, token: u64, frozen: bool) -> bool {
    let leases = terminal_selection_leases().lock_or_recover();
    matches!(
        leases.get(name),
        Some(TerminalSelectionLease::Dragging { token: current })
            if !frozen && *current == token
    ) || matches!(
        leases.get(name),
        Some(TerminalSelectionLease::Frozen { token: current, .. })
            if frozen && *current == token
    )
}

fn frozen_selection_status(
    name: &str,
    token: u64,
    mut status: TerminalSelectionStatus,
) -> Result<TerminalSelectionStatus, String> {
    let leases = terminal_selection_leases().lock_or_recover();
    let Some(TerminalSelectionLease::Frozen {
        token: current,
        selection_start_row,
        selection_start_col,
        selection_end_row,
        selection_end_col,
        ..
    }) = leases.get(name)
    else {
        return Err("selection-missing".into());
    };
    if *current != token {
        return Err("selection-missing".into());
    }
    status.selection_present = true;
    status.selection_start_row = *selection_start_row;
    status.selection_start_col = *selection_start_col;
    status.selection_end_row = *selection_end_row;
    status.selection_end_col = *selection_end_col;
    Ok(status)
}

fn parse_u32_or_zero(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn terminal_selection_status_for(target: &str) -> Result<TerminalSelectionStatus, String> {
    let raw = tmux(&[
        "display-message",
        "-p",
        "-t",
        target,
        "#{pane_in_mode}\t#{selection_present}\t#{history_size}\t#{history_limit}\t#{pane_height}\t#{pane_width}\t#{scroll_position}\t#{copy_cursor_y}\t#{copy_cursor_x}\t#{selection_start_y}\t#{selection_start_x}\t#{selection_end_y}\t#{selection_end_x}",
    ])?;
    let mut f = raw.trim_end().split('\t');
    let active = f.next() == Some("1");
    let selection_present = f.next() == Some("1");
    let history_rows = parse_u32_or_zero(f.next());
    let history_limit = parse_u32_or_zero(f.next());
    let pane_rows = parse_u32_or_zero(f.next());
    let pane_cols = parse_u32_or_zero(f.next());
    let scroll_position = parse_u32_or_zero(f.next());
    let cursor_row = parse_u32_or_zero(f.next());
    let cursor_col = parse_u32_or_zero(f.next());
    let selection_start_row = parse_u32_or_zero(f.next());
    let selection_start_col = parse_u32_or_zero(f.next());
    let selection_end_row = parse_u32_or_zero(f.next());
    let selection_end_col = parse_u32_or_zero(f.next());
    if pane_rows == 0 || pane_cols == 0 {
        return Err("tmux returned invalid terminal dimensions".into());
    }
    let visible_start = history_rows.saturating_sub(scroll_position) as u64;
    let absolute_row = visible_start.saturating_add(cursor_row as u64);
    let last_row = history_rows as u64 + pane_rows.saturating_sub(1) as u64;
    Ok(TerminalSelectionStatus {
        active,
        cursor_visible: true,
        selection_present,
        history_rows,
        history_limit,
        pane_rows,
        pane_cols,
        scroll_position,
        cursor_row,
        cursor_col,
        absolute_row,
        at_top: absolute_row == 0,
        at_bottom: absolute_row >= last_row,
        history_at_limit: history_limit > 0 && history_rows >= history_limit,
        selection_start_row,
        selection_start_col,
        selection_end_row,
        selection_end_col,
    })
}

fn require_terminal_selection_dimensions(
    actual_cols: u32,
    actual_rows: u32,
    expected_cols: u32,
    expected_rows: u32,
) -> Result<(), String> {
    if actual_cols == expected_cols && actual_rows == expected_rows {
        Ok(())
    } else {
        Err("selection-dimensions-changed".into())
    }
}

fn push_tmux_command(batch: &mut Vec<String>, command: &[String]) {
    if !batch.is_empty() {
        batch.push(";".into());
    }
    batch.extend(command.iter().cloned());
}

fn push_copy_motion(batch: &mut Vec<String>, target: &str, count: u32, action: &str) {
    if count == 0 {
        return;
    }
    let mut command = vec!["send-keys".into(), "-t".into(), target.into(), "-X".into()];
    if count > 1 {
        command.push("-N".into());
        command.push(count.to_string());
    }
    command.push(action.into());
    push_tmux_command(batch, &command);
}

/// Place tmux's copy cursor on the visible cell (`row`, `col`). The move plan
/// and the tmux motions it may use are documented on
/// `terminal_selection::copy_cursor_moves`.
fn push_copy_cursor(batch: &mut Vec<String>, target: &str, rows: &[String], row: u32, col: u32) {
    let moves = crate::terminal_selection::copy_cursor_moves(rows, row, col);
    push_copy_motion(batch, target, 1, "top-line");
    push_copy_motion(batch, target, moves.descend, "cursor-down");
    if moves.wrap {
        push_copy_motion(batch, target, 1, "cursor-right");
    }
    push_copy_motion(batch, target, moves.descend_after_wrap, "cursor-down");
    push_copy_motion(batch, target, moves.steps, "cursor-right");
}

/// The visible frame's rows 0..=`through_row`, measured the way
/// `terminal_selection::frame_rows` documents.
fn visible_rows_through(
    target: &str,
    scroll_position: u32,
    through_row: u32,
) -> Result<Vec<String>, String> {
    let top = -(scroll_position as i64);
    let bottom = through_row as i64 - scroll_position as i64;
    let captured = tmux(&[
        "capture-pane",
        "-p",
        "-S",
        &top.to_string(),
        "-E",
        &bottom.to_string(),
        "-t",
        target,
    ])?;
    Ok(crate::terminal_selection::frame_rows(
        &captured,
        through_row,
    ))
}

#[tauri::command]
pub(crate) fn terminal_selection_start(
    name: String,
    token: u64,
    anchor_row: u32,
    anchor_col: u32,
    active_row: u32,
    active_col: u32,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if terminal_selection_leases()
        .lock()
        .unwrap()
        .get(&name)
        .is_some_and(|lease| lease.token() >= token)
    {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let dims = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(dims.pane_cols, dims.pane_rows, grid.cols, grid.rows)?;
    let clamp_row = |row: u32| row.min(dims.pane_rows.saturating_sub(1));
    let clamp_col = |col: u32| col.min(dims.pane_cols.saturating_sub(1));
    let anchor_row = clamp_row(anchor_row);
    let anchor_col = clamp_col(anchor_col);
    let active_row = clamp_row(active_row);
    let active_col = clamp_col(active_col);
    let rows = visible_rows_through(&target, dims.scroll_position, anchor_row.max(active_row))?;
    let mut batch = Vec::new();
    // A wheel-scrolled pane is already in copy-mode at the user's chosen
    // history position. Re-entering copy-mode here jumps it back to the live
    // frame and makes a downward cross-screen drag impossible.
    if !dims.active {
        push_tmux_command(
            &mut batch,
            &["copy-mode".into(), "-H".into(), "-t".into(), target.clone()],
        );
    } else if dims.selection_present {
        // begin-selection is a toggle in tmux: invoking it while an older
        // selection is still present clears that selection instead of moving
        // its anchor. This can happen when a second physical drag starts
        // before the first start reply has crossed the webview boundary.
        // Clear explicitly so every start command has restart semantics.
        push_tmux_command(
            &mut batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.clone(),
                "-X".into(),
                "clear-selection".into(),
            ],
        );
    }
    push_copy_cursor(&mut batch, &target, &rows, anchor_row, anchor_col);
    push_tmux_command(
        &mut batch,
        &[
            "send-keys".into(),
            "-t".into(),
            target.clone(),
            "-X".into(),
            "begin-selection".into(),
        ],
    );
    push_copy_cursor(&mut batch, &target, &rows, active_row, active_col);
    tmux_owned(&batch).map_err(|e| {
        format!(
            "terminal selection could not start ({})",
            crate::applog::err_code(&e)
        )
    })?;
    let status = terminal_selection_status_for(&target)?;
    terminal_selection_leases()
        .lock()
        .unwrap()
        .insert(name, TerminalSelectionLease::Dragging { token });
    Ok(status)
}

#[tauri::command]
pub(crate) fn terminal_selection_update(
    name: String,
    token: u64,
    row: u32,
    col: u32,
    edge_lines: i32,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let before = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(
        before.pane_cols,
        before.pane_rows,
        grid.cols,
        grid.rows,
    )?;
    // A freshly begun selection has no selected cells until its cursor first
    // leaves the anchor, so selection_present=0 is valid while a drag is
    // still inside that cell. Moving the copy cursor is what makes it present.
    if !before.active {
        return Err("terminal selection is no longer active".into());
    }
    let row = row.min(before.pane_rows.saturating_sub(1));
    let col = col.min(before.pane_cols.saturating_sub(1));
    let rows = visible_rows_through(&target, before.scroll_position, row)?;
    let mut batch = Vec::new();
    push_copy_cursor(&mut batch, &target, &rows, row, col);
    if edge_lines != 0 {
        push_tmux_command(
            &mut batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.clone(),
                "-X".into(),
                "-N".into(),
                edge_lines.unsigned_abs().clamp(1, 8).to_string(),
                if edge_lines < 0 {
                    "cursor-up".into()
                } else {
                    "cursor-down".into()
                },
            ],
        );
    }
    tmux_owned(&batch).map_err(|e| {
        format!(
            "terminal selection could not move ({})",
            crate::applog::err_code(&e)
        )
    })?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    terminal_selection_status_for(&target)
}

const MAX_TERMINAL_SELECTION_BYTES: u64 = 64 * 1024 * 1024;
static TERMINAL_SELECTION_BUFFER_NONCE: AtomicU64 = AtomicU64::new(0);

fn terminal_selection_buffer_prefix(token: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let nonce = TERMINAL_SELECTION_BUFFER_NONCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "deck-copy-{:x}-{token:x}-{nanos:x}-{nonce:x}-",
        std::process::id()
    )
}

#[derive(Serialize)]
pub(crate) struct TerminalSelectionCopy {
    text: String,
    bytes: u64,
    history_limit: u32,
}

#[tauri::command]
pub(crate) fn terminal_selection_finish(
    name: String,
    token: u64,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    let status = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(
        status.pane_cols,
        status.pane_rows,
        grid.cols,
        grid.rows,
    )?;
    // Field logs show finishes failing ~100–250 ms after a successful start
    // with the frontend token untouched, i.e. tmux itself dropped the
    // selection. Say WHICH half went: the pane left copy-mode, or copy-mode
    // survived with its selection cleared. Both still read as
    // `selection-missing` to the caller; the suffix only feeds the closed
    // `finish-failed` reason code in the log.
    if !status.active {
        return Err("selection-missing-inactive".into());
    }
    if !status.selection_present {
        return Err("selection-missing-cleared".into());
    }
    let prefix = terminal_selection_buffer_prefix(token);
    let text = crate::terminal_selection::snapshot_selection(&target, &prefix, tmux_owned)
        .map_err(|_| "snapshot-failed".to_string())?;
    let bytes = text.len() as u64;
    if bytes > MAX_TERMINAL_SELECTION_BYTES {
        return Err(
            "terminal selection exceeds the 64 MiB clipboard limit; narrow the selection".into(),
        );
    }
    if !selection_token_matches(&name, token, false) {
        return Err("selection-missing".into());
    }
    // The snapshot above is the immutable selection authority from now on.
    // Clear tmux's cursor-bound highlight but keep copy-mode and its viewport;
    // the frontend renders the frozen content coordinates with public cell
    // geometry, so later scroll commands cannot move either endpoint.
    tmux(&["send-keys", "-t", &target, "-X", "clear-selection"])
        .map_err(|_| "snapshot-failed".to_string())?;
    terminal_selection_leases().lock_or_recover().insert(
        name.clone(),
        TerminalSelectionLease::Frozen {
            token,
            text,
            bytes,
            history_limit: status.history_limit,
            selection_start_row: status.selection_start_row,
            selection_start_col: status.selection_start_col,
            selection_end_row: status.selection_end_row,
            selection_end_col: status.selection_end_col,
        },
    );
    let viewport = terminal_selection_status_for(&target)?;
    frozen_selection_status(&name, token, viewport)
}

#[tauri::command]
pub(crate) fn terminal_selection_copy(
    name: String,
    token: u64,
) -> Result<TerminalSelectionCopy, String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    let lease = terminal_selection_leases()
        .lock()
        .unwrap()
        .get(&name)
        .cloned();
    match lease {
        Some(TerminalSelectionLease::Frozen {
            token: current,
            text,
            bytes,
            history_limit,
            ..
        }) if current == token => Ok(TerminalSelectionCopy {
            text,
            bytes,
            history_limit,
        }),
        Some(TerminalSelectionLease::Dragging { token: current }) if current == token => {
            let target = pane_target(&name);
            let status = terminal_selection_status_for(&target)?;
            if !status.active || !status.selection_present {
                return Err("selection-missing".into());
            }
            let prefix = terminal_selection_buffer_prefix(token);
            let text = crate::terminal_selection::snapshot_selection(&target, &prefix, tmux_owned)
                .map_err(|_| "snapshot-failed".to_string())?;
            let bytes = text.len() as u64;
            if bytes > MAX_TERMINAL_SELECTION_BYTES {
                return Err("snapshot-failed".into());
            }
            Ok(TerminalSelectionCopy {
                text,
                bytes,
                history_limit: status.history_limit,
            })
        }
        _ => Err("selection-missing".into()),
    }
}

#[tauri::command]
pub(crate) fn terminal_selection_scroll(
    name: String,
    token: u64,
    lines: i32,
) -> Result<TerminalSelectionStatus, String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, true) {
        return Err("selection-missing".into());
    }
    let target = pane_target(&name);
    // Pointerup froze the selection's bytes and absolute endpoints, so the
    // copy cursor is no longer selection state. Re-anchor it to the live
    // input row just like ordinary wheel scrolling; otherwise it stays on
    // the selected cell while the viewport moves and appears as a detached,
    // fixed cursor. The visibility bit lets xterm hide it once that live row
    // has left the viewport.
    let after = tmux_owned(&crate::terminal_scroll::cursor_following_args(
        &target, lines,
    ))?;
    let scroll = parse_terminal_scroll_result(&after)?;
    let mut viewport = terminal_selection_status_for(&target)?;
    viewport.cursor_visible = scroll.cursor_visible;
    frozen_selection_status(&name, token, viewport)
}

#[tauri::command]
pub(crate) fn terminal_selection_cancel(name: String, token: u64) -> Result<(), String> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    let should_cancel = {
        let mut leases = terminal_selection_leases().lock_or_recover();
        let matches = match leases.get(&name) {
            Some(TerminalSelectionLease::Dragging { token: current })
            | Some(TerminalSelectionLease::Frozen { token: current, .. }) => *current == token,
            Some(TerminalSelectionLease::Cancelled { .. }) => false,
            None => false,
        };
        if matches {
            leases.insert(name.clone(), TerminalSelectionLease::Cancelled { token });
        }
        matches
    };
    if !should_cancel {
        return Ok(());
    }
    let target = pane_target(&name);
    let _ = tmux(&["send-keys", "-t", &target, "-X", "cancel"]);
    let _ = tmux(&[
        "set-option",
        "-p",
        "-u",
        "-t",
        &target,
        crate::terminal_scroll::CURSOR_ROW_OPTION,
    ]);
    Ok(())
}

#[derive(Serialize)]
/// Read-only pane metrics for the real-WKWebView smoke (`ui/test/wk-smoke.mjs`);
/// the production frontend never calls this.
pub(crate) struct TerminalMetrics {
    history_rows: u32,
    history_limit: u32,
    pane_rows: u32,
    pane_cols: u32,
    in_copy_mode: bool,
    scroll_position: u32,
}

#[tauri::command]
pub(crate) fn terminal_metrics(name: String) -> Result<TerminalMetrics, String> {
    validate_session_name(&name)?;
    let status = terminal_selection_status_for(&pane_target(&name))?;
    Ok(TerminalMetrics {
        history_rows: status.history_rows,
        history_limit: status.history_limit,
        pane_rows: status.pane_rows,
        pane_cols: status.pane_cols,
        in_copy_mode: status.active,
        scroll_position: status.scroll_position,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_selection_rejects_a_stale_frontend_grid_instead_of_clamping_it() {
        assert!(require_terminal_selection_dimensions(80, 24, 80, 24).is_ok());
        assert_eq!(
            require_terminal_selection_dimensions(79, 24, 80, 24).unwrap_err(),
            "selection-dimensions-changed"
        );
        assert_eq!(
            require_terminal_selection_dimensions(80, 23, 80, 24).unwrap_err(),
            "selection-dimensions-changed"
        );
    }

    #[test]
    fn copy_cursor_batch_uses_only_top_line_cursor_down_and_cursor_right() {
        let rows: Vec<String> = ["", "", "text row"].iter().map(|l| l.to_string()).collect();
        let mut batch = Vec::new();
        push_copy_cursor(&mut batch, "=card:", &rows, 2, 4);
        assert_eq!(
            batch,
            [
                "send-keys",
                "-t",
                "=card:",
                "-X",
                "top-line",
                ";",
                "send-keys",
                "-t",
                "=card:",
                "-X",
                "cursor-down",
                ";",
                "send-keys",
                "-t",
                "=card:",
                "-X",
                "cursor-right",
                ";",
                "send-keys",
                "-t",
                "=card:",
                "-X",
                "-N",
                "4",
                "cursor-right",
            ]
        );
        // start-of-line / end-of-line / back-to-indentation / cursor-left all
        // leave the visible row on wrapped or wide-character content.
        for banned in [
            "start-of-line",
            "end-of-line",
            "back-to-indentation",
            "cursor-left",
        ] {
            assert!(
                !batch.iter().any(|arg| arg == banned),
                "{banned} is not placement-safe"
            );
        }
    }

    #[test]
    fn terminal_scroll_parser_rejects_truncated_and_extra_state() {
        let active = parse_terminal_scroll_result("1\t0\n").unwrap();
        assert!(active.active);
        assert!(!active.cursor_visible);
        let bottom = parse_terminal_scroll_result("0\t1").unwrap();
        assert!(!bottom.active);
        assert!(bottom.cursor_visible);
        for bad in ["", "1", "1\t0\textra"] {
            assert_eq!(
                parse_terminal_scroll_result(bad).unwrap_err(),
                "scroll-status-invalid"
            );
        }
    }

    #[test]
    fn command_boundaries_reject_invalid_sessions_before_any_tmux_effect() {
        let bad = "bad:name".to_string();
        assert!(
            crate::commands::start_session(bad.clone(), "/tmp".into(), "".into(), false).is_err()
        );
        assert!(scroll_session(bad.clone(), 1).is_err());
        assert!(scroll_bottom(bad.clone()).is_err());
        clear_history(bad.clone());
        assert!(terminal_metrics(bad.clone()).is_err());
        assert!(crate::commands::kill_session(bad.clone()).is_err());

        let grid = TerminalSelectionGrid { cols: 80, rows: 24 };
        assert!(terminal_selection_start(bad.clone(), 1, 0, 0, 0, 0, grid).is_err());
        assert!(terminal_selection_update(bad.clone(), 1, 0, 0, 0, grid).is_err());
        assert!(terminal_selection_finish(bad.clone(), 1, grid).is_err());
        assert!(terminal_selection_copy(bad.clone(), 1).is_err());
        assert!(terminal_selection_scroll(bad.clone(), 1, 1).is_err());
        assert!(terminal_selection_cancel(bad, 1).is_err());
    }

    #[test]
    fn frozen_selection_lease_is_copyable_and_keeps_absolute_endpoints() {
        let name = format!("deck-selection-unit-{}", std::process::id());
        let lease = TerminalSelectionLease::Frozen {
            token: 77,
            text: "exact\ntext".into(),
            bytes: 10,
            history_limit: 50000,
            selection_start_row: 11,
            selection_start_col: 2,
            selection_end_row: 13,
            selection_end_col: 7,
        };
        terminal_selection_leases()
            .lock()
            .unwrap()
            .insert(name.clone(), lease);
        assert!(selection_token_matches(&name, 77, true));
        assert!(!selection_token_matches(&name, 76, true));

        let copy = terminal_selection_copy(name.clone(), 77).unwrap();
        assert_eq!(copy.text, "exact\ntext");
        assert_eq!(copy.bytes, 10);
        assert_eq!(copy.history_limit, 50000);

        let status = TerminalSelectionStatus {
            active: true,
            cursor_visible: false,
            selection_present: false,
            history_rows: 100,
            history_limit: 50000,
            pane_rows: 24,
            pane_cols: 80,
            scroll_position: 10,
            cursor_row: 1,
            cursor_col: 2,
            absolute_row: 91,
            at_top: false,
            at_bottom: false,
            history_at_limit: false,
            selection_start_row: 0,
            selection_start_col: 0,
            selection_end_row: 0,
            selection_end_col: 0,
        };
        let frozen = frozen_selection_status(&name, 77, status).unwrap();
        assert!(frozen.selection_present);
        assert_eq!(
            (
                frozen.selection_start_row,
                frozen.selection_start_col,
                frozen.selection_end_row,
                frozen.selection_end_col,
            ),
            (11, 2, 13, 7)
        );
        assert!(
            frozen_selection_status(&name, 76, TerminalSelectionStatus { ..frozen.clone() })
                .is_err()
        );
        terminal_selection_leases().lock().unwrap().remove(&name);
        assert!(terminal_selection_cancel(name, 77).is_ok());
    }
}
