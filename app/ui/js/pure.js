// pure.js — DOM-free logic, tested headlessly with `node --test`
// (../test/pure.test.mjs). Keep this module free of imports, window/document
// access and Tauri APIs: everything here must run in bare Node.

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
  const rank = { waiting: 0, running: 1, stopped: 2 };
  return project.columns.map(column => {
    const sessions = cards
      .map((card, index) => ({ card, index }))
      .filter(x => x.card.columnId === column.id)
      .sort((a, b) => (rank[a.card.status] ?? 3) - (rank[b.card.status] ?? 3) || a.index - b.index)
      .map(x => x.card);
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
