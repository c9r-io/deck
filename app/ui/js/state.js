// state.js — shared helpers ($/inv/listen/log), the store, and the global mutable slots
// Part of deck's no-build frontend: native ES modules, no bundler.

/* Shared mutable runtime slots. Modules are strict-mode; these were
   script-globals before the split and several are assigned from more than
   one module, so they live on globalThis (bare names resolve to these
   properties everywhere, exactly like the pre-split behavior). */
Object.assign(globalThis, {
  HOME: '~',
  attachedName: null,
  cfmResolve: null,
  creatingSession: false,
  freshShell: false,
  ghostEl: null,
  ghostRemainder: '',
  ghostTimer: null,
  histCache: [],
  kdLogged: 0,
  layout: null,
  lineBuf: '',
  nextIdCounter: 1,
  pendingUpdate: null,
  lastPollError: null,
  pollDiagCount: 0,
  pollTimer: null,
  qTpl: null,
  queueCache: { items: [], last_fired: {} },
  queueOpen: false,
  resizeTimer: null,
  rxLogged: 0,
  rxBytes: 0,
  saveTimer: null,
  sepLogged: 0,
  settings: { editor: '', debug: false },
  term: null,
  wheelTimer: null,
});

'use strict';
/* ================================================================
   deck 0.2 — real frontend. tmux owns the sessions; the Rust side
   (src-tauri) provides board persistence, a poll endpoint, and a
   PTY bridge for the one session that is open.
   ================================================================ */

/* guarded so the page also loads in a plain browser (headless UI testing) */
export const inv = (cmd, args) => window.__TAURI__
  ? window.__TAURI__.core.invoke(cmd, args)
  : Promise.reject('no tauri runtime');
export const listen = (ev, cb) => window.__TAURI__
  ? window.__TAURI__.event.listen(ev, cb)
  : Promise.reject('no tauri runtime');

/* the window starts hidden (no white flash) — show as soon as the dark UI
   shell is parsed and styled, before any async boot work */
try {
  if (window.__TAURI__) {
    const w = window.__TAURI__.window.getCurrentWindow();
    w.show().then(() => w.setFocus()).catch(() => {});
  }
} catch (e) { /* plain browser */ }

/* diagnostics → ~/.deck/app.log (webview console is invisible in production) */
export const ulog = m => inv('ui_log', { msg: String(m) }).catch(() => {});
/* verbose diagnostics: OFF unless settings.debug — and even then, never log
   typed characters, IME text, command lines or prompt contents (privacy) */
export const dlog = m => { if (window.__DECK_DEBUG) ulog(m); };
window.onerror = (msg, src, line) => ulog(`error: ${msg} @${line}`);
document.addEventListener('keydown', e => {
  if (kdLogged < 20) {
    kdLogged++;
    dlog(`keydown ${e.key.length === 1 ? '<char>' : e.key} composing=${e.isComposing} target=${e.target.tagName}`);
  }
}, true);
document.addEventListener('compositionstart', e => dlog(`compositionstart len=${(e.data || '').length}`), true);
document.addEventListener('compositionend', e => dlog(`compositionend len=${(e.data || '').length}`), true);

export const $ = id => document.getElementById(id);
/* how long without output before a live session counts as "quiet" —
   amber means "no output for a while, may be waiting for you" */
export const QUIET_SECS = 15;
export const POLL_MS = 2500;

/* ---------- state ---------- */
export const store = {
  projects: [],   // {id, name, columns: [{id, name}]}
  cards: [],      // {id, projectId, columnId, title, desc, cmd, dir, session}
                  // + runtime (not persisted): status, mem, tail, idle
};
export const state = {
  projectId: null,
  view: 'board',
  sessionId: null,
};

export const genId = p => p + Date.now().toString(36) + (nextIdCounter++).toString(36);

export function sessionName(title, id) {
  let slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  if (!slug) slug = 'card';
  return 'deck-' + slug.slice(0, 24).replace(/-+$/, '') + '-' + id.slice(-4);
}

/* ---------- tiny event bus (same contract as the mock) ---------- */
export const listeners = new Set();
export const emit = (ev, s) => listeners.forEach(fn => fn(ev, s));

/* ---------- formatting ---------- */
export function fmtMem(mb) {
  return mb >= 1024 ? (mb / 1024).toFixed(1) + 'G' : Math.round(mb) + 'M';
}
export function setMemChip(chip, s) {
  if (!chip) return;
  chip.textContent = s.mem == null ? '' : fmtMem(s.mem);
  chip.classList.toggle('high', s.mem != null && s.mem > 1536);
}


export const COL_HINTS = {
  Attention: 'things you want to deal with next',
  Working: 'running autonomously, nothing needed from you',
  Queued: 'created but not started yet',
  Parked: 'kept around, low priority',
};

export const DOT_TITLES = {
  running: 'output within the last few seconds',
  waiting: 'no output for a while — may be waiting for input',
  stopped: 'no live session',
};
