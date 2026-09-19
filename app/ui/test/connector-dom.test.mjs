// Production Connector dispatcher with a fake native durable journal. Board
// writes still use the real serialized persistence queue.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { drainConnector } = await import('../js/connector.js');
const { store } = await import('../js/state.js');

test('claimed buffer-add validates inside the Board transaction, persists once, then completes', async () => {
  const handle = 'a'.repeat(64);
  const pending = { handle, request: { id: 'remote-1', kind: 'buffer-add', cardId: 'S1',
    expectedRevision: '0', payload: { text: 'remote note' } } };
  let items = [pending]; const calls = []; const writes = [];
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [{ id: 'S1', projectId: 'P1', columnId: 'C1', title: 'Card', desc: '', cmd: '', dir: '/tmp',
    session: 'deck-card-S1', launched: true }];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'connector_pending') { const out = items; items = []; return out; }
    if (cmd === 'connector_claim') return pending;
    if (cmd === 'connector_validate') return true;
    if (cmd === 'save_board') { writes.push(args.data); return; }
    if (cmd === 'connector_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainConnector(); await drainConnector();
  assert.equal(store.cards[0].buffer.entries.length, 1);
  assert.equal(store.cards[0].buffer.entries[0].text, 'remote note');
  assert.equal(writes.length, 1, 'a replay with no pending journal command cannot duplicate the note');
  assert.ok(calls.find(([cmd]) => cmd === 'connector_validate'));
  const completed = calls.find(([cmd]) => cmd === 'connector_complete')[1];
  assert.equal(completed.state, 'applied', JSON.stringify(calls));
  assert.deepEqual(completed.result.cardId, 'S1');
  assert.equal(completed.result.revision, '1');
});

test('a result-journal failure after a durable Board write is classified ambiguous', async () => {
  const handle = 'b'.repeat(64); const pending = { handle, request: { id: 'remote-2', kind: 'buffer-add', cardId: 'S1',
    expectedRevision: '0', payload: { text: 'durable note' } } };
  let items = [pending]; let completions = 0; const states = [];
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [{ id: 'S1', projectId: 'P1', columnId: 'C1', title: 'Card', desc: '', cmd: '', dir: '/tmp', session: 'deck-card-S1' }];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'connector_pending') { const out = items; items = []; return out; }
    if (cmd === 'connector_claim') return pending;
    if (cmd === 'connector_validate' || cmd === 'save_board') return true;
    if (cmd === 'connector_complete') { states.push(args.state); if (++completions === 1) throw new Error('journal save'); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainConnector(); await drainConnector();
  assert.equal(store.cards[0].buffer.entries[0].text, 'durable note');
  assert.deepEqual(states, ['applied', 'ambiguous']);
});

test('task session start followed by Board save failure is ambiguous and never relaunched', async () => {
  const handle = 'c'.repeat(64); const pending = { handle, request: { id: 'remote-3', kind: 'task-create',
    expectedRevision: 'rev', payload: { projectId: 'P1', presetId: 'R1' } } };
  let items = [pending]; let starts = 0; let kills = 0; const states = [];
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }],
    presets: [{ id: 'R1', name: 'Task', columnId: 'C1', title: 'Remote', dir: '/tmp', cmd: 'codex', steps: [] }] }];
  store.cards = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'connector_pending') { const out = items; items = []; return out; }
    if (cmd === 'connector_claim') return pending;
    if (cmd === 'connector_validate') return true;
    if (cmd === 'start_session') { starts++; return { created: true, restored: false }; }
    if (cmd === 'save_board') throw new Error('disk full');
    if (cmd === 'kill_session') { kills++; return; }
    if (cmd === 'connector_complete') { states.push(args.state); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainConnector(); await drainConnector();
  assert.equal(starts, 1); assert.equal(kills, 1); assert.deepEqual(states, ['ambiguous']);
  assert.equal(store.cards.length, 0);
});

test('native string rejections retain the closed unsupported-target result code', async () => {
  const handle = 'd'.repeat(64); const pending = { handle, request: { id: 'remote-4', kind: 'buffer-queue', cardId: 'S1',
    expectedRevision: '1', payload: { entryIds: ['N1'] } } };
  let items = [pending]; const completions = []; let writes = 0;
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [{ id: 'S1', projectId: 'P1', columnId: 'C1', title: 'Shell', desc: '', cmd: '', dir: '/tmp',
    session: 'deck-card-S1', buffer: { revision: 1, collecting: false, entries: [{ id: 'N1', kind: 'manual',
      text: 'note', revision: 1, createdAt: 1, updatedAt: 1, copies: [] }] } }];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'connector_pending') { const out = items; items = []; return out; }
    if (cmd === 'connector_claim') return pending;
    if (cmd === 'connector_validate') throw 'unsupported target: saved card is not an agent';
    if (cmd === 'save_board') { writes++; return; }
    if (cmd === 'connector_complete') { completions.push(args); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainConnector();
  assert.equal(writes, 0); assert.equal(store.cards[0].buffer.entries[0].copies.length, 0);
  assert.equal(completions[0].state, 'rejected'); assert.equal(completions[0].code, 'unsupported-target');
});
