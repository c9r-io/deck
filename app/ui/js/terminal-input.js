// The production onData boundary: only non-replies reach input-only effects.
// Keep cleanup and writes on the pane's existing queue, including replies that
// arrive during cleanup. Dependencies are the existing layout callbacks.
import { isTerminalAutoReply } from './pure.js';

// Content-free onData shape, never a claim about keyboard/paste provenance.
// Categories: 0 empty, 1 text, 2 control, 3 escape-bearing, 4 bracketed paste.
export function terminalInputDiagnostic(data) {
  const category = !data.length ? 0
    : data.startsWith('\x1b[200~') && data.endsWith('\x1b[201~') ? 4
      : data.includes('\x1b') ? 3 : /[\x00-\x1f\x7f]/.test(data) ? 2 : 1;
  return { category, length: Math.min(data.length, 99999) };
}

export function createTerminalDataHandler({ pane, blocked, onInput, hasSelection,
  cancelSelection, scrolled, goLive, write }) {
  return data => {
    const reply = isTerminalAutoReply(data);
    if (!reply && blocked()) return;
    if (!reply) onInput(data);
    if (!reply && hasSelection()) pane.liveQ = cancelSelection(terminalInputDiagnostic(data));
    if (!reply && scrolled()) pane.liveQ = goLive();
    if (pane.liveQ) {
      const q = pane.liveQ.then(() => write(data));
      pane.liveQ = q;
      q.then(() => { if (pane.liveQ === q) pane.liveQ = null; });
    } else {
      write(data);
    }
  };
}
