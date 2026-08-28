// app.js — in-app updates and boot
// Part of deck's no-build frontend: native ES modules, no bundler.
import './state.js';
import './persistence.js';
import './dialogs.js';
import './board.js';
import './layout.js';
import './terminal.js';
import './scheduler.js';
import { $, genId, inv, listen, state, store, uev } from './state.js';
import { loadSettings, toast } from './dialogs.js';
import { provider, render, startPolling } from './board.js';
import { refreshQueue } from './scheduler.js';

/* ---------- in-app updates (tauri-plugin-updater) ---------- */
export async function checkForUpdate() {
  const up = window.__TAURI__ && window.__TAURI__.updater;
  if (!up) return;
  if ($('update-btn').disabled) return;   // download/install in progress
  try {
    const update = await up.check();
    if (!update) return;
    pendingUpdate = update;
    const btn = $('update-btn');
    btn.querySelector('.label').textContent = `Update to v${update.version}`;
    btn.title = `deck v${update.version} is available — click to download and restart`;
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
  const up = window.__TAURI__ && window.__TAURI__.updater;
  if (!up) { say('updater unavailable in dev builds'); return; }
  say('Checking…');
  try {
    const update = await up.check();
    if (update) {
      pendingUpdate = update;
      const btn = $('update-btn');
      btn.querySelector('.label').textContent = `Update to v${update.version}`;
      btn.style.display = 'flex';
      say(`v${update.version} is available — click “Update to v${update.version}” in the sidebar to install.`);
    } else {
      say(`deck is up to date (v${$('app-ver').textContent})`);
    }
  } catch (e) {
    say('update check failed — are you online?');
    uev('update-check-fail', 'manual');
  }
}

$('set-check').onclick = () => manualUpdateCheck();

$('update-btn').onclick = async () => {
  if (!pendingUpdate) return;
  const btn = $('update-btn');
  const label = btn.querySelector('.label');
  btn.disabled = true;
  try {
    let got = 0, total = 0;
    await pendingUpdate.downloadAndInstall(ev => {
      if (ev.event === 'Started') total = ev.data.contentLength || 0;
      if (ev.event === 'Progress') {
        got += ev.data.chunkLength;
        label.textContent = total
          ? `Downloading ${Math.round(got / total * 100)}%`
          : `Downloading ${(got / 1048576).toFixed(1)}MB`;
      }
      if (ev.event === 'Finished') label.textContent = 'Installing…';
    });
    label.textContent = 'Restarting…';
    await window.__TAURI__.process.relaunch();
  } catch (e) {
    btn.disabled = false;
    label.textContent = 'Update failed — retry';
    toast('update failed: ' + e);
    uev('update-install-fail');
  }
};

/* ---------- boot ---------- */
export async function boot() {
  try {
    if (window.__TAURI__ && window.__TAURI__.app) {
      $('app-ver').textContent = await window.__TAURI__.app.getVersion();
    }
  } catch (e) { /* label stays empty */ }
  try {
    await listen('deck-ping', () => uev('ping-recv'));
    await inv('ping_event');
  } catch (e) {
    uev('ping-fail');
  }
  inv('storage_warnings').then(ws => (ws || []).forEach(w => toast(w))).catch(() => {});
  HOME = await inv('default_dir').catch(() => '~');
  const ok = await inv('tmux_available').catch(() => false);
  if (!ok) $('banner').style.display = 'block';

  /* load_board resolves even on a first run (source "none"); a REJECTION
     means the board exists but could not be loaded — never treat that as a
     first run, and never auto-save defaults over whatever is on disk */
  let doc = null, loadErr = null;
  try { doc = await inv('load_board'); } catch (e) { loadErr = String(e); }
  if (doc && doc.warning) toast(doc.warning);
  if (doc && doc.data) {
    const data = JSON.parse(doc.data);   // backend already validated the shape
    store.projects = data.projects || [];
    store.cards = (data.cards || []).map(c => ({ ...c, status: 'stopped', mem: null, tail: [], idle: null }));
  }
  if (loadErr) {
    toast('board could not be loaded: ' + loadErr);
    uev('board-load-fail');
  }
  if (!store.projects.length) {
    if (loadErr) {
      /* in-memory board only — nothing touches disk until the user actually
         changes something (the damaged file was already quarantined) */
      store.projects.push({
        id: genId('P'), name: 'main',
        columns: ['Attention', 'Working', 'Queued', 'Parked'].map(n => ({ id: genId('C'), name: n })),
      });
    } else {
      try {
        await provider.createProject('main');
      } catch (e) {
        store.projects.push({
          id: genId('P'), name: 'main',
          columns: ['Attention', 'Working', 'Queued', 'Parked'].map(n => ({ id: genId('C'), name: n })),
        });
        toast('the first board could not be saved; it is temporary until storage recovers');
      }
    }
  }
  state.projectId = store.projects[0].id;
  render();
  startPolling();
  refreshQueue();
  setTimeout(checkForUpdate, 4000);
  /* runtime cadence comes from a Rust thread (App Nap freezes JS timers) */
  listen('update-check', checkForUpdate).catch(() => uev('listen-fail', 'update-check'));
  listen('update-check-manual', manualUpdateCheck).catch(() => uev('listen-fail', 'update-check-manual'));
  loadSettings();
}
boot();
