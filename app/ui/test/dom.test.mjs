// DOM-contract tests for production inlineRename. This deliberately imports
// dialogs.js (not the pure value helper) and drives its real event handlers.
import test from 'node:test';
import assert from 'node:assert/strict';

class FakeElement {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.listeners = new Map();
    this.style = {};
    this.classList = { add() {}, remove() {}, toggle() {}, contains() { return false; } };
    this.isConnected = true;
    this.value = '';
    this._textContent = '';
  }
  addEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    list.push(fn); this.listeners.set(type, list);
  }
  fire(type, extra = {}) {
    const event = {
      key: '', keyCode: 0, isComposing: false,
      prevented: 0, stopped: 0,
      preventDefault() { this.prevented++; },
      stopPropagation() { this.stopped++; },
      ...extra,
    };
    for (const fn of this.listeners.get(type) || []) fn(event);
    if (typeof this[`on${type}`] === 'function') this[`on${type}`](event);
    return event;
  }
  replaceChildren(...nodes) { this.children = nodes; }
  appendChild(node) { this.children.push(node); return node; }
  append(node) { this.children.push(node); }
  remove() { this.isConnected = false; }
  focus() { fakeDocument.activeElement = this; }
  select() { this.selected = true; }
  closest() { return null; }
  setAttribute(name, value) { this[name] = value; }
  set textContent(value) { this._textContent = String(value); this.children = []; }
  get textContent() { return this._textContent; }
}

const ids = new Map();
const documentListeners = new Map();
const fakeDocument = {
  activeElement: null,
  addEventListener(type, fn) {
    const list = documentListeners.get(type) || [];
    list.push(fn); documentListeners.set(type, list);
  },
  fire(type, extra = {}) {
    const event = {
      key: '', keyCode: 0, isComposing: false, prevented: 0, stopped: 0,
      preventDefault() { this.prevented++; },
      stopPropagation() { this.stopped++; },
      ...extra,
    };
    for (const fn of documentListeners.get(type) || []) fn(event);
    return event;
  },
  createElement: tag => new FakeElement(tag),
  getElementById(id) {
    if (!ids.has(id)) ids.set(id, new FakeElement());
    return ids.get(id);
  },
};
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const {
  cfmDone, confirmDangerDialog, confirmDialog, inlineRename, persistSessionRestoreChoice, persistUpdateChannelChoice,
  promptDialog, persistThemeChoice, filterSettings, selectSettingsSection, resetApplicationLogs, refreshLogSize,
} = await import('../js/dialogs.js');
const { store } = await import('../js/state.js');
const { boardData, flushBoardMutations, mutateBoard, mutateBoardDebounced } = await import('../js/persistence.js');

const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('inline rename Enter ends DOM editing immediately and blur commits only once', async () => {
  const host = new FakeElement();
  let commits = 0;
  inlineRename(host, 'old', async value => { if (value !== null) commits++; });
  const input = host.children[0];
  input.value = 'new';
  const enter = input.fire('keydown', { key: 'Enter' });
  assert.equal(enter.prevented, 1);
  assert.equal(enter.stopped, 1);
  assert.equal(host.textContent, 'new');
  assert.equal(host.children.length, 0, 'input is gone in the Enter gesture');
  input.fire('blur');
  await tick();
  assert.equal(commits, 1, 'Enter followed by blur persists exactly once');
});

test('inline rename Escape cancels, blur commits, and IME Enter waits for composition', async () => {
  const escaped = new FakeElement();
  let persisted = 0;
  inlineRename(escaped, 'old', value => { if (value !== null) persisted++; });
  const escapeInput = escaped.children[0];
  escapeInput.value = 'discard';
  escapeInput.fire('keydown', { key: 'Escape' });
  assert.equal(escaped.textContent, 'old');
  assert.equal(persisted, 0);

  const blurred = new FakeElement();
  inlineRename(blurred, 'old', value => { if (value !== null) persisted++; });
  blurred.children[0].value = 'blurred';
  blurred.children[0].fire('blur');
  assert.equal(blurred.textContent, 'blurred');
  assert.equal(persisted, 1);

  const ime = new FakeElement();
  inlineRename(ime, 'old', value => { if (value !== null) persisted++; });
  const imeInput = ime.children[0];
  imeInput.value = '中文';
  const composing = imeInput.fire('keydown', { key: 'Enter', keyCode: 229, isComposing: true });
  assert.equal(composing.prevented, 0);
  assert.equal(ime.children[0], imeInput, 'composition Enter keeps editor alive');
  imeInput.fire('keydown', { key: 'Enter', isComposing: false });
  assert.equal(ime.textContent, '中文');
  assert.equal(persisted, 2);
});

test('prompt dialog does not submit Chinese IME preedit on Enter', async () => {
  const pending = promptDialog('说明', '');
  const input = fakeDocument.getElementById('ppd-input');
  input.value = '中文输入';
  input.fire('keydown', { key: 'Enter', keyCode: 229, isComposing: true });
  assert.equal(fakeDocument.getElementById('ppd').style.display, 'flex');
  input.fire('keydown', { key: 'Enter', keyCode: 13, isComposing: false });
  assert.equal(await pending, '中文输入');
});

test('inline rename restores the old DOM value when async persistence rejects', async () => {
  const host = new FakeElement();
  inlineRename(host, 'old', async () => { throw new Error('disk full'); });
  const input = host.children[0];
  input.value = 'new';
  input.fire('keydown', { key: 'Enter' });
  assert.equal(host.textContent, 'new');
  await tick();
  assert.equal(host.textContent, 'old');
});

test('production debounce is flushed before an immediate destructive Board barrier', async () => {
  const writes = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    assert.equal(cmd, 'save_board');
    writes.push(args.data);
  } } };
  store.projects = [{ id: 'p', name: 'p', columns: [{ id: 'c', name: 'c' }] }];
  store.cards = [
    { id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: '', dir: '/tmp', session: 'deck-a-0001', pinned: true },
    { id: 'b', projectId: 'p', columnId: 'c', title: 'B', desc: '', cmd: '', dir: '/tmp', session: 'deck-b-0002', pinned: false },
  ];
  mutateBoardDebounced(draft => { draft.projects[0].selected = 'c'; }, { delay: 10_000 });
  await mutateBoard(draft => { draft.cards = draft.cards.filter(c => c.id !== 'a'); });
  await flushBoardMutations();
  assert.equal(writes.length, 2, 'debounced write reaches disk before close barrier');
  const final = JSON.parse(writes.at(-1));
  assert.equal(final.projects[0].selected, 'c');
  assert.deepEqual(final.cards.map(c => c.id), ['b']);
  assert.deepEqual(final, JSON.parse(JSON.stringify({ projects: store.projects, cards: store.cards })));
});

test('Board serialization persists important marks and excludes runtime card state', () => {
  const cards = [{
    id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: '', dir: '/tmp',
    session: 'deck-a-0001', pinned: true, status: 'running', mem: 42, tail: ['private output'],
  }];
  const serialized = boardData([], cards);
  assert.equal(serialized.cards[0].pinned, true);
  assert.equal('status' in serialized.cards[0], false);
  assert.equal('mem' in serialized.cards[0], false);
  assert.equal('tail' in serialized.cards[0], false);
});

test('an inbound card keeps its origin across Board writes, and only identifiers', () => {
  const base = { projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: '', dir: '/tmp' };
  const cards = [
    { ...base, id: 'a', session: 'deck-a-0001',
      origin: { source: 'slack', key: 'C1:1.2', badge: 'eyes', text: 'the message', from: 'alice' } },
    { ...base, id: 'b', session: 'deck-b-0002', origin: { source: 'slack', key: 7 } },
    { ...base, id: 'c', session: 'deck-c-0003' },
  ];
  const serialized = boardData([], cards);
  assert.deepEqual(serialized.cards[0].origin, { source: 'slack', key: 'C1:1.2', badge: 'eyes' },
    'the idempotency key survives; message content never does');
  assert.equal('origin' in serialized.cards[1], false, 'a malformed origin is dropped, not persisted');
  assert.equal('origin' in serialized.cards[2], false);
});

test('a high-risk confirmation cannot be accepted by Enter, an ordinary one can', async () => {
  let settled = null;
  const danger = confirmDangerDialog('bypass the process check?').then(v => { settled = v; });
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  const enter = fakeDocument.fire('keydown', { key: 'Enter' });
  await tick();
  assert.equal(settled, null, 'Enter must not accept a pointer-only confirmation');
  assert.ok(enter.prevented && enter.stopped, 'Enter is swallowed rather than reaching the terminal');
  fakeDocument.fire('keydown', { key: 'Escape' });
  await danger;
  assert.equal(settled, false, 'Escape still declines');

  settled = null;
  const ordinary = confirmDialog('close this card?').then(v => { settled = v; });
  fakeDocument.fire('keydown', { key: 'Enter' });
  await ordinary;
  assert.equal(settled, true, 'an ordinary confirmation accepts Enter');
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'none');
});

test('failed theme persistence restores the prior palette and selectors', async () => {
  globalThis.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal', future: { kept: 1 },
  };
  fakeDocument.getElementById('set-theme').value = 'light';
  fakeDocument.getElementById('set-accent').value = 'purple';
  window.__TAURI__ = { core: { invoke: async cmd => {
    assert.equal(cmd, 'save_settings');
    throw new Error('disk full');
  } } };
  await persistThemeChoice();
  assert.equal(globalThis.settings.theme, 'deck-dark');
  assert.equal(globalThis.settings.accent, 'teal');
  assert.equal(fakeDocument.getElementById('set-theme').value, 'deck-dark');
  assert.equal(fakeDocument.getElementById('set-accent').value, 'teal');
  assert.equal(fakeDocument.getElementById('set-theme').disabled, false);
});

test('Nightly requires confirmation before a durable closed-enum save', async () => {
  globalThis.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'stable', future: { kept: 1 },
  };
  fakeDocument.getElementById('set-channel').value = 'nightly';
  let saved = null;
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    assert.equal(cmd, 'save_settings');
    saved = JSON.parse(args.data);
  } } };
  const pending = persistUpdateChannelChoice();
  await tick();
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  assert.equal(saved, null, 'no Nightly setting is written before consent');
  cfmDone(true);
  await pending;
  assert.equal(globalThis.settings.updateChannel, 'nightly');
  assert.equal(saved.updateChannel, 'nightly');
  assert.deepEqual(saved.future, { kept: 1 });
});

test('channel save failure rolls back and Stable switch never invokes install', async () => {
  globalThis.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'nightly',
  };
  fakeDocument.getElementById('set-channel').value = 'stable';
  const calls = [];
  window.__TAURI__ = { core: { invoke: async cmd => {
    calls.push(cmd);
    throw new Error('disk full');
  } } };
  await persistUpdateChannelChoice();
  assert.equal(calls.filter(cmd => cmd === 'save_settings').length, 1);
  assert.equal(calls.includes('install_update'), false);
  assert.equal(globalThis.settings.updateChannel, 'nightly');
  assert.equal(fakeDocument.getElementById('set-channel').value, 'nightly');
});

test('disabling shell recovery persists first and then clears every snapshot', async () => {
  globalThis.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'stable', sessionRestore: true,
  };
  fakeDocument.getElementById('set-session-restore').checked = false;
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push(cmd);
    if (cmd === 'save_settings') assert.equal(JSON.parse(args.data).sessionRestore, false);
  } } };
  await persistSessionRestoreChoice();
  assert.deepEqual(calls, ['save_settings', 'shell_snapshots_clear']);
  assert.equal(globalThis.settings.sessionRestore, false);
  assert.equal(fakeDocument.getElementById('set-session-restore').disabled, false);
});

test('enabling shell recovery is opt-in and saves only after disclosure', async () => {
  globalThis.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
    updateChannel: 'stable', sessionRestore: false,
  };
  fakeDocument.getElementById('set-session-restore').checked = true;
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push(cmd);
    assert.equal(cmd, 'save_settings');
    assert.equal(JSON.parse(args.data).sessionRestore, true);
  } } };
  const pending = persistSessionRestoreChoice();
  await tick();
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  assert.deepEqual(calls, [], 'no recovery preference is written before consent');
  cfmDone(true);
  await pending;
  assert.deepEqual(calls, ['save_settings']);
  assert.equal(globalThis.settings.sessionRestore, true);
});


test('settings navigation and search expose matching localized sections without losing controls', () => {
  const names = ['general', 'shortcuts', 'terminal', 'integrations', 'data', 'about'];
  names.forEach(name => { fakeDocument.getElementById('set-panel-' + name).textContent = name; });
  fakeDocument.getElementById('set-panel-integrations').textContent = '集成 Slack 自动响应';
  selectSettingsSection('terminal');
  assert.equal(fakeDocument.getElementById('set-panel-terminal').hidden, false);
  assert.equal(fakeDocument.getElementById('set-panel-general').hidden, true);
  assert.equal(fakeDocument.getElementById('set-nav-terminal')['aria-current'], 'page');
  const search = fakeDocument.getElementById('set-search');
  search.value = ' SLACK ';
  filterSettings();
  assert.equal(fakeDocument.getElementById('set-panel-integrations').hidden, false);
  assert.equal(fakeDocument.getElementById('set-panel-terminal').hidden, true);
  search.value = '自动响应';
  filterSettings();
  assert.equal(fakeDocument.getElementById('set-no-results').hidden, true);
  search.value = 'no such setting';
  filterSettings();
  assert.equal(fakeDocument.getElementById('set-no-results').hidden, false);
  search.value = '';
  filterSettings();
  assert.equal(fakeDocument.getElementById('set-panel-terminal').hidden, false);
});

test('log reset requires confirmation, suppresses double clicks, and refreshes size after success', async () => {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async cmd => { calls.push(cmd); return 0; } } };
  const cancelled = resetApplicationLogs();
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('cfm-no'));
  await resetApplicationLogs();
  assert.deepEqual(calls, []);
  cfmDone(false);
  await cancelled;
  assert.deepEqual(calls, []);
  assert.equal(fakeDocument.getElementById('set-reset-logs').disabled, false);
  const accepted = resetApplicationLogs();
  cfmDone(true);
  await accepted;
  assert.deepEqual(calls, ['reset_logs', 'log_size']);
  assert.match(fakeDocument.getElementById('set-log-size').textContent, /0 B$/);
});

test('failed log reset gives failure feedback and releases both buttons for retry', async () => {
  window.__TAURI__ = { core: { invoke: async () => { throw new Error('disk unavailable'); } } };
  const operation = resetApplicationLogs();
  cfmDone(true);
  await operation;
  assert.match(fakeDocument.getElementById('toasts').children.at(-1).textContent, /Could not reset/);
  assert.equal(fakeDocument.getElementById('set-reset-logs').disabled, false);
  assert.equal(fakeDocument.getElementById('set-export-logs').disabled, false);
  await refreshLogSize();
  assert.match(fakeDocument.getElementById('set-log-size').textContent, /unavailable/);
});

test('settings keyboard navigation and Escape keep focus inside the workflow', () => {
  selectSettingsSection('general');
  fakeDocument.getElementById('set-nav-general').fire('keydown', { key: 'ArrowDown' });
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-nav-shortcuts'));
  assert.equal(fakeDocument.getElementById('set-panel-shortcuts').hidden, false);
  fakeDocument.getElementById('set-nav-shortcuts').fire('keydown', { key: 'End' });
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-nav-about'));
  fakeDocument.getElementById('set-nav-about').fire('keydown', { key: 'Home' });
  const search = fakeDocument.getElementById('set-search');
  search.value = 'terminal';
  search.fire('input');
  assert.equal(fakeDocument.getElementById('set-panel-terminal').hidden, false);
  const box = fakeDocument.getElementById('settings-box');
  box.fire('keydown', { key: 'Escape' });
  assert.equal(search.value, '');
  assert.equal(fakeDocument.activeElement, search);
  box.fire('keydown', { key: 'Escape' });
  assert.equal(fakeDocument.getElementById('settings-modal').style.display, 'none');
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('settings-btn'));
});

test('log export blocks a concurrent reset and recovers from export failure', async () => {
  let complete;
  const calls = [];
  window.__TAURI__ = { core: { invoke: cmd => {
    calls.push(cmd);
    return new Promise(resolve => { complete = resolve; });
  } } };
  const button = fakeDocument.getElementById('set-export-logs');
  const exporting = button.onclick();
  await resetApplicationLogs();
  assert.deepEqual(calls, ['export_logs']);
  assert.equal(fakeDocument.getElementById('set-reset-logs').disabled, true);
  complete('/isolated/export.txt');
  await exporting;
  assert.match(fakeDocument.getElementById('toasts').children.at(-1).textContent, /Logs exported/);
  window.__TAURI__ = { core: { invoke: async () => { throw new Error('disk unavailable'); } } };
  await button.onclick();
  assert.match(fakeDocument.getElementById('toasts').children.at(-1).textContent, /Could not export/);
  assert.equal(button.disabled, false);
});

test('late log size responses cannot overwrite the newest post-reset size', async () => {
  const reads = [];
  window.__TAURI__ = { core: { invoke: () => new Promise(resolve => reads.push(resolve)) } };
  const oldRead = refreshLogSize();
  const newRead = refreshLogSize();
  reads[1](0);
  await newRead;
  reads[0](128000);
  await oldRead;
  assert.match(fakeDocument.getElementById('set-log-size').textContent, /0 B$/);
});
