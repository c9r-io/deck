// dialogs.js — confirm/prompt dialogs, toasts, inline rename, settings modal
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, inv, uev } from './state.js';
import { inlineRenameValue } from './pure.js';
import { applyTranslations, onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import {
  FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP, SHORTCUT_ACTIONS,
  normalizeSettings, parseSettings, serializeSettings,
} from './settings-model.js';
import { activateTheme } from './theme.js';
import { applyFontScale } from './font-scale.js';
import {
  formatShortcut, isSafeShortcut, registerShortcutAction, shortcutConflict, shortcutFromEvent,
} from './shortcuts.js';

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
  activateTheme(settings);
  applyFontScale(settings.fontScale);
  announceShortcutChange();
  inv('set_native_locale', { locale: settings.locale }).catch(() => {});
}

let settingsWriteChain = Promise.resolve();
function saveSettingsCandidate(candidate) {
  const data = serializeSettings(candidate);
  const operation = settingsWriteChain.catch(() => {}).then(() => inv('save_settings', { data }));
  settingsWriteChain = operation;
  return operation;
}

export function persistSettings() {
  return saveSettingsCandidate(settings).catch(() => uev('settings-save-fail'));
}

function renderFontScale() {
  $('set-font-value').textContent = `${Math.round(settings.fontScale * 100)}%`;
  $('set-font-down').disabled = settings.fontScale <= FONT_SCALE_MIN;
  $('set-font-up').disabled = settings.fontScale >= FONT_SCALE_MAX;
  $('set-font-reset').disabled = settings.fontScale === 1;
}

function shortcutLabel(actionId) { return t(`settings.shortcut.${actionId}`); }

export function renderShortcutSettings() {
  const list = $('set-shortcuts');
  list.replaceChildren();
  for (const action of SHORTCUT_ACTIONS) {
    const row = document.createElement('div');
    row.className = 'shortcut-row';
    const label = document.createElement('span');
    label.textContent = shortcutLabel(action.id);
    const capture = document.createElement('button');
    capture.className = 'shortcut-capture';
    capture.dataset.action = action.id;
    capture.textContent = formatShortcut(settings.shortcuts[action.id]);
    capture.title = t('settings.shortcutCapture');
    capture.addEventListener('focus', () => {
      capture.classList.add('capturing');
      capture.textContent = t('settings.shortcutRecording');
    });
    capture.addEventListener('blur', () => {
      capture.classList.remove('capturing');
      capture.textContent = formatShortcut(settings.shortcuts[action.id]);
    });
    capture.addEventListener('keydown', event => {
      if (event.key === 'Tab' && !event.metaKey && !event.ctrlKey && !event.altKey) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.key === 'Escape') { capture.blur(); return; }
      if ((event.key === 'Backspace' || event.key === 'Delete')
          && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
        setShortcut(action.id, '');
        capture.blur();
        return;
      }
      const binding = shortcutFromEvent(event);
      if (!isSafeShortcut(binding)) { toast(t('settings.shortcutUnsafe')); return; }
      const conflict = shortcutConflict(settings.shortcuts, action.id, binding);
      if (conflict) {
        toast(t('settings.shortcutConflict', { action: shortcutLabel(conflict) }));
        return;
      }
      setShortcut(action.id, binding);
      capture.blur();
    });
    row.append(label, capture);
    list.appendChild(row);
  }
}

function announceShortcutChange() {
  if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
    window.dispatchEvent(new Event('deck-shortcuts-changed'));
  }
}

let fontGeneration = 0;
export async function setFontScale(value) {
  const generation = ++fontGeneration;
  const previous = settings;
  const bounded = Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Number(value)));
  const candidate = normalizeSettings({ ...settings, fontScale: bounded });
  if (candidate.fontScale === settings.fontScale) return;
  settings = candidate;
  applyFontScale(candidate.fontScale);
  renderFontScale();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== fontGeneration) return;
    settings = previous;
    applyFontScale(previous.fontScale);
    renderFontScale();
    toast(t('error.fontSave'));
    uev('settings-save-fail');
  }
}

let shortcutGeneration = 0;
export async function setShortcut(actionId, binding) {
  const generation = ++shortcutGeneration;
  const previous = settings;
  const candidate = normalizeSettings({
    ...settings, shortcuts: { ...settings.shortcuts, [actionId]: binding },
  });
  settings = candidate;
  renderShortcutSettings();
  announceShortcutChange();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== shortcutGeneration) return;
    settings = previous;
    renderShortcutSettings();
    announceShortcutChange();
    toast(t('error.shortcutSave'));
    uev('settings-save-fail');
  }
}

export async function resetShortcuts() {
  const generation = ++shortcutGeneration;
  const previous = settings;
  const known = new Set(SHORTCUT_ACTIONS.map(action => action.id));
  const extensions = Object.fromEntries(Object.entries(settings.shortcuts)
    .filter(([actionId]) => !known.has(actionId)));
  const candidate = normalizeSettings({ ...settings, shortcuts: extensions });
  settings = candidate;
  renderShortcutSettings();
  announceShortcutChange();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== shortcutGeneration) return;
    settings = previous;
    renderShortcutSettings();
    announceShortcutChange();
    toast(t('error.shortcutSave'));
    uev('settings-save-fail');
  }
}

registerShortcutAction('fontIncrease', () => setFontScale(settings.fontScale + FONT_SCALE_STEP));
registerShortcutAction('fontDecrease', () => setFontScale(settings.fontScale - FONT_SCALE_STEP));
registerShortcutAction('fontReset', () => setFontScale(1));
onLocaleChange(() => {
  if ($('settings-modal').style.display === 'flex') renderShortcutSettings();
});

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
  $('set-theme').value = settings.theme || 'deck-dark';
  $('set-accent').value = settings.accent || 'teal';
  $('set-channel').value = settings.updateChannel || 'stable';
  $('set-session-restore').checked = !!settings.sessionRestore;
  renderFontScale();
  renderShortcutSettings();
  $('set-ver').textContent = 'deck ' + ($('app-ver').textContent || 'v?');
  $('set-upd-status').textContent = '';
  $('settings-modal').style.display = 'flex';
  if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
    window.dispatchEvent(new Event('deck-settings-opened'));
  }
}

$('settings-btn').onclick = openSettings;
$('set-close').onclick = () => { $('settings-modal').style.display = 'none'; };
$('settings-modal').addEventListener('mousedown', e => {
  if (e.target === $('settings-modal')) $('settings-modal').style.display = 'none';
});
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
$('set-theme').onchange = () => persistThemeChoice();
$('set-accent').onchange = () => persistThemeChoice();
$('set-channel').onchange = () => persistUpdateChannelChoice();
$('set-session-restore').onchange = () => persistSessionRestoreChoice();
$('set-font-down').onclick = () => setFontScale(settings.fontScale - FONT_SCALE_STEP);
$('set-font-up').onclick = () => setFontScale(settings.fontScale + FONT_SCALE_STEP);
$('set-font-reset').onclick = () => setFontScale(1);
$('set-shortcuts-reset').onclick = resetShortcuts;

let themeSavePending = false;
export async function persistThemeChoice() {
  if (themeSavePending) return;
  const previous = { theme: settings.theme, accent: settings.accent };
  const candidate = normalizeSettings({
    ...settings,
    theme: $('set-theme').value,
    accent: $('set-accent').value,
  });
  themeSavePending = true;
  const locked = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'].map($);
  locked.forEach(control => { control.disabled = true; });
  activateTheme(candidate); // immediate preview; commit only after durable save
  try {
    await saveSettingsCandidate(candidate);
    settings = candidate;
  } catch (_) {
    activateTheme({ ...settings, ...previous });
    $('set-theme').value = previous.theme;
    $('set-accent').value = previous.accent;
    toast(t('error.themeSave'));
    uev('settings-save-fail');
  } finally {
    themeSavePending = false;
    locked.forEach(control => { control.disabled = false; });
  }
}

let channelSavePending = false;
export async function persistUpdateChannelChoice() {
  if (channelSavePending) return;
  const previous = settings.updateChannel || 'stable';
  const desired = $('set-channel').value;
  if (desired === 'nightly' && previous !== 'nightly') {
    const accepted = await confirmDialog(t('settings.channelNightlyConfirm'));
    if (!accepted) {
      $('set-channel').value = previous;
      return;
    }
  }
  const candidate = normalizeSettings({ ...settings, updateChannel: desired });
  channelSavePending = true;
  const locked = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'].map($);
  locked.forEach(control => { control.disabled = true; });
  try {
    await saveSettingsCandidate(candidate);
    settings = candidate;
    toast(t(candidate.updateChannel === 'nightly'
      ? 'settings.channelNightlyEnabled' : 'settings.channelStableEnabled'));
    if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
      window.dispatchEvent(new Event('deck-update-channel-changed'));
    }
    $('set-ver').textContent = 'deck ' + ($('app-ver').textContent || 'v?');
  } catch (_) {
    $('set-channel').value = previous;
    toast(t('error.settingsSave'));
    uev('settings-save-fail');
  } finally {
    channelSavePending = false;
    locked.forEach(control => { control.disabled = false; });
  }
}
let shellRestoreSavePending = false;
export async function persistSessionRestoreChoice() {
  if (shellRestoreSavePending) return;
  const previous = !!settings.sessionRestore;
  const desired = $('set-session-restore').checked;
  if (desired && !previous) {
    const accepted = await confirmDialog(t('settings.shellRecoveryEnableConfirm'));
    if (!accepted) {
      $('set-session-restore').checked = false;
      return;
    }
  }
  const candidate = normalizeSettings({ ...settings, sessionRestore: desired });
  shellRestoreSavePending = true;
  $('set-session-restore').disabled = true;
  $('set-clear-shell').disabled = true;
  try {
    // Persist the privacy preference first. A failed disable keeps the old
    // behavior visible instead of claiming recovery is off when it is not.
    await saveSettingsCandidate(candidate);
    settings = candidate;
    if (!desired) {
      try {
        await inv('shell_snapshots_clear');
      } catch (_) {
        toast(t('settings.shellRecoveryClearFailed'));
      }
    }
    toast(t(desired ? 'settings.shellRecoveryEnabled' : 'settings.shellRecoveryDisabled'));
  } catch (_) {
    $('set-session-restore').checked = previous;
    toast(t('error.restoreSave'));
    uev('settings-save-fail');
  } finally {
    shellRestoreSavePending = false;
    $('set-session-restore').disabled = false;
    $('set-clear-shell').disabled = false;
  }
}

/* set-check's click handler is wired by app.js (which owns update checks) —
   keeps dialogs.js from importing app.js back (no module cycle) */
$('set-clear-hist').onclick = async () => {
  if (!(await confirmDialog(t('settings.clearHistoryConfirm')))) return;
  inv('history_clear')
    .then(() => toast(t('settings.historyCleared')))
    .catch(() => toast(t('error.operation', { operation: t('common.clear') })));
};
$('set-clear-shell').onclick = async () => {
  if (!(await confirmDialog(t('settings.clearShellRecoveryConfirm')))) return;
  inv('shell_snapshots_clear')
    .then(() => toast(t('settings.shellRecoveryCleared')))
    .catch(() => toast(t('settings.shellRecoveryClearFailed')));
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
