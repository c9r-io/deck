// dialogs.js — confirm/prompt dialogs, toasts, inline rename, settings modal
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, inv, uev } from './state.js';
import { inlineRenameValue } from './pure.js';

/* ---------- confirm dialog (window.confirm is a silent no-op in WKWebView) ---------- */
export function confirmDialog(msg) {
  return new Promise(resolve => {
    cfmResolve = resolve;
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-yes').focus();
  });
}
export function cfmDone(v) {
  $('cfm').style.display = 'none';
  if (cfmResolve) { cfmResolve(v); cfmResolve = null; }
}
$('cfm-yes').onclick = () => cfmDone(true);
$('cfm-no').onclick = () => cfmDone(false);
$('cfm').addEventListener('mousedown', e => { if (e.target === $('cfm')) cfmDone(false); });
document.addEventListener('keydown', e => {
  if ($('cfm').style.display !== 'flex') return;
  if (e.key === 'Enter') { e.stopPropagation(); e.preventDefault(); cfmDone(true); }
  if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); cfmDone(false); }
}, true);

/* ---------- toasts ---------- */
export function toast(msg) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.textContent = msg;
  $('toasts').appendChild(el);
  setTimeout(() => el.remove(), 2600);
}

/* ---------- inline rename helper ---------- */
export function inlineRename(host, current, onDone, allowEmpty = false) {
  const input = document.createElement('input');
  input.value = current;
  host.replaceChildren(input);
  input.focus();
  input.select();
  let done = false;
  const finish = commit => {
    if (done) return;
    done = true;
    onDone(inlineRenameValue(current, input.value, commit, allowEmpty));
  };
  input.addEventListener('keydown', e => {
    if (e.key === 'Enter') finish(true);
    if (e.key === 'Escape') finish(false);
    e.stopPropagation();
  });
  input.addEventListener('blur', () => finish(true));
  input.addEventListener('click', e => e.stopPropagation());
  input.addEventListener('dblclick', e => e.stopPropagation());
}

/* ---------- settings ---------- */
export async function loadSettings() {
  try {
    const doc = await inv('load_settings');
    if (doc && doc.warning) toast(doc.warning);
    if (doc && doc.data) settings = { editor: '', debug: false, ...JSON.parse(doc.data) };
  } catch (e) {
    toast('settings could not be loaded: ' + e);   // NOT a first run — defaults stay in memory only
    uev('settings-load-fail');
  }
  window.__DECK_DEBUG = !!settings.debug;
}

export function persistSettings() {
  inv('save_settings', { data: JSON.stringify(settings, null, 2) })
    .catch(() => uev('settings-save-fail'));
}

export async function openSettings() {
  const sel = $('set-editor');
  sel.innerHTML = '';
  const mk = (v, t) => { const o = document.createElement('option'); o.value = v; o.textContent = t; sel.appendChild(o); };
  mk('', 'System default (TextEdit)');
  const eds = await inv('detect_editors').catch(() => []);
  eds.forEach(name => mk(name, name));
  if (settings.editor && !eds.includes(settings.editor)) mk(settings.editor, settings.editor + ' (not found)');
  sel.value = settings.editor || '';
  $('set-debug').checked = !!settings.debug;
  $('set-ver').textContent = 'deck v' + ($('app-ver').textContent || '?');
  $('set-upd-status').textContent = '';
  $('settings-modal').style.display = 'flex';
}

$('settings-btn').onclick = openSettings;
$('set-close').onclick = () => { $('settings-modal').style.display = 'none'; };
$('settings-modal').addEventListener('mousedown', e => {
  if (e.target === $('settings-modal')) $('settings-modal').style.display = 'none';
});
$('set-debug').onchange = () => {
  settings.debug = $('set-debug').checked;
  window.__DECK_DEBUG = settings.debug;
  persistSettings();
};
$('set-editor').onchange = () => {
  settings.editor = $('set-editor').value;
  persistSettings();
  toast(settings.editor ? 'file links open in ' + settings.editor : 'file links open in system default');
};
/* set-check's click handler is wired by app.js (which owns update checks) —
   keeps dialogs.js from importing app.js back (no module cycle) */
$('set-clear-hist').onclick = async () => {
  if (!(await confirmDialog('Clear deck’s command history (and its backup)? Completion chips will start over.'))) return;
  inv('history_clear')
    .then(() => toast('command history cleared'))
    .catch(e => toast('clear failed: ' + e));
};

export function promptDialog(msg, initial = '') {
  return new Promise(res => {
    $('ppd-msg').textContent = msg;
    const inp = $('ppd-input');
    inp.value = initial;
    $('ppd').style.display = 'flex';
    inp.focus();
    inp.select();
    const done = v => { $('ppd').style.display = 'none'; inp.onkeydown = null; res(v); };
    $('ppd-yes').onclick = () => done(inp.value.trim() || null);
    $('ppd-no').onclick = () => done(null);
    inp.onkeydown = e => {
      if (e.key === 'Enter') done(inp.value.trim() || null);
      if (e.key === 'Escape') done(null);
    };
  });
}
