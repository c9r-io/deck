// Production dispatcher and Board transaction tests, using synthetic IPC only.
// These are DOM tests, not the real WKWebView/background acceptance carrier.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { drainChannel } = await import('../js/inbound.js');
const { provider } = await import('../js/board.js');
const { ctx, store } = await import('../js/state.js');
const createStarted = provider.createStarted;
const event = n => ({ id: `default/T1/E${n}/R1`, operationKey: `channel:default/T1/E${n}/R1`,
  groupKey: 'default/T1/C1/R1', connectionId: 'default', workspaceId: 'T1', eventId: `E${n}`,
  ruleId: 'R1', channelId: 'C1', messageTs: `${n}.000001`, senderUserId: 'U1', body: `incident ${n}`,
  occurredAt: 1000, target: { projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 } });

function setup(events) {
  ctx.HOME = '/tmp/isolated-home';
  store.cards = [];
  store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}', 'Follow up'] }] }];
  const state = { pending: events, calls: [], operations: new Map(), ackFailure: false, queueFailure: false, creates: 0 };
  window.__TAURI__ = { core: { invoke: async (name, args) => {
    state.calls.push([name, structuredClone(args)]);
    if (name === 'channel_pending') return structuredClone(state.pending);
    if (name === 'channel_check_pending') return state.pending.some(item => item.id === args.id);
    if (name === 'channel_ack') {
      if (state.ackFailure) throw Error('synthetic ACK failure');
      state.pending = state.pending.filter(item => item.id !== args.id);
    }
    if (name === 'channel_queue_add') {
      if (state.queueFailure) throw Error('synthetic unavailable authority');
      const old = state.operations.get(args.args.operationId);
      if (old) assert.deepEqual(args.args, old, 'replay uses the complete original intent');
      else state.operations.set(args.args.operationId, structuredClone(args.args));
    }
    if (name === 'start_session') { state.creates++; return { created: true }; }
  } } };
  provider.createStarted = createStarted;
  return state;
}

test('channel ACK follows full enqueue and durable initialQueued, including exact-card recovery', async () => {
  const state = setup([event(601)]);
  state.queueFailure = true;
  await drainChannel();
  assert.equal(store.cards.length, 1);
  assert.equal(store.cards[0].channelRun.initialQueued, false);
  assert.equal(state.calls.some(([name]) => name === 'channel_ack'), false);
  state.queueFailure = false;
  state.calls = [];
  await drainChannel();
  const ack = state.calls.findIndex(([name]) => name === 'channel_ack');
  const committed = state.calls.findIndex(([name, args]) => name === 'save_board'
    && JSON.parse(args.data).cards[0].channelRun.initialQueued === true);
  assert.ok(committed >= 0 && ack > committed);
  assert.equal(state.operations.size, 2);
  assert.equal(state.creates, 1);
});

test('collected event with failed ACK only retries ACK after stop, expiry, rule edit and restart', async () => {
  const state = setup([event(611)]);
  await drainChannel();
  state.pending = [event(612)];
  state.ackFailure = true;
  await drainChannel();
  assert.equal(store.cards[0].buffer.entries.length, 2);
  assert.equal(state.operations.size, 2, 'collected events never enter the queue');
  const id = store.cards[0].id;
  await provider.setChannelRun(id, event(612).groupKey, { collecting: false, lastCollectedAt: 0 });
  store.cards = JSON.parse(JSON.stringify(store.cards));
  state.pending[0].groupKey = 'different-after-edit';
  state.ackFailure = false;
  state.calls = [];
  await drainChannel();
  assert.deepEqual(state.calls.map(([name]) => name), ['channel_pending', 'channel_ack']);
  assert.equal(state.creates, 1);
  assert.equal(store.cards.length, 1);
  assert.equal(state.pending.length, 0);
});

test('frozen channel retry ignores observed cwd, current template and current rule option', async () => {
  const item = { ...event(621), firstSendGrant: { id: 'native-grant', digest: 'native-digest', skeleton: 'Inspect {{msg.text}}' } };
  const state = setup([item]);
  state.ackFailure = true;
  await drainChannel();
  const original = [...state.operations.values()];
  const card = store.cards[0];
  card.dir = '/tmp/observed-cwd';
  card.channelRun.initialQueued = false;
  store.projects[0].templates[0].steps = ['Changed first', 'Changed follow-up'];
  state.ackFailure = false;
  await drainChannel();
  assert.equal(state.operations.size, 2);
  assert.deepEqual([...state.operations.values()], original);
  assert.equal(original[0].dir, '/tmp');
  assert.equal(original[0].channelFirstSend.skeleton, 'Inspect {{msg.text}}');
  assert.equal(original[1].channelFirstSend, undefined);
  assert.ok(original.every(args => args.text !== 'Changed first'));
});

test('an ACK response failure after complete enqueue does not enqueue again', async () => {
  const state = setup([event(631)]);
  state.ackFailure = true;
  await drainChannel();
  assert.equal(store.cards[0].channelRun.initialQueued, true);
  state.calls = [];
  state.ackFailure = false;
  await drainChannel();
  assert.deepEqual(state.calls.map(([name]) => name), ['channel_pending', 'channel_ack']);
  assert.equal(state.operations.size, 2);
});

test('a canceled event in an already-pulled drain snapshot does not create a run', async () => {
  const state = setup([event(641)]);
  const invoke = window.__TAURI__.core.invoke;
  window.__TAURI__.core.invoke = async (name, args) => {
    if (name === 'channel_check_pending') state.pending = [];
    return invoke(name, args);
  };
  await drainChannel();
  assert.equal(state.creates, 0);
  assert.equal(store.cards.length, 0);
  assert.equal(state.operations.size, 0);
});
