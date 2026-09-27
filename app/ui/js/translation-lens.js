// Local Translation is an optional session companion. It reads the focused
// xterm viewport, a user selection, or a focus-bounded clipboard snapshot;
// it never reconstructs Agent messages or writes into a terminal.
import { $, ctx, state } from './state.js';
import { t, onLocaleChange } from './i18n.js';
import { toast } from './dialogs.js';
import { registerShortcutAction } from './shortcuts.js';
import { copyTerminalSelection, hasTerminalSelection } from './selection.js';
import { TranslationLensModel, LiveCadence, translationShortcutAction } from './translation-lens-model.js';
import { capability, translate, cancel, clipboardArm, clipboardDisarm,
  clipboardPoll, clipboardCurrent, copyTranslation, unload, closedCode } from './local-intelligence.js';

const model = new TranslationLensModel();
let backend = { available: false }, deps, shortcutDispose = null;
let pollTimer = null, polling = false, clipboardEpoch = 0, requestCounter = Date.now() * 1000;
let clipboardChain = Promise.resolve();
const activeRequests = new Set();
const bytes = text => new TextEncoder().encode(text).length;
const enabled = () => ctx.settings.localIntelligence?.translation?.enabled === true;
const meaningful = () => enabled() && model.open && state.view === 'session' && !document.hidden && document.hasFocus();
const focusedPane = () => deps.panes.get(ctx.attachedName);

export function visibleViewport(pane) {
  if (!pane?.term) return '';
  const { term } = pane, buffer = term.buffer.active;
  const lines = Array.from({ length: term.rows }, (_, row) =>
    buffer.getLine(buffer.viewportY + row)?.translateToString(true) || '');
  while (lines.length && !lines.at(-1).trim()) lines.pop();
  return lines.join('\n');
}
function cancelRequests() { for (const id of activeRequests) cancel(id); activeRequests.clear(); }
function stopClipboard() {
  clipboardEpoch++; clearInterval(pollTimer); pollTimer = null;
  clipboardChain = clipboardChain.then(clipboardDisarm);
  return clipboardChain;
}
function render() {
  $('translation-btn').hidden = !enabled() || !backend.available;
  $('translation-panel').hidden = !model.open;
  $('translation-btn').setAttribute('aria-pressed', String(model.open));
  if (!model.open) return;
  const mode = $('translation-mode');
  if (model.mode === 'selection') mode.selectedIndex = -1;
  else mode.value = model.mode;
  $('translation-result').textContent = model.result;
  $('translation-copy').disabled = !model.result;
  $('translation-copy-source').disabled = !model.sourceForCopy();
  $('translation-resume').hidden = !(model.mode === 'live' && model.paused);
  $('translation-use-current').hidden = model.mode !== 'clipboard';
  let key;
  if (model.paused) key = 'translation.paused';
  else if (model.error) key = `translation.error.${model.error}`;
  else if (model.mode === 'selection') key = model.result ? 'translation.ready' : 'translation.selected';
  else if (model.mode === 'clipboard' && !model.source) key = 'translation.waiting';
  else if (model.mode === 'live' && !model.source) key = 'translation.empty';
  else key = model.isUpdating() ? 'translation.updating' : model.result ? 'translation.ready' : 'translation.translating';
  $('translation-status').textContent = t(key) || t('translation.error.translation-failed');
}
async function refreshCapability() {
  if (!enabled()) { backend = { available: false }; closeTranslationLens(); shortcutDispose?.(); shortcutDispose = null; render(); return; }
  try { backend = await capability(); }
  catch { backend = { available: false }; }
  if (backend.available && !shortcutDispose) shortcutDispose = registerShortcutAction('translationLens', translationShortcut);
  if (!backend.available && shortcutDispose) { shortcutDispose(); shortcutDispose = null; closeTranslationLens(); }
  render();
}
function execute(ticket) {
  if (!ticket || !meaningful()) return;
  const id = ++requestCounter;
  activeRequests.add(id);
  translate(id, ticket.text, 'zh-Hans', ticket.mode).then(reply => {
    activeRequests.delete(id);
    const { next } = model.finish(ticket, reply.text);
    render(); if (next) execute(next);
  }).catch(error => {
    activeRequests.delete(id);
    const { next } = model.finish(ticket, null, closedCode(error));
    render(); if (next) execute(next);
  });
  render();
}
function captureLive() {
  if (!meaningful() || model.mode !== 'live' || model.paused || !backend.available) return;
  const pane = focusedPane();
  execute(model.observeLive(visibleViewport(pane), ctx.attachedName || null));
  render();
}
function scheduleLive() { if (meaningful() && model.mode === 'live' && !model.paused) cadence.dirty(); }
const cadence = new LiveCadence({ onCapture: captureLive,
  clock: { now: () => Date.now(), setTimeout: (fn, ms) => window.setTimeout(fn, ms),
    clearTimeout: id => window.clearTimeout(id) } });

async function pollClipboard() {
  if (polling || !meaningful() || model.mode !== 'clipboard') return;
  polling = true; const generation = clipboardEpoch;
  try {
    const text = await clipboardPoll();
    if (generation === clipboardEpoch && typeof text === 'string' && meaningful() && model.mode === 'clipboard') {
      cancelRequests(); execute(model.snapshot(text, 'clipboard'));
    }
  } catch (error) {
    if (meaningful() && model.mode === 'clipboard') {
      const code = closedCode(error);
      if (code === 'clipboard-not-focused') stopClipboard();
      else { model.error = code; render(); }
    }
  } finally { polling = false; }
}
async function startClipboard() {
  await stopClipboard();
  if (!meaningful() || model.mode !== 'clipboard') return;
  const generation = clipboardEpoch;
  try {
    await clipboardArm(); // new baseline only; never reads existing clipboard
    if (generation !== clipboardEpoch || !meaningful() || model.mode !== 'clipboard') return;
    pollTimer = setInterval(pollClipboard, 350);
  } catch { /* native focus gate is authoritative */ }
}
export function closeTranslationLens() {
  if (!model.open) return;
  cadence.stop(); stopClipboard(); cancelRequests(); model.close(); render();
  unload().catch(() => {});
}
function open(mode = 'live') {
  if (!enabled() || !backend.available || state.view !== 'session') return;
  deps.closeBuffer(); model.show(mode);
  $('session-workspace').classList.toggle('translation-overlay',
    $('session-workspace').clientWidth < 440 + 480);
  render(); if (mode === 'live') scheduleLive();
}
async function selectedText() {
  const pane = focusedPane(); let text = null;
  if (pane && hasTerminalSelection(pane)) text = await copyTerminalSelection(pane).catch(() => null);
  else if (pane?.term.hasSelection?.()) text = pane.term.getSelection();
  return text;
}
function translateSelection(text) {
  if (!model.open) open('selection');
  else { cadence.stop(); stopClipboard(); cancelRequests(); }
  if (!model.open) return;
  if (bytes(text) > 16384) { model.snapshot('', 'selection'); model.running = null;
    model.error = 'text-too-large'; render(); return; }
  execute(model.snapshot(text, 'selection'));
}
async function translationShortcut() {
  if (!enabled() || state.view !== 'session') return;
  const text = await selectedText();
  const action = translationShortcutAction(!!text, model.open);
  if (action === 'selection') translateSelection(text);
  else if (action === 'close') closeTranslationLens();
  else open();
}
async function copySnapshot(text, source = false) {
  if (!text) return;
  const clipboardMode = model.mode === 'clipboard';
  if (clipboardMode) await stopClipboard();
  try { await copyTranslation(text); toast(t(source ? 'translation.sourceCopied' : 'translation.copied')); }
  catch { toast(t('translation.copyFailed')); }
  finally { if (clipboardMode && meaningful() && model.mode === 'clipboard') startClipboard(); }
}
export function initTranslationLens({ panes, closeBuffer }) {
  deps = { panes, closeBuffer };
  $('translation-btn').onclick = () => { if (model.open) closeTranslationLens(); else open(); };
  $('translation-close').onclick = closeTranslationLens;
  $('translation-mode').onchange = event => {
    const mode = event.target.value;
    if (!model.modeTo(mode)) return;
    cadence.stop(); stopClipboard(); cancelRequests(); render();
    if (mode === 'live') scheduleLive(); else startClipboard();
  };
  $('translation-use-current').onclick = async () => {
    if (!meaningful() || model.mode !== 'clipboard') return;
    try { const text = await clipboardCurrent();
      if (!meaningful() || model.mode !== 'clipboard') return;
      cancelRequests(); execute(model.snapshot(text, 'clipboard'));
    } catch (error) { model.error = closedCode(error); render(); }
  };
  $('translation-resume').onclick = () => { if (model.resume()) scheduleLive(); };
  $('translation-copy').onclick = () => copySnapshot(model.result);
  $('translation-copy-source').onclick = () => copySnapshot(model.sourceForCopy(), true);
  const result = $('translation-result');
  const pause = () => { if (model.pause()) { cadence.stop(); cancelRequests(); render(); } };
  result.addEventListener('pointerdown', pause);
  result.addEventListener('wheel', pause, { passive: true });
  result.addEventListener('keydown', pause);
  window.addEventListener('deck-terminal-changed', event => { if (event.detail === ctx.attachedName) scheduleLive(); });
  window.addEventListener('deck-pane-focused', scheduleLive);
  window.addEventListener('deck-session-leave', closeTranslationLens);
  window.addEventListener('deck-buffer-open', closeTranslationLens);
  window.addEventListener('deck-translation-disabled', closeTranslationLens);
  window.addEventListener('deck-translation-enabled-changed', refreshCapability);
  window.addEventListener('blur', () => { stopClipboard(); cadence.stop(); cancelRequests();
    model.running = null; model.pending = null; if (model.mode === 'live') model.source = ''; });
  window.addEventListener('focus', () => { if (model.mode === 'clipboard') startClipboard(); else scheduleLive(); });
  new ResizeObserver(() => {
    $('session-workspace').classList.toggle('translation-overlay',
      $('session-workspace').clientWidth < 440 + 480);
    scheduleLive();
  }).observe($('session-workspace'));
  onLocaleChange(render);
  refreshCapability(); // default OFF never verifies or loads a model
}
