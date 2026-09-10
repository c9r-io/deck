//! Terminal commands over the attached tmux pane: wheel scrolling, history
//! clearing, the token-bound selection lease state machine (start/update/
//! finish/copy/scroll/cancel) and pane metrics. Pure helpers live in
//! `terminal_selection.rs` / `terminal_scroll.rs`.
//!
//! A drag keeps tmux selection-FREE: tmux repaints the whole selected region
//! after every motion repetition, so re-placing the copy cursor from
//! `top-line` on each pointer move cost ~19 KB of PTY traffic per move on a
//! full-screen selection, while the identical walk with no selection costs
//! nothing. Deck tracks the two endpoints as absolute CONTENT rows
//! (`SelectionPoint`) — tmux's copy cursor is a VISIBLE row, so on a pane
//! that keeps printing it walks off the text the pointer was on — and
//! `materialize_selection` builds the real tmux selection once, in a single
//! command list, at pointerup or at a mid-drag ⌘C. An endpoint that has left
//! the visible frame (every cross-screen drag's anchor) is reached by moving
//! the copy-mode viewport inside that list; it is never clamped to the frame
//! edge, and a placement tmux reports on other rows is refused.
//!
//! The `[selection]` probe prices all of it: one integers-only summary per
//! finished or abandoned drag (always), plus a per-update line behind
//! --debug-logging.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use crate::terminal_selection::{
    copy_cursor_moves, push_copy_cursor, push_tmux_command, CopyCursorMoves, EndpointFrame,
    EndpointPlacement,
};
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

fn parse_terminal_scroll_result(raw: &str) -> Result<TerminalScrollResult, DeckError> {
    let mut fields = raw.trim_end().split('\t');
    let active = fields
        .next()
        .ok_or(DeckError::new(ErrorKind::Other, "scroll-status-invalid"))?
        == "1";
    let cursor_visible = fields
        .next()
        .ok_or(DeckError::new(ErrorKind::Other, "scroll-status-invalid"))?
        == "1";
    if fields.next().is_some() {
        return Err(DeckError::new(ErrorKind::Other, "scroll-status-invalid"));
    }
    Ok(TerminalScrollResult {
        active,
        cursor_visible,
    })
}

#[tauri::command]
pub(crate) fn scroll_session(name: String, lines: i32) -> Result<TerminalScrollResult, DeckError> {
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
pub(crate) fn scroll_bottom(name: String) -> Result<(), DeckError> {
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
    /// The content row on the pane's first visible line, in the coordinates
    /// the selection rows use — the copy-mode SNAPSHOT's, once a lease
    /// exists (`SelectionPoint`); the live history's for a plain status.
    frame_top: u32,
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

/// One selection endpoint in CONTENT coordinates: `absolute_row` counts from
/// the first row of the copy-mode SNAPSHOT's history, so it keeps naming the
/// same text while the pane scrolls. A visible row index does not.
///
/// The coordinate system is the snapshot's, not the live pane's: tmux
/// copy-mode clones the screen when it is entered, and everything it reports
/// or accepts — `scroll_position`, `copy_cursor_y`, `selection_*_y`,
/// `goto-line` — counts from that clone's history size (`snapshot_history`
/// on the lease), while `#{history_size}` keeps growing with output the
/// user cannot see until copy-mode ends. Mixing the two moved every endpoint
/// by the number of lines printed since entry; that was the "drift" 0.6.1
/// measured with `capture-pane`, which reads the LIVE screen, not the clone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionPoint {
    absolute_row: u32,
    col: u32,
}

#[derive(Clone, Debug)]
enum TerminalSelectionLease {
    Cancelled {
        token: u64,
    },
    /// A drag in progress. tmux holds only a copy CURSOR here, never a
    /// selection: moving a cursor is free, while moving it with a selection
    /// active makes tmux repaint the whole selected region after every single
    /// motion repetition (~19 KB down the PTY for a full-screen drag, per
    /// pointer move). Deck owns the two endpoints until pointerup, and
    /// `materialize_selection` builds the real tmux selection once.
    Dragging {
        token: u64,
        anchor: SelectionPoint,
        active: SelectionPoint,
        /// The copy-mode snapshot's history size: the origin of every row
        /// in this lease (`SelectionPoint`).
        snapshot_history: u32,
    },
    Frozen {
        token: u64,
        text: String,
        bytes: u64,
        history_limit: u32,
        snapshot_history: u32,
        selection_start_row: u32,
        selection_start_col: u32,
        selection_end_row: u32,
        selection_end_col: u32,
    },
}

impl TerminalSelectionLease {
    fn token(&self) -> u64 {
        match self {
            Self::Cancelled { token }
            | Self::Dragging { token, .. }
            | Self::Frozen { token, .. } => *token,
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
        Some(TerminalSelectionLease::Dragging { token: current, .. })
            if !frozen && *current == token
    ) || matches!(
        leases.get(name),
        Some(TerminalSelectionLease::Frozen { token: current, .. })
            if frozen && *current == token
    )
}

/// The snapshot content row on the pane's first VISIBLE line.
fn viewport_top(snapshot_history: u32, scroll_position: u32) -> u32 {
    snapshot_history.saturating_sub(scroll_position)
}

fn selection_point(
    snapshot_history: u32,
    scroll_position: u32,
    row: u32,
    col: u32,
) -> SelectionPoint {
    SelectionPoint {
        absolute_row: viewport_top(snapshot_history, scroll_position).saturating_add(row),
        col,
    }
}

/// The frame an endpoint is placed in when the selection is materialized:
/// the current one while its row is visible, otherwise the viewport that
/// shows it (`terminal_selection::endpoint_frame`).
fn endpoint_frame(
    snapshot_history: u32,
    status: &TerminalSelectionStatus,
    point: SelectionPoint,
) -> EndpointFrame {
    crate::terminal_selection::endpoint_frame(
        snapshot_history,
        status.scroll_position,
        status.pane_rows,
        point.absolute_row,
    )
}

/// Lines the pane printed since copy-mode was entered: invisible in the
/// snapshot, but they shift where the snapshot's rows sit in the live grid
/// that `capture-pane` reads.
fn live_offset(snapshot_history: u32, status: &TerminalSelectionStatus) -> u32 {
    status.history_rows.saturating_sub(snapshot_history)
}

/// A drag's status: tmux has no selection of its own until pointerup, so the
/// endpoints deck owns are reported in the exact shape tmux would report them
/// (directional, `start` = anchor, `end` = the cursor, both inclusive of the
/// anchor cell and exclusive of the cursor cell). The frame top and the
/// history edges are the snapshot's, like the endpoints.
fn dragging_selection_status(
    mut status: TerminalSelectionStatus,
    snapshot_history: u32,
    anchor: SelectionPoint,
    active: SelectionPoint,
) -> TerminalSelectionStatus {
    status.selection_present = true;
    status.frame_top = viewport_top(snapshot_history, status.scroll_position);
    status.at_top = status.scroll_position >= snapshot_history && status.cursor_row == 0;
    status.at_bottom =
        status.scroll_position == 0 && status.cursor_row >= status.pane_rows.saturating_sub(1);
    status.selection_start_row = anchor.absolute_row;
    status.selection_start_col = anchor.col;
    status.selection_end_row = active.absolute_row;
    status.selection_end_col = active.col;
    status
}

/// The endpoints deck tracked for a live drag and their snapshot origin, or
/// `selection-missing`.
fn dragging_endpoints(
    name: &str,
    token: u64,
) -> Result<(SelectionPoint, SelectionPoint, u32), DeckError> {
    match terminal_selection_leases().lock_or_recover().get(name) {
        Some(TerminalSelectionLease::Dragging {
            token: current,
            anchor,
            active,
            snapshot_history,
        }) if *current == token => Ok((*anchor, *active, *snapshot_history)),
        _ => Err(DeckError::new(ErrorKind::Other, "selection-missing")),
    }
}

fn frozen_selection_status(
    name: &str,
    token: u64,
    mut status: TerminalSelectionStatus,
) -> Result<TerminalSelectionStatus, DeckError> {
    let leases = terminal_selection_leases().lock_or_recover();
    let Some(TerminalSelectionLease::Frozen {
        token: current,
        snapshot_history,
        selection_start_row,
        selection_start_col,
        selection_end_row,
        selection_end_col,
        ..
    }) = leases.get(name)
    else {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    };
    if *current != token {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    status.selection_present = true;
    status.frame_top = viewport_top(*snapshot_history, status.scroll_position);
    status.selection_start_row = *selection_start_row;
    status.selection_start_col = *selection_start_col;
    status.selection_end_row = *selection_end_row;
    status.selection_end_col = *selection_end_col;
    Ok(status)
}

fn parse_u32_or_zero(raw: Option<&str>) -> u32 {
    raw.and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn terminal_selection_status_for(target: &str) -> Result<TerminalSelectionStatus, DeckError> {
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
        return Err(DeckError::new(
            ErrorKind::Tmux,
            "tmux returned invalid terminal dimensions",
        ));
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
        frame_top: history_rows.saturating_sub(scroll_position),
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
) -> Result<(), DeckError> {
    if actual_cols == expected_cols && actual_rows == expected_rows {
        Ok(())
    } else {
        Err(DeckError::new(
            ErrorKind::Other,
            "selection-dimensions-changed",
        ))
    }
}

/// The SNAPSHOT frame's rows 0..=`through_row` at viewport `scroll_position`,
/// measured the way `terminal_selection::frame_rows` documents. `capture-pane`
/// reads the live screen, whose rows sit `live_offset` lines further down
/// than the snapshot's (the lines printed since copy-mode was entered), so
/// the capture is shifted by that much; below the history limit the live
/// grid only ever appends, so the snapshot's rows keep their live index.
fn frame_rows_through(
    target: &str,
    live_offset: u32,
    scroll_position: u32,
    through_row: u32,
) -> Result<Vec<String>, DeckError> {
    let top = -(scroll_position as i64) - live_offset as i64;
    let bottom = through_row as i64 - scroll_position as i64 - live_offset as i64;
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

// ---------- selection lag / drift probe -----------------------------------
//
// Two reported symptoms need numbers that only this layer can produce: a
// multi-row drag feels sluggish, and the highlight sometimes covers text the
// pointer never crossed. The first is cost: an update re-places the copy
// cursor from `top-line` on EVERY pointer move, so it grows with the row and
// column it has to walk to. The second was the live-vs-snapshot mix-up
// (`SelectionPoint`): lines the pane prints while copy-mode is frozen move
// the live rows `capture-pane` reads, so a plan built from them could walk
// to the wrong snapshot row. The offset is now applied to every capture;
// `drift` keeps counting the output so a misplacement can still be paired
// with it.
//
// So each update records what it asked for, what tmux actually did, and what
// it cost, and one summary line per finished selection reports the worst of
// them. Integers only — the session appears as its per-run `session_tag`.

/// Copy-mode motion repetitions one update issues. tmux redraws the changed
/// selection after each repetition, so this is the update's redraw price:
/// measured against the bundled tmux, a 1-row/5-col update pushes ~0.5 KB
/// down the PTY while a 37-row/110-col one pushes ~19 KB.
fn selection_motions(moves: CopyCursorMoves) -> u32 {
    1 + moves.descend + u32::from(moves.wrap) + moves.descend_after_wrap + moves.steps
}

#[derive(Clone, Copy, Default)]
struct SelectionProbe {
    updates: u32,
    /// Updates whose copy cursor did not land on the row that was asked for.
    misplaced: u32,
    /// Updates during which the pane pushed new lines into the LIVE history
    /// (invisible in the frozen copy-mode snapshot, but they move the rows a
    /// plan's `capture-pane` reads while the plan is being applied).
    drifted: u32,
    /// Largest single-update history growth, in rows.
    worst_drift: u32,
    worst_ms: u32,
    total_ms: u32,
    worst_motions: u32,
    /// Placements whose plan raced pane output, or landed on rows other than
    /// the ones asked for, and had to be rebuilt.
    races: u32,
    /// Placements with an endpoint outside the visible frame, reached by
    /// moving the copy-mode viewport (a cross-screen drag).
    offscreen: u32,
}

fn selection_probes() -> &'static Mutex<HashMap<String, SelectionProbe>> {
    static PROBES: OnceLock<Mutex<HashMap<String, SelectionProbe>>> = OnceLock::new();
    PROBES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32
}

/// What one update asked the backend for, and how long serving it took.
#[derive(Clone, Copy)]
struct SelectionUpdateAsk {
    row: u32,
    col: u32,
    edge_lines: i32,
    ms: u32,
}

/// One update's forensics. `row` is the visible row the pointer asked for;
/// only the ROW is checked against tmux's cursor, because a column inside a
/// wide grapheme legitimately snaps to that grapheme's start. An edge-scroll
/// update moves the cursor on purpose and is never counted as misplaced.
fn record_selection_update(
    name: &str,
    ask: SelectionUpdateAsk,
    before: &TerminalSelectionStatus,
    after: &TerminalSelectionStatus,
    moves: CopyCursorMoves,
) {
    let SelectionUpdateAsk {
        row,
        col,
        edge_lines,
        ms,
    } = ask;
    let motions = selection_motions(moves);
    let drift = after.history_rows.saturating_sub(before.history_rows);
    let misplaced = edge_lines == 0 && after.cursor_row != row;
    {
        let mut probes = selection_probes().lock_or_recover();
        let probe = probes.entry(name.to_string()).or_default();
        probe.updates = probe.updates.saturating_add(1);
        probe.misplaced = probe.misplaced.saturating_add(u32::from(misplaced));
        probe.drifted = probe.drifted.saturating_add(u32::from(drift > 0));
        probe.worst_drift = probe.worst_drift.max(drift);
        probe.worst_ms = probe.worst_ms.max(ms);
        probe.total_ms = probe.total_ms.saturating_add(ms);
        probe.worst_motions = probe.worst_motions.max(motions);
    }
    if crate::diagnostics::debug_logging_enabled() {
        crate::applog::applog(&format!(
            "[selection] {} update ask=r{row}c{col} got=r{}c{} motions={motions} drift={drift} \
             scroll={}->{} edge={edge_lines} ms={ms}",
            crate::applog::session_tag(name),
            after.cursor_row,
            after.cursor_col,
            before.scroll_position,
            after.scroll_position,
        ));
    }
}

/// What `materialize_selection` had to do to build the real tmux selection.
fn record_selection_placement(name: &str, offscreen: bool, races: u32) {
    let mut probes = selection_probes().lock_or_recover();
    let probe = probes.entry(name.to_string()).or_default();
    probe.offscreen = probe.offscreen.saturating_add(u32::from(offscreen));
    probe.races = probe.races.max(races);
}

/// Drain and report one selection's counters. Always logged (one line per
/// completed or abandoned selection, integers only) because it is the record
/// that says whether a drag was slow, whether its frame moved under it, and
/// whether any endpoint missed the row it was given.
fn report_selection_probe(name: &str, outcome: &str) {
    let Some(probe) = selection_probes().lock_or_recover().remove(name) else {
        return;
    };
    if probe.updates == 0 {
        return;
    }
    crate::applog::applog(&format!(
        "[selection] {} {outcome} updates={} misplaced={} drifted={} worst_drift={} \
         worst_motions={} worst_ms={} mean_ms={} races={} offscreen={}",
        crate::applog::session_tag(name),
        probe.updates,
        probe.misplaced,
        probe.drifted,
        probe.worst_drift,
        probe.worst_motions,
        probe.worst_ms,
        probe.total_ms / probe.updates,
        probe.races,
        probe.offscreen,
    ));
}

/// Build the real tmux selection from deck's two content endpoints.
///
/// Both endpoints are placed and `begin-selection` is issued inside ONE tmux
/// command list (`terminal_selection::materialize_args`), which the server
/// runs in a single event-loop pass: no pane output can move the frame
/// between the anchor and the active endpoint, so the two can no longer be
/// measured against different frames. An endpoint outside the visible frame
/// — every cross-screen drag's anchor — is reached by moving the copy-mode
/// viewport inside that same list, never by clamping it to the frame's edge.
/// The list's leading `display-message` reports the history size the list
/// actually ran with; if the `capture-pane` the move plans were built from
/// saw a different one, or tmux reports endpoint rows other than the ones
/// asked for (copy-mode caps `goto-line` at the history it was entered with),
/// the placement is rebuilt, and after the last attempt refused: a selection
/// that names other rows than the drag is never handed out. tmux keeps the
/// resulting anchor pinned to CONTENT, so later output cannot move it.
fn materialize_selection(
    name: &str,
    target: &str,
    snapshot_history: u32,
    anchor: SelectionPoint,
    active: SelectionPoint,
) -> Result<TerminalSelectionStatus, DeckError> {
    const ATTEMPTS: u32 = 3;
    let mut offscreen = false;
    for attempt in 0..ATTEMPTS {
        let status = terminal_selection_status_for(target)?;
        let offset = live_offset(snapshot_history, &status);
        let anchor_frame = endpoint_frame(snapshot_history, &status, anchor);
        let active_frame = endpoint_frame(snapshot_history, &status, active);
        let anchor_rows = frame_rows_through(
            target,
            offset,
            anchor_frame.scroll_position,
            anchor_frame.row,
        )?;
        let active_rows = frame_rows_through(
            target,
            offset,
            active_frame.scroll_position,
            active_frame.row,
        )?;
        let batch = crate::terminal_selection::materialize_args(
            target,
            status.scroll_position,
            EndpointPlacement {
                frame: anchor_frame,
                moves: copy_cursor_moves(&anchor_rows, anchor_frame.row, anchor.col),
            },
            EndpointPlacement {
                frame: active_frame,
                moves: copy_cursor_moves(&active_rows, active_frame.row, active.col),
            },
        );
        let ran_with = tmux_owned(&batch)?;
        let ran_with: u32 = ran_with.trim().parse().unwrap_or(status.history_rows);
        offscreen = anchor_frame.offscreen || active_frame.offscreen;
        let placed = terminal_selection_status_for(target)?;
        // No selection at all is the caller's `selection-missing-cleared`;
        // a selection on the wrong rows is retried, then refused.
        let landed = !placed.selection_present
            || (placed.selection_start_row == anchor.absolute_row
                && placed.selection_end_row == active.absolute_row);
        if ran_with == status.history_rows && landed {
            record_selection_placement(name, offscreen, attempt);
            return Ok(placed);
        }
        if crate::diagnostics::debug_logging_enabled() {
            crate::applog::applog(&format!(
                "[selection] {} placement-retry attempt={attempt} ask=r{}..r{} \
                 got=r{}..r{} present={} hist={}->{} scroll={}->{} frames={}/{}",
                crate::applog::session_tag(name),
                anchor.absolute_row,
                active.absolute_row,
                placed.selection_start_row,
                placed.selection_end_row,
                u8::from(placed.selection_present),
                status.history_rows,
                ran_with,
                status.scroll_position,
                placed.scroll_position,
                anchor_frame.scroll_position,
                active_frame.scroll_position,
            ));
        }
    }
    record_selection_placement(name, offscreen, ATTEMPTS);
    Err(DeckError::new(
        ErrorKind::Other,
        "selection-missing-unreachable",
    ))
}

/// The copy-mode snapshot a drag counts from. Copy-mode is entered here when
/// the pane is not in it (`copy-mode -H`); the snapshot's history size is
/// then read back through tmux's own coordinates — a one-cell selection is
/// begun and cleared inside the same command list, and `selection_start_y`
/// is `snapshot + copy_cursor_y - scroll_position` — because no format
/// exposes it directly and `#{history_size}` is the LIVE pane's.
struct CopyModeSnapshot {
    history: u32,
    scroll_position: u32,
    live_history: u32,
}

fn copy_mode_snapshot(target: &str, enter: bool) -> Result<CopyModeSnapshot, DeckError> {
    let mut batch = Vec::new();
    if enter {
        push_tmux_command(
            &mut batch,
            &["copy-mode".into(), "-H".into(), "-t".into(), target.into()],
        );
    }
    // A selection left by an earlier drag would be repainted by every motion
    // of this one and would outlive its anchor; the probe below needs a
    // clean slate anyway. The drag itself keeps tmux selection-free.
    for action in ["clear-selection", "begin-selection"] {
        push_tmux_command(
            &mut batch,
            &[
                "send-keys".into(),
                "-t".into(),
                target.into(),
                "-X".into(),
                action.into(),
            ],
        );
    }
    push_tmux_command(
        &mut batch,
        &[
            "display-message".into(),
            "-p".into(),
            "-t".into(),
            target.into(),
            "#{selection_start_y}\t#{copy_cursor_y}\t#{scroll_position}\t#{history_size}".into(),
        ],
    );
    push_tmux_command(
        &mut batch,
        &[
            "send-keys".into(),
            "-t".into(),
            target.into(),
            "-X".into(),
            "clear-selection".into(),
        ],
    );
    let raw = tmux_owned(&batch)?;
    let mut f = raw.trim_end().split('\t');
    let anchor_row = parse_u32_or_zero(f.next());
    let cursor_row = parse_u32_or_zero(f.next());
    let scroll_position = parse_u32_or_zero(f.next());
    let live_history = parse_u32_or_zero(f.next());
    Ok(CopyModeSnapshot {
        history: anchor_row
            .saturating_add(scroll_position)
            .saturating_sub(cursor_row),
        scroll_position,
        live_history,
    })
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
) -> Result<TerminalSelectionStatus, DeckError> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if terminal_selection_leases()
        .lock()
        .unwrap()
        .get(&name)
        .is_some_and(|lease| lease.token() >= token)
    {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    let target = pane_target(&name);
    let started = Instant::now();
    let dims = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(dims.pane_cols, dims.pane_rows, grid.cols, grid.rows)?;
    let clamp_row = |row: u32| row.min(dims.pane_rows.saturating_sub(1));
    let clamp_col = |col: u32| col.min(dims.pane_cols.saturating_sub(1));
    let anchor_row = clamp_row(anchor_row);
    let anchor_col = clamp_col(anchor_col);
    let active_row = clamp_row(active_row);
    let active_col = clamp_col(active_col);
    // A wheel-scrolled pane is already in copy-mode at the user's chosen
    // history position (and its snapshot may be older than the pane's live
    // history). Re-entering copy-mode here would jump it back to the live
    // frame and make a downward cross-screen drag impossible.
    let snapshot = copy_mode_snapshot(&target, !dims.active).map_err(|e| {
        DeckError::new(
            e.kind(),
            format!("terminal selection could not start ({})", e.code()),
        )
    })?;
    let rows = frame_rows_through(
        &target,
        snapshot.live_history.saturating_sub(snapshot.history),
        snapshot.scroll_position,
        active_row,
    )?;
    let mut batch = Vec::new();
    // Only the active endpoint reaches tmux while dragging; the anchor is
    // deck's, in content coordinates, until pointerup materializes both.
    let active_moves = push_copy_cursor(&mut batch, &target, &rows, active_row, active_col);
    tmux_owned(&batch).map_err(|e| {
        DeckError::new(
            e.kind(),
            format!("terminal selection could not start ({})", e.code()),
        )
    })?;
    let status = terminal_selection_status_for(&target)?;
    let anchor = selection_point(
        snapshot.history,
        snapshot.scroll_position,
        anchor_row,
        anchor_col,
    );
    let active = selection_point(
        snapshot.history,
        status.scroll_position,
        status.cursor_row,
        status.cursor_col,
    );
    // A new drag starts a new measurement; an abandoned one leaves its own
    // record behind rather than blending into this one.
    report_selection_probe(&name, "restart");
    selection_probes()
        .lock_or_recover()
        .insert(name.clone(), SelectionProbe::default());
    if crate::diagnostics::debug_logging_enabled() {
        crate::applog::applog(&format!(
            "[selection] {} start anchor=r{anchor_row}c{anchor_col} \
             active=r{active_row}c{active_col} got=r{}c{} motions={} scroll={} ms={}",
            crate::applog::session_tag(&name),
            status.cursor_row,
            status.cursor_col,
            selection_motions(active_moves),
            dims.scroll_position,
            elapsed_ms(started),
        ));
    }
    terminal_selection_leases().lock_or_recover().insert(
        name,
        TerminalSelectionLease::Dragging {
            token,
            anchor,
            active,
            snapshot_history: snapshot.history,
        },
    );
    Ok(dragging_selection_status(
        status,
        snapshot.history,
        anchor,
        active,
    ))
}

#[tauri::command]
pub(crate) fn terminal_selection_update(
    name: String,
    token: u64,
    row: u32,
    col: u32,
    edge_lines: i32,
    grid: TerminalSelectionGrid,
) -> Result<TerminalSelectionStatus, DeckError> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    let target = pane_target(&name);
    let started = Instant::now();
    let before = terminal_selection_status_for(&target)?;
    require_terminal_selection_dimensions(
        before.pane_cols,
        before.pane_rows,
        grid.cols,
        grid.rows,
    )?;
    // tmux holds no selection during a drag, only the copy cursor, so
    // selection_present is always 0 here. Copy-mode leaving is the only
    // failure this can see.
    if !before.active {
        return Err(DeckError::new(
            ErrorKind::Other,
            "terminal selection is no longer active",
        ));
    }
    let row = row.min(before.pane_rows.saturating_sub(1));
    let col = col.min(before.pane_cols.saturating_sub(1));
    let (_, _, snapshot_history) = dragging_endpoints(&name, token)?;
    let rows = frame_rows_through(
        &target,
        live_offset(snapshot_history, &before),
        before.scroll_position,
        row,
    )?;
    let mut batch = Vec::new();
    let moves = push_copy_cursor(&mut batch, &target, &rows, row, col);
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
        DeckError::new(
            e.kind(),
            format!("terminal selection could not move ({})", e.code()),
        )
    })?;
    if !selection_token_matches(&name, token, false) {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    let after = terminal_selection_status_for(&target)?;
    record_selection_update(
        &name,
        SelectionUpdateAsk {
            row,
            col,
            edge_lines,
            ms: elapsed_ms(started),
        },
        &before,
        &after,
        moves,
    );
    // The copy cursor IS the active endpoint; reading it back in content
    // coordinates is what keeps an edge-scrolled or grapheme-snapped landing
    // authoritative without a second command.
    let active = selection_point(
        snapshot_history,
        after.scroll_position,
        after.cursor_row,
        after.cursor_col,
    );
    let mut leases = terminal_selection_leases().lock_or_recover();
    let Some(TerminalSelectionLease::Dragging {
        token: current,
        anchor,
        active: tracked,
        ..
    }) = leases.get_mut(&name)
    else {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    };
    if *current != token {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    *tracked = active;
    let anchor = *anchor;
    drop(leases);
    Ok(dragging_selection_status(
        after,
        snapshot_history,
        anchor,
        active,
    ))
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
) -> Result<TerminalSelectionStatus, DeckError> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, false) {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
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
    // survived but refused the selection this builds. Both still read as
    // `selection-missing` to the caller; the suffix only feeds the closed
    // `finish-failed` reason code in the log.
    if !status.active {
        return Err(DeckError::new(
            ErrorKind::Other,
            "selection-missing-inactive",
        ));
    }
    let (anchor, active, snapshot_history) = dragging_endpoints(&name, token)?;
    // The one place a real tmux selection exists: both endpoints placed in a
    // single atomic command list, from the content coordinates deck tracked.
    let status = materialize_selection(&name, &target, snapshot_history, anchor, active)?;
    if !status.selection_present {
        return Err(DeckError::new(
            ErrorKind::Other,
            "selection-missing-cleared",
        ));
    }
    let prefix = terminal_selection_buffer_prefix(token);
    let text = crate::terminal_selection::snapshot_selection(&target, &prefix, |args| {
        tmux_owned(args).map_err(String::from)
    })
    .map_err(|_| DeckError::new(ErrorKind::Other, "snapshot-failed"))?;
    let bytes = text.len() as u64;
    if bytes > MAX_TERMINAL_SELECTION_BYTES {
        return Err(DeckError::new(
            ErrorKind::Other,
            "terminal selection exceeds the 64 MiB clipboard limit; narrow the selection",
        ));
    }
    if !selection_token_matches(&name, token, false) {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
    }
    // The snapshot above is the immutable selection authority from now on.
    // Clear tmux's cursor-bound highlight but keep copy-mode and its viewport;
    // the frontend renders the frozen content coordinates with public cell
    // geometry, so later scroll commands cannot move either endpoint.
    tmux(&["send-keys", "-t", &target, "-X", "clear-selection"])
        .map_err(|_| DeckError::new(ErrorKind::Other, "snapshot-failed"))?;
    terminal_selection_leases().lock_or_recover().insert(
        name.clone(),
        TerminalSelectionLease::Frozen {
            token,
            text,
            bytes,
            history_limit: status.history_limit,
            snapshot_history,
            selection_start_row: status.selection_start_row,
            selection_start_col: status.selection_start_col,
            selection_end_row: status.selection_end_row,
            selection_end_col: status.selection_end_col,
        },
    );
    let viewport = terminal_selection_status_for(&target)?;
    report_selection_probe(&name, "finish");
    frozen_selection_status(&name, token, viewport)
}

#[tauri::command]
pub(crate) fn terminal_selection_copy(
    name: String,
    token: u64,
) -> Result<TerminalSelectionCopy, DeckError> {
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
        Some(TerminalSelectionLease::Dragging {
            token: current,
            anchor,
            active,
            snapshot_history,
        }) if current == token => {
            let target = pane_target(&name);
            if !terminal_selection_status_for(&target)?.active {
                return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
            }
            // ⌘C before pointerup: the drag has no tmux selection yet, so
            // build one from the same endpoints pointerup would use.
            let status = materialize_selection(&name, &target, snapshot_history, anchor, active)?;
            if !status.selection_present {
                return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
            }
            let prefix = terminal_selection_buffer_prefix(token);
            let text = crate::terminal_selection::snapshot_selection(&target, &prefix, |args| {
                tmux_owned(args).map_err(String::from)
            })
            .map_err(|_| DeckError::new(ErrorKind::Other, "snapshot-failed"))?;
            let bytes = text.len() as u64;
            if bytes > MAX_TERMINAL_SELECTION_BYTES {
                return Err(DeckError::new(ErrorKind::Other, "snapshot-failed"));
            }
            Ok(TerminalSelectionCopy {
                text,
                bytes,
                history_limit: status.history_limit,
            })
        }
        _ => Err(DeckError::new(ErrorKind::Other, "selection-missing")),
    }
}

#[tauri::command]
pub(crate) fn terminal_selection_scroll(
    name: String,
    token: u64,
    lines: i32,
) -> Result<TerminalSelectionStatus, DeckError> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    if !selection_token_matches(&name, token, true) {
        return Err(DeckError::new(ErrorKind::Other, "selection-missing"));
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
pub(crate) fn terminal_selection_cancel(name: String, token: u64) -> Result<(), DeckError> {
    let _operation = terminal_selection_operation_lock().lock_or_recover();
    validate_session_name(&name)?;
    let should_cancel = {
        let mut leases = terminal_selection_leases().lock_or_recover();
        let matches = match leases.get(&name) {
            Some(TerminalSelectionLease::Dragging { token: current, .. })
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
        selection_probes().lock_or_recover().remove(&name);
        return Ok(());
    }
    report_selection_probe(&name, "cancel");
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
pub(crate) fn terminal_metrics(name: String) -> Result<TerminalMetrics, DeckError> {
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
            snapshot_history: 90,
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
            frame_top: 90,
            selection_start_row: 0,
            selection_start_col: 0,
            selection_end_row: 0,
            selection_end_col: 0,
        };
        let frozen = frozen_selection_status(&name, 77, status).unwrap();
        assert!(frozen.selection_present);
        assert_eq!(
            frozen.frame_top, 80,
            "the frame top counts from the snapshot the endpoints count from"
        );
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

    fn status_at(history_rows: u32, scroll_position: u32) -> TerminalSelectionStatus {
        TerminalSelectionStatus {
            active: true,
            cursor_visible: true,
            selection_present: false,
            history_rows,
            history_limit: 50000,
            pane_rows: 24,
            pane_cols: 80,
            scroll_position,
            cursor_row: 0,
            cursor_col: 0,
            absolute_row: 0,
            at_top: false,
            at_bottom: false,
            history_at_limit: false,
            frame_top: history_rows.saturating_sub(scroll_position),
            selection_start_row: 0,
            selection_start_col: 0,
            selection_end_row: 0,
            selection_end_col: 0,
        }
    }

    /// The conversion the whole drift fix rests on: every row counts from
    /// the copy-mode SNAPSHOT, so lines the pane prints after entry change
    /// the live history and the capture offset only, never an endpoint.
    #[test]
    fn an_endpoint_kept_as_content_survives_output_arriving_under_it() {
        assert_eq!(viewport_top(100, 0), 100);
        let point = selection_point(100, 0, 5, 7);
        assert_eq!(
            point,
            SelectionPoint {
                absolute_row: 105,
                col: 7,
            }
        );
        // Five lines printed since entry: the snapshot still shows the
        // endpoint on row 5; only the live capture shifts by five.
        assert_eq!(endpoint_frame(100, &status_at(105, 0), point).row, 5);
        assert_eq!(live_offset(100, &status_at(105, 0)), 5);
        assert_eq!(live_offset(100, &status_at(100, 0)), 0);
        // A pane scrolled two rows up into history reads the same way: the
        // endpoint is two rows lower in the frame, on the same snapshot row.
        let scrolled = endpoint_frame(100, &status_at(105, 2), point);
        assert_eq!(
            (scrolled.scroll_position, scrolled.row, scrolled.offscreen),
            (2, 7, false)
        );
    }

    /// The 0.6.1 clamp is gone: an endpoint that left the frame names the
    /// viewport that shows it (the rules live in `terminal_selection`).
    #[test]
    fn an_endpoint_that_left_the_frame_names_the_viewport_that_shows_it() {
        let point = SelectionPoint {
            absolute_row: 105,
            col: 3,
        };
        let off_top = endpoint_frame(106, &status_at(106, 0), point);
        assert_eq!(
            (off_top.scroll_position, off_top.row, off_top.offscreen),
            (1, 0, true)
        );
        let off_bottom = endpoint_frame(200, &status_at(200, 120), point);
        assert_eq!(
            (
                off_bottom.scroll_position,
                off_bottom.row,
                off_bottom.offscreen
            ),
            (95, 0, true)
        );
        let last_row = endpoint_frame(82, &status_at(82, 0), point);
        assert_eq!(
            (last_row.scroll_position, last_row.row, last_row.offscreen),
            (0, 23, false)
        );
    }

    /// A drag reports deck's own endpoints in the shape tmux reports its own:
    /// directional, `start` = anchor, absolute content rows.
    #[test]
    fn a_drag_reports_its_endpoints_the_way_tmux_would() {
        let anchor = SelectionPoint {
            absolute_row: 105,
            col: 7,
        };
        let active = SelectionPoint {
            absolute_row: 102,
            col: 1,
        };
        // Twelve lines printed since entry: the live history says 120, the
        // snapshot 108, and the frame top counts from the snapshot.
        let status = dragging_selection_status(status_at(120, 0), 108, anchor, active);
        assert!(status.selection_present);
        assert_eq!(status.frame_top, 108);
        assert_eq!(
            (
                status.selection_start_row,
                status.selection_start_col,
                status.selection_end_row,
                status.selection_end_col,
            ),
            (105, 7, 102, 1),
            "an upward drag stays directional"
        );
    }

    #[test]
    fn dragging_endpoints_are_readable_only_under_their_own_token() {
        let name = format!("deck-drag-unit-{}", std::process::id());
        let anchor = SelectionPoint {
            absolute_row: 4,
            col: 0,
        };
        let active = SelectionPoint {
            absolute_row: 9,
            col: 6,
        };
        terminal_selection_leases().lock_or_recover().insert(
            name.clone(),
            TerminalSelectionLease::Dragging {
                token: 12,
                anchor,
                active,
                snapshot_history: 40,
            },
        );
        assert!(selection_token_matches(&name, 12, false));
        assert!(!selection_token_matches(&name, 12, true));
        assert_eq!(dragging_endpoints(&name, 12).unwrap(), (anchor, active, 40));
        assert!(dragging_endpoints(&name, 11).is_err());
        assert!(dragging_endpoints("deck-drag-unit-absent", 12).is_err());
        terminal_selection_leases().lock_or_recover().remove(&name);
    }

    /// The motion count is the update's redraw price, so it counts every
    /// repetition the plan issues, not the number of tmux commands.
    #[test]
    fn motion_count_prices_every_repetition_including_the_top_line() {
        assert_eq!(
            selection_motions(CopyCursorMoves {
                descend: 0,
                wrap: false,
                descend_after_wrap: 0,
                steps: 0,
            }),
            1
        );
        assert_eq!(
            selection_motions(CopyCursorMoves {
                descend: 3,
                wrap: true,
                descend_after_wrap: 9,
                steps: 40,
            }),
            54
        );
    }

    /// The probe is the record that says whether a drag was slow, whether the
    /// frame moved under it, and whether an endpoint missed its row.
    #[test]
    fn the_probe_counts_drift_misplacement_and_the_worst_update() {
        let name = format!("deck-probe-unit-{}", std::process::id());
        let moves = CopyCursorMoves {
            descend: 2,
            wrap: false,
            descend_after_wrap: 0,
            steps: 5,
        };
        let ask = |row, edge_lines, ms| SelectionUpdateAsk {
            row,
            col: 0,
            edge_lines,
            ms,
        };
        let mut landed = status_at(100, 0);
        landed.cursor_row = 5;

        // Landed where it was asked, on a still frame.
        record_selection_update(&name, ask(5, 0, 4), &status_at(100, 0), &landed, moves);
        // The pane printed three lines mid-update and the cursor missed.
        record_selection_update(&name, ask(7, 0, 21), &status_at(97, 0), &landed, moves);
        // An edge-scroll update moves the cursor on purpose.
        record_selection_update(&name, ask(9, -3, 6), &status_at(100, 0), &landed, moves);
        record_selection_placement(&name, true, 2);

        let probe = *selection_probes()
            .lock_or_recover()
            .get(&name)
            .expect("probe recorded");
        assert_eq!(probe.updates, 3);
        assert_eq!(probe.misplaced, 1, "only the still-frame miss counts");
        assert_eq!(probe.drifted, 1);
        assert_eq!(probe.worst_drift, 3);
        assert_eq!(probe.worst_ms, 21);
        assert_eq!(probe.total_ms, 31);
        assert_eq!(probe.worst_motions, 8);
        assert_eq!(probe.races, 2);
        assert_eq!(probe.offscreen, 1);

        report_selection_probe(&name, "finish");
        assert!(
            selection_probes().lock_or_recover().get(&name).is_none(),
            "reporting drains the probe"
        );
        // Draining twice, or a selection that never updated, says nothing.
        report_selection_probe(&name, "finish");
        assert!(elapsed_ms(Instant::now()) < 1000);
    }
}
