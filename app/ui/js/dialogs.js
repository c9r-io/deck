// dialogs.js — confirm/prompt dialogs, toasts, inline rename, settings modal
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, inv, uev } from './state.js';
import { inlineRenameValue } from './pure.js';
import { applyTranslations, setLocale, t, translateNotice } from './i18n.js';
import { parseSettings, serializeSettings } from './settings-model.js';

/* ---------- confirm dialog (window.confirm is a silent no-op in WKWebView) ---------- */
let confirmPointerOnly = false;
export function confirmDialog(msg) {
  return new Promise(resolve => {
    confirmPointerOnly = false;
    cfmResolve = resolve;
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-yes').focus();
  });
}
/* High-risk scheduler actions must not be accepted by an ordinary Enter or
   blur. The safe button receives focus and only an explicit activation of
   the confirm button can accept. */
export function confirmDangerDialog(msg) {
  return new Promise(resolve => {
    confirmPointerOnly = true;
    cfmResolve = resolve;
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-no').focus();
  });
}
export function cfmDone(v) {
  $('cfm').style.display = 'none';
  confirmPointerOnly = false;
  if (cfmResolve) { cfmResolve(v); cfmResolve = null; }
}
$('cfm-yes').onclick = () => cfmDone(true);
$('cfm-no').onclick = () => cfmDone(false);
$('cfm').addEventListener('mousedown', e => { if (e.target === $('cfm')) cfmDone(false); });
document.addEventListener('keydown', e => {
  if ($('cfm').style.display !== 'flex') return;
  if (e.key === 'Enter') {
    e.stopPropagation(); e.preventDefault();
    if (!confirmPointerOnly) cfmDone(true);
  }
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
    const value = inlineRenameValue(current, input.value, commit, allowEmpty);
    /* End the editing DOM/focus state before subscribers can render. Enter
       therefore looks committed in the same gesture and its subsequent blur
       is guaranteed to be a no-op. */
    host.textContent = value === null ? current : value;
    Promise.resolve(onDone(value)).catch(() => {
      if (host.isConnected) host.textContent = current;
      toast(t('error.changeNotSaved'));
    });
  };
  input.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      if (e.isComposing || e.keyCode === 229) return;
      e.preventDefault();
      finish(true);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      finish(false);
    }
  });
  input.addEventListener('blur', () => finish(true));
  input.addEventListener('click', e => e.stopPropagation());
  input.addEventListener('dblclick', e => e.stopPropagation());
}

/* ---------- settings ---------- */
export async function loadSettings() {
  try {
    const doc = await inv('load_settings');
    if (doc && doc.warning) toast(translateNotice(doc.warning));
    if (doc && doc.data) settings = parseSettings(doc.data);
  } catch (e) {
    toast(t('error.settingsLoad'));   // NOT a first run — defaults stay in memory only
    uev('settings-load-fail');
  }
  setLocale(settings.locale);
  inv('set_native_locale', { locale: settings.locale }).catch(() => {});
  window.__DECK_DEBUG = !!settings.debug;
}

export function persistSettings() {
  inv('save_settings', { data: serializeSettings(settings) })
    .catch(() => uev('settings-save-fail'));
}

export async function openSettings() {
  const sel = $('set-editor');
  sel.innerHTML = '';
  const mk = (v, t) => { const o = document.createElement('option'); o.value = v; o.textContent = t; sel.appendChild(o); };
  mk('', t('settings.systemEditor'));
  const eds = await inv('detect_editors').catch(() => []);
  eds.forEach(name => mk(name, name));
  if (settings.editor && !eds.includes(settings.editor)) mk(settings.editor, t('common.notFound', { name: settings.editor }));
  sel.value = settings.editor || '';
  $('set-locale').value = settings.locale || 'system';
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
  toast(settings.editor ? t('settings.editorSelected', { editor: settings.editor }) : t('settings.editorSystem'));
};
$('set-locale').onchange = () => {
  settings.locale = $('set-locale').value;
  setLocale(settings.locale);
  applyTranslations();
  const firstEditor = $('set-editor').options && $('set-editor').options[0];
  if (firstEditor && firstEditor.value === '') firstEditor.textContent = t('settings.systemEditor');
  inv('set_native_locale', { locale: settings.locale }).catch(() => {});
  persistSettings();
};
/* set-check's click handler is wired by app.js (which owns update checks) —
   keeps dialogs.js from importing app.js back (no module cycle) */
$('set-clear-hist').onclick = async () => {
  if (!(await confirmDialog(t('settings.clearHistoryConfirm')))) return;
  inv('history_clear')
    .then(() => toast(t('settings.historyCleared')))
    .catch(() => toast(t('error.operation', { operation: t('common.clear') })));
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
      if (e.key === 'Enter') {
        if (e.isComposing || e.keyCode === 229) return;
        done(inp.value.trim() || null);
      }
      if (e.key === 'Escape') done(null);
    };
  });
}
