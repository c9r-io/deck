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

/* ---------- completion bar placement ----------
   The quick-command / completion bar is an overlay (it must not reflow the
   split panes while you type). Its natural home is the bottom edge — but a
   shell prompt usually sits on the LAST line of its pane, so an overlay
   there covers exactly the line being typed. When the cursor row falls
   inside the bar (or within `gap` of it), the bar hops ABOVE the cursor
   line instead. Returns the CSS `bottom` offset in px. */
export function quickBarBottom({ viewH, cursorTop, cellH, barH, gap = 6 }) {
  if (!(viewH > 0) || !(barH > 0) || cursorTop == null || !(cellH >= 0)) return 0;
  if (viewH - (cursorTop + cellH) >= barH + gap) return 0;   // room below the prompt
  const above = viewH - cursorTop + gap;                     // bottom edge above the cursor row
  return Math.max(0, Math.min(above, viewH - barH));         // never leaves the view
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
  if (!alive) return ' · ready'; // dead session counts as quiet; fires next tick
  if (idleSecs == null) return '';
  const q = Math.min(Math.floor(idleSecs), CHAIN_QUIET_SECS);
  return q >= CHAIN_QUIET_SECS ? ' · quiet ✓' : ` · quiet ${q}s/${CHAIN_QUIET_SECS}s`;
}

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
