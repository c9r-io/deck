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
  tmuxServerStatus: null,
  tmuxRestarting: false,
  updateDownloadBytes: 0,
  lastPollError: null,
  pollTimer: null,
  ptyGens: new Map(),
  qTpl: null,
  queueCache: { items: [], last_fired: {} },
  queueOpen: false,
  resizeTimer: null,
  rxLogged: 0,
  rxBytes: 0,
  saveTimer: null,
  sepLogged: 0,
  settings: {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'stable', sessionRestore: true,
  },
  buildIdentity: { version: '', commit: 'dev' },
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

/* The native window remains hidden until settings have loaded and theme.js
   has applied the resolved palette. app.js reveals it immediately afterwards,
   preventing a dark/light first-frame flash without persisting a second copy
   of settings or user data. */

/* structured diagnostics → ~/.deck/app.log (webview console is invisible in
   production). ONLY event codes plus a short slug and numbers ever cross to
   the backend — never free-form strings, so no typed characters, IME text,
   command lines, prompt contents, paths or URLs can end up in a log. The
   backend whitelists the code and sanitizes the slug again. */
export const uev = (code, detail, a, b) => inv('ui_event', {
  code,
  detail: detail == null ? null : String(detail).slice(0, 64),
  a: a == null ? null : Math.trunc(Number(a)),
  b: b == null ? null : Math.trunc(Number(b)),
}).catch(() => {});
/* Verbose diagnostics are maintainer-only and enabled at launch with
   --debug-logging. They retain the same structured/privacy contract. */
window.__DECK_DEBUG = false;
export const duev = (code, detail, a, b) => { if (window.__DECK_DEBUG) uev(code, detail, a, b); };
/* error CLASS only — the message can quote user input, so it stays out */
export const errClass = e => {
  const m = /([A-Za-z]+Error)/.exec(String((e && e.name) || e || ''));
  return m ? m[1] : 'error';
};
window.onerror = (msg, src, line) => uev('js-error', errClass(msg), line);
/* keydown CATEGORY only — raw key names never cross into the log (the
   backend's closed allowlist would redact them anyway) */
const keyClass = k =>
  k.length === 1 ? 'char'
  : /^(Enter|Backspace|Delete|Tab|Escape)$/.test(k) ? k.toLowerCase()
  : k.startsWith('Arrow') ? 'arrow'
  : /^(Shift|Control|Alt|Meta|CapsLock)$/.test(k) ? 'mod'
  : /^F\d+$/.test(k) ? 'fn'
  : /^(Home|End|PageUp|PageDown)$/.test(k) ? 'nav'
  : /^(Dead|Process|Compose)/.test(k) ? 'compose'
  : 'other';
document.addEventListener('keydown', e => {
  if (kdLogged < 20) {
    kdLogged++;
    duev('keydown', keyClass(e.key), e.isComposing ? 1 : 0);
  }
}, true);
document.addEventListener('compositionstart', e => duev('composition', 'start', (e.data || '').length), true);
document.addEventListener('compositionend', e => duev('composition', 'end', (e.data || '').length), true);

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

/* DOM-free logic lives in pure.js (node-testable); re-exported here so the
   rest of the app keeps one import point for shared helpers */
import { fmtMem, sessionName } from './pure.js';
export { fmtMem, sessionName };

/* ---------- tiny event bus (same contract as the mock) ---------- */
export const listeners = new Set();
export const emit = (ev, s) => listeners.forEach(fn => fn(ev, s));

/* ---------- formatting ---------- */
export function setMemChip(chip, s) {
  if (!chip) return;
  chip.textContent = s.mem == null ? '' : fmtMem(s.mem);
  chip.classList.toggle('high', s.mem != null && s.mem > 1536);
}


import { t } from './i18n.js';
export const columnHint = column => column?.semantic ? t(`board.hint.${column.semantic}`) : '';
export const dotTitle = status => t(`session.status.${status}`);
