// app.js — in-app updates and boot
// Part of deck's no-build frontend: native ES modules, no bundler.
import './persistence.js';
import './board.js';
import { $, ctx, genId, initInputDiagnostics, inv, listen, state, store, uev } from './state.js';
import { initDialogs, loadSettings, toast } from './dialogs.js';
import { markSessionsStoppedForServerRestart, migrateColumnSemantics, provider, render, startPolling, stopPolling } from './board.js';
import { initLayout, leaveSessionView } from './layout.js';
import { initTerminalChrome } from './terminal.js';
import { initScheduler, refreshQueue } from './scheduler.js';
import { initTemplates } from './templates.js';
import { drainInbound, initInbound } from './inbound.js';
import { onLocaleChange, setLocale, t, translateNotice } from './i18n.js';
import { activateTheme, revealThemedWindow } from './theme.js';

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
  });
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
  list.style.display = 'none';
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
  const status = ctx.tmuxServerStatus;
  if (!status || ctx.tmuxRestarting) return;
  ctx.tmuxRestarting = true;
  renderTmuxDiagnostics(status);
  $('tmux-restart').disabled = true;
  $('tmux-later').disabled = true;
  $('tmux-view-sessions').disabled = true;
  $('tmux-restart').textContent = t('tmux.restarting');
  stopPolling();
  leaveSessionView();
  state.view = 'board';
  state.sessionId = null;
  markSessionsStoppedForServerRestart();
  render();
  try {
    ctx.tmuxServerStatus = await inv('restart_tmux_server', {
      expectedPid: status.serverPid || 0,
      expectedStartedAt: status.serverStartedAt || 0,
      expectedImpactToken: status.impactToken || '',
      expectedSessionCount: status.sessionCount || 0,
      expectedPaneCount: status.paneCount || 0,
      force: !status.pendingRestart,
    });
    $('tmux-lifecycle-modal').style.display = 'none';
    toast(t('tmux.restartComplete'));
    inv('acknowledge_tmux_lifecycle_notice').catch(() => {});
  } catch (error) {
    const changed = String(error).includes('impact-changed');
    toast(t(changed ? 'tmux.impactChanged' : 'tmux.restartFailed'));
    await refreshTmuxLifecycle();
    if (ctx.tmuxServerStatus?.pendingRestart) showTmuxLifecycle(ctx.tmuxServerStatus, false);
  } finally {
    ctx.tmuxRestarting = false;
    $('tmux-restart').disabled = false;
    $('tmux-later').disabled = false;
    $('tmux-view-sessions').disabled = false;
    $('tmux-restart').textContent = t('tmux.restart');
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
   the order of side effects is explicit. */
function initModules() {
  initInputDiagnostics();
  initDialogs();
  initTerminalChrome();
  initLayout();
  initScheduler();
  initTemplates();
  initInbound();
  wireChrome();
}

export async function boot() {
  initModules();
  window.__DECK_DEBUG = await inv('debug_logging_enabled').catch(() => false);
  await loadSettings();
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
  setTimeout(checkForUpdate, 4000);
  /* runtime cadence comes from a Rust thread (App Nap freezes JS timers) */
  listen('update-check', checkForUpdate).catch(() => uev('listen-fail', 'update-check'));
  listen('update-check-manual', manualUpdateCheck).catch(() => uev('listen-fail', 'update-check-manual'));
}
boot();

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
function wireChrome() {
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
      toast(t('error.operation', { operation: t('settings.updates') }));
      uev('update-install-fail');
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
