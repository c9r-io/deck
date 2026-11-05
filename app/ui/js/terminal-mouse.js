// tmux enables application mouse negotiation, but Deck owns local pointer
// selection. Suppress the outer client's mouse-only mode requests through
// xterm's public parser API. Actual wheel ownership is checked by the backend
// against the inner pane at dispatch, never inferred from the outer terminal.
// Bundled tmux emits these modes separately; mixed requests must fall through
// intact so unrelated terminal modes are never accidentally swallowed.
//
// A plain click Deck does not use itself (selection.js decides which) is
// offered to the pane's program the same way: `forwardTerminalClick` asks the
// backend (`click_session`), which sends a press and its release only to a
// program that asked for button reports, outside copy-mode, through the MCP
// input fence. The answer is one closed word: 'sent', 'local' (nothing to
// send), 'stale' (the grid changed; the click is dropped, never moved to
// another cell), 'refused' (MCP holds the terminal) or 'failed'. Every word
// is logged, 'local' only with --debug-logging, and announced to the page.
const mouseModes = new Set([9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]);

export function keepLocalTerminalMouse(term) {
  return term.parser.registerCsiHandler({ prefix: '?', final: 'h' }, params =>
    params.length > 0 && params.every(mode => mouseModes.has(mode)));
}

const clickAnswers = new Set(['sent', 'local', 'stale', 'refused']);

/** The closed outcome word of one `click_session` answer. */
export const terminalClickOutcome = reply =>
  (clickAnswers.has(reply?.outcome) ? reply.outcome : 'failed');

/* `cell` is zero-based, placed against `grid` (the xterm size at release).
   The pane's confirmed size must still be that grid, or the backend refuses
   the cell rather than clamp it. */
export async function forwardTerminalClick({ pane, cell, grid, invoke, log, debugLog, announce }) {
  let outcome = 'failed';
  try {
    if (pane.syncSize && !(await pane.syncSize())) outcome = 'stale';
    else outcome = terminalClickOutcome(await invoke('click_session', {
      name: pane.session, column: cell.col, row: cell.row, cols: grid.cols, rows: grid.rows,
    }));
  } catch (_) {
    outcome = 'failed';
  }
  (outcome === 'local' ? debugLog : log)('terminal-click', outcome);
  announce?.(outcome);
  return outcome;
}
