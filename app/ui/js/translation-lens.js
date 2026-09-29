// Local Translation is an optional session companion. It reads the focused
// xterm viewport, a user selection, or text newly copied while Deck is in
// front; it never reconstructs Agent messages or writes into a terminal.
//
// Contract (see translation-lens-model.js for the state machine):
// - Opening, choosing a tab, returning focus and switching panes are user
//   intent: Live captures the current viewport after layout frames, even
//   when nothing is written afterwards. Nothing needs a "resume".
// - Output uses a bounded throttle; a raw user wheel gesture on a terminal
//   (`deck-terminal-scroll`, layout.js) uses a trailing debounce, so a burst
//   never translates intermediate positions and the final view is captured.
// - At most ONE native request is outstanding (a cancelled one included:
//   cancel only removes publication authority, the native segment still
//   finishes) plus one replaceable latest snapshot in the model.
// - Copied content: the tab establishes the observation scope. The native
//   gate records a baseline (never reads existing text); a later change
//   while Deck is focused is translated automatically. Readiness is shown
//   only after the baseline exists. Focus loss disarms; nothing copied while
//   away is read. Deck's own copies (both buttons, Cmd+C in the result) go
//   through one receipted writer, and only that exact pasteboard version is
//   excluded — never text that merely looks like an old result.
// - Reading never pauses: a live selection inside the result only defers
//   showing a newer result until the selection or its focus ends.
import { $, ctx, state, listen } from './state.js';
import { t, onLocaleChange } from './i18n.js';
import { toast } from './dialogs.js';
import { registerShortcutAction } from './shortcuts.js';
import { copyTerminalSelection, hasTerminalSelection } from './selection.js';
import { TranslationLensModel, LiveCadence, translationShortcutAction } from './translation-lens-model.js';
import { capability, translate, cancel, clipboardArm, clipboardDisarm,
  clipboardPoll, copyTranslation, unload, closedCode,
  MAX_LIVE_TRANSLATION_BYTES, MAX_TRANSLATION_BYTES } from './local-intelligence.js';

const CLIPBOARD_POLL_MS = 350;
const ARM_RETRY_MS = 250, ARM_ATTEMPTS = 12;
const MODES = ['live', 'clipboard'];
const model = new TranslationLensModel();
let backend = { available: false }, deps, shortcutDispose = null;
let requestCounter = Date.now() * 1000, inflight = null, retryTimer = null;
let clipboardEpoch = 0, pollTimer = null, armTimer = null, polling = false;
let clipboardChain = Promise.resolve();
let renderedText = null, pointerHeld = false;
// "Deck is in front" comes from the native window focus (tauri://focus/blur).
// document.hasFocus() is not that fact: Tab out of the last control moves
// AppKit key focus off the webview while Deck stays in front, and hiding the
// app does not reliably blur the page.
let appFocused = true;
const paneIds = new WeakMap(), submitted = new WeakMap(); let paneCounter = 0;
// Test-visible counters only (no content): schedules, requests and DOM writes.
const metrics = { captures: 0, submitted: 0, settled: 0, cancelled: 0, maxInflight: 0,
  domWrites: 0, retries: 0, clipboardAccepted: 0, clipboardIgnored: 0, last: {}, history: {}, accepted: [],
  lastWriteReceipt: null };
const stamp = name => {
  const now = performance.now(); metrics.last[name] = now;
  const list = metrics.history[name] ||= []; list.push(now); if (list.length > 64) list.shift();
};
export const translationLensMetrics = () => ({ ...metrics, last: { ...metrics.last }, history: { ...metrics.history }, accepted: [...metrics.accepted],
  inflight: inflight ? 1 : 0, pending: model.pending ? 1 : 0, status: model.status({ active: meaningful() }),
  activity: (enabled() ? 1 : 0) | (appFocused ? 2 : 0) | (document.hidden ? 4 : 0) | (state.view === 'session' ? 8 : 0) });

// In-page view of the shown snapshot for the WKWebView smoke's assertions
// (the DOM already shows it); it is never logged or sent anywhere.
export const translationLensView = () => ({ text: model.resultText(), source: model.sourceForCopy() });
const enabled = () => ctx.settings.localIntelligence?.translation?.enabled === true;
const meaningful = () => enabled() && model.open && state.view === 'session' && !document.hidden && appFocused;
const focusedPane = () => deps.panes.get(ctx.attachedName);
const paneId = pane => {
  if (!pane) return null;
  if (!paneIds.has(pane)) paneIds.set(pane, ++paneCounter);
  return paneIds.get(pane);
};
const documentLimit = () => ctx.settings.localIntelligence?.translation?.documentLimitBytes || MAX_TRANSLATION_BYTES;

export function visibleViewport(pane) {
  if (!pane?.term) return '';
  const { term } = pane, buffer = term.buffer.active;
  const lines = Array.from({ length: term.rows }, (_, row) =>
    buffer.getLine(buffer.viewportY + row)?.translateToString(true) || '');
  while (lines.length && !lines.at(-1).trim()) lines.pop();
  return lines.join('\n');
}

/* ----- requests: one outstanding native call, pumped from the model ----- */
function pump() {
  if (inflight) {
    if (inflight.ticket !== model.running && !inflight.cancelled) {
      inflight.cancelled = true; metrics.cancelled++; cancel(inflight.id);
    }
    return;
  }
  const ticket = model.running;
  if (!ticket || submitted.has(ticket) || !meaningful()) return;
  submitted.set(ticket, true);
  const id = ++requestCounter;
  inflight = { id, ticket, cancelled: false };
  metrics.submitted++; metrics.maxInflight = Math.max(metrics.maxInflight, 1); stamp('submit');
  const at = performance.now();
  translate(id, ticket.text, 'zh-Hans', ticket.mode)
    .then(reply => settle(ticket, reply?.text || null, reply?.text ? null : 'translation-failed', at),
      error => settle(ticket, null, closedCode(error), at));
}
function settle(ticket, text, error, submittedAt) {
  inflight = null; metrics.settled++; stamp('settle');
  const { retry, accepted } = model.finish(ticket, text, error);
  if (accepted) {
    metrics.accepted.push({ submit: submittedAt, settle: performance.now() });
    if (metrics.accepted.length > 32) metrics.accepted.shift();
  }
  if (retry !== null && retry !== undefined) {
    clearTimeout(retryTimer);
    retryTimer = setTimeout(() => { retryTimer = null; metrics.retries++; model.retry(); pump(); render(); }, retry);
  }
  pump(); render();
}
function dropWork() {
  clearTimeout(retryTimer); retryTimer = null;
  model.interrupt(); pump();
}

/* ----- rendering: write the result only when its text changed ----- */
function readingHold() {
  const result = $('translation-result');
  if (pointerHeld) return true;
  const selection = document.getSelection?.();
  return !!selection && !selection.isCollapsed && document.activeElement === result
    && !!result.contains?.(selection.anchorNode);
}
function renderTabs() {
  for (const mode of MODES) {
    const tab = $(`translation-tab-${mode}`), selected = model.mode === mode;
    tab.setAttribute('aria-selected', String(selected));
    tab.tabIndex = selected || (model.mode === 'selection' && mode === 'live') ? 0 : -1;
    tab.classList.toggle('selected', selected);
  }
  const panel = $('translation-result');
  panel.setAttribute('aria-labelledby', MODES.includes(model.mode) ? `translation-tab-${model.mode}` : 'translation-title');
}
function render() {
  $('translation-btn').hidden = !enabled() || !backend.available;
  $('translation-panel').hidden = !model.open;
  $('translation-btn').setAttribute('aria-pressed', String(model.open));
  if (!model.open) {
    if (renderedText) { $('translation-result').textContent = ''; metrics.domWrites++; }
    renderedText = null; return;
  }
  renderTabs();
  model.present(readingHold());
  const text = model.resultText(), result = $('translation-result');
  if (text !== renderedText) {
    const top = result.scrollTop, sameContext = renderedText !== null && text.length > 0;
    result.textContent = text; renderedText = text; metrics.domWrites++; stamp('display');
    if (sameContext) result.scrollTop = top;
  }
  $('translation-copy').disabled = !text;
  $('translation-copy-source').disabled = !model.sourceForCopy();
  const status = t(model.status({ active: meaningful() })) || t('translation.error.translation-failed');
  if ($('translation-status').textContent !== status) $('translation-status').textContent = status;
}

/* ----- Live capture ----- */
function captureLive({ intent = false } = {}) {
  if (!meaningful() || model.mode !== 'live' || !backend.available) return;
  const pane = focusedPane();
  if (!pane) { render(); return; }
  metrics.captures++; stamp('capture');
  model.observeLive(visibleViewport(pane), paneId(pane), { intent, maxBytes: MAX_LIVE_TRANSLATION_BYTES });
  pump(); render();
}
const cadence = new LiveCadence({ onCapture: captureLive, clock: {
  now: () => Date.now(), setTimeout: (fn, ms) => setTimeout(fn, ms), clearTimeout: id => clearTimeout(id),
  frame: fn => (typeof requestAnimationFrame === 'function' ? requestAnimationFrame(fn) : setTimeout(fn, 16)),
} });
const liveActive = () => meaningful() && model.mode === 'live';
function liveIntent() { if (liveActive()) cadence.intent(); }

/* ----- copied content: scope = this tab while Deck is focused ----- */
function stopClipboard() {
  clipboardEpoch++; clearInterval(pollTimer); clearTimeout(armTimer);
  pollTimer = null; armTimer = null; model.clipboardReady = false;
  clipboardChain = clipboardChain.then(clipboardDisarm, clipboardDisarm);
  return clipboardChain;
}
const clipboardWanted = epoch => epoch === clipboardEpoch && meaningful() && model.mode === 'clipboard';
function startClipboard(attempt = 0) {
  if (attempt === 0) stopClipboard();
  const epoch = clipboardEpoch;
  render();
  clipboardChain = clipboardChain.then(async () => {
    if (!clipboardWanted(epoch)) return;
    try { await clipboardArm(); } // records a baseline only; never reads existing text
    catch (error) {
      if (!clipboardWanted(epoch)) return;
      const code = closedCode(error);
      if (code === 'clipboard-not-focused' && attempt + 1 < ARM_ATTEMPTS) {
        armTimer = setTimeout(() => { armTimer = null; if (clipboardWanted(epoch)) startClipboard(attempt + 1); }, ARM_RETRY_MS);
      } else { model.fail(code); render(); }
      return;
    }
    if (!clipboardWanted(epoch)) return; // the later stop's disarm follows on this chain
    model.clipboardReady = true; stamp('armed'); render();
    pollTimer = setInterval(() => pollClipboard(epoch), CLIPBOARD_POLL_MS);
    pollClipboard(epoch);
  });
}
async function pollClipboard(epoch) {
  if (polling || !clipboardWanted(epoch)) return;
  polling = true;
  try {
    const text = await clipboardPoll();
    if (!clipboardWanted(epoch) || typeof text !== 'string') return;
    if (!model.newCopy(text)) { metrics.clipboardIgnored++; return; }
    metrics.clipboardAccepted++; stamp('copied');
    model.snapshot(text, 'clipboard', documentLimit());
    pump(); render();
  } catch (error) {
    if (!clipboardWanted(epoch)) return;
    const code = closedCode(error);
    if (code === 'clipboard-not-focused') {
      clearInterval(pollTimer); pollTimer = null; model.clipboardReady = false; startClipboard(1);
    }
    else { model.fail(code); render(); }
  } finally { polling = false; }
}

/* ----- lifecycle ----- */
function enterMode(mode) {
  cadence.stop(); stopClipboard(); dropWork(); render();
  if (mode === 'live') liveIntent(); else if (mode === 'clipboard') startClipboard();
}
function selectMode(mode) {
  if (!model.modeTo(mode)) return; // re-choosing the current tab changes nothing
  enterMode(mode);
}
function interrupt() {
  cadence.stop(); stopClipboard(); dropWork(); pointerHeld = false; render();
}
function resumeFocus() {
  if (!model.open || !meaningful()) return;
  if (model.mode === 'clipboard') startClipboard();
  else if (model.mode === 'live') liveIntent();
  pump(); render();
}
async function refreshCapability() {
  if (!enabled()) { backend = { available: false }; closeTranslationLens(); shortcutDispose?.(); shortcutDispose = null; render(); return; }
  try { backend = await capability(); }
  catch { backend = { available: false }; }
  if (backend.available && !shortcutDispose) shortcutDispose = registerShortcutAction('translationLens', translationShortcut);
  if (!backend.available && shortcutDispose) { shortcutDispose(); shortcutDispose = null; closeTranslationLens(); }
  render();
}
export function closeTranslationLens() {
  if (!model.open) return;
  cadence.stop(); stopClipboard(); clearTimeout(retryTimer); retryTimer = null;
  model.close(); pump(); pointerHeld = false; render();
  unload().catch(() => {});
}
function syncOverlay() {
  $('session-workspace').classList.toggle('translation-overlay',
    $('session-workspace').clientWidth < 440 + 480);
}
function open(mode = 'live') {
  if (!enabled() || !backend.available || state.view !== 'session') return;
  deps.closeBuffer(); model.show(mode); stamp('open');
  syncOverlay(); enterMode(mode);
}
async function selectedText() {
  const pane = focusedPane(); let text = null;
  if (pane && hasTerminalSelection(pane)) text = await copyTerminalSelection(pane).catch(() => null);
  else if (pane?.term.hasSelection?.()) text = pane.term.getSelection();
  return text;
}
function translateSelection(text) {
  if (!model.open) open('selection');
  if (!model.open) return;
  cadence.stop(); stopClipboard(); clearTimeout(retryTimer); retryTimer = null;
  model.snapshot(text, 'selection', MAX_TRANSLATION_BYTES);
  pump(); render();
}
async function translationShortcut() {
  if (!enabled() || state.view !== 'session') return;
  const text = await selectedText();
  const action = translationShortcutAction(!!text, model.open);
  if (action === 'selection') translateSelection(text);
  else if (action === 'close') closeTranslationLens();
  else open();
}
async function copySnapshot(text, kind = 'translation') {
  if (!text) return;
  try {
    metrics.lastWriteReceipt = await copyTranslation(text);
    if (kind !== 'selection') toast(t(kind === 'source' ? 'translation.sourceCopied' : 'translation.copied'));
  } catch { toast(t('translation.copyFailed')); }
}
function tabKey(event) {
  const order = { ArrowLeft: -1, ArrowRight: 1, Home: -9, End: 9 }[event.key];
  if (!order) return;
  event.preventDefault();
  const index = Math.max(0, MODES.indexOf(model.mode));
  const next = order === -9 ? 0 : order === 9 ? MODES.length - 1
    : (index + order + MODES.length) % MODES.length;
  $(`translation-tab-${MODES[next]}`).focus();
  selectMode(MODES[next]);
}
export function initTranslationLens({ panes, closeBuffer }) {
  deps = { panes, closeBuffer };
  $('translation-btn').onclick = () => { if (model.open) closeTranslationLens(); else open(); };
  $('translation-close').onclick = closeTranslationLens;
  for (const mode of MODES) $(`translation-tab-${mode}`).onclick = () => selectMode(mode);
  $('translation-tabs').addEventListener('keydown', tabKey);
  $('translation-copy').onclick = () => copySnapshot(model.resultText());
  $('translation-copy-source').onclick = () => copySnapshot(model.sourceForCopy(), 'source');
  const result = $('translation-result');
  // Cmd+C of a selection inside the result is a Lens-owned write too: route
  // it through the receipted writer instead of WebKit's own pasteboard write.
  result.addEventListener('copy', event => {
    const selection = document.getSelection?.(), text = String(selection || '');
    if (!text || !result.contains?.(selection.anchorNode)) return;
    event.preventDefault();
    copySnapshot(text, 'selection');
  });
  const release = () => { if (pointerHeld) { pointerHeld = false; render(); } };
  result.addEventListener('pointerdown', () => { pointerHeld = true; });
  // Any sign that the button is no longer pressed ends the hold: a lost
  // pointerup must never leave a frozen result.
  for (const type of ['pointerup', 'pointercancel', 'mouseup', 'keydown']) window.addEventListener(type, release, true);
  window.addEventListener('pointermove', event => { if (event.buttons === 0) release(); }, true);
  const unhold = () => { if (model.open && model.heldBack() && !readingHold()) render(); };
  document.addEventListener('selectionchange', unhold);
  document.addEventListener('focusin', unhold);
  window.addEventListener('deck-terminal-changed', event => {
    if (event.detail === ctx.attachedName && liveActive()) cadence.output();
  });
  window.addEventListener('deck-terminal-scroll', event => {
    if (event.detail === ctx.attachedName && liveActive()) cadence.scroll();
  });
  window.addEventListener('deck-pane-focused', () => {
    if (!model.retarget()) return; // a snapshot tab keeps its snapshot
    cadence.stop(); dropWork(); liveIntent(); render();
  });
  window.addEventListener('deck-session-leave', closeTranslationLens);
  window.addEventListener('deck-buffer-open', closeTranslationLens);
  window.addEventListener('deck-translation-disabled', closeTranslationLens);
  window.addEventListener('deck-translation-enabled-changed', refreshCapability);
  const focusChanged = focused => { appFocused = focused; if (focused) resumeFocus(); else interrupt(); };
  window.addEventListener('focus', () => focusChanged(true)); // page focus implies the app is in front
  // Without native window events (tests, or a refused listen) page blur is
  // the fallback; this module never logs (tests/log_privacy.rs).
  const domBlur = () => window.addEventListener('blur', () => focusChanged(false));
  if (window.__TAURI__?.event) {
    listen('tauri://focus', () => focusChanged(true)).catch(domBlur);
    listen('tauri://blur', () => focusChanged(false)).catch(domBlur);
  } else domBlur();
  document.addEventListener('visibilitychange', () => { if (document.hidden) interrupt(); else resumeFocus(); });
  new ResizeObserver(() => {
    syncOverlay();
    if (liveActive()) cadence.output();
  }).observe($('session-workspace'));
  onLocaleChange(render);
  refreshCapability(); // default OFF never verifies or loads a model
}
