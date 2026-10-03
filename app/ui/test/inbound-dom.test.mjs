// Exercise the production dispatcher as well as the pure planner. This module
// used to be absent from coverage despite not being on the exclusion list.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument; globalThis.window = { __TAURI__: null };
const { drainInbound, startInbound } = await import('../js/inbound.js');
const { provider } = await import('../js/board.js');
const { listeners, store } = await import('../js/state.js');
const realQueueInboundPlan = provider.queueInboundPlan;
const realCreate = provider.create;
const item = { id: 'item-1', event: { source: 'slack', key: 'C9/1.2', badge: 'deck', text: 'sample', from: 'tester', where: '#test' },
  rule: { id: 'rule-1', projectId: 'P1', columnId: 'C1', template: 'triage', cmd: 'claude', dir: '/tmp' } };
function setup(items, fail = '', steps = ['first', 'second']) {
  const calls = [], heard = {}; let pending = items, handler;
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
    const external = card.origin.source !== 'clock';
    if (plan.reviewEach) await window.__TAURI__.core.invoke(external
      ? 'channel_queue_add_reviewed_list' : 'queue_add_reviewed_list', {
      args: { operationId: plan.operationId }, texts: plan.initialSteps.map(step => step.text),
    });
    else for (const step of plan.initialSteps) await window.__TAURI__.core.invoke(external
      ? 'channel_queue_add' : 'queue_add', { args: step });
    plan.initialQueued = true;
    plan.initialSteps = [];
    return true;
  };
  window.__TAURI__ = {
    event: { listen: async (name, fn) => { heard[name] = fn; handler = fn; } },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]); if (cmd === fail) throw Error('failed');
      if (fail === 'second-channel-add' && cmd === 'channel_queue_add'
        && calls.filter(([name]) => name === cmd).length === 2) throw Error('failed');
      if (cmd === 'inbound_pending') { const next = pending; pending = []; return next; }
    } },
  };
  return { calls, handler: () => handler, heard };
}

test('nothing is pulled until startInbound; it then listens and drains what was pending', async () => {
  const f = setup([item]);
  // app.js starts this module only once the webview holds the user's Board (a
  // Board that loaded, or the lost Board's way out). Until then the item waits
  // in the backend: nothing listens for it and nothing asks for it.
  await new Promise(resolve => setTimeout(resolve, 5));
  assert.deepEqual([Object.keys(f.heard), f.calls], [[], []]);
  await startInbound();
  assert.deepEqual(Object.keys(f.heard).sort(), ['channel-changed', 'inbound-changed']);
  // the first drains took what was pending all along: each inbox asked once,
  // the badge's card created, its plan queued, then the acknowledgement
  assert.deepEqual(['inbound_pending', 'channel_pending'].map(name => f.calls.filter(([cmd]) => cmd === name).length), [1, 1]);
  assert.equal(store.cards.length, 1);
  assert.deepEqual([f.calls.at(-1)[0], f.calls.at(-1)[1].outcome], ['inbound_ack', 'done']);
  // a later announcement is pulled by the listener it registered
  const later = setup([{ ...item, id: 'item-later', event: { ...item.event, key: 'C9/9.9' } }]);
  await f.heard['inbound-changed']();
  assert.deepEqual([later.calls.at(-1)[0], later.calls.at(-1)[1].id], ['inbound_ack', 'item-later']);
  await f.heard['channel-changed']();
  assert.equal(later.calls.filter(([cmd]) => cmd === 'channel_pending').length, 1);
});

test('dispatcher preserves reviewed-list atomicity and acks only after enqueue', async () => {
  for (const reviewEach of [false, true]) {
    const f = setup([{ ...item, rule: { ...item.rule, reviewEach } }]);
    await startInbound();
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

test('an approved badge rule freezes its approval and claims each step on the external path only', async () => {
  const { approveRule } = await import('../js/automation-model.js');
  const savedListeners = [...listeners]; listeners.clear();
  try {
    const steps = ['Review the change.', 'Investigate {{msg.text}}', 'Run the tests.'];
    const rule = await approveRule({ ...item.rule, source: 'slack', badge: 'deck' },
      { name: 'triage', steps }, { external: true });
    for (const reviewEach of [false, true]) {
      const approved = await approveRule({ ...rule, reviewEach }, { name: 'triage', steps }, { external: true });
      const f = setup([{ ...item, rule: approved }], '', steps);
      provider.queueInboundPlan = realQueueInboundPlan;
      await drainInbound();
      const plan = store.cards[0].inboundPlan;
      /* the frozen proof material: the event key and each bounded step's
         approved skeleton (fixed steps carry none) */
      assert.deepEqual(plan.authority, { rule: 'rule-1', grant: approved.autoSend.digest, trigger: 'slack-badge',
        classes: ['fixed', 'bounded', 'fixed'], event: 'C9/1.2', skeletons: [null, 'Investigate {{msg.text}}', null] });
      if (reviewEach) {
        const [[, args]] = f.calls.filter(([name]) => name === 'channel_queue_add_reviewed_list');
        assert.deepEqual(args.args.authority, { rule: 'rule-1', grant: approved.autoSend.digest, step: 0,
          event: 'C9/1.2', skeletons: [null, 'Investigate {{msg.text}}', null] });
      } else {
        const claims = f.calls.filter(([name]) => name === 'channel_queue_add').map(([, args]) => args.args.authority);
        assert.deepEqual(claims.map(claim => claim.step), [0, 1, 2]);
        assert.deepEqual(claims.map(claim => claim.skeletons), [[null], ['Investigate {{msg.text}}'], [null]]);
        assert.ok(claims.every(claim => claim.grant === approved.autoSend.digest && claim.event === 'C9/1.2'));
      }
      assert.equal(f.calls.filter(([name]) => name === 'queue_add' || name === 'queue_add_reviewed_list').length, 0);
    }
    // a template edited after approval: no approval is frozen, rows stay manual
    const f = setup([{ ...item, rule }], '', ['Review the change.', 'Something else']);
    provider.queueInboundPlan = realQueueInboundPlan;
    await drainInbound();
    assert.equal('authority' in store.cards[0].inboundPlan, false);
    assert.ok(f.calls.filter(([name]) => name === 'channel_queue_add').every(([, args]) => !('authority' in args.args)));
    // a clock run never claims one, even with an approval-shaped field
    const clock = { id: 'item-3', event: { source: 'clock', key: '1700000001', badge: 'rule-1' },
      rule: { ...item.rule, cmd: '', autoSend: rule.autoSend } };
    const c = setup([clock], '', steps);
    provider.queueInboundPlan = realQueueInboundPlan;
    await drainInbound();
    assert.ok(c.calls.filter(([name]) => name === 'queue_add').every(([, args]) => !('authority' in args.args)));
  } finally {
    provider.queueInboundPlan = realQueueInboundPlan;
    listeners.clear(); for (const listener of savedListeners) listeners.add(listener);
  }
});

test('a badge rule that accepted the first-send risk claims it on the head row only', async () => {
  const savedListeners = [...listeners]; listeners.clear();
  try {
    for (const reviewEach of [false, true]) {
      const f = setup([{ ...item, rule: { ...item.rule, source: 'slack', badge: 'deck', reviewEach,
        firstSendWithoutReadiness: true } }], '', ['first', 'second', 'third']);
      provider.queueInboundPlan = realQueueInboundPlan;
      await drainInbound();
      assert.deepEqual(store.cards[0].inboundPlan.firstSend, { rule: 'rule-1' });
      assert.equal('authority' in store.cards[0].inboundPlan, false, 'independent of the follow-up approval');
      if (reviewEach) {
        const [[, args]] = f.calls.filter(([name]) => name === 'channel_queue_add_reviewed_list');
        assert.deepEqual(args.args.firstSend, { rule: 'rule-1', event: 'C9/1.2' });
      } else {
        const claims = f.calls.filter(([name]) => name === 'channel_queue_add').map(([, args]) => args.args.firstSend);
        assert.deepEqual(claims, [{ rule: 'rule-1', event: 'C9/1.2' }, undefined, undefined]);
      }
    }
    // default (legacy) rule: nothing frozen, nothing claimed
    const f = setup([{ ...item, rule: { ...item.rule, source: 'slack', badge: 'deck' } }]);
    provider.queueInboundPlan = realQueueInboundPlan;
    await drainInbound();
    assert.equal('firstSend' in store.cards[0].inboundPlan, false);
    assert.ok(f.calls.filter(([name]) => name === 'channel_queue_add').every(([, args]) => !('firstSend' in args.args)));
    // a clock run never claims it, even with the field present
    const clock = { id: 'item-4', event: { source: 'clock', key: '1700000002', badge: 'rule-1' },
      rule: { ...item.rule, cmd: '', firstSendWithoutReadiness: true } };
    const c = setup([clock]);
    provider.queueInboundPlan = realQueueInboundPlan;
    await drainInbound();
    assert.equal('firstSend' in store.cards[0].inboundPlan, false);
    assert.ok(c.calls.filter(([name]) => name === 'queue_add').every(([, args]) => !('firstSend' in args.args)));
  } finally {
    provider.queueInboundPlan = realQueueInboundPlan;
    listeners.clear(); for (const listener of savedListeners) listeners.add(listener);
  }
});

/* Which commands a frozen plan's rows enter through is decided from the
   card's origin alone, in the webview: the backend's owner commands cannot
   tell whose text they are given. Only a clock run's rows are the rule
   owner's own; every other origin, a source added later included, must take
   the external commands, which hold the native admission. */
test('only a clock run takes the owner commands; every other origin takes the external ones', async () => {
  const savedListeners = [...listeners]; listeners.clear();
  try {
    const step = (n, mode) => ({ operationId: `B${n}`, text: `step ${n}`, mode, at: mode === 'at' ? 1 : null,
      tpl: 'triage', tplIdx: n, tplTotal: 2 });
    const route = async (source, reviewEach) => {
      const f = setup([]);
      provider.queueInboundPlan = realQueueInboundPlan;
      store.cards = [{ id: 'S-route', projectId: 'P1', columnId: 'C1', title: 'route', session: 'deck-test', cmd: 'claude', dir: '/tmp',
        origin: { ...(source === undefined ? {} : { source }), key: 'K1', badge: 'deck' },
        inboundPlan: { operationId: 'B0', reviewEach, initialSteps: [step(1, 'at'), step(2, 'chain')], initialQueued: false } }];
      assert.equal(await provider.queueInboundPlan('S-route', 'K1'), true);
      assert.equal(store.cards[0].inboundPlan.initialQueued, true);
      return f.calls.map(([cmd]) => cmd).filter(cmd => cmd.includes('queue_add'));
    };
    // the two sources there are today, unchanged
    assert.deepEqual(await route('clock', false), ['queue_add', 'queue_add']);
    assert.deepEqual(await route('clock', true), ['queue_add_reviewed_list']);
    assert.deepEqual(await route('slack', false), ['channel_queue_add', 'channel_queue_add']);
    assert.deepEqual(await route('slack', true), ['channel_queue_add_reviewed_list']);
    // anything else is not known to be the owner's text: another kind of card,
    // a source added later, a near miss, a missing or malformed value
    for (const source of ['channel', 'connector', 'mcp', 'a-source-added-later', 'Clock', '', undefined, null, 42]) {
      assert.deepEqual(await route(source, false), ['channel_queue_add', 'channel_queue_add'], String(source));
      assert.deepEqual(await route(source, true), ['channel_queue_add_reviewed_list'], String(source));
    }
  } finally {
    provider.queueInboundPlan = realQueueInboundPlan;
    listeners.clear(); for (const listener of savedListeners) listeners.add(listener);
  }
});

/* The head of a clock run with the first-send option, pinned on both sides.
   The backend admits that head by reading the persisted card and the call's
   arguments as untyped JSON, key by key (scheduler/first_send.rs
   `clock_head_matches`); nothing else ties those keys to what this side
   writes. fixtures/clock-first-send.json is that tie: this test proves it is
   exactly what the real dispatcher persists and queues, and
   scheduler/tests.rs that the backend accepts exactly it. A renamed key
   fails here first, and there once the fixture follows. */
test('the clock first-send fixture is what the dispatcher persists and queues', async () => {
  const { readFileSync } = await import('node:fs');
  const fixture = JSON.parse(readFileSync(new URL('./fixtures/clock-first-send.json', import.meta.url), 'utf8'));
  const { INBOUND_SOURCES, normalizeInbound } = await import('../js/settings-model.js');
  const { withFirstSend } = await import('../js/automation-model.js');
  const savedListeners = [...listeners]; listeners.clear();
  const realNow = Date.now;
  try {
    for (const [name, run] of Object.entries(fixture.runs)) {
      assert.deepEqual(normalizeInbound({ rules: [run.rule] }).rules, [run.rule], `${name}: the rule is in its saved form`);
      const calls = [];
      let pending = [{ id: `item-${name}`, event: fixture.event, rule: run.rule }];
      store.cards = []; store.projects = [structuredClone(fixture.project)];
      provider.create = realCreate; provider.queueInboundPlan = realQueueInboundPlan;
      window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (cmd, args) => {
        calls.push([cmd, structuredClone(args)]);
        if (cmd === 'inbound_pending') { const next = pending; pending = []; return next; }
      } } };
      Date.now = () => fixture.now * 1000;
      await drainInbound();
      Date.now = realNow;
      // the Board as it was on disk when the head row was queued
      const queued = calls.findIndex(([cmd]) => cmd.includes('queue_add'));
      const saved = calls.slice(0, queued).filter(([cmd]) => cmd === 'save_board').at(-1);
      const card = JSON.parse(saved[1].data).cards[0];
      const [command, payload] = calls[queued];
      // generated values, mapped to the fixture's fixed ones wherever they occur
      const ids = new Map([[card.id, run.card.id], [card.session, run.card.session],
        [card.inboundPlan.operationId, run.card.inboundPlan.operationId],
        ...card.inboundPlan.initialSteps.map((step, index) => [step.operationId, run.card.inboundPlan.initialSteps[index].operationId])]);
      assert.equal(ids.size, 3 + card.inboundPlan.initialSteps.length, 'the generated ids are distinct');
      const mapped = value => (typeof value === 'string' && ids.has(value) ? ids.get(value)
        : Array.isArray(value) ? value.map(mapped)
        : value && typeof value === 'object' ? Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, mapped(inner)]))
        : value);
      assert.match(card.title, /^Daily summary · \d{2}-\d{2}$/, 'the title ends in the run\'s local date');
      assert.deepEqual({ ...mapped(card), title: run.card.title }, run.card, `${name}: the persisted card`);
      assert.deepEqual({ command, ...mapped(payload) }, run.call, `${name}: the head row's call`);
    }
    // one list says which rule sources may carry the option
    const slack = { ...fixture.runs.plain.rule, id: 'rslack1', source: 'slack', badge: 'eyes' };
    for (const source of [...INBOUND_SOURCES, 'channel', 'connector']) {
      const listed = fixture.firstSendSources.includes(source);
      assert.equal(withFirstSend({ source }, true).firstSendWithoutReadiness === true, listed, source);
      const saved = normalizeInbound({ rules: [source === 'clock' ? fixture.runs.plain.rule : { ...slack, source }] }).rules[0];
      assert.equal(saved?.firstSendWithoutReadiness === true, listed, `${source}: kept by the settings reader`);
    }
  } finally {
    Date.now = realNow;
    provider.create = realCreate; provider.queueInboundPlan = realQueueInboundPlan;
    listeners.clear(); for (const listener of savedListeners) listeners.add(listener);
  }
});

test('a legacy staged Channel inbox item drains without any Slack credentials', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const { removeLegacySlackCredentials } = await import('../js/slack-legacy-cleanup.js');
  const staged = { id: 'default/T1/E1/rule', operationKey: 'channel:default/T1/E1/rule', groupKey: 'default/T1/C1/rule',
    connectionId: 'default', workspaceId: 'T1', eventId: 'E1', ruleId: 'rule', channelId: 'C1',
    messageTs: '1.0', occurredAt: Math.floor(Date.now() / 1000), senderUserId: 'U1', body: 'incident',
    target: { projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 } };
  store.cards = [];
  store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}'] }] }];
  const calls = [];
  const credentials = { legacyBot: true, legacyApp: true, canonicalUser: false, canonicalBot: false, canonicalApp: false };
  await removeLegacySlackCredentials({
    confirm: async () => true,
    invoke: async command => {
      calls.push(command);
      credentials.legacyBot = false;
      credentials.legacyApp = false;
    },
    refresh: async () => calls.push('refresh'),
    notice: () => {},
  });
  assert.deepEqual(credentials, { legacyBot: false, legacyApp: false, canonicalUser: false, canonicalBot: false, canonicalApp: false });
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
  assert.ok(calls.indexOf('channel_pending') > calls.indexOf('slack_legacy_credentials_clear'), calls.join(','));
  assert.ok(calls.indexOf('board-persist') >= 0, calls.join(','));
  assert.ok(calls.indexOf('channel_ack') > calls.indexOf('board-persist'), calls.join(','));
  assert.equal(store.cards[0].origin.source, 'channel');
});

/* ---------- a pending channel event that cannot be placed ---------- */
const channelEvent = (n, target = {}) => ({ id: `default/T1/E${n}/rule`, operationKey: `channel:default/T1/E${n}/rule`,
  groupKey: 'default/T1/C1/rule', connectionId: 'default', workspaceId: 'T1', eventId: `E${n}`, ruleId: 'rule',
  channelId: 'C1', messageTs: `${n}.0`, occurredAt: Math.floor(Date.now() / 1000), senderUserId: 'U1', body: 'incident',
  target: { projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30, ...target } });
function channelInbox(events) {
  const inbox = { pending: events, acks: [], creates: 0 };
  store.cards = [];
  store.projects = [{ id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}'] }] }];
  provider.createStarted = async card => { inbox.creates += 1; const saved = { ...card, session: 'deck-test' }; store.cards.push(saved); return { card: saved }; };
  provider.queueChannelPlan = async () => true;
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'channel_pending') return inbox.pending;
    if (cmd === 'channel_ack') { inbox.acks.push(args.id); inbox.pending = inbox.pending.filter(event => event.id !== args.id); }
  } } };
  return inbox;
}
const toastTexts = () => fakeDocument.getElementById('toasts').children.map(el => el.textContent);

test('a channel event whose target is gone is announced once per run and stays pending', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const inbox = channelInbox([channelEvent(101, { columnId: 'GONE' })]);
  const before = toastTexts().length;
  // boot, a `channel-changed`, and the 60 s tick are all this same call
  await drainChannel(); await drainChannel(); await drainChannel();
  const shown = toastTexts().slice(before);
  assert.equal(shown.length, 1, shown.join(' | '));
  assert.match(shown[0], /project or group is missing.*remains pending/);
  assert.deepEqual([inbox.acks, inbox.pending.length, inbox.creates], [[], 1, 0], 'nothing acknowledged, nothing created');
  // a later event of the same broken rule is news once; the first stays silent
  inbox.pending = [...inbox.pending, channelEvent(102, { columnId: 'GONE' })];
  await drainChannel(); await drainChannel();
  assert.equal(toastTexts().length - before, 2);
  assert.equal(inbox.pending.length, 2);
});

test('events that share a reason make one toast per drain; each closed reason has its sentence', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const inbox = channelInbox([
    channelEvent(201, { columnId: 'GONE' }), channelEvent(202, { columnId: 'GONE' }), channelEvent(203, { columnId: 'GONE' }),
    channelEvent(204, { template: 'missing' }), channelEvent(205, { cmd: 'claude;zsh' }),
  ]);
  const before = toastTexts().length;
  await drainChannel();
  const shown = toastTexts().slice(before);
  assert.equal(shown.length, 3, shown.join(' | '));
  assert.match(shown[0], /project or group is missing/);
  assert.match(shown[1], /template is missing or empty/);
  assert.match(shown[2], /blocked: use claude or codex/);
  await drainChannel(); await drainChannel();
  assert.equal(toastTexts().length - before, 3, 'every one of the five was announced by the first drain');
  assert.deepEqual([inbox.acks, inbox.pending.length], [[], 5]);
});

test('a toast nobody could see is not counted: the first visible drain announces the event', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  channelInbox([channelEvent(301, { columnId: 'GONE' })]);
  const before = toastTexts().length;
  fakeDocument.hidden = true;
  try {
    await drainChannel(); await drainChannel();
    assert.equal(toastTexts().length - before, 0, 'hidden page: nothing shown');
  } finally { fakeDocument.hidden = false; }
  await drainChannel(); await drainChannel();
  assert.equal(toastTexts().length - before, 1);
});

test('a channel event whose project or group is gone says so, whatever else is wrong with it', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  // without its project no template can be found either; that is not the reason to give
  const gone = [[501, { projectId: 'GONE' }], [502, { projectId: 'GONE', cmd: 'claude;zsh' }],
    [503, { columnId: 'GONE', template: 'missing' }], [504, { columnId: 'GONE' }]];
  for (const [n, target] of gone) {
    channelInbox([channelEvent(n, target)]);
    const before = toastTexts().length;
    await drainChannel();
    const shown = toastTexts().slice(before);
    assert.equal(shown.length, 1, JSON.stringify(target));
    assert.match(shown[0], /project or group is missing.*remains pending/, JSON.stringify(target));
  }
  // with both in place the narrower reason is the one given
  const narrower = [[511, { template: 'missing' }, /template is missing or empty/], [512, { cmd: 'claude;zsh' }, /blocked: use claude or codex/]];
  for (const [n, target, sentence] of narrower) {
    channelInbox([channelEvent(n, target)]);
    const before = toastTexts().length;
    await drainChannel();
    assert.match(toastTexts().slice(before).join(' | '), sentence, JSON.stringify(target));
  }
});

test('a card that cannot be created is announced once; the event is tried again at every drain', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const inbox = channelInbox([channelEvent(351)]);
  let attempts = 0;
  provider.createStarted = async () => { attempts += 1; throw Error('tmux'); };
  const before = toastTexts().length;
  await drainChannel(); await drainChannel(); await drainChannel();
  const shown = toastTexts().slice(before);
  assert.equal(shown.length, 1, shown.join(' | '));
  assert.match(shown[0], /could not be created.*remains pending/);
  assert.deepEqual([attempts, inbox.acks, inbox.pending.length], [3, [], 1], 'silence is not giving up');
});

test('a placeable channel event is created and acknowledged without a pending notice', async () => {
  const { drainChannel } = await import('../js/inbound.js');
  const inbox = channelInbox([channelEvent(401)]);
  const before = toastTexts().length;
  await drainChannel();
  assert.deepEqual([inbox.acks, inbox.creates, inbox.pending.length], [['default/T1/E401/rule'], 1, 0]);
  assert.equal(toastTexts().length - before, 0);
});
