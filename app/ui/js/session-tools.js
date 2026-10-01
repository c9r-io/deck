// session-tools.js — the session workspace's ONE right-hand tool slot
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// Scheduled prompts (scheduler.js), the card scratchpad while the session
// workspace hosts it (board.js) and Local Translation (translation-lens.js)
// share one drawer to the right of the panes; at most one is open. This
// module records only WHICH tool holds the slot. Each owner keeps its own
// state (queue cache / drafts / expandedRows, bufferTargetId / bufferUiEpoch,
// TranslationLensModel) and its own close, which must be idempotent and run
// its full cleanup — never a bare `hidden = true`.
//
// - An owner's open calls `claimSessionTool(name)` BEFORE it shows: every
//   other owner closes with reason 'replace', and transient popups close.
// - An owner's close calls `releaseSessionTool(name)`.
// - `closeSessionTools(reason)` closes EVERY owner and popup, whatever the
//   slot believes is open: it is what leaving the session workspace calls
//   (leaveSessionView, so project switch, Board, Attention, another session,
//   a removed card). Only a user's own close (reason 'user') may hand focus
//   back to a terminal; 'replace' and 'leave' never do — on leave the pane
//   is about to be disposed.
// Pane focus inside one session layout is NOT a leave: nothing here runs.
// The scratchpad on the Board / Attention views is not a session tool; its
// close is still idempotent, so leaving closes it exactly as before.
// DOM-free: the drawer geometry is CSS (`.session-tool`,
// `--session-tool-width`) plus the one overlay class layout.js toggles.

export const SESSION_TOOLS = ['queue', 'buffer', 'translation'];

export function createSessionToolSlot() {
  const owners = new Map();
  const popups = new Set();
  let active = null;
  const closePopups = () => { for (const close of popups) close(); };
  return {
    register(name, close) {
      if (!SESSION_TOOLS.includes(name)) throw new Error(`unknown session tool ${name}`);
      owners.set(name, close);
    },
    registerPopup(close) { popups.add(close); },
    claim(name) {
      for (const [other, close] of owners) if (other !== name) close('replace');
      closePopups();
      active = name;
    },
    release(name) { if (active === name) active = null; },
    closeAll(reason = 'leave') {
      active = null;
      for (const close of owners.values()) close(reason);
      closePopups();
    },
    active: () => active,
  };
}

const slot = createSessionToolSlot();
export const registerSessionTool = (name, close) => slot.register(name, close);
export const registerSessionPopup = close => slot.registerPopup(close);
export const claimSessionTool = name => slot.claim(name);
export const releaseSessionTool = name => slot.release(name);
export const closeSessionTools = (reason = 'leave') => slot.closeAll(reason);
export const activeSessionTool = () => slot.active();
