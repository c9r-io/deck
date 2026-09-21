// dialogs.js — confirm/prompt/choice dialogs, project defaults, toasts, inline rename, settings modal
// Voice preferences commit through the settings writer before notifying the
// recorder; edits never request microphone access or download language assets.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, genId, inv, store, uev } from './state.js';
import { inlineRenameValue, isComposingKeyEvent } from './pure.js';
import { applyTranslations, formatNumber, getLocale, onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import {
  CUSTOMIZABLE_SHORTCUT_ACTIONS, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP, SHORTCUT_ACTIONS,
  normalizeSettings, parseSettings, serializeSettings,
} from './settings-model.js';
import { normalizeVoicePreferences } from './voice-preferences-model.js';
import { createVoiceSettings } from './voice-settings.js';
import { activateTheme } from './theme.js';
import { applyFontScale } from './font-scale.js';
import { normalizeTaskPreset, normalizeTaskPresets } from './connector-model.js';
import {
  formatShortcut, isSafeShortcut, registerShortcutAction, shortcutConflict, shortcutFromEvent,
} from './shortcuts.js';

/* ---------- confirm dialog (window.confirm is a silent no-op in WKWebView) ---------- */
let confirmPointerOnly = false;
export function confirmDialog(msg) {
  return new Promise(resolve => {
    confirmPointerOnly = false;
    ctx.cfmResolve = resolve;
    $('cfm-yes').textContent = t('common.confirm');
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-yes').focus();
  });
}
/* High-risk scheduler actions must not be accepted by an ordinary Enter or
   blur. The safe button receives focus and only an explicit activation of
   the confirm button can accept. */
export function confirmDangerDialog(msg, confirmLabel = t('common.confirm')) {
  return new Promise(resolve => {
    confirmPointerOnly = true;
    ctx.cfmResolve = resolve;
    $('cfm-yes').textContent = confirmLabel;
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-no').focus();
  });
}
export function cfmDone(v) {
  $('cfm').style.display = 'none';
  confirmPointerOnly = false;
  if (ctx.cfmResolve) { ctx.cfmResolve(v); ctx.cfmResolve = null; }
}

/* ---------- choice dialog: one question, explicit answers ----------
   Used where deck must not guess (a directory that no longer exists).
   Resolves the chosen id, or null on Cancel / Escape / a click outside.
   Enter is not bound: the focused button (the caller's primary choice)
   receives it natively, so a stray Enter can only take that one choice. */
let chdResolve = null;
export function choiceDialog(msg, choices) {
  return new Promise(resolve => {
    if (chdResolve) chdResolve(null);
    $('chd-msg').textContent = msg;
    const actions = $('chd-actions');
    actions.replaceChildren();
    const done = v => { $('chd').style.display = 'none'; $('chd').onkeydown = null; chdResolve = null; resolve(v); };
    chdResolve = done;
    const cancel = document.createElement('button');
    cancel.type = 'button'; cancel.className = 'btn'; cancel.textContent = t('common.cancel');
    cancel.onclick = () => done(null);
    actions.appendChild(cancel);
    let primary = null;
    for (const c of choices) {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'btn' + (c.primary ? ' primary' : ''); b.textContent = c.label;
      b.onclick = () => done(c.id);
      actions.appendChild(b);
      if (c.primary && !primary) primary = b;
    }
    $('chd').onkeydown = e => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); } };
    $('chd').style.display = 'flex';
    (primary || cancel).focus();
  });
}

/* ---------- project defaults dialog (04 A v01) ----------
   The project's default directory and launch command, edited together.
   Resolves { dir, cmd } (trimmed; blank = no default) on Save / Enter, null
   on Cancel / Escape / a click outside. `recent` are command chips that only
   FILL the command field — nothing in this dialog runs anything. */
let pdfResolve = null;
export function projectDefaultsDialog({ name, dir = '', cmd = '', recent = [], presets = [], columns = [] }) {
  return new Promise(resolve => {
    if (pdfResolve) pdfResolve(null);
    $('pdf-title').textContent = t('projectDefaults.title', { name });
    const dirInput = $('pdf-dir'), cmdInput = $('pdf-cmd');
    dirInput.value = dir; cmdInput.value = cmd;
    const chips = $('pdf-chips');
    chips.replaceChildren();
    for (const c of recent.slice(0, 6)) {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'pdf-chip'; b.textContent = c;
      b.onclick = () => { cmdInput.value = c; cmdInput.focus(); };
      chips.appendChild(b);
    }
    chips.hidden = !recent.length;
    let draftPresets = normalizeTaskPresets(presets, columns); let editingPreset = null;
    const editor = $('pdf-preset-editor');
    const renderPresets = () => {
      const list = $('pdf-presets'); list.replaceChildren();
      for (const preset of draftPresets) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'btn'; button.textContent = preset.name;
        button.onclick = () => openPreset(preset); list.appendChild(button);
      }
      $('pdf-preset-add').disabled = draftPresets.length >= 50;
    };
    const openPreset = preset => {
      editingPreset = preset?.id || genId('R');
      $('pdf-preset-name').value = preset?.name || '';
      $('pdf-preset-title').value = preset?.title || '';
      $('pdf-preset-dir').value = preset?.dir || dirInput.value.trim();
      $('pdf-preset-cmd').value = preset?.cmd || cmdInput.value.trim() || 'codex';
      $('pdf-preset-steps').value = (preset?.steps || []).join('\n');
      const target = $('pdf-preset-column'); target.replaceChildren();
      for (const column of columns) { const option = document.createElement('option'); option.value = column.id; option.textContent = column.name; target.appendChild(option); }
      target.value = preset?.columnId || columns[0]?.id || '';
      $('pdf-preset-delete').hidden = !preset;
      editor.hidden = false; $('pdf-preset-name').focus();
    };
    const commitPreset = () => {
      if (!editingPreset) return true;
      const preset = normalizeTaskPreset({ id: editingPreset, name: $('pdf-preset-name').value,
        columnId: $('pdf-preset-column').value, title: $('pdf-preset-title').value,
        dir: $('pdf-preset-dir').value, cmd: $('pdf-preset-cmd').value,
        steps: $('pdf-preset-steps').value.split('\n') }, columns);
      if (!preset) { toast(t('presets.invalid')); return false; }
      draftPresets = [...draftPresets.filter(value => value.id !== editingPreset), preset];
      editingPreset = null; editor.hidden = true; renderPresets(); return true;
    };
    $('pdf-preset-add').onclick = () => openPreset(null);
    $('pdf-preset-done').onclick = commitPreset;
    $('pdf-preset-delete').onclick = () => {
      draftPresets = draftPresets.filter(value => value.id !== editingPreset);
      editingPreset = null; editor.hidden = true; renderPresets();
    };
    renderPresets(); editor.hidden = true;
    const read = () => commitPreset() ? ({ dir: dirInput.value.trim(), cmd: cmdInput.value.trim(),
      ...(draftPresets.length || presets.length ? { presets: draftPresets } : {}) }) : null;
    const done = v => { $('pdf').style.display = 'none'; $('pdf').onkeydown = null; pdfResolve = null; resolve(v); };
    pdfResolve = done;
    $('pdf-yes').onclick = () => { const value = read(); if (value) done(value); };
    $('pdf-no').onclick = () => done(null);
    $('pdf').onkeydown = e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); return; }
      if (e.key === 'Enter' && e.target && e.target.tagName === 'INPUT') {
        if (isComposingKeyEvent(e)) return;
        e.preventDefault(); e.stopPropagation();
        const value = read(); if (value) done(value);
      }
    };
    $('pdf').style.display = 'flex';
    dirInput.focus();
    dirInput.select();
  });
}

/* ---------- toasts ---------- */
export function toast(msg) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.textContent = msg;
  $('toasts').appendChild(el);
  setTimeout(() => el.remove(), 2600);
}

/* ---------- inline rename helper ----------
   A textarea that shows all of its content instead of one scrolling line.
   The ceiling is CSS (`max-height` on the field), so an over-tall height set
   here is simply clamped and the field scrolls — no computed style is read,
   and a host without layout (the DOM-contract tests) is left alone. */
export function autoGrowField(field) {
  if (!field || !field.style) return;
  field.style.height = 'auto';
  if (typeof field.scrollHeight === 'number') field.style.height = field.scrollHeight + 'px';
}

/* One-shot edit lifecycle for a value shown in place. `multiline` swaps the
   input for a growing textarea: a queued prompt or a template step can be
   many lines, so Enter there TYPES a newline and ⌘↵/⌃↵ is what commits.
   Escape restores, blur commits, and an IME's Enter never submits. */
export function inlineRename(host, current, onDone, { allowEmpty = false, multiline = false } = {}) {
  const field = document.createElement(multiline ? 'textarea' : 'input');
  field.value = current;
  if (multiline) {
    field.className = 'inline-multiline';
    field.rows = 1;
    field.spellcheck = false;
  }
  host.replaceChildren(field);
  if (multiline) autoGrowField(field);
  field.focus();
  field.select();
  let done = false;
  const finish = commit => {
    if (done) return;
    done = true;
    const value = inlineRenameValue(current, field.value, commit, allowEmpty);
    /* End the editing DOM/focus state before subscribers can render. Enter
       therefore looks committed in the same gesture and its subsequent blur
       is guaranteed to be a no-op. */
    host.textContent = value === null ? current : value;
    Promise.resolve(onDone(value)).catch(() => {
      if (host.isConnected) host.textContent = current;
      toast(t('error.changeNotSaved'));
    });
  };
  field.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      if (e.isComposing || e.keyCode === 229) return;
      if (multiline && !(e.metaKey || e.ctrlKey)) return;   // the newline is the content
      e.preventDefault();
      finish(true);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      finish(false);
    }
  });
  if (multiline) field.addEventListener('input', () => autoGrowField(field));
  field.addEventListener('blur', () => finish(true));
  field.addEventListener('click', e => e.stopPropagation());
  field.addEventListener('dblclick', e => e.stopPropagation());
}

/* ---------- settings ---------- */
const SETTINGS_SECTIONS = ['general', 'shortcuts', 'terminal', 'integrations', 'data', 'about'];
let activeSettingsSection = 'general';

export function filterSettings() {
  const query = $('set-search').value.trim().toLocaleLowerCase();
  let matches = 0;
  for (const id of SETTINGS_SECTIONS) {
    const panel = $('set-panel-' + id);
    const nav = $('set-nav-' + id);
    const match = !query || panel.textContent.toLocaleLowerCase().includes(query);
    nav.hidden = !match;
    panel.hidden = query ? !match : id !== activeSettingsSection;
    nav.setAttribute('aria-current', !query && id === activeSettingsSection ? 'page' : 'false');
    if (match) matches++;
  }
  $('set-no-results').hidden = matches > 0;
}

export function selectSettingsSection(id) {
  if (!SETTINGS_SECTIONS.includes(id)) return;
  activeSettingsSection = id;
  $('set-search').value = '';
  filterSettings();
  $('set-content').scrollTop = 0;
  if (id === 'data') refreshLogSize();
}

function closeSettings() {
  $('settings-modal').style.display = 'none';
  $('settings-btn').focus();
}

let logOperationPending = false;
let logSizeGeneration = 0;
export async function refreshLogSize() {
  const generation = ++logSizeGeneration;
  $('set-log-size').textContent = 'app.log · …';
  try {
    const bytes = await inv('log_size');
    if (generation !== logSizeGeneration) return;
    const size = bytes < 1024 ? `${formatNumber(bytes)} B`
      : bytes < 1024 * 1024 ? `${formatNumber(Math.round(bytes / 1024))} KB`
        : `${formatNumber(Math.round(bytes / (1024 * 1024) * 10) / 10)} MB`;
    $('set-log-size').textContent = `app.log · ${size}`;
  } catch (_) {
    if (generation === logSizeGeneration) $('set-log-size').textContent = t('settings.logSizeFailed');
  }
}

export async function resetApplicationLogs() {
  if (logOperationPending) return;
  logOperationPending = true;
  const buttons = ['set-reset-logs', 'set-export-logs'].map($);
  buttons.forEach(button => { button.disabled = true; });
  try {
    if (!(await confirmDangerDialog(t('settings.resetLogsConfirm'), t('settings.resetLogsAction')))) return;
    await inv('reset_logs');
    toast(t('settings.logsReset'));
    await refreshLogSize();
  } catch (_) {
    toast(t('settings.logsResetFailed'));
  } finally {
    logOperationPending = false;
    buttons.forEach(button => { button.disabled = false; });
    $('set-reset-logs').focus();
  }
}

export async function loadSettings() {
  try {
    const doc = await inv('load_settings');
    if (doc && doc.warning) toast(translateNotice(doc.warning));
    if (doc && doc.data) ctx.settings = parseSettings(doc.data);
  } catch (e) {
    toast(t('error.settingsLoad'));   // NOT a first run — defaults stay in memory only
    uev('settings-load-fail');
  }
  setLocale(ctx.settings.locale);
  activateTheme(ctx.settings);
  applyFontScale(ctx.settings.fontScale);
  announceShortcutChange();
  announceVoicePreferences();
  inv('set_native_locale', { locale: getLocale() }).catch(() => {});
}

let settingsWriteChain = Promise.resolve();
function saveSettingsCandidate(candidate) {
  const data = serializeSettings(candidate);
  const operation = settingsWriteChain.catch(() => {}).then(() => inv('save_settings', { data }));
  settingsWriteChain = operation;
  return operation;
}

export function persistSettings() {
  return saveSettingsCandidate(ctx.settings).catch(() => uev('settings-save-fail'));
}

const voiceSettings = createVoiceSettings({
  drain: () => settingsWriteChain.catch(() => {}), save: saveSettingsCandidate,
  failed: () => { toast(t('settings.voiceSaveFailed')); uev('settings-save-fail'); },
});
const { render: renderVoicePreferences, save: persistVoicePreferences, announce: announceVoicePreferences } = voiceSettings;
export { renderVoicePreferences, persistVoicePreferences };

function renderFontScale() {
  $('set-font-value').textContent = `${Math.round(ctx.settings.fontScale * 100)}%`;
  $('set-font-down').disabled = ctx.settings.fontScale <= FONT_SCALE_MIN;
  $('set-font-up').disabled = ctx.settings.fontScale >= FONT_SCALE_MAX;
  $('set-font-reset').disabled = ctx.settings.fontScale === 1;
}

function shortcutLabel(actionId) { return t(`settings.shortcut.${actionId}`); }

export function renderShortcutSettings() {
  const list = $('set-shortcuts');
  list.replaceChildren();
  for (const action of CUSTOMIZABLE_SHORTCUT_ACTIONS) {
    const row = document.createElement('div');
    row.className = 'shortcut-row';
    const label = document.createElement('span');
    label.textContent = shortcutLabel(action.id);
    const capture = document.createElement('button');
    capture.className = 'shortcut-capture';
    capture.dataset.action = action.id;
    capture.textContent = formatShortcut(ctx.settings.shortcuts[action.id]);
    capture.title = t('settings.shortcutCapture');
    capture.addEventListener('focus', () => {
      capture.classList.add('capturing');
      capture.textContent = t('settings.shortcutRecording');
    });
    capture.addEventListener('blur', () => {
      capture.classList.remove('capturing');
      capture.textContent = formatShortcut(ctx.settings.shortcuts[action.id]);
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
      const conflict = shortcutConflict(ctx.settings.shortcuts, action.id, binding);
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
  if (voiceSettings.isPending()) return;
  const generation = ++fontGeneration;
  const previous = ctx.settings;
  const bounded = Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Number(value)));
  const candidate = normalizeSettings({ ...ctx.settings, fontScale: bounded });
  if (candidate.fontScale === ctx.settings.fontScale) return;
  ctx.settings = candidate;
  applyFontScale(candidate.fontScale);
  renderFontScale();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== fontGeneration) return;
    ctx.settings = previous;
    applyFontScale(previous.fontScale);
    renderFontScale();
    toast(t('error.fontSave'));
    uev('settings-save-fail');
  }
}

let shortcutGeneration = 0;
export async function setShortcut(actionId, binding) {
  const generation = ++shortcutGeneration;
  const previous = ctx.settings;
  const candidate = normalizeSettings({
    ...ctx.settings, shortcuts: { ...ctx.settings.shortcuts, [actionId]: binding },
  });
  ctx.settings = candidate;
  renderShortcutSettings();
  announceShortcutChange();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== shortcutGeneration) return;
    ctx.settings = previous;
    renderShortcutSettings();
    announceShortcutChange();
    toast(t('error.shortcutSave'));
    uev('settings-save-fail');
  }
}

export async function resetShortcuts() {
  const generation = ++shortcutGeneration;
  const previous = ctx.settings;
  const known = new Set(SHORTCUT_ACTIONS.map(action => action.id));
  const extensions = Object.fromEntries(Object.entries(ctx.settings.shortcuts)
    .filter(([actionId]) => !known.has(actionId)));
  const candidate = normalizeSettings({ ...ctx.settings, shortcuts: extensions });
  ctx.settings = candidate;
  renderShortcutSettings();
  announceShortcutChange();
  try {
    await saveSettingsCandidate(candidate);
  } catch (_) {
    if (generation !== shortcutGeneration) return;
    ctx.settings = previous;
    renderShortcutSettings();
    announceShortcutChange();
    toast(t('error.shortcutSave'));
    uev('settings-save-fail');
  }
}

export async function openSettings() {
  const sel = $('set-editor');
  sel.innerHTML = '';
  const mk = (v, t) => { const o = document.createElement('option'); o.value = v; o.textContent = t; sel.appendChild(o); };
  mk('', t('settings.systemEditor'));
  const eds = await inv('detect_editors').catch(() => []);
  eds.forEach(name => mk(name, name));
  if (ctx.settings.editor && !eds.includes(ctx.settings.editor)) mk(ctx.settings.editor, t('common.notFound', { name: ctx.settings.editor }));
  sel.value = ctx.settings.editor || '';
  $('set-locale').value = ctx.settings.locale || 'system';
  $('set-theme').value = ctx.settings.theme || 'deck-dark';
  $('set-accent').value = ctx.settings.accent || 'teal';
  $('set-channel').value = ctx.settings.updateChannel || 'stable';
  $('set-session-restore').checked = !!ctx.settings.sessionRestore;
  $('set-agent-hooks').checked = false;
  $('set-codex-hooks').checked = false;
  inv('agent_hooks_status')
    .then(status => {
      $('set-agent-hooks').checked = !!(status && status.claude);
      $('set-codex-hooks').checked = !!(status && status.codex);
    })
    .catch(() => {});
  renderFontScale();
  renderShortcutSettings();
  renderVoicePreferences();
  renderInboundSettings();
  renderConnectorSettings();
  renderMcpSettings();
  $('set-ver').textContent = 'deck ' + ($('app-ver').textContent || 'v?');
  $('set-upd-status').textContent = '';
  $('settings-modal').style.display = 'flex';
  selectSettingsSection(activeSettingsSection);
  if (activeSettingsSection !== 'data') refreshLogSize();
  $('set-search').focus();
  if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
    window.dispatchEvent(new Event('deck-settings-opened'));
  }
}

let themeSavePending = false;
export async function persistThemeChoice() {
  if (themeSavePending) return;
  const previous = { theme: ctx.settings.theme, accent: ctx.settings.accent };
  const candidate = normalizeSettings({
    ...ctx.settings,
    theme: $('set-theme').value,
    accent: $('set-accent').value,
  });
  themeSavePending = true;
  const locked = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'].map($);
  locked.forEach(control => { control.disabled = true; });
  activateTheme(candidate); // immediate preview; commit only after durable save
  try {
    await saveSettingsCandidate(candidate);
    ctx.settings = candidate;
  } catch (_) {
    activateTheme({ ...ctx.settings, ...previous });
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
  const previous = ctx.settings.updateChannel || 'stable';
  const desired = $('set-channel').value;
  if (desired === 'nightly' && previous !== 'nightly') {
    const accepted = await confirmDialog(t('settings.channelNightlyConfirm'));
    if (!accepted) {
      $('set-channel').value = previous;
      return;
    }
  }
  const candidate = normalizeSettings({ ...ctx.settings, updateChannel: desired });
  channelSavePending = true;
  const locked = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'].map($);
  locked.forEach(control => { control.disabled = true; });
  try {
    await saveSettingsCandidate(candidate);
    ctx.settings = candidate;
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
  const previous = !!ctx.settings.sessionRestore;
  const desired = $('set-session-restore').checked;
  if (desired && !previous) {
    const accepted = await confirmDialog(t('settings.shellRecoveryEnableConfirm'));
    if (!accepted) {
      $('set-session-restore').checked = false;
      return;
    }
  }
  const candidate = normalizeSettings({ ...ctx.settings, sessionRestore: desired });
  shellRestoreSavePending = true;
  $('set-session-restore').disabled = true;
  $('set-clear-shell').disabled = true;
  try {
    // Persist the privacy preference first. A failed disable keeps the old
    // behavior visible instead of claiming recovery is off when it is not.
    await saveSettingsCandidate(candidate);
    ctx.settings = candidate;
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

/* Agent-status hooks: the checkbox reflects ~/.claude/settings.json itself
   (the backend derives it), so there is no second copy of the state to keep
   in sync and a manual edit of that file shows up here truthfully. */
let agentHooksPending = false;
export async function persistAgentHooksChoice(agent, boxId, confirmKey) {
  if (agentHooksPending) return;
  const box = $(boxId);
  const desired = box.checked;
  if (desired && !(await confirmDialog(t(confirmKey)))) {
    box.checked = false;
    return;
  }
  agentHooksPending = true;
  box.disabled = true;
  try {
    await inv('agent_hooks_set', { agent, enable: desired });
    toast(t(desired ? 'settings.agentHooksEnabled' : 'settings.agentHooksDisabled'));
  } catch (_) {
    box.checked = !desired;
    toast(t('error.agentHooks'));
    uev('settings-save-fail');
  } finally {
    agentHooksPending = false;
    box.disabled = false;
  }
}

/* ---------- Slack connection (inbound): the switch and the tokens ----------
   Rules are the project's automations (automation.js, the ↻ drawer); this
   is only the account-level connection. Tokens live in the Keychain and are
   never read back into the page — the backend only reports whether a slot
   is filled. */
let inboundSavePending = false;

function inboundSlackStatusText(status) {
  const slack = (status && status.sources || []).find(s => s.id === 'slack');
  if (!ctx.settings.inbound.sources.slack.enabled) return t('settings.inboundStatus.off');
  if (!slack) return '';
  const user = slack.secrets.find(x => x.slot === 'slack-user-token');
  if (!user || !user.present) return t('settings.inboundStatus.noToken');
  const parts = [t(slack.live ? 'settings.inboundStatus.live' : 'settings.inboundStatus.polling')];
  if (slack.lastPoll) {
    const ago = Math.max(0, Math.floor(Date.now() / 1000) - slack.lastPoll);
    const label = ago < 60
      ? t('settings.inboundAgoSeconds', { count: formatNumber(ago) })
      : t('settings.inboundAgoMinutes', { count: formatNumber(Math.floor(ago / 60)) });
    parts.push(t('settings.inboundStatus.lastPoll', { ago: label }));
  }
  if (slack.lastError) parts.push(t('settings.inboundStatus.error', { code: slack.lastError }));
  return parts.join(' · ');
}

export async function renderInboundSettings() {
  $('set-inbound-slack').checked = !!ctx.settings.inbound.sources.slack.enabled;
  let status = null;
  try { status = await inv('inbound_status'); } catch (_) { status = null; }
  const present = slot => !!(status && status.sources || []).some(s => s.secrets.some(x => x.slot === slot && x.present));
  for (const [slot, id] of [['slack-user-token', 'set-inbound-slack-user'], ['slack-app-token', 'set-inbound-slack-app']]) {
    const box = $(id);
    box.value = '';
    box.placeholder = present(slot) ? t('settings.inboundTokenSaved') : (slot === 'slack-user-token' ? 'xoxp-…' : 'xapp-…');
    $(id + '-clear').style.display = present(slot) ? '' : 'none';
  }
  $('set-inbound-status').textContent = inboundSlackStatusText(status);
  let channel = null;
  try { channel = await inv('channel_status'); } catch (_) { channel = null; }
  $('set-channel-enabled').checked = !!ctx.settings.inbound.channelConnection?.enabled;
  const channelParts = [];
  if (!channel?.enabled) channelParts.push(t('settings.inboundStatus.off'));
  else if (!channel.tokenReady) channelParts.push(t('settings.inboundStatus.noToken'));
  else channelParts.push(t(channel.connected ? 'settings.inboundStatus.live' : 'settings.channelStatus.disconnected'));
  if (channel?.pendingCount) channelParts.push(t('settings.channelPending', { count: formatNumber(channel.pendingCount) }));
  if (channel?.rejectedCount) channelParts.push(t('settings.channelRejected', { count: formatNumber(channel.rejectedCount) }));
  if (channel?.gapUnresolved) channelParts.push(t('settings.channelGap'));
  if (channel?.lastError) channelParts.push(t('settings.inboundStatus.error', { code: channel.lastError }));
  $('set-channel-status').textContent = channelParts.join(' · ');
  for (const slot of ['bot', 'app']) {
    const box = $(`set-channel-${slot}`); box.value = '';
    box.placeholder = channel?.tokenReady ? t('settings.inboundTokenSaved') : (slot === 'bot' ? 'xoxb-…' : 'xapp-…');
  }
}

export async function renderConnectorSettings() {
  let status = null; let addresses = [];
  try { [status, addresses] = await Promise.all([inv('connector_status'), inv('connector_addresses')]); } catch (_) {}
  const select = $('set-connector-address'); select.replaceChildren();
  for (const address of addresses || []) {
    const option = document.createElement('option'); option.value = address; option.textContent = address; select.appendChild(option);
  }
  if (status?.address && !(addresses || []).includes(status.address)) {
    const option = document.createElement('option'); option.value = status.address; option.textContent = status.address; select.appendChild(option);
  }
  if (status?.address) select.value = status.address;
  $('set-connector-port').value = String(status?.port || 47631);
  $('set-connector-status').textContent = !status?.enabled ? t('connector.off')
    : status.running ? t('connector.listening', { origin: status.origin || '' }) : t('connector.notRunning');
  $('set-connector-toggle').textContent = t(status?.enabled ? 'connector.disable' : 'connector.enable');
  $('set-connector-toggle').dataset.enabled = String(status?.enabled === true);
  $('set-connector-pair').disabled = !status?.running;
  $('set-connector-reset').disabled = status?.enabled === true;
  const devices = $('set-connector-devices'); devices.replaceChildren();
  for (const device of status?.devices || []) {
    const row = document.createElement('div'); row.className = 'set-row';
    const label = document.createElement('span'); label.textContent = device.name;
    const state = document.createElement('span'); state.textContent = t(device.revoked ? 'connector.revoked' : 'connector.paired');
    row.append(label, state);
    if (!device.revoked) {
      const revoke = document.createElement('button'); revoke.className = 'btn'; revoke.textContent = t('connector.revoke');
      revoke.onclick = async () => {
        if (!await confirmDialog(t('connector.revokeConfirm', { name: device.name }))) return;
        try { await inv('connector_revoke', { deviceId: device.id }); await renderConnectorSettings(); }
        catch (_) { toast(t('connector.actionFailed')); }
      };
      row.appendChild(revoke);
    }
    devices.appendChild(row);
  }
}

export async function renderMcpSettings() {
  let status = null;
  try { status = await inv('mcp_status'); } catch (_) {}
  $('set-mcp-status').textContent = t(status?.enabled ? 'mcp.on' : 'mcp.off');
  $('set-mcp-toggle').textContent = t(status?.enabled ? 'mcp.disable' : 'mcp.enable');
  $('set-mcp-toggle').dataset.enabled = String(status?.enabled === true);
  $('set-mcp-add').disabled = !status?.enabled;
  $('set-mcp-retention').value = String(status?.outputRetentionMs || 24 * 60 * 60 * 1000);
  const clients = $('set-mcp-clients'); clients.replaceChildren();
  for (const client of status?.clients || []) {
    const row = document.createElement('div'); row.className = 'set-row mcp-client-row';
    const label = document.createElement('span');
    label.textContent = client.name;
    const scope = document.createElement('span'); scope.style.color = 'var(--muted)';
    scope.textContent = t('mcp.projectCount', { count: formatNumber(client.projects?.length || 0) })
      + (client.revoked ? ` · ${t('mcp.revoked')}` : '');
    row.append(label, scope);
    if (!client.revoked) {
      const copy = document.createElement('button'); copy.className = 'btn'; copy.textContent = t('mcp.copyConfig');
      copy.onclick = async () => {
        try {
          const command = await inv('mcp_adapter_path');
          const config = `[mcp_servers.deck]\ncommand = ${JSON.stringify(command)}\nargs = ["--client-id", ${JSON.stringify(client.id)}]\ndefault_tools_approval_mode = "writes"\n`;
          await inv('write_clipboard', { text: config }); toast(t('mcp.configCopied'));
        } catch (_) { toast(t('mcp.actionFailed')); }
      };
      const revoke = document.createElement('button'); revoke.className = 'btn'; revoke.textContent = t('mcp.revoke');
      revoke.onclick = async () => {
        if (!await confirmDangerDialog(t('mcp.revokeConfirm', { name: client.name }), t('mcp.revoke'))) return;
        try { await inv('mcp_client_revoke', { clientId: client.id }); await renderMcpSettings(); }
        catch (_) { toast(t('mcp.actionFailed')); }
      };
      row.append(copy, revoke);
    } else {
      const remove = document.createElement('button'); remove.className = 'btn set-danger'; remove.textContent = t('mcp.delete');
      remove.onclick = async () => {
        if (!await confirmDangerDialog(t('mcp.deleteConfirm', { name: client.name }), t('mcp.delete'))) return;
        try { await inv('mcp_client_delete', { clientId: client.id }); await renderMcpSettings(); }
        catch (_) { toast(t('mcp.actionFailed')); }
      };
      row.append(remove);
    }
    clients.appendChild(row);
  }
}

/* One durable write for every rule/source change; a failed save leaves the
   previous settings visible instead of a rule the poller never learned. */
export async function persistInbound(inbound) {
  if (inboundSavePending) return false;
  const previous = ctx.settings;
  const candidate = normalizeSettings({ ...ctx.settings, inbound });
  inboundSavePending = true;
  try {
    await saveSettingsCandidate(candidate);
    ctx.settings = candidate;
    inv('inbound_check_now').catch(() => {});
    renderInboundSettings();
    return true;
  } catch (_) {
    ctx.settings = previous;
    renderInboundSettings();
    toast(t('error.inboundSave'));
    uev('settings-save-fail');
    return false;
  } finally {
    inboundSavePending = false;
  }
}

export async function persistInboundSlackChoice() {
  const desired = $('set-inbound-slack').checked;
  const previous = !!ctx.settings.inbound.sources.slack.enabled;
  if (desired && !previous && !(await confirmDialog(t('settings.inboundEnableConfirm')))) {
    $('set-inbound-slack').checked = false;
    return;
  }
  const ok = await persistInbound({ ...ctx.settings.inbound, sources: { ...ctx.settings.inbound.sources, slack: { enabled: desired } } });
  if (ok) toast(t(desired ? 'settings.inboundEnabled' : 'settings.inboundDisabled'));
}

const INBOUND_TOKEN_ERRORS = { shape: 'error.inboundTokenShape', auth: 'error.inboundTokenAuth', network: 'error.inboundTokenNetwork', slack: 'error.inboundTokenSlack', keychain: 'error.inboundToken' };
async function storeInboundSecret(slot, inputId) {
  const box = $(inputId);
  const value = box.value.trim();
  if (!value) return;
  box.disabled = true;
  try {
    await inv('inbound_set_secret', { slot, value });
    toast(t('settings.inboundTokenStored'));
  } catch (e) {
    const code = String(e);
    if (code.startsWith('slack:')) toast(t('error.inboundTokenSlack', { code: code.slice(6) || '?' }));
    else toast(t(INBOUND_TOKEN_ERRORS[code] || 'error.inboundToken'));
  } finally {
    box.disabled = false;
    renderInboundSettings();
  }
}

async function clearInboundSecret(slot) {
  if (!(await confirmDialog(t('settings.inboundTokenClearConfirm')))) return;
  try {
    await inv('inbound_set_secret', { slot, value: '' });
    toast(t('settings.inboundTokenCleared'));
  } catch (_) {
    toast(t('error.inboundToken'));
  }
  renderInboundSettings();
}

async function storeChannelSecret(slot) {
  const box = $(`set-channel-${slot}`); const value = box.value.trim(); if (!value) return;
  box.disabled = true;
  try { await inv('channel_token_set', { slot, value }); toast(t('settings.inboundTokenStored')); }
  catch (error) { toast(t(INBOUND_TOKEN_ERRORS[String(error)] || 'error.inboundToken')); }
  finally { box.disabled = false; renderInboundSettings(); }
}

async function clearChannelSecret(slot) {
  if (!(await confirmDialog(t('settings.inboundTokenClearConfirm')))) return;
  try { await inv('channel_token_clear', { slot }); toast(t('settings.inboundTokenCleared')); }
  catch (_) { toast(t('error.inboundToken')); }
  renderInboundSettings();
}

/* set-check's click handler is wired by app.js (which owns update checks) —
   keeps dialogs.js from importing app.js back (no module cycle) */

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

/* MCP authorization must never inherit an invisible global project choice.
   This dialog starts with no selection and requires an explicit directory.
   A project's configured directory is only a visible, editable initial value;
   the native backend canonicalizes the submitted authorization root. */
let mcpAuthDone = null;
export function mcpAuthorizationDialog(projects, options = {}) {
  return new Promise(resolve => {
    if (mcpAuthDone) mcpAuthDone(null);
    const modal = $('mcp-auth');
    const name = $('mcp-auth-name');
    const select = $('mcp-auth-project');
    const root = $('mcp-auth-root');
    const save = $('mcp-auth-yes');
    const projectError = $('mcp-auth-project-error');
    const rootError = $('mcp-auth-root-error');
    const previewRoot = options.previewRoot || (async value => ({ ok: true, root: value, error: null }));
    const projectExists = options.projectExists || (projectId => available.some(project => project.id === projectId));
    let generation = 0;
    let pending = false;
    let closed = false;
    const available = (projects || []).filter(project => project?.id && typeof project.name === 'string');
    select.replaceChildren();
    const placeholder = document.createElement('option');
    placeholder.value = ''; placeholder.textContent = t('mcp.chooseProject');
    select.appendChild(placeholder);
    for (const project of available) {
      const option = document.createElement('option');
      option.value = project.id; option.textContent = project.name;
      select.appendChild(option);
    }
    name.value = 'ChatGPT'; select.value = ''; root.value = ''; save.disabled = false;
    const selected = () => available.find(project => project.id === select.value) || null;
    const showError = (element, field, message) => {
      element.textContent = message || '';
      element.hidden = !message;
      field.setAttribute('aria-invalid', message ? 'true' : 'false');
    };
    const done = value => {
      if (closed) return;
      closed = true; generation++;
      modal.style.display = 'none'; modal.onkeydown = null;
      name.oninput = null; root.oninput = null; select.onchange = null; mcpAuthDone = null;
      resolve(value);
    };
    mcpAuthDone = done;
    name.oninput = () => { generation++; };
    root.oninput = () => { generation++; showError(rootError, root, ''); };
    select.onchange = () => {
      generation++; showError(projectError, select, ''); showError(rootError, root, '');
      const project = selected();
      root.value = typeof project?.dir === 'string' ? project.dir.trim() : '';
    };
    save.onclick = async () => {
      if (pending || closed) return;
      const project = selected();
      const clientName = name.value.trim();
      const authorizationRoot = root.value.trim();
      if (!project) {
        showError(projectError, select, t('mcp.projectRequired')); select.focus(); return;
      }
      if (!authorizationRoot) {
        showError(rootError, root, t('mcp.rootRequired')); root.focus(); return;
      }
      if (!clientName) { name.focus(); return; }
      const requestGeneration = generation;
      pending = true; save.disabled = true;
      let preview;
      try { preview = await previewRoot(authorizationRoot); }
      catch (_) { preview = { ok: false, error: 'unavailable' }; }
      pending = false;
      if (closed || requestGeneration !== generation) { save.disabled = false; return; }
      save.disabled = false;
      if (!selected() || selected().id !== project.id || !projectExists(project.id)) {
        showError(projectError, select, t('mcp.projectUnavailable')); select.focus(); return;
      }
      if (!preview?.ok || !preview.root) {
        const key = ({ not_found: 'mcp.rootNotFound', not_directory: 'mcp.rootNotDirectory', not_accessible: 'mcp.rootNotAccessible' })[preview?.error] || 'mcp.rootUnavailable';
        showError(rootError, root, t(key)); root.focus(); return;
      }
      done({ name: clientName, project, root: preview.root });
    };
    $('mcp-auth-no').onclick = () => done(null);
    modal.onkeydown = event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); done(null); }
    };
    modal.style.display = 'flex';
    name.focus(); name.select();
  });
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initDialogs() {
  $('cfm-yes').onclick = () => cfmDone(true);

  $('set-connector-toggle').onclick = async () => {
    const enabled = $('set-connector-toggle').dataset.enabled === 'true';
    try {
      if (enabled) await inv('connector_disable');
      else await inv('connector_enable', { address: $('set-connector-address').value, port: Number($('set-connector-port').value) });
    } catch (_) { toast(t('connector.actionFailed')); }
    await renderConnectorSettings();
  };
  $('set-mcp-toggle').onclick = async () => {
    const enabled = $('set-mcp-toggle').dataset.enabled === 'true';
    if (!enabled && !(await confirmDangerDialog(t('mcp.enableConfirm'), t('mcp.enable')))) return;
    try { await inv(enabled ? 'mcp_disable' : 'mcp_enable'); }
    catch (_) { toast(t('mcp.actionFailed')); }
    await renderMcpSettings();
  };
  $('set-mcp-add').onclick = async () => {
    const authorization = await mcpAuthorizationDialog(store.projects, {
      previewRoot: root => inv('mcp_scope_preview', { root }),
      projectExists: projectId => store.projects.some(project => project.id === projectId),
    });
    if (!authorization) return;
    const { name, project } = authorization;
    const root = authorization.root;
    if (!store.projects.some(candidate => candidate.id === project.id)) { toast(t('mcp.projectUnavailable')); return; }
    if (!(await confirmDangerDialog(t('mcp.authorizeConfirm', { name: project.name, root }), t('mcp.add')))) return;
    const allowCreate = await confirmDialog(t('mcp.allowCreateConfirm'));
    try {
      await inv('mcp_client_add', { name, projects: [{ projectId: project.id, roots: [root] }], allowCreate });
      await renderMcpSettings();
    } catch (error) {
      toast(String(error).includes('MCP project no longer exists') ? t('mcp.projectUnavailable') : t('mcp.actionFailed'));
    }
  };
  $('set-mcp-retention').onchange = async () => {
    try {
      await inv('mcp_output_retention', { durationMs: Number($('set-mcp-retention').value) });
    } catch (_) { toast(t('mcp.actionFailed')); }
    await renderMcpSettings();
  };
  $('set-connector-pair').onclick = async () => {
    try {
      const pairing = await inv('connector_pairing');
      $('set-connector-qr').src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(pairing.svg)}`;
      $('set-connector-qr').alt = t('connector.qrAlt');
      $('set-connector-expiry').textContent = t('connector.expires', { seconds: formatNumber(Math.max(0, pairing.expiresAt - Math.floor(Date.now() / 1000))) });
      $('set-connector-pairing').hidden = false;
    } catch (_) { toast(t('connector.actionFailed')); }
  };
  $('set-connector-reset').onclick = async () => {
    if (!await confirmDangerDialog(t('connector.resetConfirm'), t('connector.reset'))) return;
    try { await inv('connector_reset_identity'); $('set-connector-pairing').hidden = true; }
    catch (_) { toast(t('connector.actionFailed')); }
    await renderConnectorSettings();
  };

  $('cfm-no').onclick = () => cfmDone(false);

  $('cfm').addEventListener('mousedown', e => { if (e.target === $('cfm')) cfmDone(false); });
  $('chd').addEventListener('mousedown', e => { if (e.target === $('chd') && chdResolve) chdResolve(null); });
  $('pdf').addEventListener('mousedown', e => { if (e.target === $('pdf') && pdfResolve) pdfResolve(null); });

  document.addEventListener('keydown', e => {
    if ($('cfm').style.display !== 'flex') return;
    if (e.key === 'Enter') {
      e.stopPropagation(); e.preventDefault();
      if (!confirmPointerOnly) cfmDone(true);
    }
    if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); cfmDone(false); }
  }, true);

  for (const id of SETTINGS_SECTIONS) {
    $('set-nav-' + id).onclick = () => selectSettingsSection(id);
    $('set-nav-' + id).addEventListener('keydown', event => {
      const keys = ['ArrowDown', 'ArrowUp', 'Home', 'End'];
      if (!keys.includes(event.key)) return;
      event.preventDefault();
      const visible = SETTINGS_SECTIONS.filter(section => !$('set-nav-' + section).hidden);
      const index = visible.indexOf(id);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? visible.length - 1
        : (index + (event.key === 'ArrowDown' ? 1 : -1) + visible.length) % visible.length;
      selectSettingsSection(visible[next]);
      $('set-nav-' + visible[next]).focus();
    });
  }

  $('set-search').addEventListener('input', () => {
    filterSettings();
    $('set-content').scrollTop = 0;
  });

  $('settings-box').addEventListener('keydown', event => {
    if (['cfm', 'ppd', 'chd', 'pdf', 'tmux-lifecycle-modal'].some(id => $(id).style.display === 'flex')) return;
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation();
      if ($('set-search').value) {
        $('set-search').value = '';
        filterSettings();
        $('set-search').focus();
      } else closeSettings();
    }
    if (event.key === 'Tab') {
      const controls = [...$('settings-box').querySelectorAll('button, input, select, summary, [tabindex="0"]')]
        .filter(control => !control.disabled && control.getClientRects().length);
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault(); last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault(); first.focus();
      }
    }
  });

  $('set-reset-logs').onclick = resetApplicationLogs;

  $('set-export-logs').onclick = async () => {
    if (logOperationPending) return;
    logOperationPending = true;
    const buttons = ['set-reset-logs', 'set-export-logs'].map($);
    buttons.forEach(button => { button.disabled = true; });
    try {
      await inv('export_logs');
      toast(t('settings.logsExported'));
    } catch (_) {
      toast(t('settings.logsExportFailed'));
    } finally {
      logOperationPending = false;
      buttons.forEach(button => { button.disabled = false; });
    }
  };

  registerShortcutAction('fontIncrease', () => setFontScale(ctx.settings.fontScale + FONT_SCALE_STEP));

  registerShortcutAction('fontDecrease', () => setFontScale(ctx.settings.fontScale - FONT_SCALE_STEP));

  registerShortcutAction('fontReset', () => setFontScale(1));

  onLocaleChange(() => {
    if ($('settings-modal').style.display === 'flex') {
      renderShortcutSettings();
      renderVoicePreferences();
      filterSettings();
    }
  });

  $('settings-btn').onclick = openSettings;

  $('set-close').onclick = closeSettings;

  $('settings-modal').addEventListener('mousedown', e => {
    if (e.target === $('settings-modal')) closeSettings();
  });

  $('set-editor').onchange = () => {
    ctx.settings.editor = $('set-editor').value;
    persistSettings();
    toast(ctx.settings.editor ? t('settings.editorSelected', { editor: ctx.settings.editor }) : t('settings.editorSystem'));
  };

  $('set-locale').onchange = () => {
    ctx.settings.locale = $('set-locale').value;
    setLocale(ctx.settings.locale);
    applyTranslations();
    const firstEditor = $('set-editor').options && $('set-editor').options[0];
    if (firstEditor && firstEditor.value === '') firstEditor.textContent = t('settings.systemEditor');
    inv('set_native_locale', { locale: getLocale() }).catch(() => {});
    persistSettings();
  };

  $('set-theme').onchange = () => persistThemeChoice();

  $('set-accent').onchange = () => persistThemeChoice();

  $('set-channel').onchange = () => persistUpdateChannelChoice();

  $('set-voice-default').onchange = () => persistVoicePreferences({
    ...normalizeVoicePreferences(ctx.settings.voice), defaultLanguage: $('set-voice-default').value,
  });
  $('set-voice-languages').onchange = () => persistVoicePreferences({
    ...normalizeVoicePreferences(ctx.settings.voice),
    languages: [...$('set-voice-languages').querySelectorAll('input:checked')].map(input => input.value),
  });

  $('set-session-restore').onchange = () => persistSessionRestoreChoice();

  $('set-font-down').onclick = () => setFontScale(ctx.settings.fontScale - FONT_SCALE_STEP);

  $('set-font-up').onclick = () => setFontScale(ctx.settings.fontScale + FONT_SCALE_STEP);

  $('set-font-reset').onclick = () => setFontScale(1);

  $('set-shortcuts-reset').onclick = resetShortcuts;

  $('set-agent-hooks').onchange = () =>
    persistAgentHooksChoice('claude-code', 'set-agent-hooks', 'settings.agentHooksEnableConfirm');

  $('set-codex-hooks').onchange = () =>
    persistAgentHooksChoice('codex', 'set-codex-hooks', 'settings.codexHooksEnableConfirm');

  $('set-inbound-slack').onchange = persistInboundSlackChoice;
  $('set-channel-enabled').onchange = async () => {
    const enabled = $('set-channel-enabled').checked;
    await persistInbound({ ...ctx.settings.inbound, channelConnection: { enabled, connectionId: 'default' } });
  };
  $('set-channel-setup').onclick = () => inv('channel_setup').catch(() => toast(t('error.inboundSetup')));
  $('set-channel-bot').addEventListener('change', () => storeChannelSecret('bot'));
  $('set-channel-app').addEventListener('change', () => storeChannelSecret('app'));
  $('set-channel-bot-clear').onclick = () => clearChannelSecret('bot');
  $('set-channel-app-clear').onclick = () => clearChannelSecret('app');

  $('set-inbound-setup').onclick = async () => {
    try { await inv('inbound_setup', { source: 'slack' }); }
    catch (_) { toast(t('error.inboundSetup')); }
  };

  $('set-inbound-slack-user').addEventListener('change', () => storeInboundSecret('slack-user-token', 'set-inbound-slack-user'));

  $('set-inbound-slack-app').addEventListener('change', () => storeInboundSecret('slack-app-token', 'set-inbound-slack-app'));

  $('set-inbound-slack-user-clear').onclick = () => clearInboundSecret('slack-user-token');

  $('set-inbound-slack-app-clear').onclick = () => clearInboundSecret('slack-app-token');

  $('set-inbound-check').onclick = () => {
    inv('inbound_check_now').catch(() => {});
    setTimeout(renderInboundSettings, 4000);
  };





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
}
