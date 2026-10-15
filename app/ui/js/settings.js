// settings.js — the settings modal: sections, search, font scale, shortcuts,
// theme/locale/update channel/session restore, shared extension lifecycle,
// logs, the settings writer every choice goes through and the ONE commit
// shape (`commitSettings`: field edit over the last durable settings) behind
// every choice. Pending edits are previews only: each write rebases its own
// fields when it reaches the writer. On settlement the visible settings are
// rebuilt from the durable baseline and the remaining previews, so a failed
// edit cannot hitchhike on another save or return through an older rollback.
// Slack channel setup waits for token saves, then verifies and commits once.
// Closing/canceling invalidates an unfinished enable; switching to Slack does not.
// Draft credentials survive status refreshes and are cleared only after saving.
// After enabling, a bounded, read-only refresh follows connection establishment
// while the guide stays open; readiness is never inferred from elapsed time.
// Settings feedback is presentation only. Each setting keeps its latest
// save/apply result beside its controls; failures survive closing Settings
// until another edit of that setting or explicit dismissal. A quiet sidebar
// count leads back to them. Closing never cancels a write or requires a click.
// First-party Agent, Slack and local-translation panels are declared by the
// generated extension registry and mounted by settings-extensions.js. This
// file injects the ONE writer and shared feedback/dialog/native services;
// extension controllers own feature state, markup, actions and refresh/close hooks.
// Connector/MCP guards and authorization remain native owners regardless of UI.
// Compatibility exports below only delegate to those registered controllers.
// Explicit channel first-step decisions travel only with the editor's save.
// Native canonical channel grants replace frontend copies after commit;
// unrelated queued saves preserve that latest native result. A native
// template retirement emits a content-free refresh even if Board save fails.
// A rule save may carry `proceed`, asked once when the writer reaches it and
// before anything is sent: a save whose edit was replaced or given up while
// it waited in the queue is withdrawn there (nothing written, no permission
// requested, no error shown). Once `proceed` answered yes the write is under
// way and is not taken back.
// WHAT THE FILE DOES NOT HOLD IS NOBODY ELSE'S BASELINE. A rule save keeps
// its rules to itself until the native save answered that they were written:
// they enter ctx.settings only then, so no other save can copy a rule save
// that is waiting, was given up, or was refused. A save of another setting
// writes its own change over the automations last known to be in the file
// (`committedInbound`), never over the copy it was built with: it carries
// no rule save that did not land, loses none that landed while it waited,
// and asks for no permission. One that fails drops only its own preview,
// retaining that baseline's automations, so a rule save that landed
// while it waited stays in memory and in the next rule save, and a choice
// or permission that save withdrew stays withdrawn. That baseline is told by the launch's
// `loadSettings` (the file as loaded; a backup as native hands it over,
// without channel choices; a first run's none), by `refreshChannelAuthority`
// and by every save that answered; a load that failed leaves it unknown and
// writes nothing. It never comes from ctx.settings and is never authority:
// native decides what a save may keep.
// The Lens may explicitly download and enable through installTranslationPack;
// it shares the same settings writer and installation guard as Settings.
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
// Pi work status is one more switch of the agent-status group. It shows the
// `pi` entry of `agent_hooks_status.agents`, which the backend derives from Deck's one
// entry in Pi's own settings file, and is written by `agent_hooks_set` like
// the other two. What differs is said by pi-model.js: the note under the
// switch names the entry's state (configured is never shown as connected),
// a build that may not write Pi's settings cannot use the switch, and a
// refusal has its own sentence for each closed code.
// Pi session observation is a second, separate switch on the same entry
// (`pi_lifecycle`, written by `agent_pi_lifecycle_set`): it turns on no work
// status, and nothing it observes lets Deck reopen a Pi session.
// Claude session re-entry observation is its own explicit opt-in: the box
// shows `claude_reentry` of `agent_hooks_status` (the backend derives it from
// the installed lifecycle hooks; there is no settings key), is written only
// by `agent_lifecycle_hooks_set`, and neither follows nor moves the Claude
// Code agent-status switch.
// Voice preferences commit through the settings writer before notifying the
// recorder; edits never request microphone access or download language assets.
// Enabling Phone Connector needs an explicit danger confirmation that states
// what a paired phone can do; a pairing is announced (name, time) at once.
// Dialog primitives (confirm, danger confirm, choice, toast) come from
// dialogs.js; set-check's click handler is wired by app.js (which owns
// update checks) so this module never imports app.js back.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, emit, inv, listen, store, uev } from './state.js';
import { applyTranslations, dictionaries, formatDateTime, formatNumber, getLocale, onLocaleChange, resolveLocale, setLocale, t, translateNotice } from './i18n.js';
import {
  CUSTOMIZABLE_SHORTCUT_ACTIONS, FONT_SCALE_MAX, FONT_SCALE_MIN, FONT_SCALE_STEP, SHORTCUT_ACTIONS,
  normalizeSettings, parseSettings, serializeSettings,
} from './settings-model.js';
import { activateTheme } from './theme.js';
import { applyFontScale } from './font-scale.js';
import { NOTIFY_STATUS_WORDS, notifyNeedsAgentStatus, notifyStatusKey } from './notify-model.js';
import { SETTINGS_SECTIONS, isSettingsSection, searchSettings, sectionItems, settingItem } from './settings-search-model.js';
import {
  initializeExtensionContributions, persistAgentContribution, refreshAgentContributions,
  renderAgentContributions, configureExtensionContributions, extensionController, runExtensionLifecycle, extensionControlsBusy,
  extensionFeedbackFields,
} from './settings-extensions.js';
import {
  formatShortcut, isSafeShortcut, registerShortcutAction, shortcutConflict, shortcutFromEvent,
} from './shortcuts.js';
import { choiceDialog, confirmDangerDialog, confirmDialog, toast } from './dialogs.js';

/* ---------- settings ---------- */
const SECTION_IDS = SETTINGS_SECTIONS.map(section => section.id);
let activeSettingsSection = 'general';

// Only controls in Settings get Settings feedback. Rule saves already have
// their own editor/result path and do not become an account-level notice.
const FEEDBACK_FIELDS = {
  locale: ['language', 'settings.language'], editor: ['editor', 'settings.openFiles'],
  theme: ['appearance', 'settings.theme'], font: ['appearance', 'settings.fontSize'],
  channel: ['updates', 'settings.updateChannel'], restore: ['session-restore', 'settings.shellRecovery'],
  notify: ['away-notifications', 'settings.group.awayNotifications'],
  ...extensionFeedbackFields(),
  'shortcuts-reset': ['shortcuts', 'settings.shortcuts'],
};
const settingsFeedback = new Map();
const feedbackRows = new Map();
const feedbackFailed = entry => ['failed', 'apply-failed'].includes(entry.phase);
function beginSettingsFeedback(key, field = FEEDBACK_FIELDS[key]) {
  if (!field && key.startsWith('shortcut:')) field = ['shortcuts', `settings.shortcut.${key.slice(9)}`];
  if (!field) return null;
  const entry = { key, item: field[0], label: field[1], phase: 'saving', error: null };
  settingsFeedback.set(key, entry);
  renderSettingsFeedback();
  return entry;
}
function finishSettingsFeedback(entry, phase, error = null) {
  if (!entry || settingsFeedback.get(entry.key) !== entry) return;
  entry.phase = phase; entry.error = error;
  renderSettingsFeedback();
}
function renderSettingsFeedback() {
  for (const [key, row] of feedbackRows) row.el.hidden = !settingsFeedback.has(key);
  for (const entry of settingsFeedback.values()) {
    let row = feedbackRows.get(entry.key);
    if (!row) {
      const el = document.createElement('div'); el.className = 'set-save-feedback'; el.dataset.settingKey = entry.key;
      el.setAttribute('role', 'status'); el.setAttribute('aria-live', 'polite'); el.setAttribute('aria-atomic', 'true');
      const label = document.createElement('span'), status = document.createElement('span');
      const dismiss = document.createElement('button'); dismiss.type = 'button'; dismiss.className = 'btn';
      dismiss.onclick = () => {
        settingsFeedback.delete(entry.key); renderSettingsFeedback();
        $('set-item-' + entry.item).focus({ preventScroll: true });
      };
      el.append(label, status, dismiss);
      $('set-item-' + entry.item).appendChild(el);
      row = { el, label, status, dismiss }; feedbackRows.set(entry.key, row);
    }
    row.el.hidden = false; row.el.dataset.phase = entry.phase;
    row.label.textContent = t(entry.label);
    row.status.textContent = t(entry.error || ({ saving: 'settings.saving', applying: 'settings.applying',
      saved: 'settings.saved', 'apply-failed': 'error.settingApply' })[entry.phase]);
    row.dismiss.hidden = !feedbackFailed(entry);
    row.dismiss.textContent = t('settings.dismissFeedback');
    row.dismiss.setAttribute('aria-label', t('settings.dismissNamed', { name: t(entry.label) }));
  }
  const entries = [...settingsFeedback.values()];
  const failed = entries.filter(feedbackFailed);
  const pending = entries.filter(entry => ['saving', 'applying'].includes(entry.phase));
  $('set-save-summary').textContent = [
    pending.length ? t('settings.pendingChanges', { count: formatNumber(pending.length) }) : '',
    failed.length ? t('settings.failedChanges', { count: formatNumber(failed.length) }) : '',
  ].filter(Boolean).join(' · ') || (entries.length ? t('settings.saved') : '');
  $('set-save-issues').hidden = !failed.length;
  $('set-save-issues').onclick = () => { if (failed[0]) locateSetting(failed[0].item); };
  const badge = $('settings-issue-count'); badge.hidden = !failed.length;
  badge.textContent = failed.length ? formatNumber(failed.length) : '';
  badge.setAttribute('aria-label', t('settings.failedChanges', { count: formatNumber(failed.length) }));
  badge.title = t('settings.failedChanges', { count: formatNumber(failed.length) });
}

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
  runExtensionLifecycle('close');
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
    /* what was loaded is what the writer starts from; a first run holds none */
    fileHolds(doc && doc.data ? ctx.settings.inbound : normalizeSettings({}).inbound);
  } catch (e) {
    fileHolds(null);                  // unknown, not empty
    toast(t('error.settingsLoad'));   // NOT a first run — defaults stay in memory only
    uev('settings-load-fail');
  }
  setLocale(ctx.settings.locale);
  activateTheme(ctx.settings);
  applyFontScale(ctx.settings.fontScale);
  announceShortcutChange();
  runExtensionLifecycle('loaded');
  inv('set_native_locale', { locale: getLocale() }).catch(() => {});
}

let settingsWriteChain = Promise.resolve();
let nativeChannelSettings = null;
let committedInbound = null;   // the automations of the last settings read from or written to the file
/* The automations the settings file is known to hold: what saves of other
   settings write, and the channel rules a rule save is answered with. */
function fileHolds(inbound) {
  committedInbound = inbound && structuredClone(inbound);
  nativeChannelSettings = inbound && structuredClone({ channelRules: inbound.channelRules,
    channelConnection: inbound.channelConnection });
}
export function refreshChannelAuthority() {
  const operation = settingsWriteChain.catch(() => {}).then(async () => {
    const loaded = await inv('load_settings');
    if (!loaded?.data) return;
    const canonical = parseSettings(loaded.data);
    fileHolds(canonical.inbound);
    if (committedSettings) committedSettings.inbound = structuredClone(canonical.inbound);
    Object.assign(ctx.settings.inbound, structuredClone(nativeChannelSettings));
    emit('channel-authority');
  });
  settingsWriteChain = operation;
  return operation;
}
const WITHDRAWN = Object.freeze({ withdrawn: true });   // what a write its owner gave up is rejected with
async function writeSettingsCandidate(candidate, { channelFirstSendRequests = [], channelIntent = false } = {}) {
  // Ordinary edits cannot carry a pending rule or replace native grants.
  if (!channelIntent && committedInbound) candidate.inbound = structuredClone(committedInbound);
  if (!channelIntent && nativeChannelSettings) Object.assign(candidate.inbound, structuredClone(nativeChannelSettings));
  const saved = await inv('save_settings', { data: serializeSettings(candidate),
    ...(channelFirstSendRequests.length ? { channelFirstSendRequests } : {}) });
  if (typeof saved === 'string') {
    const canonical = normalizeSettings(JSON.parse(saved));
    fileHolds(canonical.inbound);
    return canonical;
  }
  fileHolds(candidate.inbound);
  return candidate;
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

/* Each choice edits only the fields it owns. The edit is replayable and
   contains captured values, never reads controls or ctx.settings. Writes run
   over the last successful document; previews run over that document plus
   every still-pending optimistic edit in order. No pending choice is another
   write's baseline. `proceed` choices (rules) and preview:false choices (voice)
   remain invisible until durable. Native authority retains its separate file
   baseline above. Post-commit effects share the writer's ordering, but their
   failure cannot roll back a document that was already written. */
let committedSettings = null;
const pendingChoices = new Set();
const commitPending = new Set();
const controlLocks = new Map();
function projectSettings() {
  const visible = structuredClone(committedSettings);
  for (const choice of pendingChoices) if (!choice.held) choice.edit(visible);
  ctx.settings = normalizeSettings(visible);
}
async function commitSettings(choice) {
  const { key, exclusive } = choice;
  if (exclusive && commitPending.has(key)) return false;
  const apply = choice.apply || (() => {});
  const onCommit = choice.onCommit || (() => {});
  const errorKey = choice.errorKey || 'error.settingSave';
  const feedback = beginSettingsFeedback(key);
  // At rest ctx.settings is the settled projection (also the boot defaults
  // on first run). This never seeds the separate native authority baseline.
  if (!pendingChoices.size) committedSettings = normalizeSettings(structuredClone(ctx.settings));
  const pending = { ...choice, held: !!choice.proceed || choice.preview === false };
  pendingChoices.add(pending);
  commitPending.add(key);
  const controls = (choice.locked || []).map($);
  for (const control of controls) {
    controlLocks.set(control, (controlLocks.get(control) || 0) + 1);
    control.disabled = true;
  }
  if (!pending.held) { projectSettings(); apply(ctx.settings); }
  for (const control of controlLocks.keys()) control.disabled = true;
  const operation = settingsWriteChain.catch(() => {}).then(async () => {
    let saved = null, failure = null;
    try {
      if (choice.proceed && !choice.proceed()) throw WITHDRAWN;
      const candidate = structuredClone(committedSettings);
      choice.edit(candidate);
      saved = await writeSettingsCandidate(normalizeSettings(candidate), { channelIntent: key === 'inbound',
        channelFirstSendRequests: choice.channelFirstSendRequests || [] });
      committedSettings = saved;
    } catch (error) { failure = error; }
    pendingChoices.delete(pending);
    projectSettings();
    apply(ctx.settings);
    for (const control of controlLocks.keys()) control.disabled = true;
    try {
      if (saved) {
        // Effects receive the document this choice actually committed, even
        // while ctx.settings shows a newer pending preview of the same field.
        if (choice.onCommit) finishSettingsFeedback(feedback, 'applying');
        await onCommit(saved);
        finishSettingsFeedback(feedback, 'saved');
      } else if (failure !== WITHDRAWN && ![...pendingChoices].some(next => next.key === key)) {
        finishSettingsFeedback(feedback, 'failed', errorKey);
        toast(t(errorKey)); uev('settings-save-fail');
      }
    } catch (_) {
      finishSettingsFeedback(feedback, 'apply-failed');
      toast(t('error.settingApply')); uev('settings-save-fail');
    } finally {
      if (![...pendingChoices].some(next => next.key === key)) commitPending.delete(key);
      for (const control of controls) {
        const count = controlLocks.get(control) - 1;
        if (count) controlLocks.set(control, count);
        else { controlLocks.delete(control); control.disabled = false; }
      }
      // Released controls can have semantic disabled states (notifications
      // off). Choices with no locks need no second rendering or status read.
      if (controls.length) apply(ctx.settings);
      for (const control of controlLocks.keys()) control.disabled = true;
    }
    return !!saved;
  });
  settingsWriteChain = operation;
  return operation;
}
const GENERAL_CONTROLS = ['set-theme', 'set-accent', 'set-channel', 'set-locale', 'set-editor', 'set-session-restore'];

export async function setFontScale(value) {
  if (extensionControlsBusy()) return;
  const bounded = Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, Number(value)));
  const candidate = normalizeSettings({ ...ctx.settings, fontScale: bounded });
  if (candidate.fontScale === ctx.settings.fontScale) return;
  await commitSettings({
    key: 'font', edit: settings => { settings.fontScale = candidate.fontScale; }, errorKey: 'error.fontSave',
    apply: settings => { applyFontScale(settings.fontScale); renderFontScale(); },
  });
}

const commitShortcuts = (key, edit) => commitSettings({
  key, edit, errorKey: 'error.shortcutSave',
  apply: () => { renderShortcutSettings(); announceShortcutChange(); },
});

export async function setShortcut(actionId, binding) {
  const candidate = normalizeSettings({
    ...ctx.settings, shortcuts: { ...ctx.settings.shortcuts, [actionId]: binding },
  });
  await commitShortcuts(`shortcut:${actionId}`, settings => { settings.shortcuts[actionId] = candidate.shortcuts[actionId]; });
}

export async function resetShortcuts() {
  const known = new Set(SHORTCUT_ACTIONS.map(action => action.id));
  await commitShortcuts('shortcuts-reset', settings => {
    settings.shortcuts = Object.fromEntries(Object.entries(settings.shortcuts)
      .filter(([actionId]) => !known.has(actionId)));
    settings.shortcuts = normalizeSettings(settings).shortcuts;
  });
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
  for (const [key, entry] of settingsFeedback) if (entry.phase === 'saved') settingsFeedback.delete(key);
  renderSettingsFeedback();
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
  refreshAgentContributions();
  renderFontScale();
  renderShortcutSettings();
  runExtensionLifecycle('refresh');
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
    key: 'theme', edit: settings => { settings.theme = candidate.theme; settings.accent = candidate.accent; },
    exclusive: true, locked: GENERAL_CONTROLS, errorKey: 'error.themeSave',
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
    key: 'channel', edit: settings => { settings.updateChannel = desired; },
    exclusive: true, locked: GENERAL_CONTROLS, errorKey: 'error.settingsSave',
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
    key: 'restore', edit: settings => { settings.sessionRestore = desired; },
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
    key: 'notify', edit: settings => { settings.notifyAway = candidate.notifyAway; settings.notifySound = candidate.notifySound; },
    locked: ['set-notify-away', 'set-notify-sound'],
    apply: settings => renderNotifySettings(settings),
    onCommit: async settings => {
      const status = await inv('notify_configure', {
        enabled: settings.notifyAway, sound: settings.notifySound, request: true,
      });
      renderNotifyStatus(status);
    },
  });
}

// Compatibility exports for callers and focused tests. The contribution
// registry owns the controls and the generic host owns their state machine.
export const persistAgentHooksChoice = (_agent, boxId) => persistAgentContribution(boxId);
export const persistPiChoice = () => persistAgentContribution('set-pi-hooks');
export const persistPiLifecycle = () => persistAgentContribution('set-pi-lifecycle');
export const persistClaudeReentryChoice = () => persistAgentContribution('set-claude-reentry');

function renderNotifyDependency(agentSignals) {
  const known = agentSignals && typeof agentSignals['claude-code'] === 'boolean'
    && typeof agentSignals.codex === 'boolean'
    ? { claude: agentSignals['claude-code'], codex: agentSignals.codex } : null;
  $('set-notify-dependency').hidden = !notifyNeedsAgentStatus(known);
}

/* One durable write for every rule/source change; a failed save leaves the
   previous settings visible instead of a rule the poller never learned. */
export function persistInbound(inbound, channelFirstSendRequests = [], { proceed = null } = {}) {
  const value = structuredClone(inbound);
  return commitSettings({
    proceed,
    key: 'inbound', edit: settings => { settings.inbound = structuredClone(value); },
    exclusive: true, errorKey: 'error.inboundSave',
    channelFirstSendRequests,
    apply: () => runExtensionLifecycle('inboundChanged'),
    onCommit: () => { inv('inbound_check_now').catch(() => {}); },
  });
}

/* DOM wiring, run once at boot (app.js) after initDialogs() so the module
   can be imported without a document. */
export function initSettings() {
  initializeExtensionContributions();
  listen('channel-authority-changed', () => refreshChannelAuthority().catch(() => uev('settings-load-fail')))
    .catch(() => uev('settings-load-fail'));

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

  renderSettingsFeedback();
  onLocaleChange(() => {
    renderSettingsFeedback();
    if ($('settings-modal').style.display === 'flex') {
      renderShortcutSettings();
      runExtensionLifecycle('localeChanged');
      renderAgentContributions();
      filterSettings();
    }
  });

  $('settings-btn').onclick = () => {
    const failed = [...settingsFeedback.values()].find(feedbackFailed);
    return openSettings(failed ? { setting: failed.item } : undefined);
  };
  $('set-close').onclick = closeSettings;

  $('settings-modal').addEventListener('mousedown', e => {
    if (e.target === $('settings-modal')) closeSettings();
  });

  $('set-editor').onchange = () => {
    const editor = $('set-editor').value;
    return commitSettings({ key: 'editor', edit: settings => { settings.editor = editor; },
      apply: renderEditorOptions,
      onCommit: settings => toast(settings.editor ? t('settings.editorSelected', { editor: settings.editor }) : t('settings.editorSystem')),
    });
  };

  $('set-locale').onchange = () => {
    const locale = $('set-locale').value;
    return commitSettings({ key: 'locale', edit: settings => { settings.locale = locale; },
      apply: settings => {
        $('set-locale').value = settings.locale;
        setLocale(settings.locale);
        applyTranslations();
        const firstEditor = $('set-editor').options && $('set-editor').options[0];
        if (firstEditor && firstEditor.value === '') firstEditor.textContent = t('settings.systemEditor');
      },
      onCommit: settings => inv('set_native_locale', { locale: resolveLocale(settings.locale) }),
    });
  };

  $('set-theme').onchange = () => persistThemeChoice();

  $('set-accent').onchange = () => persistThemeChoice();

  $('set-channel').onchange = () => persistUpdateChannelChoice();

  $('set-session-restore').onchange = () => persistSessionRestoreChoice();

  $('set-font-down').onclick = () => setFontScale(ctx.settings.fontScale - FONT_SCALE_STEP);

  $('set-font-up').onclick = () => setFontScale(ctx.settings.fontScale + FONT_SCALE_STEP);

  $('set-font-reset').onclick = () => setFontScale(1);

  $('set-shortcuts-reset').onclick = resetShortcuts;

  $('set-notify-away').onchange = persistNotifyChoice;
  $('set-notify-sound').onchange = persistNotifyChoice;

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

// Existing callers delegate to the registered controller; no second state owner.
export const installTranslationPack = (...args) => extensionController('bergamot').installTranslationPack(...args);
export const renderInboundSettings = (...args) => extensionController('slack').renderInboundSettings(...args);
export const persistInboundSlackChoice = (...args) => extensionController('slack').persistInboundSlackChoice(...args);
export const renderConnectorSettings = (...args) => extensionController('connector').renderConnectorSettings(...args);
export const connectorPairingChanged = (...args) => extensionController('connector').connectorPairingChanged(...args);
export const renderMcpSettings = (...args) => extensionController('mcp').renderMcpSettings(...args);
export const mcpAuthorizationDialog = (...args) => extensionController('mcp').mcpAuthorizationDialog(...args);
export const renderVoicePreferences = (...args) => extensionController('voice').renderVoicePreferences(...args);
export const persistVoicePreferences = (...args) => extensionController('voice').persistVoicePreferences(...args);

configureExtensionContributions({
  $, ctx, inv, listen, store, t, toast, confirmDialog, confirmDangerDialog, choiceDialog, uev,
  formatNumber, formatDateTime, commitSettings, persistInbound,
  drainWrites: () => settingsWriteChain.catch(() => {}),
  beginFeedback: beginSettingsFeedback, finishFeedback: finishSettingsFeedback, renderNotifyDependency,
});
