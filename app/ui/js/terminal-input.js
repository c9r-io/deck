// The production onData boundary: only non-replies reach input-only effects.
// Keep cleanup and writes on the pane's existing queue, including replies that
// arrive during cleanup. Dependencies are the existing layout callbacks.
//
// Ordering contract. `pane.liveQ` is the ONE tail of the pane's input work. A
// cleanup (selection revoke, return to live, a composition's preparation) is
// APPENDED to that tail and never replaces it: its synchronous frontend part
// has already run when it is appended, and every later write waits for the
// previous tail AND for that cleanup. No cleanup, composition or timer lets a
// later input pass an earlier one. A cleanup that fails counts as finished
// (its owner already handled the failure); the tail itself never rejects, so
// one failure cannot strand what follows. A link clears the tail only while
// it still is the tail.
//
// Target contract. An input is bound, when it is accepted, to the identity
// `bind()` returns (layout.js: this pane's attachment generation, or null
// while an attach has not named one yet). Just before its write, `stale()`
// decides from that same identity; a stale input is cancelled with one closed
// reason through `cancelled` and is never moved to another pane or attachment,
// re-bound or replayed. Each input is ATTEMPTED at most once: a failed write
// is not retried, and an attempt is not a claim that the bytes were delivered.
import { isTerminalAutoReply } from './pure.js';

// Content-free onData shape, never a claim about keyboard/paste provenance.
// Categories: 0 empty, 1 text, 2 control, 3 escape-bearing, 4 bracketed paste.
export function terminalInputDiagnostic(data) {
  const category = !data.length ? 0
    : data.startsWith('\x1b[200~') && data.endsWith('\x1b[201~') ? 4
      : data.includes('\x1b') ? 3 : /[\x00-\x1f\x7f]/.test(data) ? 2 : 1;
  return { category, length: Math.min(data.length, 99999) };
}

const settled = () => {};

// Append an already started cleanup to the pane's input tail.
export function appendInputCleanup(pane, cleanup) {
  const done = Promise.resolve(cleanup).then(settled, settled);
  const previous = pane.liveQ;
  const q = previous ? previous.then(() => done, () => done) : done;
  pane.liveQ = q;
  q.then(() => { if (pane.liveQ === q) pane.liveQ = null; });
  return q;
}

export function createTerminalDataHandler(deps) {
  const { pane, blocked, onInput, hasSelection, cancelSelection, scrolled, goLive, write } = deps;
  // A caller without a target identity (a test of ordering alone) binds nothing.
  const bind = deps.bind || (() => null);
  const send = (data, bound) => {
    const reason = deps.stale ? deps.stale(bound) : null;
    if (reason) { deps.cancelled?.(reason); return undefined; }
    return write(data, bound);
  };
  return data => {
    const reply = isTerminalAutoReply(data);
    if (!reply && blocked()) return;
    if (!reply) onInput(data);
    if (!reply && hasSelection()) appendInputCleanup(pane, cancelSelection(terminalInputDiagnostic(data)));
    if (!reply && scrolled()) appendInputCleanup(pane, goLive());
    const bound = bind();
    if (pane.liveQ) {
      const attempt = () => send(data, bound);
      const q = pane.liveQ.then(attempt, attempt).then(settled, settled);
      pane.liveQ = q;
      q.then(() => { if (pane.liveQ === q) pane.liveQ = null; });
    } else {
      send(data, bound);
    }
  };
}
