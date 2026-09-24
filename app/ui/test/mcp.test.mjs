// Production MCP Board bridge with a fake native ledger and the real Board
// persistence transaction.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { closeOutcome, drainMcp } = await import('../js/mcp.js');
const { listeners, store } = await import('../js/state.js');
const { panes } = await import('../js/board.js');
const { mcpErrorKey } = await import('../js/pure.js');
const { t } = await import('../js/i18n.js');
listeners.clear();

function project() {
  store.projects = [{ id: 'P1', name: 'Project', selected: 'C1', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [];
}

test('create starts the managed runner before persisting and commits the real card', async () => {
  project();
  const pending = { operationId: 'op_1', kind: 'session-create', result: {
    cardId: 'M1', sessionId: 'mcp_1', projectId: 'P1', title: 'MCP shell', cwd: '/tmp', generation: 'g_1',
  } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_start_session') return { created: true };
    if (cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 1);
  assert.equal(store.cards[0].origin.source, 'mcp');
  assert.equal(store.cards[0].cmd, '', 'project default commands never enter MCP sessions');
  assert.ok(calls.findIndex(([cmd]) => cmd === 'mcp_start_session') < calls.findIndex(([cmd]) => cmd === 'save_board'));
  const complete = calls.find(([cmd]) => cmd === 'mcp_complete')[1];
  assert.equal(complete.state, 'committed', JSON.stringify(calls));
  assert.equal(complete.tmuxSession, store.cards[0].session);
});

test('a Board save failure kills only the newly created session and records ambiguity', async () => {
  project();
  const pending = { operationId: 'op_2', kind: 'session-create', result: {
    cardId: 'M2', sessionId: 'mcp_2', projectId: 'P1', title: 'MCP shell', cwd: '/tmp', generation: 'g_2',
  } };
  let items = [pending]; let killed = 0; const states = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_start_session') return { created: true };
    if (cmd === 'save_board') throw new Error('disk full');
    if (cmd === 'kill_session') { killed++; return; }
    if (cmd === 'mcp_complete') { states.push(args.state); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(killed, 1);
  assert.equal(store.cards.length, 0);
  assert.deepEqual(states, ['ambiguous']);
});

test('close uses the ordinary Board close transaction and commits after persistence', async () => {
  project();
  store.cards = [{ id: 'M3', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M3', origin: { source: 'mcp' } }];
  const pending = { operationId: 'op_3', kind: 'session-close', result: { cardId: 'M3', sessionId: 'mcp_3' } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_close_admit') return { admission: 'close_admission' };
    if (cmd === 'queue_clear_sessions') return { removed: 0 };
    if (cmd === 'kill_session' || cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 0, JSON.stringify(calls));
  const admitted = calls.findIndex(([cmd]) => cmd === 'mcp_close_admit');
  const cleared = calls.findIndex(([cmd]) => cmd === 'queue_clear_sessions');
  const killed = calls.findIndex(([cmd]) => cmd === 'kill_session');
  assert.ok(admitted < cleared && cleared < killed, JSON.stringify(calls));
  assert.equal(calls[cleared][1].mcpAdmission, 'close_admission');
  assert.equal(calls[killed][1].mcpAdmission, 'close_admission');
  assert.equal(calls.find(([cmd]) => cmd === 'mcp_complete')[1].state, 'committed');
});

test('a close that fails after admission is ambiguous, never rejected', async () => {
  project();
  store.cards = [{ id: 'M4', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M4', origin: { source: 'mcp' } }];
  const pending = { operationId: 'op_4', kind: 'session-close', result: { cardId: 'M4', sessionId: 'mcp_4' } };
  let items = [pending]; const completions = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_close_admit') return { admission: 'close_admission' };
    if (cmd === 'queue_clear_sessions') return { removed: 0 };
    if (cmd === 'kill_session') throw new Error('tmux failed');
    if (cmd === 'mcp_complete') { completions.push(args); return; }
    if (cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 1, 'the card is kept');
  assert.deepEqual(completions.map(({ state, code }) => [state, code]), [['ambiguous', 'close-failed']]);
});

test('a remote close never retires a card that a pane is showing', async () => {
  project();
  store.cards = [{ id: 'M5', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M5', origin: { source: 'mcp' } }];
  panes.set('deck-mcp-M5', { sid: 'M5', session: 'deck-mcp-M5' });
  const pending = { operationId: 'op_5', kind: 'session-close', result: { cardId: 'M5', sessionId: 'mcp_5' } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_complete' || cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  try {
    await drainMcp();
  } finally {
    panes.delete('deck-mcp-M5');
  }
  assert.equal(store.cards.length, 1);
  assert.ok(!calls.some(([cmd]) => cmd === 'mcp_close_admit' || cmd === 'kill_session'), JSON.stringify(calls));
  const complete = calls.find(([cmd]) => cmd === 'mcp_complete')[1];
  assert.deepEqual([complete.state, complete.code], ['rejected', 'card-shown']);
});

test('a failed native completion is surfaced as a fixed sentence, not swallowed or thrown', async () => {
  project();
  const pending = { operationId: 'op_6', kind: 'unknown-kind', result: {} };
  let items = [pending]; let completions = 0;
  window.__TAURI__ = { core: { invoke: async (cmd) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_complete') { completions++; throw new Error('MCP operation cannot take that result'); }
    throw new Error(`unexpected ${cmd}`);
  } } };
  const toasts = document.getElementById('toasts');
  const before = toasts.children.length;
  await drainMcp();
  assert.equal(completions, 1, 'one report, no retry storm');
  assert.equal(toasts.children.length, before + 1);
  assert.equal(toasts.children.at(-1).textContent, t('mcp.boardSyncFailed'));
  assert.doesNotMatch(toasts.children.at(-1).textContent, /cannot take that result/);
});

test('close outcomes and local error codes map to fixed states and sentences', () => {
  assert.deepEqual(closeOutcome({ ok: true }), { state: 'committed', code: null });
  assert.deepEqual(closeOutcome({ ok: false, stage: 'shown' }), { state: 'rejected', code: 'card-shown' });
  assert.deepEqual(closeOutcome({ ok: false, admitted: true, stage: 'kill' }), { state: 'ambiguous', code: 'close-failed' });
  assert.deepEqual(closeOutcome({ ok: false, admitted: false }), { state: 'rejected', code: 'close-failed' });
  assert.equal(mcpErrorKey(new Error('mcp-session-busy')), 'mcp.errorBusy');
  assert.equal(mcpErrorKey('mcp-runner-stale'), 'mcp.errorStale');
  assert.equal(mcpErrorKey('mcp-client-revoked'), 'mcp.errorRevoked');
  assert.equal(mcpErrorKey('mcp-feature-disabled'), 'mcp.errorDisabled');
  assert.equal(mcpErrorKey('mcp-runner-unconfirmed'), 'mcp.errorUnconfirmed');
  assert.equal(mcpErrorKey('mcp-fence-unpersisted'), 'mcp.errorUnpersisted');
  assert.equal(mcpErrorKey('/Users/someone/private path'), 'mcp.actionFailed');
});
