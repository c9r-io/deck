// Always-on, content-free IME commit probe for one terminal pane.
//
// Why. Users see an IME commit arrive as completely different text, and
// switching to another card (which disposes the pane's xterm and its hidden
// textarea) makes it go away. xterm 5.5 never sends `compositionend.data`: it
// remembers the textarea's length at `compositionstart` and, on a timer,
// sends `textarea.value.substring(start)`. If the caret is not at the end of
// the textarea, or its value changed under the composition, it sends other
// text. This probe tells, per commit, whether what xterm sent is what the IME
// committed, and records the textarea state that would explain a mismatch.
//
// Contract.
// - Content-free: text is compared in memory and dropped; only fixed labels,
//   lengths, offsets and flags are logged. In memory it holds the textarea
//   text seen at compositionstart only until that commit is judged, and the
//   last committed string only to answer "was the emitted text the previous
//   commit"; both are dropped on dispose.
// - Observation only: it never changes an event, the textarea, focus or the
//   bytes written; it reads `value`, `selectionStart`, `selectionEnd`.
//   A textarea without a string `value` (headless tests) skips those checks.
// - Volume: a match is counted, never logged. Each anomaly label logs at most
//   CAP lines per pane; `ime-summary` (compositions, non-matches) is logged
//   once when the pane is disposed, if it composed at all.
// - Attribution: emitted data is collected for WINDOW_MS after a commit (xterm
//   sends from a zero-delay timer). Data that starts with the commit is a
//   match even if more input follows inside the window. Terminal replies are
//   ignored. A commit is judged once.
//
// Logged lines (diagnostics.rs closed policies):
//   ime-commit  <missing|partial|differs|doubled|extra>  a=committed len  b=emitted len
//   ime-context <caret-end|caret-inside|range|unknown>   a=flags          b=textarea len at start
//      flags: 1 emitted == previous commit · 2 emitted occurs earlier in the
//      textarea · 4 the commit is not at the composition's start offset ·
//      8 the next composition started before this one was judged ·
//      16 blur while composing · 32 textarea shrank during the composition
//   ime-textarea <caret-inside|range> a=caret distance from end  b=textarea len   (at compositionstart)
//   ime-state   <blur-composing|start-while-composing|end-without-start>
//   ime-summary a=compositions b=non-matches
import { isTerminalAutoReply } from './pure.js';

export const IME_PROBE_CAP = 20;
export const IME_PROBE_WINDOW_MS = 150;
const clamp = n => Math.max(0, Math.min(99999, Math.trunc(n) || 0));

export const IME_FLAGS = Object.freeze({
  previous: 1, earlier: 2, displaced: 4, overlapped: 8, blurred: 16, shrank: 32,
});

function readTextarea(textarea) {
  const value = textarea?.value;
  if (typeof value !== 'string') return null;
  const start = Number.isInteger(textarea.selectionStart) ? textarea.selectionStart : value.length;
  const end = Number.isInteger(textarea.selectionEnd) ? textarea.selectionEnd : start;
  return { value, length: value.length, range: start !== end, fromEnd: value.length - Math.max(start, end) };
}

/** How the emitted text compares with the commit. `ok` is not logged. */
export function imeCommitVerdict(committed, emitted) {
  if (!emitted) return committed ? 'missing' : 'ok';
  if (!committed) return 'extra';
  if (emitted.startsWith(committed + committed)) return 'doubled';
  if (emitted.startsWith(committed)) return 'ok';
  if (committed.startsWith(emitted)) return 'partial';
  return 'differs';
}

export function createImeProbe({ textarea, log, setTimer = setTimeout, clearTimer = clearTimeout }) {
  const counts = new Map();
  const capped = (code, detail, a, b) => {
    const key = code + ' ' + detail;
    const n = counts.get(key) || 0;
    if (n >= IME_PROBE_CAP) return;
    counts.set(key, n + 1);
    log(code, detail, clamp(a), clamp(b));
  };
  let composing = null;      // { state, blurred } while a composition is open
  const pending = [];        // commits awaiting their emitted text
  let previous = null;
  let compositions = 0, mismatches = 0, disposed = false;

  const judge = item => {
    const i = pending.indexOf(item);
    if (i < 0) return;
    pending.splice(i, 1);
    clearTimer(item.timer);
    const verdict = imeCommitVerdict(item.committed, item.emitted);
    if (verdict !== 'ok') {
      mismatches++;
      let flags = item.flags;
      if (item.emitted && previous != null && item.emitted === previous) flags |= IME_FLAGS.previous;
      if (item.emitted && item.before?.includes(item.emitted)) flags |= IME_FLAGS.earlier;
      const at = !item.start ? 'unknown' : item.start.range ? 'range'
        : item.start.fromEnd > 0 ? 'caret-inside' : 'caret-end';
      capped('ime-commit', verdict, item.committed.length, item.emitted.length);
      capped('ime-context', at, flags, item.start ? item.start.length : 0);
    }
    previous = item.committed;
  };

  return {
    compositionstart() {
      if (disposed) return;
      if (composing) capped('ime-state', 'start-while-composing', 0, 0);
      for (const item of pending) item.flags |= IME_FLAGS.overlapped;
      const state = readTextarea(textarea);
      if (state?.range) capped('ime-textarea', 'range', state.fromEnd, state.length);
      else if (state && state.fromEnd > 0) capped('ime-textarea', 'caret-inside', state.fromEnd, state.length);
      composing = { state, blurred: false };
    },
    compositionend(event) {
      if (disposed) return;
      if (!composing) capped('ime-state', 'end-without-start', 0, 0);
      const begun = composing;
      composing = null;
      compositions++;
      const committed = typeof event?.data === 'string' ? event.data : '';
      const start = begun?.state || null;
      const now = readTextarea(textarea);
      let flags = begun?.blurred ? IME_FLAGS.blurred : 0;
      if (start && now) {
        if (now.length < start.length) flags |= IME_FLAGS.shrank;
        if (committed && now.value.substr(start.length, committed.length) !== committed) flags |= IME_FLAGS.displaced;
      }
      const item = { committed, emitted: '', flags, start,
        before: start ? start.value : null };
      if (start) start.value = null;  // keep lengths, not text, past this point
      item.timer = setTimer(() => judge(item), IME_PROBE_WINDOW_MS);
      pending.push(item);
    },
    blur() {
      if (disposed || !composing) return;
      composing.blurred = true;
      capped('ime-state', 'blur-composing', 0, 0);
    },
    data(text) {
      if (disposed || typeof text !== 'string' || !text || isTerminalAutoReply(text)) return;
      // the oldest commit that has not yet received its text takes this chunk
      const item = pending.find(p => !p.committed || !p.emitted.startsWith(p.committed)) || null;
      if (item) item.emitted += text;
    },
    dispose() {
      if (disposed) return;
      for (const item of [...pending]) judge(item);
      disposed = true;
      previous = null;
      if (compositions) log('ime-summary', null, clamp(compositions), clamp(mismatches));
    },
  };
}

/** Wire the probe onto a pane's xterm textarea; returns the probe. */
export function wireImeProbe(textarea, log) {
  const probe = createImeProbe({ textarea, log });
  if (!textarea?.addEventListener) return probe;
  textarea.addEventListener('compositionstart', () => probe.compositionstart(), true);
  textarea.addEventListener('compositionend', event => probe.compositionend(event), true);
  textarea.addEventListener('blur', () => probe.blur(), true);
  return probe;
}
