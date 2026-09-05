// pure.js — DOM-free logic, tested headlessly with `node --test`
// (../test/pure.test.mjs). Keep this module free of imports, window/document
// access and Tauri APIs: everything here must run in bare Node.
//
// # Contract
// Status semantics: green = output <15s ago; amber "waiting" = alive but quiet ≥15s
// (honest heuristic — may be waiting for input, may be a silent build); gray = no
// session.

/* ---------- session naming ---------- */
export function sessionName(title, id) {
  let slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  if (!slug) slug = 'card';
  return 'deck-' + slug.slice(0, 24).replace(/-+$/, '') + '-' + id.slice(-4);
}

/* POSIX single-quote escaping for a path typed into a shell or agent
   prompt; simple safe paths pass through unquoted. */
export function shQuote(p) {
  const s = String(p);
  return /^[A-Za-z0-9_/.~-]+$/.test(s) ? s : "'" + s.replace(/'/g, "'\\''") + "'";
}

/* ---------- card preview ---------- */
export const CARD_PREVIEW_ROWS = 6;

/** Keep the newest terminal rows in natural reading order and pad above them
 * so a short tail stays anchored to the bottom of the fixed-height preview. */
export function cardPreviewRows(lines, rows = CARD_PREVIEW_ROWS) {
  const requested = Number(rows);
  const count = Number.isFinite(requested) ? Math.max(0, Math.trunc(requested)) : 0;
  if (!count) return [];
  const tail = (Array.isArray(lines) ? lines : []).slice(-count);
  return Array(count - tail.length).fill('').concat(tail);
}

/* Terminal links are tokenized before they are classified. A URL consumes
   its complete interval first, so `/api` inside it can never become a path.
   Path tokens are only candidates here: the provider asks the backend to
   confirm that they exist relative to the pane cwd before making them links. */
const URL_SCHEMES = ['https://', 'http://'];
const PATH_START_DELIMS = '=:([{<,;|';
const TOKEN_END_DELIMS = '"\'`<>|\\';
const PATH_HARD_END_DELIMS = '=,;';
const PATH_TRAILING = '.,;!?)}]';

const isSpace = ch => !!ch && ch.trim() === '';
const isAsciiDigit = ch => ch >= '0' && ch <= '9';
const allAsciiDigits = value => !!value && [...value].every(isAsciiDigit);
const isStartBoundary = ch => !ch || isSpace(ch) || PATH_START_DELIMS.includes(ch)
  || TOKEN_END_DELIMS.includes(ch);
const isTokenEnd = ch => !ch || isSpace(ch) || TOKEN_END_DELIMS.includes(ch);

function withoutLineLocation(value) {
  let end = value.length;
  for (let count = 0; count < 2; count++) {
    const colon = value.lastIndexOf(':', end - 1);
    if (colon < 0 || !allAsciiDigits(value.slice(colon + 1, end))) break;
    end = colon;
  }
  return value.slice(0, end);
}

function isDottedNumber(value) {
  let candidate = value;
  if ((candidate[0] === 'v' || candidate[0] === 'V') && isAsciiDigit(candidate[1])) {
    candidate = candidate.slice(1);
  }
  const parts = candidate.split('.');
  return parts.length > 1 && parts.every(allAsciiDigits);
}

export function looksLikeTerminalPathCandidate(value) {
  let raw = String(value);
  if ((raw[0] === '"' || raw[0] === '\'') && raw.lastIndexOf(raw[0]) > 0) {
    const close = raw.lastIndexOf(raw[0]);
    raw = raw.slice(1, close) + raw.slice(close + 1);
  }
  const path = withoutLineLocation(raw);
  if (!path || isDottedNumber(path)) return false;
  const lower = path.toLowerCase();
  if (URL_SCHEMES.some(prefix => lower.startsWith(prefix))) return false;
  if (path === '~' || path.startsWith('~/') || path.startsWith('/')
      || path.startsWith('./') || path.startsWith('../')) return true;
  if (path.includes('/')) return true;
  if (path.startsWith('.') && path.length > 1 && !path.endsWith('.')) return true;
  const dot = path.lastIndexOf('.');
  return dot > 0 && dot < path.length - 1;
}

function urlAt(text, index) {
  if (!isStartBoundary(text[index - 1])) return null;
  const scheme = URL_SCHEMES.find(prefix =>
    text.slice(index, index + prefix.length).toLowerCase() === prefix);
  if (!scheme) return null;
  let end = index + scheme.length;
  while (end < text.length && !isTokenEnd(text[end])) end++;
  let value = text.slice(index, end);

  // Closing prose punctuation is not part of a URL. Keep balanced brackets,
  // which are valid in paths and queries, but remove unmatched closers.
  const bracketPairs = [['(', ')'], ['[', ']'], ['{', '}']];
  let trimming = true;
  while (value && trimming) {
    trimming = false;
    if ('.,;!:'.includes(value.at(-1))) {
      value = value.slice(0, -1);
      trimming = true;
      continue;
    }
    const pair = bracketPairs.find(([, close]) => value.endsWith(close));
    if (pair) {
      const [open, close] = pair;
      const opens = [...value].filter(ch => ch === open).length;
      const closes = [...value].filter(ch => ch === close).length;
      if (closes > opens) {
        value = value.slice(0, -1);
        trimming = true;
      }
    }
  }
  try {
    const parsed = new URL(value);
    if (!URL_SCHEMES.some(prefix => parsed.protocol === prefix.slice(0, -2))
        || !parsed.hostname) return null;
  } catch (_) {
    return null;
  }
  return { kind: 'url', value, index, end: index + value.length };
}

function quotedPathAt(text, index) {
  const quote = text[index];
  if ((quote !== '"' && quote !== '\'') || !isStartBoundary(text[index - 1])) return null;
  const close = text.indexOf(quote, index + 1);
  if (close < 0) return null;
  let end = close + 1;
  for (let locations = 0; locations < 2 && text[end] === ':'; locations++) {
    let digitEnd = end + 1;
    while (isAsciiDigit(text[digitEnd])) digitEnd++;
    if (digitEnd === end + 1) break;
    end = digitEnd;
  }
  const value = text.slice(index, end);
  return looksLikeTerminalPathCandidate(value)
    ? { kind: 'path', value, index, end }
    : null;
}

function unquotedPathAt(text, index) {
  if (!isStartBoundary(text[index - 1]) || isTokenEnd(text[index])
      || PATH_START_DELIMS.includes(text[index])) return null;
  let end = index;
  while (end < text.length && !isTokenEnd(text[end])
         && !PATH_HARD_END_DELIMS.includes(text[end])) end++;
  let value = text.slice(index, end);
  while (value && PATH_TRAILING.includes(value.at(-1))) value = value.slice(0, -1);
  if (!looksLikeTerminalPathCandidate(value)) return null;
  return { kind: 'path', value, index, end: index + value.length };
}

export function tokenizeTerminalLinks(input) {
  const text = String(input);
  const links = [];
  let index = 0;
  while (index < text.length) {
    const token = urlAt(text, index)
      || quotedPathAt(text, index)
      || unquotedPathAt(text, index);
    if (token) {
      links.push(token);
      index = token.end;
    } else {
      index++;
    }
  }
  return links;
}

/* ---------- formatting ---------- */
export function fmtMem(mb) {
  return mb >= 1024 ? (mb / 1024).toFixed(1) + 'G' : Math.round(mb) + 'M';
}
export const fmtEvery = s => s % 3600 === 0 ? (s / 3600) + ' h' : (s / 60) + ' min';
export const minToHM = m => String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0');
export const hmToMin = t => { const [h, m] = t.split(':').map(Number); return h * 60 + m; };

/* ---------- completion bar layout ----------
   The bar owns a real flex row inside the focused pane. This returns the two
   non-overlapping rectangles used by layout/tests; no shell cell can ever be
   under the bar because xterm is fitted only into `terminal`. */
export function quickBarLayout({ width, height, barHeight, visible }) {
  const w = Math.max(0, Number(width) || 0);
  const h = Math.max(0, Number(height) || 0);
  const bh = visible ? Math.min(h, Math.max(0, Number(barHeight) || 0)) : 0;
  return {
    terminal: { left: 0, top: 0, right: w, bottom: h - bh },
    bar: { left: 0, top: h - bh, right: w, bottom: h },
  };
}

export function rectsOverlap(a, b) {
  return a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
}

/* ---------- daily windows ---------- */
export const winHas = (m, f, t) => f < t ? (m >= f && m < t) : (m >= f || m < t);
export const hasWindow = i => i.win_from != null && i.win_to != null && i.win_from !== i.win_to;

/* earliest instant an "every" rule can fire again, window-aware */
export function nextFire(i, now = Math.floor(Date.now() / 1000)) {
  let t = i.last ? i.last + i.every : now;
  if (t < now) t = now;
  if (hasWindow(i)) {
    const d = new Date(t * 1000);
    if (!winHas(d.getHours() * 60 + d.getMinutes(), i.win_from, i.win_to)) {
      d.setHours(Math.floor(i.win_from / 60), i.win_from % 60, 0, 0);
      if (Math.floor(d.getTime() / 1000) < t) d.setDate(d.getDate() + 1);
      t = Math.floor(d.getTime() / 1000);
    }
  }
  return t;
}

/* ---------- card status ---------- */
/* One place decides what a card's status word is. `agent` is the closed
   agent-hook state from poll_sessions ("working" | "needs-input" |
   "turn-done", agent_status.rs) and OUTRANKS the output-recency heuristic:
   a hook said what the agent is actually doing, so a long silent tool run
   stays "running" instead of drifting to the ambiguous amber "waiting".
   Without agent state the classic heuristic applies unchanged. */
export function effectiveCardStatus(alive, agent, quiet) {
  if (!alive) return 'stopped';
  if (agent === 'needs-input') return 'attention';
  if (agent === 'turn-done') return 'done';
  if (agent === 'working') return 'running';
  return quiet ? 'waiting' : 'running';
}

/* the statuses that mean "a human look would help" — drives the tab hint */
export const attentionStatus = status => status === 'waiting' || status === 'attention';

/* how long a session must stay quiet before a chain prompt fires —
   keep in sync with CHAIN_QUIET_SECS in scheduler.rs */
export const CHAIN_QUIET_SECS = 180;

/* progress hint for a chain head: how far the quiet timer has come. Any
   output (including the user typing in the pane) resets it — that's the
   product semantics, and exactly why the wait deserves a visible counter. */
export function chainQuietHint(idleSecs, alive) {
  if (!alive) return ' · session stopped'; // quiet/due is not context readiness
  if (idleSecs == null) return '';
  const q = Math.min(Math.floor(idleSecs), CHAIN_QUIET_SECS);
  return q >= CHAIN_QUIET_SECS ? ' · quiet ✓' : ` · quiet ${q}s/${CHAIN_QUIET_SECS}s`;
}

export const contextStatusKey = status => ({
  ready: 'queue.context.ready',
  'foreground-different': 'queue.context.differentProcess',
  'session-replaced': 'queue.context.replaced',
  unavailable: 'queue.context.unavailable',
  starting: 'queue.context.starting',
  unknown: 'queue.context.unknown',
}[status] || 'queue.context.unknown');

/* ---------- queue grouping (mirrors the backend's group semantics) ---------- */
/* items carry an explicit group id (assigned by the backend, which is also
   what the scheduler's chain ordering runs on) — one panel group per group
   id. Rules ("every") have no group and stand alone. */
export function groupQueue(items) {
  const groups = [];
  const byKey = new Map();
  for (const i of items) {
    const key = i.mode === 'every' ? i.id : (i.group || i.id);
    let g = byKey.get(key);
    if (!g) { g = { head: i, rows: [] }; byKey.set(key, g); groups.push(g); }
    g.rows.push(i);
  }
  return groups;
}

/* mirror of the backend's blocking rule: a step whose group head is dead
   (failed, attempts exhausted) waits for the user to retry/skip/remove it */
export const itemDead = i => i.state === 'failed' && i.attempts >= 8;
export function blockedBy(i, rows) {
  if (!i.group) return null;
  const sibs = rows.filter(x => x.group === i.group);
  if (!sibs.length) return null;
  const head = sibs.reduce((a, b) => ((a.seq ?? 1) <= (b.seq ?? 1) ? a : b));
  return head.id !== i.id && itemDead(head) ? head : null;
}

/* all prompt texts of a group in fire order (rule rows contribute their
   embedded template steps too) */
export function groupSteps(g) {
  const steps = [];
  for (const i of g.rows) {
    steps.push(i.text);
    if (i.steps && i.steps.length) steps.push(...i.steps);
  }
  return steps;
}

/* ---------- board deletion transaction ---------- */
/**
 * Run the irreversible parts of deleting cards, but call commit only after
 * every kill and the candidate board save succeeded. Keeping this orchestration
 * DOM-free makes the partial-failure contract executable in Node tests.
 */
export async function deleteSessionsTransaction(cards, { cancel, kill, persist, commit }) {
  if (!(await cancel(cards))) return { ok: false, stage: 'cancel', failed: [] };
  const failed = [];
  for (const card of cards) {
    try { await kill(card); }
    catch (error) { failed.push({ card, error }); }
  }
  if (failed.length) return { ok: false, stage: 'kill', failed };
  try { await persist(); }
  catch (error) { return { ok: false, stage: 'persist', failed: [], error }; }
  commit();
  return { ok: true, stage: 'commit', failed: [] };
}

/* ---------- serialized board transactions ---------- */
const cloneJson = value => JSON.parse(JSON.stringify(value));

/**
 * A tiny, dependency-injected transaction queue used by the frontend and by
 * Node tests. `snapshot` is called only when the transaction reaches the
 * head of the queue, so no caller can write a candidate derived from a stale
 * Board. A failed transaction never poisons the next one.
 */
export function createSerialTransactionQueue({ snapshot, persist, commit, serialize = JSON.stringify }) {
  let chain = Promise.resolve();
  const enqueue = mutate => {
    const run = async () => {
      const candidate = cloneJson(snapshot());
      const result = await mutate(candidate);
      if (result && result.noop) return result;
      const json = serialize(candidate);
      await persist(candidate, json);
      commit(candidate, json);
      return result;
    };
    const pending = chain.catch(() => {}).then(run);
    chain = pending;
    return pending;
  };
  return { enqueue, idle: () => chain.catch(() => {}) };
}

/** Stable retry/no-spam lifecycle for shells discovered dead by polling. */
export function createExitRetirementTracker() {
  const pending = new Set();
  const warned = new Set();
  const inFlight = new Set();
  let lifecycle = 0;
  return {
    observe(sid) { pending.add(sid); },
    async drain({ get, markStopped, close, failed, succeeded }) {
      const turn = lifecycle;
      const tasks = [];
      for (const sid of [...pending]) {
        const card = get(sid);
        if (!card) { pending.delete(sid); warned.delete(sid); continue; }
        if (inFlight.has(sid)) continue;
        markStopped(card);
        inFlight.add(sid);
        tasks.push((async () => {
          try {
            const result = await close(card);
            if (turn !== lifecycle) return;
            const ok = typeof result === 'object' ? !!result.ok : !!result;
            const applied = typeof result === 'object' ? !!result.applied : ok;
            if (!ok) {
              if (!warned.has(sid)) { warned.add(sid); failed(card); }
              return;
            }
            pending.delete(sid); warned.delete(sid);
            if (applied) succeeded(card);
          } finally {
            inFlight.delete(sid);
          }
        })());
      }
      await Promise.all(tasks);
    },
    pending: sid => pending.has(sid),
    inFlight: sid => inFlight.has(sid),
    clear() { lifecycle++; pending.clear(); warned.clear(); },
  };
}

export async function copyExact(text, writer) {
  await writer(text);
  return text.length;
}

/** Privacy-safe stage tracker for native text paste. It records only closed
 * lifecycle labels and byte/character counts, never clipboard contents. */
export function createTerminalPasteTrace({ emit, schedule = setTimeout,
  cancel = clearTimeout, timeoutMs = 750 }) {
  const eventDetails = new Set(['event-text', 'event-empty', 'event-file', 'event-unavailable']);
  let nextId = 1;
  let current = null;

  const clearDeadline = trace => {
    if (trace?.timer != null) cancel(trace.timer);
    if (trace) trace.timer = null;
  };
  const fresh = () => ({ id: nextId++, stage: null, timer: null });
  const take = (...stages) => current && stages.includes(current.stage) ? current : fresh();
  const arm = (trace, stage, missing) => {
    clearDeadline(trace);
    trace.stage = stage;
    current = trace;
    trace.timer = schedule(() => {
      if (current !== trace || trace.stage !== stage) return;
      emit(missing, undefined, trace.id);
      current = null;
    }, timeoutMs);
  };

  return {
    keyCapture() {
      clearDeadline(current);
      const trace = fresh();
      emit('key-capture', undefined, trace.id);
      arm(trace, 'capture', 'handler-missing');
    },
    keyHandler() {
      const trace = take('capture');
      clearDeadline(trace);
      emit('key-handler', undefined, trace.id);
      arm(trace, 'handler', 'event-missing');
    },
    event(detail, length = 0) {
      const trace = take('capture', 'handler');
      clearDeadline(trace);
      const safeDetail = eventDetails.has(detail) ? detail : 'event-unavailable';
      const safeLength = Math.max(0, Math.trunc(Number(length) || 0));
      emit(safeDetail, safeLength, trace.id);
      if (safeDetail === 'event-text' && safeLength > 0) {
        arm(trace, 'event', 'ondata-missing');
      } else {
        current = null;
      }
    },
    onData(length) {
      if (!current || current.stage !== 'event') return null;
      const trace = current;
      clearDeadline(trace);
      trace.stage = 'write';
      emit('ondata', Math.max(0, Math.trunc(Number(length) || 0)), trace.id);
      return trace.id;
    },
    write(id, ok) {
      if (!id) return;
      emit(ok ? 'pty-success' : 'pty-failed', undefined, id);
      if (current?.id === id) current = null;
    },
    dispose() {
      clearDeadline(current);
      current = null;
    },
  };
}

/** Return a new array with one id-addressed item inserted immediately before
 * or after another. Invalid/self moves are no-ops, which makes stale drag
 * payloads harmless when another Board transaction completed first. */
export function reorderById(items, movingId, targetId, after = false) {
  if (!Array.isArray(items) || !movingId || !targetId || movingId === targetId) return items;
  const from = items.findIndex(item => item?.id === movingId);
  const target = items.findIndex(item => item?.id === targetId);
  if (from < 0 || target < 0) return items;
  const next = items.slice();
  const [moving] = next.splice(from, 1);
  const targetNow = next.findIndex(item => item?.id === targetId);
  next.splice(targetNow + (after ? 1 : 0), 0, moving);
  return next.every((item, index) => item === items[index]) ? items : next;
}

/** Resolve Command-C ownership without depending on xterm or the DOM. Deck's
 * token selection wins while present; otherwise xterm's native word/line
 * selection may supply the clipboard text. */
export function terminalCopyRoute(event, hasDeckSelection, hasNativeSelection) {
  if (!event || event.type !== 'keydown' || !event.metaKey
      || String(event.key || '').toLowerCase() !== 'c') return null;
  if (hasDeckSelection) return 'deck';
  if (hasNativeSelection) return 'native';
  return null;
}

/** Serialize PTY resizes and remember only the newest confirmed grid. A
 * failed or explicitly invalidated grid is retryable instead of becoming a
 * permanent false confirmation. */
export function createTerminalResizeCoordinator(send) {
  let epoch = 0;
  let confirmed = null;
  let target = null;
  let chain = Promise.resolve(true);
  return {
    sync(cols, rows) {
      cols = Math.trunc(Number(cols));
      rows = Math.trunc(Number(rows));
      if (cols < 1 || rows < 1) return Promise.resolve(false);
      if (confirmed?.cols === cols && confirmed?.rows === rows) return Promise.resolve(true);
      if (target?.cols === cols && target?.rows === rows) return target.promise;
      const currentEpoch = ++epoch;
      const promise = chain.catch(() => false)
        .then(() => send(cols, rows))
        .then(() => {
          if (epoch === currentEpoch) confirmed = { cols, rows };
          return true;
        })
        .catch(() => {
          if (epoch === currentEpoch) target = null;
          return false;
        });
      chain = promise;
      target = { cols, rows, promise };
      return promise;
    },
    invalidate() {
      epoch++;
      confirmed = null;
      target = null;
    },
    snapshot: () => ({
      confirmed: confirmed && { ...confirmed },
      target: target && { cols: target.cols, rows: target.rows },
    }),
  };
}

export function linkMenuItems(kind) {
  return kind === 'url'
    ? [
      { action: 'url', label: 'Open in browser' },
      { action: 'copy', label: 'Copy URL' },
    ]
    : [
      { action: 'editor', label: 'Open in editor' },
      { action: 'editor-parent', label: 'Open parent folder in editor' },
      { action: 'session-parent', label: 'New session in parent folder' },
      { action: 'reveal', label: 'Reveal in Finder' },
      { action: 'copy', label: 'Copy path' },
    ];
}

/** Signed rows per selection update for the terminal's edge hot zones. */
export function terminalSelectionEdgeLines({ pointerY, top, bottom, hotZone = 48, maxLines = 6 }) {
  if (![pointerY, top, bottom, hotZone, maxLines].every(Number.isFinite) || bottom <= top || hotZone <= 0) return 0;
  if (pointerY < top + hotZone) {
    const depth = Math.min(1, (top + hotZone - pointerY) / hotZone);
    return -Math.max(1, Math.round(maxLines * depth * depth));
  }
  if (pointerY > bottom - hotZone) {
    const depth = Math.min(1, (pointerY - (bottom - hotZone)) / hotZone);
    return Math.max(1, Math.round(maxLines * depth * depth));
  }
  return 0;
}

/** Identify a key owned by IME/dead-key composition. Text-input callers must
 * defer it to the final InputEvent; shortcut routing may separately recognize
 * an explicit Command/Control chord from its physical code. */
export const isComposingKeyEvent = event => !!event && (
  event.isComposing || event.keyCode === 229
  || /^(Process|Dead|Compose)$/.test(event.key || '')
);

/** Some macOS IMEs deliver printable punctuation as keyCode=229 and let the
 * final text arrive through a later InputEvent. xterm 5.5's keydown fallback
 * can mark that input as already handled and drop it. Keep only IME-owned
 * printable/unknown keydowns out of that fallback; never derive text here. */
export const shouldRouteImeKeydownThroughInput = event => {
  if (!event || (event.keyCode !== 229 && event.key !== 'Process')) return false;
  const key = String(event.key || '');
  const printable = [...key].length === 1;
  return !!event.isComposing || printable
    || key === 'Process' || key === 'Unidentified';
};

/** A modifier-only Shift keydown carries no terminal bytes. Letting xterm
 * process it sets its `_keyDownSeen` flag, which makes WKWebView's following
 * input-before-keydown IME punctuation look duplicated and get discarded. */
export const isPlainShiftKeydown = event => !!event
  && event.key === 'Shift'
  && !event.ctrlKey && !event.altKey && !event.metaKey;

/** Locate the first visible row of an agent composer from its prompt glyph.
 * The scan is intentionally geometry-only: no prompt text leaves the xterm
 * buffer. Codex uses › (» at Ultra effort) and Claude Code uses ❯. */
export function terminalAgentComposerGeometry({ lines, cursorRow, cursorCol, maxScanRows = 256 }) {
  if (!Array.isArray(lines) || !Number.isFinite(cursorRow) || !Number.isFinite(cursorCol)) {
    return null;
  }
  const row = Math.trunc(cursorRow);
  const col = Math.trunc(cursorCol);
  if (row < 0 || row >= lines.length || col < 0) return null;
  const first = Math.max(0, row - Math.max(1, Math.trunc(maxScanRows)) + 1);
  for (let y = row; y >= first; y--) {
    const match = /^(\s{0,4})([›»❯])(?=\s|$)/u.exec(String(lines[y] || ''));
    if (!match) continue;
    const markerCol = match[1].length;
    return {
      markerRow: y,
      markerCol,
      continuation: row > y,
      // Agent composers reserve two cells for the prompt glyph and gutter.
      atStart: row === y && col <= markerCol + 2,
    };
  }
  return null;
}

/** A recalled agent prompt at its end is a history boundary to the agent.
 * Deck only supplies the one-key vertical escape when the public terminal
 * geometry proves that the recalled prompt has a visible continuation row. */
export function terminalAgentHistoryUpRoute({ foreground, browsing, composer }) {
  if (!/^-?(?:codex|claude)$/.test(String(foreground || ''))) return 'passthrough';
  if (browsing && composer?.continuation) return 'vertical';
  if (composer?.atStart) return 'history';
  return 'passthrough';
}

// Move off the history boundary, move one visual row up, then restore the
// desired column. Standard CSI cursor keys are accepted in either cursor mode.
export const AGENT_HISTORY_VERTICAL_UP = '\x1b[D\x1b[A\x1b[C';

/** Convert xterm's public absolute-buffer selection into tmux viewport cells.
 * Native word/line selections are adopted only while both endpoints belong
 * to the visible frame which tmux is about to scroll. */
export function terminalNativeSelectionCells({ position, viewportY, rows, cols }) {
  if (!position?.start || !position?.end
      || ![viewportY, rows, cols, position.start.x, position.start.y,
        position.end.x, position.end.y].every(Number.isFinite)
      || rows <= 0 || cols <= 0) return null;
  const top = Math.trunc(viewportY);
  const anchor = {
    row: Math.trunc(position.start.y) - top,
    col: Math.trunc(position.start.x),
  };
  const active = {
    row: Math.trunc(position.end.y) - top,
    col: Math.trunc(position.end.x),
  };
  if (anchor.row < 0 || active.row < 0 || anchor.row >= rows || active.row >= rows
      || anchor.col < 0 || anchor.col >= cols || active.col < 0 || active.col > cols
      || (anchor.row === active.row && anchor.col === active.col)) return null;
  return { anchor, active };
}

/** Choose exactly one scroll authority. A drag which Deck already owns must
 * never be replaced; an idle native word/line selection is adopted first. */
export function terminalSelectionWheelRoute({ tokenSelected, frozen, nativeSelected }) {
  if (tokenSelected) return frozen ? 'frozen' : 'ordinary';
  return nativeSelected ? 'native' : 'ordinary';
}

/** Visible row/column spans for an immutable half-open content selection.
 * tmux rows are absolute buffer rows; only viewportTop changes while scrolling. */
export function terminalSelectionOverlayRows({
  startRow, startCol, endRow, endCol, viewportTop, rows, cols,
}) {
  if (![startRow, startCol, endRow, endCol, viewportTop, rows, cols].every(Number.isFinite)
      || rows <= 0 || cols <= 0) return [];
  let a = { row: Math.trunc(startRow), col: Math.trunc(startCol) };
  let b = { row: Math.trunc(endRow), col: Math.trunc(endCol) };
  if (a.row > b.row || (a.row === b.row && a.col > b.col)) [a, b] = [b, a];
  const out = [];
  const first = Math.max(a.row, Math.trunc(viewportTop));
  const last = Math.min(b.row, Math.trunc(viewportTop) + Math.trunc(rows) - 1);
  for (let absoluteRow = first; absoluteRow <= last; absoluteRow++) {
    const from = absoluteRow === a.row ? a.col : 0;
    const to = absoluteRow === b.row ? b.col : cols;
    const left = Math.max(0, Math.min(cols, from));
    const right = Math.max(left, Math.min(cols, to));
    if (right > left) out.push({
      row: absoluteRow - Math.trunc(viewportTop), col: left, width: right - left,
      absoluteRow,
    });
  }
  return out;
}

/** Pixel bands for the settled selection overlay: one per visible row of the
 * immutable lease, derived only from public geometry (screen rect, grid). */
export function terminalSelectionOverlayBands({ status, rect, rows, cols }) {
  if (!status || !rect || !(rows > 0) || !(cols > 0)) return [];
  const spans = terminalSelectionOverlayRows({
    startRow: status.selection_start_row, startCol: status.selection_start_col,
    endRow: status.selection_end_row, endCol: status.selection_end_col,
    viewportTop: status.history_rows - status.scroll_position, rows, cols,
  });
  const cellWidth = rect.width / cols;
  const cellHeight = rect.height / rows;
  return spans.map(span => ({
    absoluteRow: span.absoluteRow,
    left: span.col * cellWidth, top: span.row * cellHeight,
    width: span.width * cellWidth, height: cellHeight,
  }));
}

/** The terminal cell under a client point, clamped into the screen rect;
 * null when the screen has no geometry yet. */
export function terminalCellAt({ rect, rows, cols, clientX, clientY }) {
  if (!rect || !rect.width || !rect.height || !(rows > 0) || !(cols > 0)) return null;
  const x = Math.max(rect.left, Math.min(rect.right - 0.01, clientX));
  const y = Math.max(rect.top, Math.min(rect.bottom - 0.01, clientY));
  return {
    row: Math.max(0, Math.min(rows - 1, Math.floor((y - rect.top) / (rect.height / rows)))),
    col: Math.max(0, Math.min(cols - 1, Math.floor((x - rect.left) / (rect.width / cols)))),
    rect,
  };
}

/** Edge-scroll lines for a promoted drag: the hot-zone amount, or 0 when the
 * pointer is not in a hot zone or tmux already sits at that end of history. */
export function selectionEdgeScrollLines({ pointerY, cell, status }) {
  if (!cell?.rect) return 0;
  const edge = terminalSelectionEdgeLines({ pointerY, top: cell.rect.top, bottom: cell.rect.bottom });
  const stopped = edge < 0 ? !!status?.at_top : edge > 0 ? !!status?.at_bottom : false;
  return !edge || stopped ? 0 : edge;
}

export const selectionStatusRows = status => (status
  ? Math.abs(status.selection_end_row - status.selection_start_row) + 1
  : 0);

export const selectionOwnerLabel = ({ promoted, pending, selected }) => (promoted ? 'drag-selection'
  : pending ? 'pointer-pending'
    : selected ? 'frozen-selection' : 'xterm');

/* Closed reason codes derived from the backend's error SUFFIXES, never from
   error text, so they are safe to log. */
export const selectionCopyFailureCode = error => (String(error || '').includes('selection-missing')
  ? 'selection-missing' : 'snapshot-failed');
/** `finish-failed` reason for the log's `a` slot: 1 = the pane had left tmux
 * copy-mode, 2 = copy-mode survived but its selection was cleared, 0 = else. */
export const selectionFinishFailureReason = error => {
  const value = String(error || '');
  if (value.includes('selection-missing-inactive')) return 1;
  if (value.includes('selection-missing-cleared')) return 2;
  return 0;
};
export const selectionDimensionsChanged = error => String(error || '').includes('selection-dimensions-changed');

/** Run one grid-bound selection command. `prepare` (size sync, cell lookup)
 * fails fast; `send` is retried after `invalidate` while the backend rejects
 * stale dimensions, at most `attempts` times. Every other error is final. */
export async function retryOnStaleGrid({ prepare, send, invalidate, attempts = 3 }) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const prepared = await prepare();
    try {
      return await send(prepared);
    } catch (error) {
      if (!selectionDimensionsChanged(error) || attempt === attempts - 1) throw error;
      invalidate();
    }
  }
  throw new Error('selection-dimensions-changed');
}

/* xterm auto-answers terminal queries through onData. Those are not user
   input — without this filter they desync the input mirror on every attach,
   and the first command typed after attaching is never captured:
   - CSI with a ?/> prefix (DA1/DA2, DECRPM `$y`, mode reports …) — user keys
     never carry those prefixes
   - DSR cursor reports `ESC[..R`, focus events `ESC[I`/`ESC[O`
   - OSC / DCS responses */
const TERMINAL_AUTO_REPLY = /^(?:\x1b\[[?>][0-9;$]*[a-zA-Z]|\x1b\[[0-9;]*R|\x1b\[[IO]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP[^\x1b]*\x1b\\)+$/;
export const isTerminalAutoReply = data => TERMINAL_AUTO_REPLY.test(data);

/** Link ranges for one xterm buffer line: every tokenized match whose start
 * and end cells are known, restricted to the rows that line covers; path
 * candidates the backend did not confirm are dropped. */
export function terminalLinkRanges({ matches, positions, lineNo, validPaths }) {
  const links = [];
  for (const match of matches) {
    if (match.kind === 'path' && !validPaths.get(match)) continue;
    const start = positions[match.index];
    const end = positions[match.index + match.value.length - 1];
    if (!start || !end) continue;
    if (lineNo < start.y || lineNo > end.y) continue;
    links.push({
      range: { start: { x: start.x, y: start.y }, end: { x: end.endX, y: end.y } },
      text: match.value, kind: match.kind,
    });
  }
  return links;
}

/** A scroll command's reply as the pane chrome reads it: `scroll_session`
 * answers a boolean, selection scrolls answer a status object. */
export const scrollResultView = result => ({
  inMode: typeof result === 'object' ? result?.active : result,
  cursorVisible: typeof result === 'object' ? result?.cursor_visible : true,
});

/** Normalize browser wheel units into fractional terminal lines. */
export function terminalWheelLines(deltaY, deltaMode, rows, pixelsPerLine = 14) {
  if (![deltaY, deltaMode, rows, pixelsPerLine].every(Number.isFinite)
      || rows <= 0 || pixelsPerLine <= 0) return 0;
  if (deltaMode === 1) return deltaY;        // DOM_DELTA_LINE
  if (deltaMode === 2) return deltaY * rows; // DOM_DELTA_PAGE
  return deltaY / pixelsPerLine;             // DOM_DELTA_PIXEL
}

/**
 * Fraction-preserving wheel accumulator. take() error-diffuses sub-line
 * input instead of dropping the trackpad's small inertial tail every frame.
 */
export function createTerminalWheelAccumulator(maxLines = 60) {
  let pending = 0;
  const limit = Number.isFinite(maxLines) ? Math.max(1, Math.floor(maxLines)) : 60;
  return {
    add(lines) {
      if (Number.isFinite(lines)) pending += lines;
      return pending;
    },
    ready: () => Math.abs(pending) >= 0.5,
    take() {
      const rounded = pending < 0 ? -Math.round(-pending) : Math.round(pending);
      const lines = Math.max(-limit, Math.min(limit, rounded));
      pending -= lines;
      return lines;
    },
    pending: () => pending,
  };
}

/**
 * Display-frame scheduler for terminal wheel work. At most one asynchronous
 * backend mutation may be in flight, but a pending wheel delta keeps one RAF
 * armed while that mutation completes. This avoids registering the next RAF
 * from a late Promise microtask and missing WebKit's next-frame cutoff.
 */
export function createTerminalWheelFrameScheduler({
  requestFrame, ready, take, run, active = () => true,
}) {
  let frame = null;
  let inFlight = false;

  const schedule = () => {
    if (frame !== null || !ready()) return false;
    frame = requestFrame(flush);
    return true;
  };

  const flush = () => {
    frame = null;
    if (!active()) return;
    if (inFlight) {
      schedule();
      return;
    }
    const value = take();
    if (!value) return;
    inFlight = true;
    let request;
    try {
      request = run(value);
    } catch (error) {
      inFlight = false;
      schedule();
      return;
    }
    Promise.resolve(request).catch(() => {}).finally(() => {
      inFlight = false;
      schedule();
    });
  };

  return {
    schedule,
    state: () => ({ framePending: frame !== null, inFlight }),
  };
}

/** Pure generation/state core shared by production terminal selection and tests. */
export function createTerminalSelectionModel() {
  let generation = 0;
  let phase = 'idle';
  let anchor = null;
  let active = null;
  let status = null;
  return {
    begin(point) {
      generation++;
      phase = 'starting';
      anchor = { ...point };
      active = { ...point };
      status = null;
      return generation;
    },
    move(point) {
      if (phase !== 'starting' && phase !== 'dragging') return false;
      active = { ...point };
      return true;
    },
    apply(id, next) {
      if (id !== generation || phase === 'idle' || phase === 'cancelled') return false;
      status = { ...next };
      // A final backend reply may arrive after pointerup. Keep the completed
      // lifecycle completed instead of reopening it as a dragging gesture.
      if (phase !== 'selected') phase = 'dragging';
      return true;
    },
    finish() {
      if (phase === 'starting' || phase === 'dragging') phase = 'selected';
    },
    cancel() {
      generation++;
      phase = 'cancelled';
      anchor = null;
      active = null;
      status = null;
      return generation;
    },
    reset() { phase = 'idle'; },
    snapshot() {
      return {
        generation, phase,
        anchor: anchor && { ...anchor },
        active: active && { ...active },
        status: status && { ...status },
      };
    },
  };
}

/* ---------- sidebar grouping ---------- */
export function sidebarGroups(project, cards) {
  if (!project) return [];
  return project.columns.map(column => {
    /* `cards` is the durable Board array. Preserve that order inside each
       Board: status is volatile (and often changes when a session is opened),
       so using it as a sort key makes the navigation move under the user. */
    const sessions = cards.filter(card => card.columnId === column.id);
    return { column, sessions, count: sessions.length };
  }).filter(group => group.count > 0);
}

export function inlineRenameValue(current, input, commit, allowEmpty = false) {
  if (!commit) return null;
  const value = String(input).trim();
  if (!value && !allowEmpty) return null;
  return value === current ? null : value;
}

export async function persistOptimistically({ apply, persist, rollback }) {
  apply();
  try {
    await persist();
    return true;
  } catch (error) {
    rollback(error);
    return false;
  }
}

/* ---------- inbound (自动响应): pure decisions, no DOM, no Tauri ---------- */

/* Source-neutral placeholders. Unknown ones stay literal so a typo is
   visible in the prompt instead of silently vanishing. Prompts are one
   line: the queue pastes a literal buffer and a raw newline would submit
   the prompt early in most agents, so message newlines become spaces. */
export const INBOUND_PLACEHOLDERS = ['text', 'from', 'where', 'link'];
export function fillInboundTemplate(step, msg) {
  const filled = String(step).replace(/\{\{\s*msg\.([a-z]+)\s*\}\}/g, (m, name) =>
    INBOUND_PLACEHOLDERS.includes(name) ? String(msg[name] ?? '') : m);
  return filled.replace(/\s*[\r\n]+\s*/g, ' ').replace(/[ \t]{2,}/g, ' ').trim();
}

export function inboundTitle(text, max = 40) {
  const line = String(text).split(/\r?\n/).map(l => l.trim()).find(Boolean) || '';
  const chars = Array.from(line);
  return chars.length > max ? chars.slice(0, max - 1).join('') + '…' : line;
}

export const INBOUND_BADGE_RE = /^[a-z0-9_+-]{1,64}$/;

/* Decide what to do with one pending item against the current Board.
   Returns { outcome, ...details }. `outcome` is one of the closed codes the
   backend accepts for `uev('inbound', …)`: duplicate | no-rule-target |
   no-template | create. */
export function planInbound(item, { cards, projects, home }) {
  const { event, rule } = item;
  const dup = cards.find(c => c.origin
    && c.origin.source === event.source && c.origin.key === event.key && c.origin.badge === event.badge);
  if (dup) return { outcome: 'duplicate', card: dup };
  const project = projects.find(p => p.id === rule.projectId);
  const column = project && project.columns.find(c => c.id === rule.columnId);
  if (!project || !column) return { outcome: 'no-rule-target' };
  const template = (project.templates || []).find(tp => tp.name === rule.template);
  const msg = { text: event.text, from: event.from, where: event.where, link: event.link };
  const steps = template ? template.steps.map(s => fillInboundTemplate(s, msg)).filter(Boolean) : [];
  if (!steps.length) return { outcome: 'no-template', template: rule.template };
  const dir = expandHome(rule.dir, home);
  return {
    outcome: 'create',
    card: {
      projectId: project.id, columnId: column.id,
      title: inboundTitle(event.text) || `:${event.badge}:`,
      cmd: rule.cmd || '',
      dir,
      desc: [`:${event.badge}:`, event.where, event.from].filter(Boolean).join(' · '),
      origin: { source: event.source, key: event.key, badge: event.badge },
    },
    steps,
    template: template.name,
  };
}

export function expandHome(dir, home) {
  const d = String(dir || '').trim();
  if (!d) return home;
  if (d === '~') return home;
  if (d.startsWith('~/')) return home.replace(/\/+$/, '') + d.slice(1);
  return d;
}

/* ---------- project prompt templates ----------
   A template is {name, steps[]} on the project, so the Board manager and the
   card queue edit exactly the same object. A step is ONE queued prompt: the
   queue pastes a literal buffer and a raw newline submits early, so a step
   is flattened to a single line here, like an inbound message. The name
   bound is the one an inbound rule already enforces (settings-model drops a
   rule whose template name is longer), so a template built on the Board can
   never be one a rule is unable to name. */
export const TEMPLATE_NAME_MAX = 120;
export const TEMPLATE_STEP_MAX = 2000;
export const TEMPLATE_STEPS_MAX = 20;
export const TEMPLATES_MAX = 50;

export function normalizeTemplateStep(text) {
  const line = String(text ?? '')
    .replace(/\s*[\r\n\t]+\s*/g, ' ')
    .replace(/[ ]{2,}/g, ' ')
    .trim();
  return Array.from(line).slice(0, TEMPLATE_STEP_MAX).join('');
}

/* null when the name is usable; otherwise the reason, which the caller
   turns into a message. `current` is the name being edited, so keeping a
   template's own name is not a duplicate. */
export function templateNameProblem(name, templates = [], current = null) {
  const clean = String(name ?? '').trim();
  if (!clean) return 'empty';
  if (Array.from(clean).length > TEMPLATE_NAME_MAX) return 'long';
  if (templates.some(tp => tp.name === clean && tp.name !== current)) return 'duplicate';
  return null;
}

export function nextTemplateName(base, templates = []) {
  const taken = new Set(templates.map(tp => tp.name));
  if (!taken.has(base)) return base;
  for (let n = 2; n <= TEMPLATES_MAX + 1; n++) {
    const candidate = `${base} ${n}`;
    if (!taken.has(candidate)) return candidate;
  }
  return `${base} ${taken.size + 1}`;
}

/* Swap with the neighbour, or return the SAME array when the move would
   leave the list — the caller persists only when something changed. */
export function moveTemplateStep(steps, index, delta) {
  const target = index + delta;
  if (!Array.isArray(steps) || index < 0 || index >= steps.length
      || target < 0 || target >= steps.length) return steps;
  const next = steps.slice();
  next[index] = steps[target];
  next[target] = steps[index];
  return next;
}

/* Inbound rules name a template by NAME, so renaming or deleting one
   silently stops those rules from matching. The manager warns with this
   count; deck never rewrites the user's rules behind their back. */
export function inboundRulesUsingTemplate(rules, projectId, name) {
  return (Array.isArray(rules) ? rules : [])
    .filter(rule => rule && rule.projectId === projectId && rule.template === name).length;
}
