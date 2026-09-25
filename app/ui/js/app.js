// app.js — in-app updates and boot
// Part of deck's no-build frontend: native ES modules, no bundler.
import './persistence.js';
import './board.js';
import { $, ctx, genId, initInputDiagnostics, inv, listen, state, store, uev } from './state.js';
import { initDialogs, toast } from './dialogs.js';
import { initSettings, loadSettings } from './settings.js';
import {
  activeProject, closeBuffer, initBuffer, panes, markSessionsStoppedForServerRestart, migrateColumnSemantics, newSessionSummary, openProjectDefaults, pollNow,
  projectDefaultsSummary, prepareCardsForServerRestart, provider, render, startPolling, stopPolling, switchProject,
} from './board.js';
import { closePaneBySid, initLayout, leaveSessionView, openSession } from './layout.js';
import { initTerminalChrome, newDefaultSession } from './terminal.js';
import { initScheduler, refreshQueue } from './scheduler.js';
import { initTemplates } from './templates.js';
import { drainChannel, drainInbound, initInbound } from './inbound.js';
import { drainConnector, initConnector } from './connector.js';
import { initMcp } from './mcp.js';
import { initAutomation } from './automation.js';
import { initDropdowns } from './dropdown.js';
import { initAttention, openFromNotification } from './attention.js';
import { onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import { activateTheme, revealThemedWindow } from './theme.js';
import { initVoice } from './voice.js';
import { createVoiceTarget } from './voice-target.js';
import { initInputSource } from './input-source.js';
import { cancelTerminalSelection } from './selection.js';
import { closeManagedForRestart, managedBlockers } from './restart-managed.js';

setLocale('system');
activateTheme({ theme: 'deck-dark', accent: 'teal' });

// A held trackpad click can become Force Touch and open macOS Look Up on
// button text, even with user-select:none. Cancel WebKit's native action at
// its preflight event; ordinary pointer/mouse/click events retain their defaults.
// Delegation also covers nested labels/icons and buttons created after boot.

// Deck owns context menus throughout its app surface; WebKit's Reload/Inspect
// and text-search menus are browser chrome. Keep native editing menus in real
// form fields, but not xterm's hidden input. Never stop propagation: Deck's
// card/project handlers must still receive the event and open their own menu.

function renderPendingUpdate() {
  if (!ctx.pendingUpdate) return;
  const btn = $('update-btn');
  btn.querySelector('.label').textContent = t('update.available', { version: ctx.pendingUpdate.version });
  btn.title = t('update.availableTitle', { version: ctx.pendingUpdate.version });
}

function channelLabel(channel = ctx.settings.updateChannel) {
  return t(channel === 'nightly' ? 'settings.channel.nightly' : 'settings.channel.stable');
}

export function renderBuildIdentity(identity = ctx.buildIdentity) {
  const version = identity.version || '?';
  const commit = identity.commit || 'dev';
  $('app-ver').textContent = `v${version} · ${channelLabel()} · ${commit}`;
  if ($('settings-modal').style.display === 'flex') $('set-ver').textContent = `deck ${$('app-ver').textContent}`;
}

/* ---------- upgrade-aware tmux server lifecycle ---------- */
function tmuxStateText(status) {
  if (!status) return t('tmux.state.unavailable');
  if (status.status === 'CompatibleCurrentBuild' || status.status === 'CompatibleDifferentBuild') {
    return t('tmux.state.current');
  }
  if (status.status === 'LegacyUnknown') return t('tmux.state.legacy');
  if (status.status === 'SourceUnstable') return t('tmux.state.sourceUnstable');
  if (status.status === 'CorruptOrUnreachable') return t('tmux.state.unavailable');
  return t('tmux.state.restartRequired');
}

function tmuxBuildText(build) {
  if (!build) return t('tmux.buildUnknown');
  return `${build.appVersion || '?'} · ${build.buildIdentifier || '?'} · ${build.source || '?'}`;
}

function renderTmuxDiagnostics(status = ctx.tmuxServerStatus) {
  if (!status) return;
  const pending = !!status.pendingRestart;
  const side = $('tmux-restart-btn');
  side.style.display = pending ? 'flex' : 'none';
  side.disabled = !!ctx.tmuxRestarting;
  side.title = t('tmux.pendingTitle', { count: status.sessionCount || 0 });
  $('board-new').disabled = pending || ctx.tmuxRestarting;

  $('set-tmux-state').textContent = tmuxStateText(status);
  $('set-tmux-restart').disabled = !status.canRestart || ctx.tmuxRestarting;
  const current = tmuxBuildText(status.currentBuild);
  const server = status.serverBuild ? tmuxBuildText(status.serverBuild) : t('tmux.buildUnknown');
  const pid = status.serverPid == null ? '—' : String(status.serverPid);
  const started = status.serverStartedAt
    ? new Date(status.serverStartedAt * 1000).toLocaleString() : '—';
  $('set-tmux-details').textContent = t('tmux.diagnostics', {
    current, server, pid, started,
  }) + (managedBlockers(status).length ? '\n' + t('tmux.managedStatus', { count: managedBlockers(status).length }) : '');
}

function renderImpactList(status) {
  const list = $('tmux-impact-list');
  list.replaceChildren();
  for (const session of status.sessions || []) {
    const row = document.createElement('div');
    row.className = 'tmux-impact-row';
    const name = document.createElement('div');
    name.textContent = session.name;
    const meta = document.createElement('div');
    meta.className = 'tmux-impact-meta';
    const parts = [t(session.attachedClients > 0 ? 'tmux.session.attached' : 'tmux.session.detached')];
    parts.push(t('tmux.session.panes', { count: session.paneCount }));
    if (session.hasForegroundProcess) parts.push(t('tmux.session.foreground'));
    if (session.recentlyActive) parts.push(t('tmux.session.recent'));
    meta.textContent = parts.join(' · ');
    row.append(name, meta);
    list.appendChild(row);
  }
  for (const blocker of managedBlockers(status)) {
    const row = document.createElement('div');
    row.className = 'tmux-impact-row';
    const card = provider.get(blocker.cardId);
    row.textContent = t('tmux.managedCard', {
      card: card ? `${card.title} (${blocker.cardId})` : blocker.cardId,
      session: blocker.session,
    });
    list.appendChild(row);
  }
  list.style.display = managedBlockers(status).length ? 'block' : 'none';
  $('tmux-view-sessions').style.display = status.sessionCount > 0 ? '' : 'none';
  $('tmux-view-sessions').textContent = t('tmux.viewSessions');
}

function showTmuxLifecycle(status, manual = false) {
  if (!status) return;
  ctx.tmuxServerStatus = status;
  const count = status.sessionCount || 0;
  $('tmux-lifecycle-title').textContent = t(manual && !status.pendingRestart
    ? 'tmux.manualTitle' : 'tmux.title');
  $('tmux-lifecycle-message').textContent = t(manual && !status.pendingRestart
    ? 'tmux.manualMessage' : 'tmux.upgradeMessage', {
      count,
      attached: status.attachedSessionCount || 0,
      active: status.foregroundSessionCount || 0,
    });
  if (!ctx.settings.sessionRestore) $('tmux-lifecycle-message').textContent += '\n' + t('tmux.restoreOff');
  if (managedBlockers(status).length) $('tmux-lifecycle-message').textContent += '\n' + t('tmux.managedExplanation');
  $('tmux-restart').textContent = t(managedBlockers(status).length ? 'tmux.closeManagedRestart' : 'tmux.restart');
  renderImpactList(status);
  $('tmux-lifecycle-modal').dataset.manual = manual && !status.pendingRestart ? 'true' : 'false';
  $('tmux-lifecycle-modal').style.display = 'flex';
  // The destructive action is never the default focus and Enter has no
  // acceptance path for this modal.
  $('tmux-later').focus();
}

async function refreshTmuxLifecycle({ prompt = false } = {}) {
  if (!window.__TAURI__) return null;
  try {
    ctx.tmuxServerStatus = await inv('tmux_server_status');
    renderTmuxDiagnostics();
    if (ctx.tmuxServerStatus.notice) {
      toast(t(`tmux.notice.${ctx.tmuxServerStatus.notice}`));
      inv('acknowledge_tmux_lifecycle_notice').catch(() => {});
      ctx.tmuxServerStatus.notice = null;
    }
    if (prompt && ctx.tmuxServerStatus.shouldPrompt) showTmuxLifecycle(ctx.tmuxServerStatus, false);
    return ctx.tmuxServerStatus;
  } catch (_) {
    return null;
  }
}

async function deferTmuxRestart() {
  if (ctx.tmuxRestarting) return;
  $('tmux-lifecycle-modal').style.display = 'none';
  if (!ctx.tmuxServerStatus?.pendingRestart) return;
  try {
    ctx.tmuxServerStatus = await inv('defer_tmux_restart');
    renderTmuxDiagnostics();
  } catch (_) {
    toast(t('tmux.deferFailed'));
  }
}

async function restartTmuxServer() {
  const review = ctx.tmuxServerStatus;
  if (!review || ctx.tmuxRestarting) return;
  ctx.tmuxRestarting = true;
  renderTmuxDiagnostics(review);
  $('tmux-restart').disabled = true;
  $('tmux-later').disabled = true;
  $('tmux-view-sessions').disabled = true;
  $('tmux-restart').textContent = t('tmux.restarting');
  stopPolling();
  let detachedSessions = [];
  let replacementStarted = false;
  let invokedStatus = null;
  const requestId = genId('R');
  let unlisten = null;
  const began = performance.now();
  try {
    const status = managedBlockers(review).length
      ? await closeManagedForRestart(review, {
        readStatus: () => inv('tmux_server_status'),
        getCard: id => provider.get(id),
        closeCard: id => provider.close(id, { detail: true, quiet: true }),
        closePane: id => { closePaneBySid(id, { detach: false }); render(); },
      })
      : review;
    if ((status.restartBlockers || []).length) throw new Error('tmux-restart-mcp-managed-sessions');
    ctx.tmuxServerStatus = status;
    detachedSessions = [...panes.keys()];
    // Backend validates the reviewed attached-client counts before detaching.
    leaveSessionView({ detach: false });
    state.view = 'board';
    state.sessionId = null;
    unlisten = await listen('tmux-restart-progress', event => {
      const progress = event.payload;
      if (progress.requestId === requestId && progress.phase === 'replacing') replacementStarted = true;
      if (!ctx.tmuxRestarting || progress.requestId !== requestId || performance.now() - began < 300) return;
      const key = { exiting: 'tmux.progress.exiting', saving: 'tmux.progress.saving', replacing: 'tmux.progress.replacing' }[progress.phase];
      if (key) $('tmux-restart').textContent = t(key, progress);
    });
    await prepareCardsForServerRestart(status.sessions || []);
    invokedStatus = status;
    const restarted = await inv('restart_tmux_server', {
      expectedPid: status.serverPid || 0,
      expectedStartedAt: status.serverStartedAt || 0,
      expectedImpactToken: status.impactToken || '',
      expectedSessionCount: status.sessionCount || 0,
      expectedPaneCount: status.paneCount || 0,
      force: !status.pendingRestart,
      restoreShells: !!ctx.settings.sessionRestore,
      requestId,
    });
    if (restarted.serverPid === status.serverPid && restarted.serverStartedAt === status.serverStartedAt) {
      throw new Error('tmux-server-impact-changed');
    }
    ctx.tmuxServerStatus = restarted;
    markSessionsStoppedForServerRestart();
    render();
    $('tmux-lifecycle-modal').style.display = 'none';
    toast(t('tmux.restartComplete'));
    inv('acknowledge_tmux_lifecycle_notice').catch(() => {});
  } catch (error) {
    const message = error?.message || String(error);
    const key = {
      'managed-review-changed': 'tmux.impactChanged',
      'managed-status-unavailable': 'tmux.managedStatusUnavailable',
      'managed-close-rejected': 'tmux.managedCloseRejected',
      'managed-close-ambiguous': 'tmux.managedCloseAmbiguous',
      'tmux-restart-mcp-managed-sessions': 'tmux.managedStillBlocked',
      'tmux-server-impact-changed': 'tmux.impactChanged',
      'tmux-restart-agent-timeout': 'tmux.agentTimeout',
      'tmux-restart-snapshot-failed': 'tmux.snapshotFailed',
      'tmux-restart-busy': 'tmux.restartBusy',
      'tmux-restart-timeout': 'tmux.restartTimeout',
    }[message] || 'tmux.restartFailed';
    toast(t(key, { card: error?.cardId || '?', count: error?.closedCount || 0 }));
    const observed = await refreshTmuxLifecycle();
    if (invokedStatus && (observed
      ? (observed.serverPid !== invokedStatus.serverPid || observed.serverStartedAt !== invokedStatus.serverStartedAt)
      : replacementStarted)) {
      markSessionsStoppedForServerRestart();
      render();
    }
    if (message !== 'tmux-restart-timeout') {
      if (ctx.tmuxServerStatus?.pendingRestart) showTmuxLifecycle(ctx.tmuxServerStatus, false);
    }
  } finally {
    unlisten?.();
    await Promise.allSettled(detachedSessions.map(name => inv('detach_session', { name })));
    ctx.tmuxRestarting = false;
    $('tmux-restart').disabled = false;
    $('tmux-later').disabled = false;
    $('tmux-view-sessions').disabled = false;
    $('tmux-restart').textContent = t(managedBlockers(ctx.tmuxServerStatus).length ? 'tmux.closeManagedRestart' : 'tmux.restart');
    renderTmuxDiagnostics();
    startPolling();
  }
}

/* ---------- in-app updates (tauri-plugin-updater) ---------- */
export async function checkForUpdate() {
  if (!window.__TAURI__) return;
  if ($('update-btn').disabled) return;   // download/install in progress
  try {
    const update = await inv('check_for_update', { channel: ctx.settings.updateChannel });
    if (!update) return;
    ctx.pendingUpdate = update;
    const btn = $('update-btn');
    renderPendingUpdate();
    btn.style.display = 'flex';
    uev('update-avail', update.version);
  } catch (e) {
    uev('update-check-fail');   // offline / endpoint unreachable — silent
  }
}

export async function manualUpdateCheck() {
  const st = $('set-upd-status');
  const inModal = $('settings-modal').style.display === 'flex';
  const say = msg => { if (inModal) st.textContent = msg; else toast(msg); };
  if (!window.__TAURI__) { say(t('update.unavailableDev')); return; }
  say(t('update.checking'));
  try {
    const update = await inv('check_for_update', { channel: ctx.settings.updateChannel });
    if (update) {
      ctx.pendingUpdate = update;
      const btn = $('update-btn');
      btn.querySelector('.label').textContent = t('update.available', { version: update.version });
      btn.style.display = 'flex';
      say(t('update.installHint', { version: update.version }));
    } else {
      say(t('update.current', { version: ctx.buildIdentity.version || '?' }));
    }
  } catch (e) {
    say(t('update.checkFailed'));
    uev('update-check-fail', 'manual');
  }
}

/* ---------- boot ---------- */
/* Every module wires its DOM once here, in dependency order, instead of at
   import time: modules stay importable without a document (node tests), and
   the order of side effects is explicit. The leaf modules (scheduler, attention,
   templates, automation) receive the Board/layout/terminal actions they call
   as `deps` here instead of importing them, which keeps the import cycles
   confined to the view core (check.mjs enforces that). */
function initModules() {
  initInputDiagnostics();
  initDropdowns();
  initDialogs();
  initSettings();
  initTerminalChrome();
  initAttention({ pollNow, provider, render, switchProject, leaveSessionView, closeBuffer, openSession });
  initLayout();
  initScheduler({ provider, pollNow, closeBuffer });
  initBuffer();
  initTemplates({ provider });
  initInbound();
  initConnector();
  initMcp();
  initAutomation({ activeProject, newSessionSummary, openProjectDefaults, projectDefaultsSummary, provider, openSession, newDefaultSession });
  wireChrome();
}

export async function boot() {
  initModules();
  window.__DECK_DEBUG = await inv('debug_logging_enabled').catch(() => false);
  await loadSettings();
  await initInputSource();
  /* away notifications: apply the saved switch without asking macOS
     (that is the user's click in Settings); the badge follows from here */
  inv('notify_configure', {
    enabled: !!ctx.settings.notifyAway, sound: !!ctx.settings.notifySound, request: false,
  }).catch(() => {});
  listen('notify-open', event => {
    uev('notify-open');
    openFromNotification(String(event.payload?.session || ''));
  }).catch(() => uev('listen-fail', 'notify-open'));
  await revealThemedWindow();
  try {
    ctx.buildIdentity = await inv('build_identity');
  } catch (e) { /* label stays empty */ }
  renderBuildIdentity();
  await refreshTmuxLifecycle({ prompt: true });
  window.addEventListener('deck-update-channel-changed', () => {
    ctx.pendingUpdate = null;
    $('update-btn').style.display = 'none';
    renderBuildIdentity();
  });
  listen('update-download-progress', event => {
    const data = event.payload || {};
    const label = $('update-btn').querySelector('.label');
    if (data.event === 'finished') {
      label.textContent = t('update.installing');
      return;
    }
    ctx.updateDownloadBytes += Number(data.chunkLength) || 0;
    const total = Number(data.contentLength) || 0;
    label.textContent = total
      ? t('update.downloadingPercent', { percent: Math.min(100, Math.round(ctx.updateDownloadBytes / total * 100)) })
      : t('update.downloadingSize', { size: (ctx.updateDownloadBytes / 1048576).toFixed(1) });
  }).catch(() => uev('listen-fail', 'update-download-progress'));
  try {
    await listen('deck-ping', () => uev('ping-recv'));
    await inv('ping_event');
  } catch (e) {
    uev('ping-fail');
  }
  inv('storage_warnings').then(ws => (ws || []).forEach(w => toast(translateNotice(w)))).catch(() => {});
  ctx.HOME = await inv('default_dir').catch(() => '~');
  const ok = await inv('tmux_available').catch(() => false);
  if (!ok) $('banner').style.display = 'block';

  /* load_board resolves even on a first run (source "none"); a REJECTION
     means the board exists but could not be loaded — never treat that as a
     first run, and never auto-save defaults over whatever is on disk */
  let doc = null, loadErr = null;
  try { doc = await inv('load_board'); } catch (e) { loadErr = String(e); }
  if (doc && doc.warning) toast(translateNotice(doc.warning));
  if (doc && doc.data) {
    const data = JSON.parse(doc.data);   // backend already validated the shape
    store.projects = migrateColumnSemantics(data.projects || []);
    store.cards = (data.cards || []).map(c => ({
      ...c, pinned: c.pinned === true,
      status: 'stopped', mem: null, tail: [], idle: null,
    }));
  }
  if (loadErr) {
    toast(t('error.boardLoad'));
    uev('board-load-fail');
  }
  if (!store.projects.length) {
    if (loadErr) {
      /* in-memory board only — nothing touches disk until the user actually
         changes something (the damaged file was already quarantined) */
      store.projects.push({
        id: genId('P'), name: 'main',
        columns: ['attention', 'working', 'queued', 'parked'].map(semantic => ({ id: genId('C'), semantic, name: t(`board.default.${semantic}`) })),
      });
    } else {
      try {
        await provider.createProject('main');
      } catch (e) {
        store.projects.push({
          id: genId('P'), name: 'main',
          columns: ['attention', 'working', 'queued', 'parked'].map(semantic => ({ id: genId('C'), semantic, name: t(`board.default.${semantic}`) })),
        });
        toast(t('error.firstBoardSave'));
      }
    }
  }
  state.projectId = store.projects[0].id;
  render();
  startPolling();
  refreshQueue();
  drainInbound();
  drainChannel();
  drainConnector();
  setTimeout(checkForUpdate, 4000);
  /* runtime cadence comes from a Rust thread (App Nap freezes JS timers) */
  listen('update-check', checkForUpdate).catch(() => uev('listen-fail', 'update-check'));
  listen('update-check-manual', manualUpdateCheck).catch(() => uev('listen-fail', 'update-check-manual'));
}
boot();

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
function wireChrome() {
  initVoice({
    selectedTarget: () => {
      const card = provider.get(state.sessionId);
      return card && ctx.attachedName ? { session: ctx.attachedName, cardId: card.id, title: card.title } : null;
    },
    ...createVoiceTarget({
      getPane: session => panes.get(session),
      hasCard: id => !!provider.get(id),
      cancelSelection: pane => cancelTerminalSelection(pane, 'input'),
      scrollBottom: name => inv('scroll_bottom', { name }),
      setDelivering: session => { ctx.voiceDelivering = session; },
      resetInput: () => { ctx.lineBuf = null; },
    }),
    focusTerminal: () => ctx.term?.focus(),
    toast,
  });
  window.addEventListener('beforeunload', stopPolling, { once: true });

  document.addEventListener('webkitmouseforcewillbegin', event => {
    if (event.target?.closest?.('button, [role="button"]')) event.preventDefault();
  }, { capture: true, passive: false });

  document.addEventListener('contextmenu', event => {
    const editable = event.target?.closest?.('input, textarea, [contenteditable="true"]');
    if (!editable || editable.closest('#terminal')
        || event.target?.closest?.('button, [role="button"]')) event.preventDefault();
  }, { capture: true, passive: false });

  $('tmux-restart-btn').onclick = async () => {
    const status = await refreshTmuxLifecycle();
    if (status) showTmuxLifecycle(status, false);
  };

  $('set-tmux-restart').onclick = async () => {
    const status = await refreshTmuxLifecycle();
    if (status) showTmuxLifecycle(status, true);
  };

  $('tmux-later').onclick = deferTmuxRestart;

  $('tmux-restart').onclick = restartTmuxServer;

  $('tmux-view-sessions').onclick = () => {
    const list = $('tmux-impact-list');
    const showing = list.style.display === 'block';
    list.style.display = showing ? 'none' : 'block';
    $('tmux-view-sessions').textContent = t(showing ? 'tmux.viewSessions' : 'tmux.hideSessions');
  };

  document.addEventListener('keydown', event => {
    if ($('tmux-lifecycle-modal').style.display !== 'flex') return;
    if (ctx.tmuxRestarting && (event.key === 'Enter' || event.key === 'Escape')) {
      event.preventDefault(); event.stopPropagation(); return;
    }
    if (event.key === 'Enter') { event.preventDefault(); event.stopPropagation(); }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); deferTmuxRestart(); }
  }, true);

  window.addEventListener('deck-settings-opened', () => refreshTmuxLifecycle());

  $('set-check').onclick = () => manualUpdateCheck();

  $('update-btn').onclick = async () => {
    if (!ctx.pendingUpdate) return;
    const btn = $('update-btn');
    const label = btn.querySelector('.label');
    btn.disabled = true;
    ctx.updateDownloadBytes = 0;
    try {
      await inv('install_update', {
        channel: ctx.settings.updateChannel,
        expectedVersion: ctx.pendingUpdate.version,
      });
      label.textContent = t('update.restarting');
      await inv('relaunch_after_update');
    } catch (e) {
      btn.disabled = false;
      label.textContent = t('update.failedRetry');
      // the backend's fixed refusal for an admin-installed bundle (updater.rs)
      const notWritable = String(e).includes('not writable');
      toast(notWritable ? t('update.bundleNotWritable')
                        : t('error.operation', { operation: t('settings.updates') }));
      uev('update-install-fail', notWritable ? 'not-writable' : null);
    }
  };

  onLocaleChange(() => {
    renderPendingUpdate();
    renderBuildIdentity();
    renderTmuxDiagnostics();
    render();
    refreshQueue();
  });
}
