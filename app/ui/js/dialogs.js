// dialogs.js — confirm/prompt dialogs, toasts, inline rename, settings modal
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, genId, inv, store, uev } from './state.js';
import { INBOUND_BADGE_RE, inlineRenameValue } from './pure.js';
import { applyTranslations, formatNumber, onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import {
  CUSTOMIZABLE_SHORTCUT_ACTIONS, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP, SHORTCUT_ACTIONS,
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
  inv('set_native_locale', { locale: ctx.settings.locale }).catch(() => {});
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
  renderInboundSettings();
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

/* ---------- 自动响应 (inbound): sources, credentials, rules ---------- */
/* Rules live in settings.json (persist-then-commit like every other setting);
   tokens live in the Keychain and are never read back into the page — the
   backend only reports whether a slot is filled. */
let inboundSavePending = false;
let inboundEditing = null;   // null | { id } (existing) | { id: null } (new)

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
  renderInboundRules();
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
}

function inboundTargetLabel(rule) {
  const p = store.projects.find(x => x.id === rule.projectId);
  const c = p && p.columns.find(x => x.id === rule.columnId);
  return p && c ? `${p.name} ▸ ${c.name}` : t('settings.inboundRuleMissingTarget');
}

function renderInboundRules() {
  const box = $('set-inbound-rules');
  box.innerHTML = '';
  const rules = ctx.settings.inbound.rules;
  if (!rules.length) {
    const empty = document.createElement('div');
    empty.className = 'set-hint';
    empty.style.margin = '2px 0 4px';
    empty.textContent = t('settings.inboundNoRules');
    box.appendChild(empty);
  }
  for (const rule of rules) {
    const row = document.createElement('div');
    row.className = 'inbound-rule';
    const main = document.createElement('div');
    main.className = 'ir-main';
    const badge = document.createElement('span');
    badge.className = 'ir-badge';
    badge.textContent = `:${rule.badge}:`;
    const sub = document.createElement('span');
    sub.className = 'ir-sub';
    sub.textContent = [inboundTargetLabel(rule), rule.cmd || t('settings.inboundRuleShellOnly'), rule.template].join(' · ');
    sub.title = rule.dir || ctx.HOME;
    main.append(badge, sub);
    const edit = document.createElement('button');
    edit.textContent = '✎';
    edit.title = t('common.rename');
    edit.onclick = () => openInboundEditor(rule);
    const del = document.createElement('button');
    del.className = 'ir-del';
    del.textContent = '✕';
    del.title = t('common.delete');
    del.onclick = async () => {
      if (!(await confirmDialog(t('settings.inboundRuleDelete', { badge: rule.badge })))) return;
      await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) });
    };
    row.append(main, edit, del);
    box.appendChild(row);
  }
  const add = document.createElement('button');
  add.className = 'btn inbound-add';
  add.textContent = t('settings.inboundAddRule');
  add.onclick = () => openInboundEditor(null);
  box.appendChild(add);
}

function fillInboundProjectSelects(projectId, columnId, template) {
  const ps = $('set-inbound-project'), cs = $('set-inbound-column'), ts = $('set-inbound-template');
  ps.innerHTML = '';
  for (const p of store.projects) {
    const o = document.createElement('option');
    o.value = p.id; o.textContent = p.name;
    ps.appendChild(o);
  }
  ps.value = store.projects.some(p => p.id === projectId) ? projectId : (store.projects[0] ? store.projects[0].id : '');
  const p = store.projects.find(x => x.id === ps.value);
  cs.innerHTML = '';
  for (const c of (p ? p.columns : [])) {
    const o = document.createElement('option');
    o.value = c.id; o.textContent = c.name;
    cs.appendChild(o);
  }
  if (p && p.columns.some(c => c.id === columnId)) cs.value = columnId;
  ts.innerHTML = '';
  const tpls = (p && p.templates) || [];
  if (!tpls.length) {
    const o = document.createElement('option');
    o.value = ''; o.textContent = t('settings.inboundNoTemplates');
    ts.appendChild(o);
  }
  for (const tp of tpls) {
    const o = document.createElement('option');
    o.value = tp.name; o.textContent = `${tp.name} · ${t('queue.steps', { count: tp.steps.length })}`;
    ts.appendChild(o);
  }
  if (tpls.some(tp => tp.name === template)) ts.value = template;
}

function openInboundEditor(rule) {
  inboundEditing = { id: rule ? rule.id : null };
  $('set-inbound-badge').value = rule ? rule.badge : '';
  $('set-inbound-dir').value = rule ? rule.dir : '';
  $('set-inbound-cmd').value = rule ? rule.cmd : 'claude';
  fillInboundProjectSelects(rule && rule.projectId, rule && rule.columnId, rule && rule.template);
  $('set-inbound-editor').style.display = '';
  $('set-inbound-badge').focus();
}

function closeInboundEditor() {
  inboundEditing = null;
  $('set-inbound-editor').style.display = 'none';
}

async function saveInboundEditor() {
  if (!inboundEditing) return;
  const badge = $('set-inbound-badge').value.trim().replace(/^:|:$/g, '');
  if (!INBOUND_BADGE_RE.test(badge)) { toast(t('settings.inboundRuleInvalidBadge')); return; }
  const projectId = $('set-inbound-project').value, columnId = $('set-inbound-column').value;
  if (!projectId || !columnId) { toast(t('settings.inboundRuleNeedsTarget')); return; }
  const template = $('set-inbound-template').value;
  if (!template) { toast(t('settings.inboundRuleNeedsTemplate')); return; }
  const cmd = $('set-inbound-cmd').value.trim();
  const dir = $('set-inbound-dir').value.trim();
  const id = inboundEditing.id || genId('R');
  if (ctx.settings.inbound.rules.some(r => r.id !== id && r.source === 'slack' && r.badge === badge)) {
    toast(t('settings.inboundRuleDuplicate', { badge }));
    return;
  }
  const rule = { id, source: 'slack', badge, projectId, columnId, cmd, template, dir };
  const rules = ctx.settings.inbound.rules.some(r => r.id === id)
    ? ctx.settings.inbound.rules.map(r => (r.id === id ? rule : r))
    : [...ctx.settings.inbound.rules, rule];
  if (await persistInbound({ ...ctx.settings.inbound, rules })) closeInboundEditor();
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

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initDialogs() {
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
    if (['cfm', 'ppd', 'tmux-lifecycle-modal'].some(id => $(id).style.display === 'flex')) return;
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
    inv('set_native_locale', { locale: ctx.settings.locale }).catch(() => {});
    persistSettings();
  };

  $('set-theme').onchange = () => persistThemeChoice();

  $('set-accent').onchange = () => persistThemeChoice();

  $('set-channel').onchange = () => persistUpdateChannelChoice();

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

  $('set-inbound-project').addEventListener('change', () =>
    fillInboundProjectSelects($('set-inbound-project').value, null, null));

  $('set-inbound-save').onclick = saveInboundEditor;

  $('set-inbound-cancel').onclick = closeInboundEditor;

  $('set-inbound-badge').addEventListener('keydown', e => { if (e.key === 'Enter' && !e.isComposing && e.keyCode !== 229) saveInboundEditor(); });

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
