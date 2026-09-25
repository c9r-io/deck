import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };

const { listeners, store } = await import('../js/state.js');
const { markSessionsStoppedForServerRestart, provider } = await import('../js/board.js');
listeners.clear();

function card(id, status = 'running') {
  return { id, projectId: 'P1', columnId: 'C1', title: id, desc: '', cmd: '', dir: '/tmp',
    session: `deck-${id}`, status, launched: true };
}

function setup(cards, kill = async () => {}) {
  store.projects = [{ id: 'P1', name: 'Project', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = cards;
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'queue_clear_sessions') return;
    if (cmd === 'kill_session') return kill(args);
    if (cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  return calls;
}

test('a stopped card after server restart retires through the ordinary transaction', async () => {
  const calls = setup([card('A'), card('B')]);
  markSessionsStoppedForServerRestart();
  assert.deepEqual(store.cards.map(c => c.status), ['stopped', 'stopped']);
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: true });
  assert.deepEqual(store.cards.map(c => c.id), ['B']);
  assert.deepEqual(calls.map(([cmd]) => cmd), ['queue_clear_sessions', 'kill_session', 'save_board']);
  assert.equal(calls[1][1].name, 'deck-A');
  assert.equal(JSON.parse(calls[2][1].data).cards.length, 1);
});

test('an uncertain runtime failure retains the stopped card and does not write the Board', async () => {
  const calls = setup([card('A', 'stopped')], async () => { throw new Error('tmux timeout'); });
  assert.deepEqual(await provider.close('A', { detail: true, quiet: true }),
    { ok: false, applied: false, stage: 'kill', admitted: false });
  assert.equal(store.cards.length, 1);
  assert.ok(!calls.some(([cmd]) => cmd === 'save_board'));
});

test('live close still kills before retirement and repeated close is a no-op', async () => {
  const calls = setup([card('A')]);
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: true });
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: false });
  assert.equal(store.cards.length, 0);
  assert.deepEqual(calls.map(([cmd]) => cmd), ['queue_clear_sessions', 'kill_session', 'save_board']);
});
