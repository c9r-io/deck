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
// `bind(reply)` returns (layout.js: an attachment generation, or the pane's
// own attach request while its reply has not named one yet). An identity that
// is still being named carries `wait`; that wait joins the tail like a
// cleanup, so the input and everything after it keep their order. Just before
// its write, `stale()` decides from that same identity; a stale input is
// cancelled with one closed reason through `cancelled` and is never moved to
// another pane or attachment, re-bound or replayed. Each input is ATTEMPTED
// at most once: a failed write is not retried, and an attempt is not a claim
// that the bytes were delivered.
// Programmatic input uses the returned handler's `insert` entry. It is always
// user input (never a terminal reply), with the same target, cleanup and tail.
// A function argument prepares bytes asynchronously, after the target is bound;
// its preparation reserves its place before later input. Empty/failed preparation
// writes nothing and releases the tail. Callers own completion-mirror updates.
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
  const accept = (data, reply, mirror) => {
    if (!reply && blocked()) return false;
    const bound = bind(reply);
    if (mirror && !reply) onInput(data);
    if (!reply && hasSelection()) appendInputCleanup(pane, cancelSelection(terminalInputDiagnostic(typeof data === 'string' ? data : '')));
    if (!reply && scrolled()) appendInputCleanup(pane, goLive());
    if (bound?.wait) appendInputCleanup(pane, bound.wait);
    if (typeof data === 'function') {
      const prepare = data;
      data = '';
      appendInputCleanup(pane, Promise.resolve().then(prepare).then(value => { data = value; }));
    }
    const attempt = () => { if (data) return send(data, bound); };
    if (pane.liveQ) {
      const q = pane.liveQ.then(attempt, attempt).then(settled, settled);
      pane.liveQ = q;
      q.then(() => { if (pane.liveQ === q) pane.liveQ = null; });
    } else {
      attempt();
    }
    return true;
  };
  const handle = data => accept(data, isTerminalAutoReply(data), true);
  handle.insert = data => accept(data, false, false);
  return handle;
}
