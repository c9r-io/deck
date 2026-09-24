// DOM-contract tests for production inlineRename. This deliberately imports
// dialogs.js (not the pure value helper) and drives its real event handlers.
import test from 'node:test';
import assert from 'node:assert/strict';

import { FakeElement, fakeDocument, ids, documentListeners } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const {
  cfmDone, choiceDialog, confirmDangerDialog, confirmDialog, initDialogs, inlineRename, projectDefaultsDialog, promptDialog,
} = await import('../js/dialogs.js');
const {
  connectorPairingChanged, initSettings, renderConnectorSettings, mcpAuthorizationDialog, persistSessionRestoreChoice, persistUpdateChannelChoice,
  persistThemeChoice, filterSettings, renderMcpSettings, selectSettingsSection, resetApplicationLogs, refreshLogSize,
  locateSetting, openSettings, refreshEditors, persistAgentHooksChoice,
} = await import('../js/settings.js');
const { normalizeSettings: normalizeSettingsDoc } = await import('../js/settings-model.js');
const { ctx, store } = await import('../js/state.js');
const { boardData, flushBoardMutations, mutateBoard, mutateBoardDebounced } = await import('../js/persistence.js');
initDialogs();
initSettings();

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

test('MCP authorization requires an explicit project and shows its directory', async () => {
  const pending = mcpAuthorizationDialog([
    { id: 'P1', name: 'Deck', dir: '/Users/test/deck' },
    { id: 'P2', name: 'No directory', dir: '' },
  ]);
  const modal = fakeDocument.getElementById('mcp-auth');
  const name = fakeDocument.getElementById('mcp-auth-name');
  const project = fakeDocument.getElementById('mcp-auth-project');
  const root = fakeDocument.getElementById('mcp-auth-root');
  const proceed = fakeDocument.getElementById('mcp-auth-yes');
  assert.equal(modal.style.display, 'flex');
  assert.equal(project.value, '', 'no global/current project is inherited');
  assert.equal(root.value, '');
  assert.equal(proceed.disabled, false);
  assert.equal(project.options.length, 3, 'projects without default directories remain selectable');

  project.value = 'P1';
  project.fire('change');
  assert.equal(root.value, '/Users/test/deck', 'configured directory is a visible editable initial value');
  assert.equal(proceed.disabled, false);
  project.value = 'P2';
  project.fire('change');
  assert.equal(root.value, '');
  assert.equal(proceed.disabled, false);
  root.value = '/Users/test/explicit';
  root.fire('input');
  assert.equal(proceed.disabled, false);
  proceed.fire('click');
  assert.deepEqual(await pending, {
    name: 'ChatGPT', root: '/Users/test/explicit',
    project: { id: 'P2', name: 'No directory', dir: '' },
  });
  assert.equal(modal.style.display, 'none');
  assert.equal(name.oninput, null);
});

test('MCP authorization keeps inputs and maps path failures to the root field', async () => {
  const pending = mcpAuthorizationDialog(
    [{ id: 'P1', name: 'Deck', dir: '/missing' }],
    { previewRoot: async () => ({ ok: false, root: null, error: 'not_found' }) },
  );
  const project = fakeDocument.getElementById('mcp-auth-project');
  const root = fakeDocument.getElementById('mcp-auth-root');
  project.value = 'P1'; project.fire('change');
  fakeDocument.getElementById('mcp-auth-yes').fire('click');
  await tick();
  assert.equal(fakeDocument.getElementById('mcp-auth').style.display, 'flex');
  assert.equal(project.value, 'P1');
  assert.equal(root.value, '/missing');
  assert.equal(fakeDocument.activeElement, root);
  assert.equal(fakeDocument.getElementById('mcp-auth-root-error').hidden, false);
  fakeDocument.getElementById('mcp-auth-no').fire('click');
  assert.equal(await pending, null);
});

test('MCP authorization ignores stale preview and suppresses duplicate submission', async () => {
  let resolvePreview; let calls = 0;
  const pending = mcpAuthorizationDialog(
    [{ id: 'P1', name: 'Deck', dir: '/first' }],
    { previewRoot: () => { calls++; return new Promise(resolve => { resolvePreview = resolve; }); } },
  );
  const project = fakeDocument.getElementById('mcp-auth-project');
  const root = fakeDocument.getElementById('mcp-auth-root');
  const proceed = fakeDocument.getElementById('mcp-auth-yes');
  project.value = 'P1'; project.fire('change');
  proceed.fire('click'); proceed.fire('click');
  assert.equal(calls, 1);
  root.value = '/second'; root.fire('input');
  resolvePreview({ ok: true, root: '/canonical-first', error: null });
  await tick();
  assert.equal(fakeDocument.getElementById('mcp-auth').style.display, 'flex');
  assert.equal(root.value, '/second');
  fakeDocument.getElementById('mcp-auth-no').fire('click');
  assert.equal(await pending, null);
});

test('MCP authorization distinguishes missing project and ignores preview after cancel', async () => {
  let resolvePreview;
  const pending = mcpAuthorizationDialog(
    [{ id: 'P1', name: 'Deck', dir: '/valid' }],
    { previewRoot: () => new Promise(resolve => { resolvePreview = resolve; }) },
  );
  const project = fakeDocument.getElementById('mcp-auth-project');
  const proceed = fakeDocument.getElementById('mcp-auth-yes');
  proceed.fire('click');
  assert.equal(fakeDocument.activeElement, project);
  assert.equal(fakeDocument.getElementById('mcp-auth-project-error').hidden, false);
  project.value = 'P1'; project.fire('change'); proceed.fire('click');
  fakeDocument.getElementById('mcp-auth-no').fire('click');
  resolvePreview({ ok: true, root: '/valid', error: null });
  await tick();
  assert.equal(await pending, null);
  assert.equal(fakeDocument.getElementById('mcp-auth').style.display, 'none');
});

test('MCP authorization keeps the form when the project disappears during preview', async () => {
  let exists = true;
  const pending = mcpAuthorizationDialog(
    [{ id: 'P1', name: 'Deck', dir: '/valid' }],
    {
      previewRoot: async () => { exists = false; return { ok: true, root: '/valid', error: null }; },
      projectExists: () => exists,
    },
  );
  const project = fakeDocument.getElementById('mcp-auth-project');
  project.value = 'P1'; project.fire('change');
  fakeDocument.getElementById('mcp-auth-yes').fire('click');
  await tick();
  assert.equal(fakeDocument.getElementById('mcp-auth').style.display, 'flex');
  assert.equal(project.value, 'P1');
  assert.equal(fakeDocument.activeElement, project);
  assert.equal(fakeDocument.getElementById('mcp-auth-project-error').hidden, false);
  fakeDocument.getElementById('mcp-auth-no').fire('click');
  assert.equal(await pending, null);
});

test('MCP settings delete only an already-revoked client after confirmation', async () => {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_status') return { enabled: true, outputRetentionMs: 86_400_000,
      clients: [{ id: 'client_old', name: 'Old client', revoked: true, projects: [{ projectId: 'P1', roots: ['/tmp'] }] }] };
    if (cmd === 'mcp_client_delete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await renderMcpSettings();
  const row = fakeDocument.getElementById('set-mcp-clients').children[0];
  const actions = row.children[1];
  const deleteButton = actions.children.at(-1);
  assert.equal(deleteButton.textContent, 'Delete');
  deleteButton.fire('click');
  await tick();
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  cfmDone(true);
  await tick(); await tick();
  assert.ok(calls.some(([cmd, args]) => cmd === 'mcp_client_delete' && args.clientId === 'client_old'));
});

test('revoked MCP client deletion reports an orphan runtime without coupling cleanup to delete', async () => {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_status') return { enabled: true, outputRetentionMs: 86_400_000,
      clients: [{ id: 'client_orphan', name: 'Revoked', revoked: true, projects: [] }] };
    if (cmd === 'tunnel_helper_status') return { helperState: 'installed', tunnelState: 'stopped', runtimeExists: true };
    if (cmd === 'mcp_client_delete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await renderMcpSettings();
  await tick();
  const row = fakeDocument.getElementById('set-mcp-clients').children[0];
  const actions = row.children[1];
  const deleteButton = actions.children.find(button => button.textContent === 'Delete');
  deleteButton.fire('click');
  await tick();
  assert.equal(fakeDocument.getElementById('chd').style.display, 'flex');
  assert.match(fakeDocument.getElementById('chd-msg').textContent, /local Deck authority is gone/);
  const choiceButtons = fakeDocument.getElementById('chd-actions').children;
  choiceButtons.find(button => button.textContent === 'Delete Deck Client Anyway').fire('click');
  await tick(); await tick();
  assert.ok(calls.some(([cmd]) => cmd === 'mcp_client_delete'));
  assert.ok(!calls.some(([cmd]) => cmd === 'tunnel_helper_stop' || cmd === 'tunnel_helper_remove'));
});

test('a hung optional Tunnel status cannot delay MCP client revoke', async () => {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_status') return { enabled: true, outputRetentionMs: 86_400_000,
      clients: [{ id: 'client_live', name: 'Live', revoked: false, projects: [] }] };
    if (cmd === 'tunnel_helper_status') return new Promise(() => {});
    if (cmd === 'mcp_client_revoke') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await renderMcpSettings();
  const row = fakeDocument.getElementById('set-mcp-clients').children[0];
  const actions = row.children[1];
  const revoke = actions.children.find(button => button.textContent === 'Revoke');
  revoke.fire('click');
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  cfmDone(true);
  await tick(); await tick();
  assert.ok(calls.some(([cmd, args]) => cmd === 'mcp_client_revoke' && args.clientId === 'client_live'));
});

test('Tunnel actions expose stopped state and suppress double activation while pending', async () => {
  let starts = 0;
  window.__TAURI__ = { core: { invoke: async cmd => {
    if (cmd === 'mcp_status') return { enabled: true, outputRetentionMs: 86_400_000,
      clients: [{ id: 'client_tunnel', name: 'Tunnel', revoked: false, projects: [] }] };
    if (cmd === 'tunnel_helper_status') return { helperState: 'installed', tunnelState: 'stopped', runtimeExists: true };
    if (cmd === 'tunnel_helper_start') { starts++; return new Promise(() => {}); }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await renderMcpSettings();
  await tick();
  const row = fakeDocument.getElementById('set-mcp-clients').children[0];
  assert.match(row.children.at(-1).textContent, /stopped/);
  const start = row.children[1].children.find(button => button.textContent === 'Start Tunnel');
  start.fire('click'); start.fire('click');
  await tick();
  assert.equal(starts, 1);
  assert.equal(start.disabled, true);
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
    { id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: '', dir: '/tmp', session: 'deck-a-0001', pinned: true, launched: true },
    { id: 'b', projectId: 'p', columnId: 'c', title: 'B', desc: '', cmd: '', dir: '/tmp', session: 'deck-b-0002', pinned: false, launched: true },
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

test('Board serialization persists manual follow-up and excludes runtime card state', () => {
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

test('Board serialization keeps a card buffer independent from description and runtime state', () => {
  const buffer = { revision: 1, collecting: false, entries: [{
    id: 'N1', kind: 'manual', text: 'note', revision: 1, createdAt: 1, updatedAt: 1, copies: [],
  }] };
  const [card] = boardData([], [{
    id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: 'description', cmd: '', dir: '/tmp',
    session: 'deck-a-0001', buffer, status: 'running',
  }]).cards;
  assert.deepEqual(card.buffer, buffer);
  assert.equal(card.desc, 'description');
  assert.equal('status' in card, false);
});

test('Board serialization retains the channel collection and frozen initial queue journal', () => {
  const buffer = { revision: 1, collecting: true, entries: [] };
  const channelRun = { groupKey: 'default/T1/C1/R1', firstEventId: 'Ev1', connectionId: 'default',
    workspaceId: 'T1', channelId: 'C1', ruleId: 'R1', lastCollectedAt: 10, idleMinutes: 30,
    collecting: true, initialSteps: [{ operationId: 'B1', text: 'frozen', mode: 'at', at: 10,
      tpl: 'triage', tplIdx: 1, tplTotal: 1 }], initialQueued: false };
  const [card] = boardData([], [{ id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: '',
    dir: '/tmp', session: 'deck-a-0001', buffer, channelRun }]).cards;
  assert.deepEqual(card.channelRun, channelRun);
});

test('Board serialization retains a failed inbound template for restart retry', () => {
  const inboundPlan = { operationId: 'B0', reviewEach: false, initialQueued: false,
    initialSteps: [{ operationId: 'B1', text: 'frozen', mode: 'at', at: 10,
      tpl: 'triage', tplIdx: 1, tplTotal: 1 }] };
  const [card] = boardData([], [{ id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '',
    cmd: 'codex --yolo', dir: '/tmp', session: 'deck-a-0001', inboundPlan }]).cards;
  assert.deepEqual(card.inboundPlan, inboundPlan);
});

test('Board serialization retains Connector frozen task plans and project presets', () => {
  const connectorRun = { handle: 'a'.repeat(64), presetId: 'R1', initialQueued: false,
    initialSteps: [{ operationId: 'B1', text: 'frozen', mode: 'at', at: 10, tpl: 'R1', tplIdx: 1, tplTotal: 1 }] };
  const project = { id: 'p', name: 'P', columns: [{ id: 'c', name: 'C' }],
    presets: [{ id: 'R1', name: 'Fix', columnId: 'c', title: 'Task', dir: '~/work', cmd: 'codex', steps: ['frozen'] }] };
  const card = boardData([project], [{ id: 'a', projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: 'codex', dir: '/tmp',
    session: 'deck-a-0001', connectorRun }]);
  assert.deepEqual(card.projects[0].presets, project.presets);
  assert.deepEqual(card.cards[0].connectorRun, connectorRun);
});

test('the Board fixture is exactly the shape persistence.js writes', async () => {
  // fixtures/board.json is the one document both sides pin: this test proves
  // it is what the frontend serializes, documents.rs proves what the backend
  // requires of it. Adding a persisted card key changes this fixture, and the
  // Rust side then refuses to compile a guess.
  const { readFileSync } = await import('node:fs');
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/board.json', import.meta.url), 'utf8'));
  const full = {
    ...fixture.cards[0], status: 'running', mem: 42, tail: ['runtime only'], idle: 3,
    origin: { ...fixture.cards[0].origin, text: 'never persisted' },
  };
  const serialized = boardData(fixture.projects, [full]);
  assert.deepEqual(Object.keys(serialized.cards[0]).sort(), Object.keys(fixture.cards[0]).sort());
  assert.deepEqual(serialized, fixture);
});

test('launched persists as written and reads true on boards from before the field', () => {
  const base = { projectId: 'p', columnId: 'c', title: 'A', desc: '', cmd: 'claude', dir: '/tmp', session: 'deck-a-0001' };
  const [legacy, pending, done] = boardData([], [
    { ...base, id: 'a' }, { ...base, id: 'b', launched: false }, { ...base, id: 'c', launched: true },
  ]).cards;
  assert.equal(legacy.launched, true, 'no field = the command already ran; an upgrade never re-runs it');
  assert.equal(pending.launched, false);
  assert.equal(done.launched, true);
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
  ctx.settings = {
    editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal', future: { kept: 1 },
  };
  fakeDocument.getElementById('set-theme').value = 'light';
  fakeDocument.getElementById('set-accent').value = 'purple';
  window.__TAURI__ = { core: { invoke: async cmd => {
    assert.equal(cmd, 'save_settings');
    throw new Error('disk full');
  } } };
  await persistThemeChoice();
  assert.equal(ctx.settings.theme, 'deck-dark');
  assert.equal(ctx.settings.accent, 'teal');
  assert.equal(fakeDocument.getElementById('set-theme').value, 'deck-dark');
  assert.equal(fakeDocument.getElementById('set-accent').value, 'teal');
  assert.equal(fakeDocument.getElementById('set-theme').disabled, false);
});

test('Nightly requires confirmation before a durable closed-enum save', async () => {
  ctx.settings = {
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
  assert.equal(ctx.settings.updateChannel, 'nightly');
  assert.equal(saved.updateChannel, 'nightly');
  assert.deepEqual(saved.future, { kept: 1 });
});

test('channel save failure rolls back and Stable switch never invokes install', async () => {
  ctx.settings = {
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
  assert.equal(ctx.settings.updateChannel, 'nightly');
  assert.equal(fakeDocument.getElementById('set-channel').value, 'nightly');
});

test('disabling shell recovery persists first and then clears every snapshot', async () => {
  ctx.settings = {
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
  assert.equal(ctx.settings.sessionRestore, false);
  assert.equal(fakeDocument.getElementById('set-session-restore').disabled, false);
});

test('enabling shell recovery is opt-in and saves only after disclosure', async () => {
  ctx.settings = {
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
  assert.equal(ctx.settings.sessionRestore, true);
});


const SECTIONS = ['general', 'shortcuts', 'terminal', 'agents', 'integrations', 'remote', 'data', 'about'];
const panel = id => fakeDocument.getElementById('set-panel-' + id);
const navItem = id => fakeDocument.getElementById('set-nav-' + id);
const group = id => fakeDocument.getElementById('set-item-' + id);
const visiblePanels = () => SECTIONS.filter(id => !panel(id).hidden);

test('settings navigation and search show matching setting groups under their sections and restore the active one', () => {
  selectSettingsSection('terminal');
  assert.deepEqual(visiblePanels(), ['terminal']);
  assert.deepEqual(SECTIONS.filter(id => navItem(id)['aria-current'] === 'page'), ['terminal']);
  const search = fakeDocument.getElementById('set-search');

  search.value = ' 通知 ';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['agents'], 'only the section holding the match');
  assert.equal(group('away-notifications').hidden, false);
  assert.equal(group('agent-status').hidden, true, 'the neighbouring group of the same section stays hidden');
  assert.deepEqual(SECTIONS.filter(id => !navItem(id).hidden), ['agents']);
  assert.deepEqual(SECTIONS.filter(id => navItem(id)['aria-current'] !== 'false'), [], 'no section is current during a search');
  assert.match(fakeDocument.getElementById('set-search-status').textContent, /1/);
  assert.equal(fakeDocument.getElementById('set-no-results').hidden, true);

  search.value = 'Notify me when away';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['agents']);
  search.value = 'MCP';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['remote']);
  assert.equal(group('mcp').hidden, false);
  assert.equal(group('connector').hidden, true);
  search.value = 'SLACK';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['integrations']);
  assert.equal(group('slack-reactions').hidden, false);
  assert.equal(group('slack-channel').hidden, false);
  search.value = '数据与隐私';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['data'], 'a section title shows that section');
  search.value = 'Socket Mode';
  filterSettings();
  assert.deepEqual(visiblePanels(), [], 'Learn more text is not searched');

  search.value = 'no such setting';
  filterSettings();
  assert.equal(fakeDocument.getElementById('set-no-results').hidden, false);
  assert.equal(fakeDocument.getElementById('set-search-status').textContent, '');
  assert.deepEqual(SECTIONS.filter(id => !navItem(id).hidden), []);

  search.value = '';
  filterSettings();
  assert.deepEqual(visiblePanels(), ['terminal'], 'clearing restores the section that was active');
  assert.equal(navItem('terminal')['aria-current'], 'page');
  assert.deepEqual(SECTIONS.filter(id => navItem(id).hidden), []);
  for (const id of ['agent-status', 'connector', 'mcp', 'slack-channel']) assert.equal(group(id).hidden, false, id);
  assert.equal(fakeDocument.getElementById('set-no-results').hidden, true);
  assert.equal(fakeDocument.getElementById('set-search-status').textContent, '');
});

test('locating a setting selects its section, scrolls, highlights and focuses its group; unknown ids change nothing', () => {
  const mcp = group('mcp');
  const classes = new Set();
  mcp.classList = { add: c => classes.add(c), remove: c => classes.delete(c), toggle() {}, contains: c => classes.has(c) };
  let scrolled = null;
  mcp.scrollIntoView = options => { scrolled = options; };
  selectSettingsSection('general');
  const search = fakeDocument.getElementById('set-search');
  search.value = 'slack';
  filterSettings();
  assert.equal(locateSetting('mcp'), true);
  assert.equal(search.value, '', 'locating leaves search mode');
  assert.deepEqual(visiblePanels(), ['remote']);
  assert.equal(navItem('remote')['aria-current'], 'page');
  assert.equal(mcp.hidden, false);
  assert.deepEqual(scrolled, { block: 'start' });
  assert.equal(classes.has('set-located'), true);
  assert.equal(fakeDocument.activeElement, mcp);
  for (const bad of ['no-such-setting', '', undefined, null, 'remote']) {
    assert.equal(locateSetting(bad), false, String(bad));
    assert.deepEqual(visiblePanels(), ['remote']);
    assert.equal(fakeDocument.activeElement, mcp);
  }
  selectSettingsSection('general');
});

test('Enter in the search field locates the first result; IME Enter and empty results do nothing', () => {
  selectSettingsSection('general');
  const search = fakeDocument.getElementById('set-search');
  search.value = '通知';
  search.fire('input');
  const composing = search.fire('keydown', { key: 'Enter', isComposing: true, keyCode: 229 });
  assert.equal(composing.prevented, 0);
  assert.equal(search.value, '通知');
  const enter = search.fire('keydown', { key: 'Enter' });
  assert.equal(enter.prevented, 1);
  assert.equal(search.value, '');
  assert.deepEqual(visiblePanels(), ['agents']);
  assert.equal(fakeDocument.activeElement, group('away-notifications'));
  search.value = 'no such setting';
  search.fire('input');
  assert.equal(search.fire('keydown', { key: 'Enter' }).prevented, 0);
  assert.equal(search.value, 'no such setting');
  search.value = '';
  search.fire('input');
  selectSettingsSection('general');
});

function settingsBackend(overrides = {}) {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push(cmd);
    if (cmd in overrides) return overrides[cmd](args);
    if (cmd === 'detect_editors') return [];
    if (cmd === 'log_size') return 0;
    return null;
  } } };
  return calls;
}

test('opening settings does not wait for editor detection and keeps a choice made meanwhile', async () => {
  ctx.settings = normalizeSettingsDoc({ editor: 'Cursor' });
  let detected;
  const calls = settingsBackend({ detect_editors: () => new Promise(resolve => { detected = resolve; }) });
  await openSettings();
  assert.equal(fakeDocument.getElementById('settings-modal').style.display, 'flex', 'shown before detection answers');
  assert.ok(calls.includes('detect_editors'));
  const editor = fakeDocument.getElementById('set-editor');
  assert.deepEqual(editor.children.map(o => [o.value, o.textContent]), [['', 'System default (TextEdit)'], ['Cursor', 'Cursor']],
    'the saved editor is listed plainly until detection answers');
  assert.equal(editor.value, 'Cursor');
  editor.value = 'Zed';
  editor.fire('change');
  assert.equal(ctx.settings.editor, 'Zed');
  detected(['Cursor', 'Zed']);
  await tick(); await tick();
  assert.deepEqual(editor.children.map(o => o.value), ['', 'Cursor', 'Zed']);
  assert.equal(editor.value, 'Zed', 'the choice made during detection survives the refresh');

  let first; let second;
  settingsBackend({ detect_editors: () => new Promise(resolve => { if (!first) first = resolve; else second = resolve; }) });
  const older = refreshEditors(); const newer = refreshEditors();
  second(['Nova']); await newer;
  first(['Xcode']); await older;
  assert.deepEqual(editor.children.map(o => [o.value, o.textContent]), [['', 'System default (TextEdit)'], ['Nova', 'Nova'], ['Zed', 'Zed (not found)']],
    'only the newest answer renders; a missing saved editor is labelled after detection');
  settingsBackend({ detect_editors: () => { throw new Error('io'); } });
  await refreshEditors();
  assert.deepEqual(editor.children.map(o => o.value), ['', 'Nova', 'Zed'], 'a failed detection keeps the last list');
  fakeDocument.getElementById('settings-box').fire('keydown', { key: 'Escape' });
});

test('openSettings can open at a section or a setting, and ignores anything else', async () => {
  ctx.settings = normalizeSettingsDoc({});
  settingsBackend();
  const box = fakeDocument.getElementById('settings-box');
  selectSettingsSection('about');
  await openSettings({ type: 'click' });
  assert.deepEqual(visiblePanels(), ['about'], 'a click event opens the last active section');
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-search'));
  box.fire('keydown', { key: 'Escape' });
  await openSettings({ section: 'data' });
  assert.deepEqual(visiblePanels(), ['data']);
  box.fire('keydown', { key: 'Escape' });
  await openSettings({ section: 'general', setting: 'away-notifications' });
  assert.deepEqual(visiblePanels(), ['agents'], 'the setting decides the section');
  assert.equal(fakeDocument.activeElement, group('away-notifications'));
  box.fire('keydown', { key: 'Escape' });
  await openSettings({ section: 'remote', setting: 'bogus' });
  assert.deepEqual(visiblePanels(), ['remote'], 'an unknown setting falls back to the named section');
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-search'));
  box.fire('keydown', { key: 'Escape' });
  await openSettings({ section: 'nope' });
  assert.deepEqual(visiblePanels(), ['remote'], 'an unknown section keeps the active one');
  box.fire('keydown', { key: 'Escape' });
  selectSettingsSection('general');
});

test('away notifications name the agent-status dependency only when both hooks are known to be off', async () => {
  ctx.settings = normalizeSettingsDoc({ notifyAway: true });
  const dependency = fakeDocument.getElementById('set-notify-dependency');
  const box = fakeDocument.getElementById('settings-box');
  const open = async status => {
    let answer;
    settingsBackend({ agent_hooks_status: () => new Promise(resolve => { answer = resolve; }), notify_status: () => 'authorized' });
    await openSettings();
    assert.equal(dependency.hidden, true, 'no claim while the hook state is unknown');
    answer(status);
    await tick(); await tick();
  };

  await open({ claude: false, codex: false });
  assert.equal(dependency.hidden, false, 'both off');
  assert.equal(fakeDocument.getElementById('set-notify-away').checked, true, 'the notification switch is untouched');
  assert.equal(fakeDocument.getElementById('set-notify-away').disabled, false);
  const calls = settingsBackend();
  const codex = fakeDocument.getElementById('set-codex-hooks');
  codex.checked = true;
  const enabling = persistAgentHooksChoice('codex', 'set-codex-hooks', 'settings.codexHooksEnableConfirm');
  cfmDone(true);
  await enabling;
  assert.deepEqual(calls, ['agent_hooks_set']);
  assert.equal(dependency.hidden, true, 'one enabled');
  assert.equal(ctx.settings.notifyAway, true, 'no other setting changes');
  box.fire('keydown', { key: 'Escape' });

  await open({ claude: true, codex: false });
  assert.equal(dependency.hidden, true, 'Claude Code enabled');
  box.fire('keydown', { key: 'Escape' });
  await open({ claude: true, codex: true });
  assert.equal(dependency.hidden, true, 'both enabled');
  box.fire('keydown', { key: 'Escape' });
  await open(null);
  assert.equal(dependency.hidden, true, 'an unreadable state makes no claim');
  box.fire('keydown', { key: 'Escape' });
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
  fakeDocument.getElementById('set-nav-general').fire('keydown', { key: 'ArrowUp' });
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-nav-about'), 'ArrowUp wraps to the last section');
  selectSettingsSection('terminal');
  fakeDocument.getElementById('set-nav-terminal').fire('keydown', { key: 'ArrowDown' });
  assert.equal(fakeDocument.activeElement, fakeDocument.getElementById('set-nav-agents'));
  assert.deepEqual(visiblePanels(), ['agents']);
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

test('the choice dialog resolves an explicit answer, or null on cancel and Escape', async () => {
  const dlg = ids.get('chd') || fakeDocument.getElementById('chd');
  let promise = choiceDialog('directory gone', [{ id: 'edit', label: 'Edit' }, { id: 'home', label: 'Home shell', primary: true }]);
  const actions = fakeDocument.getElementById('chd-actions');
  assert.equal(dlg.style.display, 'flex');
  assert.equal(fakeDocument.getElementById('chd-msg').textContent, 'directory gone');
  assert.deepEqual(actions.children.map(b => b.textContent), ['Cancel', 'Edit', 'Home shell'], 'cancel first, choices in order');
  assert.equal(fakeDocument.activeElement, actions.children[2], 'the primary choice holds focus; Enter can only take that one');
  actions.children[2].fire('click');
  assert.equal(await promise, 'home');
  assert.equal(dlg.style.display, 'none');
  promise = choiceDialog('again', [{ id: 'home', label: 'Home shell', primary: true }]);
  fakeDocument.getElementById('chd-actions').children[0].fire('click');
  assert.equal(await promise, null, 'cancel');
  promise = choiceDialog('again', [{ id: 'home', label: 'Home shell' }]);
  assert.equal(fakeDocument.activeElement.textContent, 'Cancel', 'no primary: cancel holds focus');
  const esc = dlg.fire('keydown', { key: 'Escape' });
  assert.equal(esc.prevented, 1);
  assert.equal(await promise, null, 'Escape');
  assert.equal(dlg.onkeydown, null, 'the handler is cleared with the dialog');
  promise = choiceDialog('outside', [{ id: 'home', label: 'Home shell' }]);
  dlg.fire('mousedown', { target: dlg });
  assert.equal(await promise, null, 'a click on the scrim cancels');
});

test('the project defaults dialog edits two trimmed strings and chips only fill the command', async () => {
  const dlg = fakeDocument.getElementById('pdf');
  const dir = fakeDocument.getElementById('pdf-dir'), cmd = fakeDocument.getElementById('pdf-cmd');
  let promise = projectDefaultsDialog({ name: 'Atlas', dir: '~/work/atlas', cmd: '', recent: ['claude', 'codex', 'cargo test'] });
  assert.equal(dlg.style.display, 'flex');
  assert.match(fakeDocument.getElementById('pdf-title').textContent, /Atlas/);
  assert.equal(dir.value, '~/work/atlas');
  assert.equal(cmd.value, '');
  assert.equal(fakeDocument.activeElement, dir, 'the directory field is focused first');
  const chips = fakeDocument.getElementById('pdf-chips');
  assert.equal(chips.hidden, false);
  assert.deepEqual(chips.children.map(c => c.textContent), ['claude', 'codex', 'cargo test']);
  chips.children[1].fire('click');
  assert.equal(cmd.value, 'codex', 'a chip fills the field');
  assert.equal(fakeDocument.activeElement, cmd);
  dir.value = '  ~/work/atlas/api ';
  const enter = dlg.fire('keydown', { key: 'Enter', target: { tagName: 'INPUT' } });
  assert.equal(enter.prevented, 1);
  assert.deepEqual(await promise, { dir: '~/work/atlas/api', cmd: 'codex' }, 'Enter saves trimmed values');
  assert.equal(dlg.style.display, 'none');
  promise = projectDefaultsDialog({ name: 'Cedar', recent: [] });
  assert.equal(chips.hidden, true, 'no recent commands, no chip row');
  assert.equal(dir.value, '');
  const composing = dlg.fire('keydown', { key: 'Enter', isComposing: true, target: { tagName: 'INPUT' } });
  assert.equal(composing.prevented, 0, 'an IME Enter never saves');
  dlg.fire('keydown', { key: 'Escape' });
  assert.equal(await promise, null);
  promise = projectDefaultsDialog({ name: 'Cedar' });
  cmd.value = 'claude';
  fakeDocument.getElementById('pdf-yes').fire('click');
  assert.deepEqual(await promise, { dir: '', cmd: 'claude' }, 'Save with a blank directory keeps only the command');
  promise = projectDefaultsDialog({ name: 'Cedar' });
  fakeDocument.getElementById('pdf-no').fire('click');
  assert.equal(await promise, null);
});

test('project defaults creates a bounded desktop task preset in the same Board draft', async () => {
  const promise = projectDefaultsDialog({ name: 'Atlas', dir: '~/work', cmd: 'codex', recent: [],
    presets: [], columns: [{ id: 'C1', name: 'Working' }] });
  fakeDocument.getElementById('pdf-preset-add').fire('click');
  fakeDocument.getElementById('pdf-preset-name').value = 'Fix issue';
  fakeDocument.getElementById('pdf-preset-column').value = 'C1';
  fakeDocument.getElementById('pdf-preset-title').value = 'Remote fix';
  fakeDocument.getElementById('pdf-preset-dir').value = '~/work';
  fakeDocument.getElementById('pdf-preset-cmd').value = 'codex';
  fakeDocument.getElementById('pdf-preset-steps').value = 'inspect\nfix';
  fakeDocument.getElementById('pdf-preset-done').fire('click');
  fakeDocument.getElementById('pdf-yes').fire('click');
  const result = await promise;
  assert.equal(result.presets.length, 1);
  assert.deepEqual({ ...result.presets[0], id: 'stable' }, { id: 'stable', name: 'Fix issue', columnId: 'C1',
    title: 'Remote fix', dir: '~/work', cmd: 'codex', steps: ['inspect', 'fix'] });
});

test('voice settings save only preferences, preserve unrelated fields and roll back on failure', async () => {
  const { persistVoicePreferences, renderVoicePreferences } = await import('../js/settings.js');
  const { normalizeSettings } = await import('../js/settings-model.js');
  const control = new FakeElement('input');
  fakeDocument.getElementById('settings-modal').querySelectorAll = () => [control];
  ctx.settings = normalizeSettings({ future: { kept: 1 }, editor: 'Zed' });
  let fail = false, resolveSave, saved, changes = 0;
  window.dispatchEvent = event => { if (event.type === 'deck-voice-preferences-changed') changes++; };
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd !== 'save_settings') return;
    saved = JSON.parse(args.data);
    if (fail) throw new Error('disk unavailable');
    await new Promise(resolve => { resolveSave = resolve; });
  } } };
  renderVoicePreferences();
  const original = ctx.settings.voice;
  const pending = persistVoicePreferences({ languages: ['ja-JP'], defaultLanguage: 'ja-JP' });
  await tick(); assert.equal(control.disabled, true); assert.equal(ctx.settings.voice, original);
  assert.equal(await persistVoicePreferences({ languages: ['en-US'], defaultLanguage: 'en-US' }), false);
  resolveSave(); assert.equal(await pending, true);
  assert.deepEqual(saved.future, { kept: 1 }); assert.equal(ctx.settings.editor, 'Zed'); assert.equal(changes, 1);
  assert.equal(ctx.settings.voice.defaultLanguage, 'ja-JP'); assert.equal(control.disabled, false);
  const choices = fakeDocument.getElementById('set-voice-languages').children.map(label => label.children[0]);
  assert.equal(choices.find(input => input.value === 'ja-JP').disabled, true, 'last enabled choice cannot be unchecked');
  fail = true;
  assert.equal(await persistVoicePreferences({ languages: ['en-US'], defaultLanguage: 'en-US' }), false);
  assert.equal(ctx.settings.voice.defaultLanguage, 'ja-JP'); assert.equal(changes, 1); assert.equal(control.disabled, false);
  assert.equal(await persistVoicePreferences({ languages: [], defaultLanguage: 'system' }), false);
  delete window.dispatchEvent;
});

test('a committed launch barrier replaces the old in-memory launched flag', async () => {
  window.__TAURI__ = { core: { invoke: async () => {} } };
  store.projects = [{ id: 'p', name: 'p', columns: [{ id: 'c', name: 'c' }] }];
  store.cards = [{ id: 'a', projectId: 'p', columnId: 'c', title: 'A', cmd: 'claude', dir: '/tmp', session: 'deck-a-0001', launched: false, status: 'stopped' }];
  await mutateBoard(draft => { draft.cards[0].launched = true; });
  assert.equal(store.cards[0].launched, true);
  assert.equal(store.cards[0].status, 'stopped', 'runtime state is still preserved');
});

test('enabling Phone Connector requires an explicit confirmation of what a paired phone can do', async () => {
  const calls = [];
  window.__TAURI__ = { core: { invoke: async cmd => {
    calls.push(cmd);
    if (cmd === 'connector_status') return { enabled: false, running: false, address: '', port: 47631, devices: [] };
    if (cmd === 'connector_addresses') return ['192.168.1.20'];
    return {};
  } } };
  await renderConnectorSettings();
  const toggle = fakeDocument.getElementById('set-connector-toggle');
  assert.equal(toggle.dataset.enabled, 'false');
  const cancelled = toggle.onclick();
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex');
  assert.match(fakeDocument.getElementById('cfm-msg').textContent, /submit prompts/);
  cfmDone(false);
  await cancelled;
  assert.equal(calls.filter(cmd => cmd === 'connector_enable').length, 0, 'cancel never enables');

  const accepted = toggle.onclick();
  cfmDone(true);
  await accepted;
  assert.equal(calls.filter(cmd => cmd === 'connector_enable').length, 1);
});

test('a pairing is announced by name and time and hides the spent QR code', async () => {
  const phone = { id: 'D1', name: 'Old phone', pairedAt: 1_790_000_000, revoked: false };
  let devices = [phone];
  window.__TAURI__ = { core: { invoke: async cmd => {
    if (cmd === 'connector_status') return { enabled: true, running: true, address: '192.168.1.20', port: 47631, devices };
    if (cmd === 'connector_addresses') return ['192.168.1.20'];
    throw new Error(`unexpected ${cmd}`);
  } } };
  await renderConnectorSettings();
  const row = fakeDocument.getElementById('set-connector-devices').children[0];
  assert.match(row.children[1].textContent, /^paired /, 'each device shows when it was paired');

  const pairing = fakeDocument.getElementById('set-connector-pairing');
  pairing.hidden = false;
  await connectorPairingChanged();
  assert.equal(pairing.hidden, false, 'no new device, the QR code stays');

  devices = [phone, { id: 'D2', name: 'Unknown iPhone', pairedAt: 1_790_000_100, revoked: false }];
  await connectorPairingChanged();
  assert.equal(pairing.hidden, true);
  assert.match(fakeDocument.getElementById('toasts').children.at(-1).textContent, /Paired “Unknown iPhone”/);
  assert.equal(fakeDocument.getElementById('set-connector-devices').children.length, 2);
});
