// state.js — shared helpers ($/inv/listen/log), the store, and the global mutable slots
// Part of deck's no-build frontend: native ES modules, no bundler.
import { createAttentionTracker } from './attention-model.js';

/* Shared mutable runtime slots. One explicit object, imported as `ctx` by
   the modules that read or assign a slot, so every cross-module dependency
   is visible at the import site and check.mjs can flag a bare name.
   Every slot is declared here: check.mjs does not check member access, so
   a write to an undeclared `ctx.x` (or `state.x`) is invisible to it. */
export const ctx = {
  // Derived attention and navigation are never serialized into Board data.
  attention: createAttentionTracker(),
  attentionFilter: 'pending',
  attentionReturn: null,
  HOME: '~',
  attachedName: null,
  voiceDelivering: null,
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
  destructiveCards: new Set(),
  pollTimer: null,
  ptyGens: new Map(),
  queueCache: { items: [], last_fired: {} },
  queueOpen: false,
  resizeTimer: null,
  rxLogged: 0,
  rxBytes: 0,
  saveTimer: null,
  sepLogged: 0,
  settings: {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'stable', sessionRestore: false, fontScale: 1,
    voice: { languages: ['zh-CN', 'en-US', 'ja-JP'], defaultLanguage: 'system' },
    localIntelligence: { translation: { enabled: false, targetLanguage: 'zh-Hans', documentLimitBytes: 16384 } },
    shortcuts: {
      newSession: 'Meta+KeyN', toggleSidebar: 'Meta+KeyB',
      splitRight: 'Meta+KeyD', splitDown: 'Meta+Shift+KeyD',
      translationLens: 'Meta+Shift+KeyT',
      fontIncrease: 'Meta+Equal', fontDecrease: 'Meta+Minus', fontReset: 'Meta+Digit0',
    },
    inbound: { sources: { slack: { enabled: false } }, rules: [],
      channelConnection: { enabled: false, connectionId: 'default' }, channelRules: [] },
  },
  buildIdentity: { version: '', commit: 'dev' },
  term: null,
  wheelTimer: null,
};

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
   backend whitelists the code and sanitizes the slug again. Terminal events
   may also carry numeric run/pane/selection/gesture/attempt IDs, never session names. */
export const uev = (code, detail, a, b, context) => inv('ui_event', {
  code,
  context: context ?? null,
  detail: detail == null ? null : String(detail).slice(0, 64),
  a: a == null ? null : Math.trunc(Number(a)),
  b: b == null ? null : Math.trunc(Number(b)),
}).catch(() => {});
/* Verbose diagnostics are maintainer-only and enabled at launch with
   --debug-logging. They retain the same structured/privacy contract. */
export const duev = (code, detail, a, b, context) => { if (globalThis.window?.__DECK_DEBUG) uev(code, detail, a, b, context); };
/* error CLASS only — the message can quote user input, so it stays out */
export const errClass = e => {
  const m = /([A-Za-z]+Error)/.exec(String((e && e.name) || e || ''));
  return m ? m[1] : 'error';
};
/* keydown CATEGORY only — raw key names never cross into the log (the
   backend's closed allowlist would redact them anyway) */
const keyClass = k =>
  /^[+＋]$/.test(k) ? 'plus'
  : /^[=＝]$/.test(k) ? 'equal'
  : /^[-−－]$/.test(k) ? 'minus'
  : k.length === 1 ? 'char'
  : /^(Enter|Backspace|Delete|Tab|Escape)$/.test(k) ? k.toLowerCase()
  : k.startsWith('Arrow') ? 'arrow'
  : /^(Shift|Control|Alt|Meta|CapsLock)$/.test(k) ? 'mod'
  : /^F\d+$/.test(k) ? 'fn'
  : /^(Home|End|PageUp|PageDown)$/.test(k) ? 'nav'
  : /^(Dead|Process|Compose)/.test(k) ? 'compose'
  : 'other';
// Debug-only, closed numeric categories: modifier flags are
// Meta=1, Control=2, Alt=4, Shift=8, composing=16; physical codes are
// Equal=1, Minus=2, Semicolon=3, NumpadAdd=4, NumpadSubtract=5.
const keyFlags = e => (e.metaKey ? 1 : 0) | (e.ctrlKey ? 2 : 0)
  | (e.altKey ? 4 : 0) | (e.shiftKey ? 8 : 0) | (e.isComposing ? 16 : 0);
const keyCodeClass = code => ({
  Equal: 1, Minus: 2, Semicolon: 3, NumpadAdd: 4, NumpadSubtract: 5,
})[code] || 0;

export const $ = id => document.getElementById(id);
/* how long without output before a live session counts as "quiet" —
   amber means "no output for a while, may be waiting for you" */
export const QUIET_SECS = CARD_QUIET_SECS;
export const POLL_MS = 2500;

/* ---------- state ---------- */
export const store = {
  projects: [],   // {id, name, columns: [{id, name}]}
  cards: [],      // {id, projectId, columnId, title, desc, cmd, dir, session, pinned}
                  // + runtime (not persisted): status, mem, idle
};
export const state = {
  projectId: null,
  view: 'board',
  sessionId: null,
};

export const genId = p => p + Date.now().toString(36) + (ctx.nextIdCounter++).toString(36);

/* DOM-free logic lives in pure.js (node-testable); re-exported here so the
   rest of the app keeps one import point for shared helpers */
import { CARD_QUIET_SECS, fmtMem, sessionName } from './pure.js';
export { fmtMem, sessionName };

/* ---------- tiny event bus ----------
   `emit` tells listeners about something that ALREADY happened: a Board
   write that was persisted and committed, a poll result, a status change.
   So a listener that throws cannot undo it and must not be able to make it
   look undone. Each listener runs on its own: its exception stops neither
   the listeners after it nor the caller, which would otherwise report a
   saved write as failed ("the template could not be saved", a phone command
   answered `ambiguous`). The failure is not hidden: it is logged as the
   closed `js-error` event with its error class, and b=1 marks it as coming
   from a listener (window.onerror logs the same code with a line number and
   no b). A failed WRITE is a different path and is unchanged: persistence
   throws inside the transaction (persistence.js), before any emit. */
export const listeners = new Set();
export const emit = (ev, s) => listeners.forEach(fn => {
  try { fn(ev, s); }
  catch (error) { uev('js-error', errClass(error), 0, 1); }
});

/* ---------- formatting ---------- */
export function setMemChip(chip, s) {
  if (!chip) return;
  chip.textContent = s.mem == null ? '' : fmtMem(s.mem);
  chip.classList.toggle('high', s.mem != null && s.mem > 1536);
}

import { t } from './i18n.js';
export const columnHint = column => column ? t('attention.manual') : '';
export const dotTitle = status => t(`session.status.${status}`);

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initInputDiagnostics() {
  window.__DECK_DEBUG = false;
  window.onerror = (msg, src, line) => uev('js-error', errClass(msg), line);
  document.addEventListener('keydown', e => {
    if (ctx.kdLogged < 40) {
      ctx.kdLogged++;
      duev('keydown', keyClass(e.key), keyFlags(e), keyCodeClass(e.code));
    }
  }, true);

  document.addEventListener('compositionstart', e => duev('composition', 'start', (e.data || '').length), true);

  document.addEventListener('compositionend', e => duev('composition', 'end', (e.data || '').length), true);
}
