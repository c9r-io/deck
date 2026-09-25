// settings.js — the settings modal: sections, search, font scale, shortcuts,
// theme/locale/update channel/session restore/agent hooks, inbound (Slack
// badge + channel) connection, capability upgrade and Keychain secrets, Phone Connector and MCP settings,
// logs, the settings writer every choice goes through and the ONE commit
// shape (`commitSettings`: candidate first, rollback on failure) behind
// every optimistic choice.
// Navigation and search: the sections and the searchable settings (stable
// ids, markup `set-item-<id>`) come from settings-search-model.js, which owns
// the matching rules. A search shows the matching setting groups under their
// section headings and hides everything else; no section is `aria-current`
// while a query is active, and clearing it restores the section that was
// active. `locateSetting(id)` / `openSettings({ section, setting })` select
// a section, scroll a group into view, highlight it briefly and focus it;
// an unknown id changes nothing. Enter in the search field locates the
// first result. The modal opens without waiting for editor detection: the
// editor list renders from the last detection and refreshes when the
// background `detect_editors` answers, keeping the saved choice selected.
// Away notifications name their dependency on agent-status hooks inline
// when both are known to be off; nothing is switched on for the user.
// Voice preferences commit through the settings writer before notifying the
// recorder; edits never request microphone access or download language assets.
// Enabling Phone Connector needs an explicit danger confirmation that states
// what a paired phone can do; a pairing is announced (name, time) at once.
// Dialog primitives (confirm, danger confirm, choice, toast) come from
// dialogs.js; set-check's click handler is wired by app.js (which owns
// update checks) so this module never imports app.js back.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, inv, listen, store, uev } from './state.js';
import { applyTranslations, dictionaries, formatDateTime, formatNumber, getLocale, onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import {
  CUSTOMIZABLE_SHORTCUT_ACTIONS, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP, SHORTCUT_ACTIONS,
  normalizeSettings, parseSettings, serializeSettings,
} from './settings-model.js';
import { normalizeVoicePreferences } from './voice-preferences-model.js';
import { createVoiceSettings } from './voice-settings.js';
import { activateTheme } from './theme.js';
import { applyFontScale } from './font-scale.js';
import { newlyPairedDevice } from './connector-model.js';
import { slackConnectionView } from './slack-connection-model.js';
import { removeLegacySlackCredentials } from './slack-legacy-cleanup.js';
import { NOTIFY_STATUS_WORDS, notifyNeedsAgentStatus, notifyStatusKey } from './notify-model.js';
import { SETTINGS_SECTIONS, isSettingsSection, searchSettings, sectionItems, settingItem } from './settings-search-model.js';
import {
  formatShortcut, isSafeShortcut, registerShortcutAction, shortcutConflict, shortcutFromEvent,
} from './shortcuts.js';
import { choiceDialog, confirmDangerDialog, confirmDialog, toast } from './dialogs.js';

/* ---------- settings ---------- */
const SECTION_IDS = SETTINGS_SECTIONS.map(section => section.id);
let activeSettingsSection = 'general';

export function filterSettings() {
  const results = searchSettings($('set-search').value, dictionaries);
  const shown = new Map((results || []).map(result => [result.section, new Set(result.items)]));
  let count = 0;
  for (const id of SECTION_IDS) {
    const items = shown.get(id);
    const nav = $('set-nav-' + id);
    nav.hidden = !!results && !items;
    $('set-panel-' + id).hidden = results ? !items : id !== activeSettingsSection;
    nav.setAttribute('aria-current', !results && id === activeSettingsSection ? 'page' : 'false');
    for (const entry of sectionItems(id)) $('set-item-' + entry.id).hidden = !!results && !items?.has(entry.id);
    count += items ? items.size : 0;
  }
  $('set-no-results').hidden = !results || count > 0;
  $('set-search-status').textContent = results && count ? t('settings.searchResults', { count: formatNumber(count) }) : '';
}

export function selectSettingsSection(id) {
  if (!isSettingsSection(id)) return;
  activeSettingsSection = id;
  $('set-search').value = '';
  filterSettings();
  $('set-content').scrollTop = 0;
  if (id === 'data') refreshLogSize();
}

let locatedGroup = null;
let locatedTimer = null;
function highlightSetting(id) {
  const group = $('set-item-' + id);
  if (locatedGroup && locatedGroup !== group) locatedGroup.classList.remove('set-located');
  clearTimeout(locatedTimer);
  locatedGroup = group;
  group.classList.add('set-located');
  locatedTimer = setTimeout(() => {
    group.classList.remove('set-located');
    if (locatedGroup === group) locatedGroup = null;
  }, 1600);
  if (typeof group.scrollIntoView === 'function') group.scrollIntoView({ block: 'start' });
  group.focus({ preventScroll: true });
}

/* Select a setting's section, bring its group into view and focus it.
   Returns false (and changes nothing) for an id that is not a setting. */
export function locateSetting(id) {
  const entry = settingItem(id);
  if (!entry) return false;
  selectSettingsSection(entry.section);
  highlightSetting(entry.id);
  return true;
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

/* The ONE commit shape for a settings choice; every writer above goes through
   it. `candidate` replaces ctx.settings at once so a concurrent write of
   another field carries it; `apply` renders a settings object (called with
   the candidate now and with the previous settings if the write fails);
   `locked` controls are disabled until the write settles; `onCommit` runs
   only after the durable write. A newer write of the same `key` supersedes
   an older one's rollback; `exclusive` refuses re-entry while that key is
   pending. Resolves true when the write landed. */
const commitGenerations = new Map();
const commitPending = new Set();
async function commitSettings(choice) {
  const { key, candidate, exclusive } = choice;
  const apply = choice.apply || (() => {});
  const onCommit = choice.onCommit || (() => {});
  const locked = choice.locked || [];
  const errorKey = choice.errorKey || 'error.settingsSave';
  if (exclusive && commitPending.has(key)) return false;
  const generation = (commitGenerations.get(key) || 0) + 1;
  commitGenerations.set(key, generation);
  commitPending.add(key);
  const previous = ctx.settings;
  const controls = locked.map($);
  controls.forEach(control => { control.disabled = true; });
  ctx.settings = candidate;
  apply(candidate);
  try {
    await saveSettingsCandidate(candidate);
    await onCommit(candidate);
    return true;
  } catch (_) {
    if (generation === commitGenerations.get(key)) {
      ctx.settings = previous;
      apply(previous);
      toast(t(errorKey));
      uev('settings-save-fail');
    }
    return false;
  } finally {
    if (generation === commitGenerations.get(key)) {
      commitPending.delete(key);
      controls.forEach(control => { control.disabled = false; });
    }
  }
}
const GENERAL_CONTROLS = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'];

export async function setFontScale(value) {
  if (voiceSettings.isPending()) return;
  const bounded = Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Number(value)));
  const candidate = normalizeSettings({ ...ctx.settings, fontScale: bounded });
  if (candidate.fontScale === ctx.settings.fontScale) return;
  await commitSettings({
    key: 'font', candidate, errorKey: 'error.fontSave',
    apply: settings => { applyFontScale(settings.fontScale); renderFontScale(); },
  });
}

const commitShortcuts = candidate => commitSettings({
  key: 'shortcuts', candidate, errorKey: 'error.shortcutSave',
  apply: () => { renderShortcutSettings(); announceShortcutChange(); },
});

export async function setShortcut(actionId, binding) {
  await commitShortcuts(normalizeSettings({
    ...ctx.settings, shortcuts: { ...ctx.settings.shortcuts, [actionId]: binding },
  }));
}

export async function resetShortcuts() {
  const known = new Set(SHORTCUT_ACTIONS.map(action => action.id));
  const extensions = Object.fromEntries(Object.entries(ctx.settings.shortcuts)
    .filter(([actionId]) => !known.has(actionId)));
  await commitShortcuts(normalizeSettings({ ...ctx.settings, shortcuts: extensions }));
}

// Editor names from the last detection; null until one has answered, so a
// saved editor is never called "not found" before anything was looked for.
let detectedEditors = null;
let editorGeneration = 0;
function renderEditorOptions() {
  const sel = $('set-editor');
  sel.replaceChildren();
  const mk = (v, text) => { const o = document.createElement('option'); o.value = v; o.textContent = text; sel.appendChild(o); };
  mk('', t('settings.systemEditor'));
  const names = detectedEditors || [];
  names.forEach(name => mk(name, name));
  const saved = ctx.settings.editor;
  if (saved && !names.includes(saved)) mk(saved, detectedEditors ? t('common.notFound', { name: saved }) : saved);
  sel.value = saved || '';
}

/* Background editor detection; only the newest answer renders, and it
   re-selects ctx.settings.editor, so a choice made meanwhile is kept. */
export async function refreshEditors() {
  const generation = ++editorGeneration;
  let names;
  try { names = await inv('detect_editors'); } catch (_) { return; }
  if (generation !== editorGeneration) return;
  detectedEditors = Array.isArray(names) ? names.filter(name => typeof name === 'string') : [];
  renderEditorOptions();
}

/* `target` optionally names { section, setting } to open at; anything else
   (including a click event) opens the section that was last active. */
export async function openSettings(target) {
  const setting = settingItem(target?.setting);
  const section = setting ? setting.section
    : isSettingsSection(target?.section) ? target.section : activeSettingsSection;
  renderEditorOptions();
  refreshEditors();
  $('set-locale').value = ctx.settings.locale || 'system';
  $('set-theme').value = ctx.settings.theme || 'deck-dark';
  $('set-accent').value = ctx.settings.accent || 'teal';
  $('set-channel').value = ctx.settings.updateChannel || 'stable';
  $('set-session-restore').checked = !!ctx.settings.sessionRestore;
  renderNotifySettings();
  if (ctx.settings.notifyAway) inv('notify_status').then(renderNotifyStatus).catch(() => {});
  $('set-agent-hooks').checked = false;
  $('set-codex-hooks').checked = false;
  agentHooksKnown = null;
  renderNotifyDependency();
  const hooksGeneration = ++agentHooksGeneration;
  inv('agent_hooks_status')
    .then(status => {
      $('set-agent-hooks').checked = !!(status && status.claude);
      $('set-codex-hooks').checked = !!(status && status.codex);
      if (hooksGeneration !== agentHooksGeneration || !status || typeof status !== 'object') return;
      agentHooksKnown = { claude: status.claude === true, codex: status.codex === true };
      renderNotifyDependency();
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
  selectSettingsSection(section);
  if (section !== 'data') refreshLogSize();
  if (setting) highlightSetting(setting.id);
  else $('set-search').focus();
  if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
    window.dispatchEvent(new Event('deck-settings-opened'));
  }
}

export async function persistThemeChoice() {
  const candidate = normalizeSettings({
    ...ctx.settings,
    theme: $('set-theme').value,
    accent: $('set-accent').value,
  });
  await commitSettings({
    key: 'theme', candidate, exclusive: true, locked: GENERAL_CONTROLS, errorKey: 'error.themeSave',
    // immediate preview; a failed save shows the previous palette again
    apply: settings => {
      activateTheme(settings);
      $('set-theme').value = settings.theme;
      $('set-accent').value = settings.accent;
    },
  });
}

export async function persistUpdateChannelChoice() {
  if (commitPending.has('channel')) return;
  const previous = ctx.settings.updateChannel || 'stable';
  const desired = $('set-channel').value;
  if (desired === 'nightly' && previous !== 'nightly') {
    const accepted = await confirmDialog(t('settings.channelNightlyConfirm'));
    if (!accepted) {
      $('set-channel').value = previous;
      return;
    }
  }
  await commitSettings({
    key: 'channel', candidate: normalizeSettings({ ...ctx.settings, updateChannel: desired }),
    exclusive: true, locked: GENERAL_CONTROLS,
    apply: settings => { $('set-channel').value = settings.updateChannel || 'stable'; },
    onCommit: settings => {
      toast(t(settings.updateChannel === 'nightly'
        ? 'settings.channelNightlyEnabled' : 'settings.channelStableEnabled'));
      if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
        window.dispatchEvent(new Event('deck-update-channel-changed'));
      }
      $('set-ver').textContent = 'deck ' + ($('app-ver').textContent || 'v?');
    },
  });
}
export async function persistSessionRestoreChoice() {
  if (commitPending.has('restore')) return;
  const previous = !!ctx.settings.sessionRestore;
  const desired = $('set-session-restore').checked;
  if (desired && !previous) {
    const accepted = await confirmDialog(t('settings.shellRecoveryEnableConfirm'));
    if (!accepted) {
      $('set-session-restore').checked = false;
      return;
    }
  }
  await commitSettings({
    key: 'restore', candidate: normalizeSettings({ ...ctx.settings, sessionRestore: desired }),
    exclusive: true, locked: ['set-session-restore', 'set-clear-shell'], errorKey: 'error.restoreSave',
    apply: settings => { $('set-session-restore').checked = !!settings.sessionRestore; },
    // The privacy preference persists first. A failed disable keeps the old
    // behavior visible instead of claiming recovery is off when it is not.
    onCommit: async () => {
      if (!desired) {
        try {
          await inv('shell_snapshots_clear');
        } catch (_) {
          toast(t('settings.shellRecoveryClearFailed'));
        }
      }
      toast(t(desired ? 'settings.shellRecoveryEnabled' : 'settings.shellRecoveryDisabled'));
    },
  });
}

/* Away notifications (notify.rs): two booleans through the one settings
   writer; the backend is told after the durable write and answers with the
   closed authorization word shown under the switch. Turning the switch on
   is the one moment macOS may be asked. */
function renderNotifySettings(settings = ctx.settings) {
  $('set-notify-away').checked = !!settings.notifyAway;
  $('set-notify-sound').checked = !!settings.notifySound;
  $('set-notify-sound').disabled = !settings.notifyAway;
  if (!settings.notifyAway) $('set-notify-status').textContent = '';
}
function renderNotifyStatus(status) {
  const word = NOTIFY_STATUS_WORDS.includes(status) ? status : 'unsupported';
  $('set-notify-status').textContent = ctx.settings.notifyAway ? t(notifyStatusKey(word)) : '';
  uev('notify-status', word);
}
async function persistNotifyChoice() {
  const candidate = normalizeSettings({
    ...ctx.settings,
    notifyAway: $('set-notify-away').checked,
    notifySound: $('set-notify-away').checked && $('set-notify-sound').checked,
  });
  await commitSettings({
    key: 'notify', candidate, locked: ['set-notify-away', 'set-notify-sound'],
    apply: settings => renderNotifySettings(settings),
    onCommit: async settings => {
      const status = await inv('notify_configure', {
        enabled: settings.notifyAway, sound: settings.notifySound, request: true,
      });
      renderNotifyStatus(status);
    },
  });
}

/* Agent-status hooks: the checkbox reflects ~/.claude/settings.json itself
   (the backend derives it), so there is no second copy of the state to keep
   in sync and a manual edit of that file shows up here truthfully. */
let agentHooksPending = false;
// { claude, codex } as last read or written; null while unknown.
let agentHooksKnown = null;
let agentHooksGeneration = 0;
function renderNotifyDependency() {
  $('set-notify-dependency').hidden = !notifyNeedsAgentStatus(agentHooksKnown);
}
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
    if (agentHooksKnown) {
      agentHooksKnown = { ...agentHooksKnown, [agent === 'codex' ? 'codex' : 'claude']: desired };
      renderNotifyDependency();
    }
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

let channelUpgradeOpen = false;
const SLACK_STATE_KEYS = Object.freeze({
  ready: 'settings.slackState.ready', off: 'settings.slackState.off',
  'not-enabled': 'settings.slackState.not-enabled',
  'upgrade-required': 'settings.slackState.upgrade-required',
  'needs-app': 'settings.slackState.needs-app', 'needs-user': 'settings.slackState.needs-user',
  'needs-scopes': 'settings.slackState.needs-scopes', invalid: 'settings.slackState.invalid',
  unverified: 'settings.slackState.unverified',
  'workspace-mismatch': 'settings.slackState.workspace-mismatch',
});

/* ---------- Slack connection (inbound): the switch and the tokens ----------
   Rules are the project's automations (automation.js, the ↻ drawer); this
   is only the account-level connection. Tokens live in the Keychain and are
   never read back into the page — the backend only reports whether a slot
   is filled. */

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
  $('set-inbound-setup').hidden = present('slack-user-token') || present('slack-app-token');
  $('set-inbound-status').textContent = inboundSlackStatusText(status);
  let connection = null;
  try { connection = await inv('slack_connection_status'); } catch (_) { connection = null; }
  const view = slackConnectionView(connection, ctx.settings);
  $('set-channel-enabled').checked = !!ctx.settings.inbound.channelConnection?.enabled;
  $('set-slack-workspace').textContent = view.workspace ? t('settings.slackWorkspace', { workspace: view.workspace }) : '';
  $('set-slack-transport').textContent = t(view.socket === 'connected' ? 'settings.slackSocketConnected' : 'settings.slackSocketDisconnected');
  $('set-slack-reaction-ready').textContent = t('settings.slackReactionState', { state: t(SLACK_STATE_KEYS[view.reaction]) });
  $('set-channel-status').textContent = t('settings.slackChannelState', { state: t(SLACK_STATE_KEYS[view.channel]) });
  if (view.channel === 'ready') $('set-channel-status').textContent += ' · ' + t('settings.slackChannelRules', { count: formatNumber(view.channelRules) });
  if (view.channel === 'ready' && view.socket === 'disconnected') $('set-channel-status').textContent += ' · ' + t('settings.channelStatus.disconnected');
  try {
    const channel = await inv('channel_status');
    const facts = [];
    if (channel.pendingCount) facts.push(t('settings.channelPending', { count: formatNumber(channel.pendingCount) }));
    if (channel.rejectedCount) facts.push(t('settings.channelRejected', { count: formatNumber(channel.rejectedCount) }));
    if (channel.gapUnresolved) facts.push(t('settings.channelGap'));
    if (channel.lastError) facts.push(t('settings.inboundStatus.error', { code: channel.lastError }));
    if (facts.length) $('set-channel-status').textContent += ' · ' + facts.join(' · ');
  } catch (_) { /* status remains explicit from credential/config state */ }
  $('set-slack-legacy').hidden = !view.legacyNotice;
  $('set-slack-legacy').textContent = view.legacyNotice
    ? t(view.legacyNotice === 'retained' ? 'settings.channelLegacyRetained' : 'settings.channelLegacy') : '';
  $('set-slack-legacy-clear').hidden = !view.legacy;
  $('set-channel-bot').value = '';
  $('set-channel-bot').placeholder = connection?.botPresent ? t('settings.inboundTokenSaved') : 'xoxb-…';
  $('set-channel-bot-clear').hidden = !connection?.botPresent;
  $('set-channel-upgrade-steps').hidden = !channelUpgradeOpen && !connection?.botPresent;
  $('set-channel-upgrade-hint').textContent = t(!connection?.userPresent && !connection?.appPresent ? 'settings.channelNewSetupSteps' : 'settings.channelUpgradeSteps');
  $('set-channel-ids').textContent = [...new Set((ctx.settings.inbound.channelRules || []).flatMap(rule => rule.channelIds || []))].join(', ');

}

// Devices seen by the last settings render; a pairing is the difference.
let connectorDevices = [];
const pairedTime = seconds => formatDateTime(new Date(seconds * 1000), { dateStyle: 'medium', timeStyle: 'short' });

// A pairing emits `connector-changed`. While a QR code is shown, the new
// device is announced by name and time and the spent code is hidden, so a
// pairing by someone who saw the code cannot go unnoticed.
export async function connectorPairingChanged() {
  if ($('set-connector-pairing').hidden) return;
  const before = connectorDevices;
  let status;
  try { status = await inv('connector_status'); } catch (_) { return; }
  const device = newlyPairedDevice(before, status?.devices);
  if (!device) return;
  $('set-connector-pairing').hidden = true;
  toast(t('connector.pairedNotice', { name: device.name, time: pairedTime(device.pairedAt) }));
  await renderConnectorSettings();
}

export async function renderConnectorSettings() {
  let status = null; let addresses = [];
  try { [status, addresses] = await Promise.all([inv('connector_status'), inv('connector_addresses')]); } catch (_) {}
  connectorDevices = status?.devices || [];
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
    const state = document.createElement('span');
    state.textContent = device.revoked ? t('connector.revoked') : t('connector.pairedAt', { time: pairedTime(device.pairedAt) });
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
    const details = document.createElement('div'); details.className = 'mcp-client-details';
    const label = document.createElement('span');
    label.textContent = client.name;
    const scope = document.createElement('span'); scope.style.color = 'var(--muted)';
    scope.textContent = t('mcp.projectCount', { count: formatNumber(client.projects?.length || 0) })
      + (client.revoked ? ` · ${t('mcp.revoked')}` : '');
    details.append(label, scope);
    const actions = document.createElement('div'); actions.className = 'mcp-client-actions';
    row.append(details, actions);
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
      actions.append(copy, revoke);
    } else {
      const remove = document.createElement('button'); remove.className = 'btn set-danger'; remove.textContent = t('mcp.delete');
      remove.onclick = async () => {
        remove.disabled = true;
        let tunnel = null;
        try { tunnel = await inv('tunnel_helper_status', { clientId: client.id }); } catch (_) {}
        remove.disabled = false;
        if (tunnel?.helperState === 'installed' && tunnel.runtimeExists) {
          const choice = await choiceDialog(t('mcp.tunnelDeleteWarning'), [
            { id: 'stop', label: t('mcp.tunnelStop') },
            { id: 'remove-runtime', label: t('mcp.tunnelRemove') },
            { id: 'delete-client', label: t('mcp.tunnelDeleteAnyway'), primary: true },
          ]);
          if (choice === 'stop' || choice === 'remove-runtime') {
            const command = choice === 'stop' ? 'tunnel_helper_stop' : 'tunnel_helper_remove';
            try { await inv(command, { clientId: client.id }); await renderMcpSettings(); }
            catch (_) { toast(t('mcp.tunnelActionFailed')); }
            return;
          }
          if (choice !== 'delete-client') return;
        } else if (!await confirmDangerDialog(t('mcp.deleteConfirm', { name: client.name }), t('mcp.delete'))) return;
        try { await inv('mcp_client_delete', { clientId: client.id }); await renderMcpSettings(); }
        catch (_) { toast(t('mcp.actionFailed')); }
      };
      actions.append(remove);
    }
    clients.appendChild(row);
    renderTunnelForClient(client, row, actions);
  }
}

/* Tunnel is an optional enhancement. This async branch never delays MCP
   rendering and never participates in revoke/delete authority changes. */
async function renderTunnelForClient(client, row, actions) {
  const line = document.createElement('div'); line.className = 'mcp-tunnel-status';
  line.textContent = t('mcp.tunnelChecking');
  row.appendChild(line);
  let status;
  try { status = await inv('tunnel_helper_status', { clientId: client.id }); }
  catch (_) { status = { helperState: 'helper_error' }; }
  if (!line.isConnected) return;
  if (status.helperState !== 'installed') {
    line.textContent = t(status.helperState === 'helper_missing' ? 'mcp.tunnelOptionalMissing' : 'mcp.tunnelUnavailable');
    if (status.helperState === 'helper_missing') {
      const instructions = document.createElement('button'); instructions.className = 'btn';
      instructions.textContent = t('mcp.tunnelInstallInstructions');
      instructions.onclick = async () => {
        instructions.disabled = true;
        try {
          await inv('write_clipboard', { text: 'docs/mcp-tunnel-helper.md' });
          toast(t('mcp.tunnelInstructionsCopied'));
        } catch (_) { toast(t('mcp.tunnelActionFailed')); }
        finally { instructions.disabled = false; }
      };
      actions.append(instructions);
    }
    return;
  }
  const stateKey = `mcp.tunnelState.${status.tunnelState || 'error'}`;
  line.textContent = `${t(status.developmentHelper ? 'mcp.tunnelDevelopmentHelper' : 'mcp.tunnelInstalled')} · ${t(stateKey)}`;
  const buttons = [];
  let pending = false;
  const addAction = (label, command) => {
    const button = document.createElement('button'); button.className = 'btn'; button.textContent = label;
    button.onclick = async () => {
      if (pending) return;
      pending = true;
      for (const item of buttons) item.disabled = true;
      try { await inv(command, { clientId: client.id }); await renderMcpSettings(); }
      catch (_) { pending = false; toast(t('mcp.tunnelActionFailed')); for (const item of buttons) item.disabled = false; }
    };
    buttons.push(button); actions.append(button);
  };
  if (status.tunnelState === 'stopped') addAction(t('mcp.tunnelStart'), 'tunnel_helper_start');
  if (['starting', 'ready', 'unhealthy', 'stale'].includes(status.tunnelState)) addAction(t('mcp.tunnelStop'), 'tunnel_helper_stop');
  if (['not_configured', 'key_missing'].includes(status.tunnelState)) {
    const setup = document.createElement('button'); setup.className = 'btn'; setup.textContent = t('mcp.tunnelSetup');
    setup.onclick = async () => {
      if (pending) return;
      pending = true;
      setup.disabled = true;
      try {
        const command = await inv('tunnel_helper_setup_command', { clientId: client.id });
        await inv('write_clipboard', { text: command }); toast(t('mcp.tunnelSetupCopied'));
      } catch (_) { toast(t('mcp.tunnelActionFailed')); }
      finally { pending = false; setup.disabled = false; }
    };
    buttons.push(setup); actions.append(setup);
  }
  if (status.tunnelId) {
    const copy = document.createElement('button'); copy.className = 'btn'; copy.textContent = t('mcp.tunnelCopyId');
    copy.onclick = async () => {
      if (pending) return;
      pending = true;
      copy.disabled = true;
      try { await inv('write_clipboard', { text: status.tunnelId }); toast(t('mcp.tunnelIdCopied')); }
      catch (_) { toast(t('mcp.tunnelActionFailed')); }
      finally { pending = false; copy.disabled = false; }
    };
    buttons.push(copy); actions.append(copy);
  }
}

/* One durable write for every rule/source change; a failed save leaves the
   previous settings visible instead of a rule the poller never learned. */
export function persistInbound(inbound) {
  return commitSettings({
    key: 'inbound', candidate: normalizeSettings({ ...ctx.settings, inbound }),
    exclusive: true, errorKey: 'error.inboundSave',
    apply: () => renderInboundSettings(),
    onCommit: () => { inv('inbound_check_now').catch(() => {}); },
  });
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

const INBOUND_TOKEN_ERRORS = { scope: 'error.inboundTokenScope', workspace: 'error.inboundWorkspace', 'other-credential': 'error.inboundOtherCredential', shape: 'error.inboundTokenShape', auth: 'error.inboundTokenAuth', network: 'error.inboundTokenNetwork', slack: 'error.inboundTokenSlack', keychain: 'error.inboundToken' };
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
  if (!(await confirmDialog(t(slot === 'slack-app-token' ? 'settings.slackAppClearConfirm' : 'settings.inboundTokenClearConfirm')))) return;
  try {
    await inv('inbound_set_secret', { slot, value: '' });
    toast(t('settings.inboundTokenCleared'));
  } catch (_) {
    toast(t('error.inboundToken'));
  }
  renderInboundSettings();
}

async function storeChannelSecret() { await storeInboundSecret('slack-bot-token', 'set-channel-bot'); }
async function clearChannelSecret() { await clearInboundSecret('slack-bot-token'); }

async function clearLegacySlackCredentials() {
  await removeLegacySlackCredentials({
    confirm: () => confirmDangerDialog(t('settings.channelLegacyClearConfirm'), t('settings.channelLegacyClear')),
    invoke: inv,
    refresh: renderInboundSettings,
    notice: outcome => toast(t(outcome === 'cleared' ? 'settings.channelLegacyCleared' : 'error.channelLegacyClear')),
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
        const key = ({ not_found: 'mcp.rootNotFound', not_directory: 'mcp.rootNotDirectory', not_accessible: 'mcp.rootNotAccessible', too_broad: 'mcp.rootTooBroad' })[preview?.error] || 'mcp.rootUnavailable';
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

/* DOM wiring, run once at boot (app.js) after initDialogs() so the module
   can be imported without a document. */
export function initSettings() {
  listen('connector-changed', connectorPairingChanged).catch(() => uev('listen-fail', 'connector-changed'));

  $('set-connector-toggle').onclick = async () => {
    const enabled = $('set-connector-toggle').dataset.enabled === 'true';
    if (!enabled && !(await confirmDangerDialog(t('connector.enableConfirm'), t('connector.enable')))) return;
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

  for (const id of SECTION_IDS) {
    $('set-nav-' + id).onclick = () => selectSettingsSection(id);
    $('set-nav-' + id).addEventListener('keydown', event => {
      const keys = ['ArrowDown', 'ArrowUp', 'Home', 'End'];
      if (!keys.includes(event.key)) return;
      event.preventDefault();
      const visible = SECTION_IDS.filter(section => !$('set-nav-' + section).hidden);
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
  $('set-search').addEventListener('keydown', event => {
    if (event.key !== 'Enter' || event.isComposing || event.keyCode === 229) return;
    const first = searchSettings($('set-search').value, dictionaries)?.[0]?.items[0];
    if (!first) return;
    event.preventDefault();
    locateSetting(first);
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

  $('settings-btn').onclick = () => openSettings();

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

  $('set-notify-away').onchange = persistNotifyChoice;
  $('set-notify-sound').onchange = persistNotifyChoice;

  $('set-inbound-slack').onchange = persistInboundSlackChoice;
  $('set-channel-enabled').onchange = async () => {
    const enabled = $('set-channel-enabled').checked;
    if (enabled) {
      const status = await inv('slack_connection_status').catch(() => null);
      if (!status?.botValid || !status?.appPresent || !status?.workspaceMatch) {
        $('set-channel-enabled').checked = false;
        channelUpgradeOpen = true;
        $('set-channel-upgrade-steps').hidden = false;
        toast(t('settings.slackState.upgrade-required'));
        return;
      }
    }
    await persistInbound({ ...ctx.settings.inbound, channelConnection: { enabled, connectionId: 'default' } });
  };
  $('set-channel-upgrade').onclick = () => { channelUpgradeOpen = true; $('set-channel-upgrade-steps').hidden = false; };
  $('set-channel-finish').onclick = () => { $('set-channel-enabled').checked = true; $('set-channel-enabled').dispatchEvent(new Event('change')); };
  $('set-channel-manifest').onclick = async () => { try { $('set-channel-manifest-text').value = await inv('slack_manifest'); $('set-channel-manifest-text').hidden = false; $('set-channel-manifest-text').select(); } catch (_) { toast(t('error.inboundSetup')); } };
  $('set-channel-bot').addEventListener('change', () => storeChannelSecret());
  $('set-channel-bot-clear').onclick = () => clearChannelSecret();
  $('set-slack-legacy-clear').onclick = clearLegacySlackCredentials;

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
