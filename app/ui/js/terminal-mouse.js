// tmux enables application mouse negotiation, but Deck owns local pointer
// selection. Suppress the outer client's mouse-only mode requests through
// xterm's public parser API. Actual wheel ownership is checked by the backend
// against the inner pane at dispatch, never inferred from the outer terminal.
// Bundled tmux emits these modes separately; mixed requests must fall through
// intact so unrelated terminal modes are never accidentally swallowed.
const mouseModes = new Set([9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]);

export function keepLocalTerminalMouse(term) {
  return term.parser.registerCsiHandler({ prefix: '?', final: 'h' }, params =>
    params.length > 0 && params.every(mode => mouseModes.has(mode)));
}
