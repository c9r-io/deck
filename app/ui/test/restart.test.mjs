import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = { ...fakeDocument, querySelectorAll: () => [], querySelector: () => null };
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {} };
const { ctx, state, store } = await import('../js/state.js');
const { pollNow, stopPolling, prepareCardsForServerRestart } = await import('../js/board.js');
const { openSession } = await import('../js/layout.js');

test('a rejected poll preserves live cards and schedules, then recovers without closing', async () => {
  const calls = [];
  let fail = true;
  const view = state.view;
  state.view = 'session';
  store.projects = [];
  store.cards = [{ id: 'live', session: 'deck-live', status: 'active', fg: 'claude' }];
  const queue = { items: [{ id: 'scheduled', session: 'deck-live' }], last_fired: {} };
  ctx.queueCache = queue;
  const infos = [{ name: 'deck-live', alive: true, fg: 'claude', agent: 'working' }];
  ctx.attention.record(store.cards, infos);
  const card = structuredClone(store.cards[0]);
  window.__TAURI__ = { core: { invoke: async cmd => {
    calls.push(cmd);
    if (cmd === 'ui_event') return;
    assert.equal(cmd, 'poll_sessions', 'poll must not close sessions or change persistence');
    if (fail) throw new Error('tmux returned a malformed pane row');
    return infos;
  } } };
  try {
    assert.equal(await pollNow(), false);
    assert.deepEqual(store.cards, [card]);
    assert.equal(ctx.queueCache, queue);
    assert.equal(ctx.attention.get(card).alive, true);
    assert.equal(ctx.attention.get(card).stale, true);
    fail = false;
    assert.equal(await pollNow(), true);
    assert.equal(store.cards.length, 1);
    assert.equal(ctx.attention.get(card).stale, false);
    assert.equal(ctx.queueCache, queue);
    assert.equal(calls.filter(cmd => cmd === 'poll_sessions').length, 2);
  } finally {
    stopPolling();
    state.view = view;
    ctx.queueCache = { items: [], last_fired: {} };
  }
});

test('restart invalidates a poll already in flight and blocks event-driven polls and reentry', async () => {
  let resolvePoll;
  const calls = [];
  window.__TAURI__ = { core: { invoke: (cmd) => {
    calls.push(cmd);
    assert.equal(cmd, 'poll_sessions');
    return new Promise(resolve => { resolvePoll = resolve; });
  } } };
  store.projects = [{ id: 'p', name: 'p', columns: [{ id: 'c', name: 'c' }] }];
  store.cards = [{ id: 'a', projectId: 'p', columnId: 'c', session: 'deck-a-0001', status: 'running' }];
  const pending = pollNow();
  const queuedFollowUp = pollNow();
  ctx.tmuxRestarting = true;
  stopPolling();
  store.cards[0].status = 'stopped';
  resolvePoll([{ name: 'deck-a-0001', alive: true, fg: 'claude', agent: 'working' }]);
  assert.equal(await pending, false);
  assert.equal(await queuedFollowUp, false);
  assert.equal(await pollNow(), false);
  assert.equal(await openSession('a'), false);
  assert.equal(store.cards[0].status, 'stopped');
  assert.deepEqual(calls, ['poll_sessions']);
  ctx.tmuxRestarting = false;
});

test('restart persists launch suppression only for the reviewed live cards', async () => {
  const writes = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    assert.equal(cmd, 'save_board'); writes.push(JSON.parse(args.data));
  } } };
  store.cards = [
    { id: 'a', projectId: 'p', columnId: 'c', title: 'A', session: 'deck-a-0001', cmd: 'claude', dir: '/tmp', launched: false },
    { id: 'b', projectId: 'p', columnId: 'c', title: 'B', session: 'deck-b-0001', cmd: 'codex', dir: '/tmp', launched: false },
  ];
  await prepareCardsForServerRestart([{ name: 'deck-a-0001' }]);
  assert.equal(store.cards[0].launched, true);
  assert.equal(store.cards[1].launched, false);
  assert.equal(writes[0].cards[0].launched, true);
});
