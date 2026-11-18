import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };

const { listeners, store } = await import('../js/state.js');
const { provider } = await import('../js/board.js');
listeners.clear();

function setup(cards) {
  store.projects = [
    { id: 'P1', name: 'One', columns: [{ id: 'A1', name: 'Todo' }, { id: 'A2', name: 'Doing' }] },
    { id: 'P2', name: 'Two', selected: 'B2', columns: [{ id: 'B1', name: 'In' }, { id: 'B2', name: 'Sorted' }, { id: 'B3', name: 'Done' }] },
  ];
  store.cards = cards.map(card => ({ projectId: 'P1', columnId: 'A1', title: card.id, desc: '', cmd: 'claude',
    dir: '/work', session: `deck-${card.id}`, launched: true, ...card }));
  const saves = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd !== 'save_board') throw new Error(`unexpected ${cmd}`);
    saves.push(JSON.parse(args.data));
  } } };
  return saves;
}

test('a card moves to another project in one write and keeps everything else', async () => {
  const run = { origin: { source: 'slack', key: 'k', badge: 'eyes' }, inboundPlan: { initialQueued: true, followUps: ['ok'] } };
  const saves = setup([{ id: 'S1', ...run }, { id: 'S2' }]);
  assert.deepEqual(await provider.moveToProject('S1', 'P2'), { ok: true, project: 'Two', column: 'Sorted', moved: true });
  assert.equal(saves.length, 1);
  const saved = saves[0].cards.find(card => card.id === 'S1');
  assert.deepEqual([saved.projectId, saved.columnId], ['P2', 'B2']);
  assert.deepEqual([saved.session, saved.dir, saved.cmd, saved.origin, saved.inboundPlan],
    ['deck-S1', '/work', 'claude', run.origin, run.inboundPlan]);
  assert.deepEqual(saves[0].cards.find(card => card.id === 'S2').projectId, 'P1');
});

test('the states that stay put write nothing and say why', async () => {
  const saves = setup([
    { id: 'S1', inboundPlan: { initialQueued: false } },
    { id: 'S2', channelRun: { initialQueued: true, collecting: true } },
    { id: 'S3', origin: { source: 'mcp', key: 'op', badge: 'managed' } },
  ]);
  assert.deepEqual(await provider.moveToProject('S1', 'P2'), { ok: false, reason: 'starting' });
  assert.deepEqual(await provider.moveToProject('S2', 'P2'), { ok: false, reason: 'collecting' });
  assert.deepEqual(await provider.moveToProject('S3', 'P2'), { ok: false, reason: 'managed' });
  assert.deepEqual(await provider.moveToProject('S1', 'gone'), { ok: false, reason: 'gone' });
  assert.deepEqual(await provider.moveToProject('gone', 'P2'), { ok: false, reason: 'gone' });
  assert.equal(saves.length, 0);
  assert.deepEqual(store.cards.map(card => card.projectId), ['P1', 'P1', 'P1']);
});

test('a named column and the place the caller saw (the MCP move)', async () => {
  const saves = setup([{ id: 'S1' }, { id: 'S2', inboundPlan: { initialQueued: false } }]);
  const seen = { projectId: 'P1', columnId: 'A1' };
  // within the project: a column move, allowed in every state
  assert.equal((await provider.moveToProject('S2', 'P1', { expect: seen, columnId: 'A2' })).moved, true);
  assert.equal(store.cards[1].columnId, 'A2');
  // the user moved it meanwhile: refused, nothing written
  assert.deepEqual(await provider.moveToProject('S2', 'P1', { expect: seen, columnId: 'A1' }), { ok: false, reason: 'changed' });
  // a column of another project is no column of the target
  assert.deepEqual(await provider.moveToProject('S1', 'P2', { expect: seen, columnId: 'A2' }), { ok: false, reason: 'gone' });
  assert.deepEqual(await provider.moveToProject('S1', 'P2', { expect: seen, columnId: 'B3' }),
    { ok: true, project: 'Two', column: 'Done', moved: true });
  // already there: settled without a write
  const writes = saves.length;
  assert.equal((await provider.moveToProject('S1', 'P2', { columnId: 'B3' })).moved, false);
  assert.equal(saves.length, writes);
});
