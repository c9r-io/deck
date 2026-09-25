// Exercise the production dispatcher as well as the pure planner. This module
// used to be absent from coverage despite not being on the exclusion list.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument; globalThis.window = { __TAURI__: null };
const { drainInbound, initInbound } = await import('../js/inbound.js');
const { provider } = await import('../js/board.js');
const { listeners, store } = await import('../js/state.js');
const realQueueInboundPlan = provider.queueInboundPlan;
const item = { id: 'item-1', event: { source: 'slack', key: 'C9/1.2', badge: 'deck', text: 'sample', from: 'tester', where: '#test' },
  rule: { id: 'rule-1', projectId: 'P1', columnId: 'C1', template: 'triage', cmd: 'claude', dir: '/tmp' } };
function setup(items, fail = '', steps = ['first', 'second']) {
  const calls = []; let pending = items, handler;
  store.cards = []; store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps }] }];
  provider.create = async card => {
    calls.push(['create', card]); if (fail === 'create') throw Error('failed');
    const created = { ...card, session: 'deck-test' };
    store.cards.push(created); return created;
  };
  provider.queueInboundPlan = async (sid, key) => {
    const card = store.cards.find(value => value.id === sid && value.origin.key === key);
    if (!card || card.inboundPlan.initialQueued) return !!card;
    const plan = card.inboundPlan;
    if (plan.reviewEach) await window.__TAURI__.core.invoke(card.origin.source === 'slack'
      ? 'channel_queue_add_reviewed_list' : 'queue_add_reviewed_list', {
      args: { operationId: plan.operationId }, texts: plan.initialSteps.map(step => step.text),
    });
    else for (const step of plan.initialSteps) await window.__TAURI__.core.invoke(card.origin.source === 'slack'
      ? 'channel_queue_add' : 'queue_add', { args: step });
    plan.initialQueued = true;
    plan.initialSteps = [];
    return true;
  };
  window.__TAURI__ = {
    event: { listen: async (_name, fn) => { handler = fn; } },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]); if (cmd === fail) throw Error('failed');
      if (fail === 'second-channel-add' && cmd === 'channel_queue_add'
        && calls.filter(([name]) => name === cmd).length === 2) throw Error('failed');
      if (cmd === 'inbound_pending') { const next = pending; pending = []; return next; }
    } },
  };
  return { calls, handler: () => handler };
}

test('dispatcher preserves reviewed-list atomicity and acks only after enqueue', async () => {
  for (const reviewEach of [false, true]) {
    const f = setup([{ ...item, rule: { ...item.rule, reviewEach } }]);
    initInbound(); await f.handler()();
    const queued = f.calls.filter(([cmd]) => cmd === (reviewEach ? 'channel_queue_add_reviewed_list' : 'channel_queue_add'));
    assert.equal(queued.length, reviewEach ? 1 : 2);
    assert.equal(f.calls.at(-1)[0], 'inbound_ack');
    assert.equal(f.calls.at(-1)[1].card, store.cards[0].id);
    if (reviewEach) assert.deepEqual(queued[0][1].texts, ['first', 'second']);
    else assert.deepEqual(queued.map(([, args]) => args.args.mode), ['at', 'chain']);
  }
});

test('a clock run queues its own template through the ordinary queue', async () => {
  const clock = { id: 'item-2', event: { source: 'clock', key: '1700000000', badge: 'rule-1' },
    rule: { ...item.rule, cmd: '' } };
  const f = setup([clock]); await drainInbound();
  assert.deepEqual(f.calls.filter(([cmd]) => cmd.includes('queue_add')).map(([cmd]) => cmd), ['queue_add', 'queue_add']);
});

test('a badge rule that fails the channel admission is skipped before any card exists', async () => {
  for (const [cmd, steps] of [['', ['first']], ['claude;zsh', ['first']], ['claude', ['{{msg.text}}']]]) {
    const f = setup([{ ...item, rule: { ...item.rule, cmd } }], '', steps);
    await drainInbound();
    assert.equal(f.calls.find(([name]) => name === 'inbound_ack')[1].outcome, 'skipped');
    assert.equal(f.calls.filter(([name]) => name === 'create' || name.includes('queue_add')).length, 0);
  }
});

test('a badge rule with agent flags creates a card and queues through the external gate', async () => {
  const f = setup([{ ...item, rule: { ...item.rule, cmd: 'codex --yolo' } }]);
  await drainInbound();
  assert.equal(f.calls.find(([name]) => name === 'create')[1].cmd, 'codex --yolo');
  assert.equal(f.calls.filter(([name]) => name === 'channel_queue_add').length, 2);
  assert.equal(store.cards[0].inboundPlan.initialQueued, true);
  assert.equal(f.calls.at(-1)[0], 'inbound_ack');
});

test('dispatcher refuses dangling targets, skips duplicates and leaves failed creation pending', async () => {
  for (const kind of ['target', 'template', 'duplicate', 'create']) {
    const value = structuredClone(item);
    if (kind === 'target') value.rule.projectId = 'missing';
    if (kind === 'template') value.rule.template = 'missing';
    const f = setup([value], kind === 'create' ? 'create' : '');
    if (kind === 'duplicate') store.cards = [{ origin: { source: 'slack', key: item.event.key, badge: 'deck' } }];
    await drainInbound();
    const ack = f.calls.find(([cmd]) => cmd === 'inbound_ack');
    assert.equal(!!ack, kind !== 'create');
    assert.equal(f.calls.filter(([cmd]) => cmd.includes('queue_add')).length, 0);
    if (kind !== 'create') assert.equal(f.calls.filter(([cmd]) => cmd === 'create').length, 0);
  }
});

test('poll and ack failures release the dispatcher; queue failure keeps the plan and event pending', async () => {
  for (const fail of ['inbound_pending', 'inbound_ack', 'channel_queue_add']) {
    const f = setup([item], fail); await drainInbound();
    if (fail === 'channel_queue_add') {
      assert.equal(f.calls.some(([cmd]) => cmd === 'inbound_ack'), false);
      assert.equal(store.cards[0].inboundPlan.initialQueued, false);
    }
    const retry = setup([]); await drainInbound(); assert.equal(retry.calls[0][0], 'inbound_pending');
  }
});

test('a failed badge enqueue resumes the same card and acks only after the frozen plan is queued', async () => {
  const failed = setup([item], 'channel_queue_add');
  await drainInbound();
  const card = store.cards[0];
  assert.equal(card.inboundPlan.initialQueued, false);
  assert.equal(failed.calls.some(([name]) => name === 'inbound_ack'), false);
  const retry = setup([item]);
  store.cards = [card];
  await drainInbound();
  assert.equal(retry.calls.some(([name]) => name === 'create'), false);
  assert.equal(retry.calls.filter(([name]) => name === 'channel_queue_add').length, 2);
  assert.equal(card.inboundPlan.initialQueued, true);
  assert.equal(retry.calls.at(-1)[0], 'inbound_ack');
  assert.equal(retry.calls.at(-1)[1].card, card.id);
});

test('the Board queue transaction preserves a frozen plan after a partial backend write', async () => {
  const savedListeners = [...listeners]; listeners.clear();
  try {
    const failed = setup([item], 'second-channel-add');
    provider.queueInboundPlan = realQueueInboundPlan;
    await drainInbound();
    const card = store.cards[0];
    assert.equal(card.inboundPlan.initialQueued, false);
    assert.equal(failed.calls.filter(([name]) => name === 'channel_queue_add').length, 2);
    assert.equal(failed.calls.some(([name]) => name === 'inbound_ack'), false);
    const originalIds = card.inboundPlan.initialSteps.map(step => step.operationId);
    const retry = setup([item]);
    provider.queueInboundPlan = realQueueInboundPlan;
    store.cards = [card];
    await drainInbound();
    assert.equal(store.cards[0].inboundPlan.initialQueued, true);
    assert.deepEqual(retry.calls.filter(([name]) => name === 'channel_queue_add')
      .map(([, args]) => args.args.operationId), originalIds);
    assert.equal(retry.calls.filter(([name]) => name === 'create').length, 0);
    assert.equal(retry.calls.filter(([name]) => name === 'inbound_ack').length, 1);
  } finally {
    listeners.clear(); for (const listener of savedListeners) listeners.add(listener);
  }
});

test('a legacy staged Channel inbox item drains without any Slack credentials', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const staged = { id: 'default/T1/E1/rule', operationKey: 'channel:default/T1/E1/rule', groupKey: 'default/T1/C1/rule',
    connectionId: 'default', workspaceId: 'T1', eventId: 'E1', ruleId: 'rule', channelId: 'C1',
    messageTs: '1.0', occurredAt: Math.floor(Date.now() / 1000), senderUserId: 'U1', body: 'incident',
    target: { projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 } };
  store.cards = [];
  store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}'] }] }];
  const calls = [];
  provider.createStarted = async card => {
    calls.push('board-persist');
    const saved = { ...card, session: 'deck-test' };
    store.cards.push(saved);
    return { card: saved };
  };
  provider.queueChannelPlan = async () => { calls.push('queue'); return true; };
  window.__TAURI__ = { core: { invoke: async cmd => {
    calls.push(cmd);
    if (cmd === 'channel_pending') return [staged];
  } } };
  // A second pending read terminates the drain; the staged item has already
  // been acknowledged only after the mocked durable Board transaction.
  let read = false;
  window.__TAURI__.core.invoke = async cmd => {
    calls.push(cmd);
    if (cmd === 'channel_pending') { if (read) return []; read = true; return [staged]; }
  };
  await drainChannel();
  assert.ok(calls.indexOf('board-persist') >= 0, calls.join(','));
  assert.ok(calls.indexOf('channel_ack') > calls.indexOf('board-persist'), calls.join(','));
  assert.equal(store.cards[0].origin.source, 'channel');
});
