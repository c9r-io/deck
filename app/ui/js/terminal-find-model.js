// terminal-find-model.js — the pure half of a pane's find bar (⌘F).
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// tmux searches (`src-tauri/src/terminal_search.rs`); this module only
// decides what to ask and how to label the integers that come back.
// - A query is checked here with the backend's own rules (non-empty, at most
//   FIND_QUERY_MAX_BYTES UTF-8 bytes, no control character) so the bar can
//   say so without a round trip; the backend still refuses on its own.
// - A step is 'start' (newest match), 'older' or 'newer'. Every search
//   starts backwards, so after a start the last direction is 'older'; the
//   first step against the last direction is a `turn` (tmux would otherwise
//   land on the same match once).
// - A step whose query differs from the one last searched is a start.
// - The label is a count, never a position ("3 of 17"): tmux reports no
//   current index, and one inferred from steps would drift while the pane
//   prints. `alternate` (a full-screen program) adds "this screen only".

export const FIND_QUERY_MAX_BYTES = 256;
export const FIND_DEBOUNCE_MS = 150;

const encoder = new TextEncoder();
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

export function findQueryState(query) {
  const text = String(query ?? '');
  if (!text) return 'empty';
  if (encoder.encode(text).byteLength > FIND_QUERY_MAX_BYTES) return 'long';
  if (CONTROL.test(text)) return 'invalid';
  return 'ok';
}

/** `last`: the direction tmux's stored search last moved (null = none). */
export function findStep({ last = null, lastQuery = null, query, direction }) {
  if (direction === 'start' || last == null || query !== lastQuery) {
    return { step: 'start', turn: false, last: 'older' };
  }
  return { step: direction, turn: direction !== last, last: direction };
}

/** What the bar says. `result` is the backend's reply, `error` a refusal. */
export function findStatus({ state = 'ok', result = null, error = false, unavailable = false } = {}) {
  if (unavailable || error) return { key: 'find.unavailable', screenOnly: false };
  if (state === 'empty') return { key: '', screenOnly: false };
  if (state !== 'ok') return { key: 'find.invalid', screenOnly: false };
  if (!result) return { key: '', screenOnly: false };
  const screenOnly = !!result.alternate;
  if (!result.present) return { key: 'find.none', screenOnly };
  return { key: result.partial ? 'find.countMore' : 'find.count', count: result.count, screenOnly };
}
