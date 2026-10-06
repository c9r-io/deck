// The event bus tells listeners about something that already happened
// (state.js `emit`). These tests run the production provider over the real
// serialized persistence queue with a listener that throws.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { provider } = await import('../js/board.js');
const { emit, listeners, store } = await import('../js/state.js');
const { drainConnector } = await import('../js/connector.js');

/* the only listeners for one test: the Board's own repaint cannot run in
   this fake DOM, which is exactly a listener that throws */
const withListeners = async (fns, body) => {
  const before = [...listeners];
  listeners.clear(); fns.forEach(fn => listeners.add(fn));
  try { return await body(); } finally { listeners.clear(); before.forEach(fn => listeners.add(fn)); }
};
const backend = (extra = {}) => {
  const calls = []; const saved = []; const events = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push(cmd);
    if (cmd === 'ui_event') { events.push(args); return; }
    if (cmd in extra) return extra[cmd](args);
    if (cmd === 'save_board') { saved.push(JSON.parse(args.data)); return; }
    if (cmd === 'start_session') return { created: true, restored: false };
    throw new Error(`unexpected ${cmd}`);
  } } };
  return { calls, saved, events };
};
const board = () => {
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [];
};
const boom = () => { throw new TypeError('the repaint failed'); };

test('a listener that throws stops neither the listeners after it nor the caller, and is logged by class', () => {
  const { events } = backend();
  const got = [];
  /* two distinct functions: the bus is a Set */
  return withListeners([boom, (ev, s) => got.push([ev, s]), () => boom(), ev => got.push(ev)], async () => {
    assert.doesNotThrow(() => emit('projects', 7));
    assert.deepEqual(got, [['projects', 7], 'projects']);
    await Promise.resolve();
    assert.deepEqual(events, [
      { code: 'js-error', context: null, detail: 'TypeError', a: 0, b: 1 },
      { code: 'js-error', context: null, detail: 'TypeError', a: 0, b: 1 },
    ], 'a closed code, the error class, and b=1 for "a listener"; never the message');
    /* anything thrown is a class, never text */
    events.length = 0;
    await withListeners([() => { throw 'the user typed a secret'; }], async () => { emit('list'); });
    assert.deepEqual(events.map(event => event.detail), ['error']);
  });
});

test('a committed Board write succeeds whatever its listeners do', async () => {
  board();
  const { calls, saved } = backend();
  let later = 0;
  await withListeners([boom, () => { later++; }], async () => {
    await provider.saveTemplate('P1', 'tpl', ['one']);
    const { card, started } = await provider.createStarted({ projectId: 'P1', columnId: 'C1', title: 'x', cmd: '', dir: '/tmp' });
    assert.equal(started.created, true);
    assert.equal(store.cards[0].id, card.id);
  });
  assert.deepEqual(saved[0].projects[0].templates, [{ name: 'tpl', steps: ['one'] }]);
  assert.equal(saved[1].cards.length, 1);
  assert.equal(calls.includes('kill_session'), false, 'nothing is rolled back');
  assert.equal(later, 2, 'the listener after the failing one heard both writes');
});

test('a write that was NOT persisted still fails, and nobody is told it happened', async () => {
  board();
  const { calls } = backend({ save_board: () => { throw new Error('disk full'); }, kill_session: () => {} });
  let told = 0;
  await withListeners([() => { told++; }], async () => {
    await assert.rejects(provider.saveTemplate('P1', 'tpl', ['one']), /disk full/);
    await assert.rejects(provider.createStarted({ projectId: 'P1', columnId: 'C1', title: 'x', cmd: '', dir: '/tmp' }), /disk full/);
  });
  assert.equal(told, 0);
  assert.equal(store.cards.length, 0);
  assert.equal(store.projects[0].templates, undefined);
  assert.equal(calls.filter(cmd => cmd === 'kill_session').length, 1, 'the session the failed creation started is killed, as before');
});

test('a phone task whose card cannot be repainted is still applied and its plan queued', async () => {
  board();
  store.projects[0].presets = [{ id: 'R1', name: 'Task', columnId: 'C1', title: 'Remote', dir: '/tmp', cmd: 'claude',
    steps: ['first', 'second'], firstSend: true }];
  const handle = 'f'.repeat(64);
  const pending = { handle, request: { id: 'remote-9', kind: 'task-create', expectedRevision: 'rev', payload: { projectId: 'P1', presetId: 'R1' } } };
  let items = [pending]; const states = []; const rows = [];
  backend({
    connector_pending: () => { const out = items; items = []; return out; },
    connector_claim: () => pending,
    connector_validate: () => true,
    connector_complete: args => { states.push(args.state); },
    channel_queue_add: args => { rows.push(args.args); },
  });
  await withListeners([boom], () => drainConnector());
  assert.deepEqual(states, ['applied'], 'not ambiguous: the card was created and saved');
  assert.deepEqual(rows.map(row => row.mode), ['at', 'chain']);
  assert.deepEqual(rows[0].firstSend, { rule: 'R1', event: handle, presetProject: 'P1' });
  assert.equal(store.cards[0].connectorRun.initialQueued, true);
});
