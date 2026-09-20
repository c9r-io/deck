// Production MCP Board bridge with a fake native ledger and the real Board
// persistence transaction.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { drainMcp } = await import('../js/mcp.js');
const { listeners, store } = await import('../js/state.js');
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
    if (cmd === 'mcp_validate') return;
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
    if (cmd === 'queue_clear_sessions') return { removed: 0 };
    if (cmd === 'mcp_validate') return;
    if (cmd === 'kill_session' || cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 0, JSON.stringify(calls));
  assert.ok(calls.findIndex(([cmd]) => cmd === 'mcp_validate') < calls.findIndex(([cmd]) => cmd === 'kill_session'));
  assert.equal(calls.find(([cmd]) => cmd === 'mcp_complete')[1].state, 'committed');
});
