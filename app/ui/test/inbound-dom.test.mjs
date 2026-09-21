// Exercise the production dispatcher as well as the pure planner. This module
// used to be absent from coverage despite not being on the exclusion list.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument; globalThis.window = { __TAURI__: null };
const { drainInbound, initInbound } = await import('../js/inbound.js');
const { provider } = await import('../js/board.js');
const { store } = await import('../js/state.js');
const item = { id: 'item-1', event: { source: 'slack', key: 'C9/1.2', badge: 'deck', text: 'sample', from: 'tester', where: '#test' },
  rule: { id: 'rule-1', projectId: 'P1', columnId: 'C1', template: 'triage', cmd: 'claude', dir: '/tmp' } };
function setup(items, fail = '', steps = ['first', 'second']) {
  const calls = []; let pending = items, handler;
  store.cards = []; store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps }] }];
  provider.create = async card => { calls.push(['create', card]); if (fail === 'create') throw Error('failed'); return { ...card, id: 'S1', session: 'deck-test' }; };
  window.__TAURI__ = {
    event: { listen: async (_name, fn) => { handler = fn; } },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]); if (cmd === fail) throw Error('failed');
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
    assert.equal(f.calls.at(-1)[1].card, 'S1');
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
  for (const [cmd, steps] of [['', ['first']], ['claude --yolo', ['first']], ['claude', ['{{msg.text}}']]]) {
    const f = setup([{ ...item, rule: { ...item.rule, cmd } }], '', steps);
    await drainInbound();
    assert.equal(f.calls.find(([name]) => name === 'inbound_ack')[1].outcome, 'skipped');
    assert.equal(f.calls.filter(([name]) => name === 'create' || name.includes('queue_add')).length, 0);
  }
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

test('poll and ack failures release the dispatcher; partial queue failure still names the created card', async () => {
  for (const fail of ['inbound_pending', 'inbound_ack', 'channel_queue_add']) {
    const f = setup([item], fail); await drainInbound();
    if (fail === 'channel_queue_add') assert.equal(f.calls.find(([cmd]) => cmd === 'inbound_ack')[1].card, 'S1');
    const retry = setup([]); await drainInbound(); assert.equal(retry.calls[0][0], 'inbound_pending');
  }
});
