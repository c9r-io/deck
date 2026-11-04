// Preset follow-ups: what a template offers and a run freezes (pure.js,
// follow-up-model.js), what the bar paints (follow-up-bar.js) and what a
// click queues (board.js `queueFollowUp`). The WKWebView attention smoke
// clicks a real pane's button and sees the row sent (`attention-follow-*`).
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {}, dispatchEvent() {} };
/* the queue refresh after a click looks for list rows: there are none here */
fakeDocument.querySelectorAll = () => [];
fakeDocument.querySelector = () => null;

const { ctx, state, store, listeners } = await import('../js/state.js');
const { FOLLOWUPS_MAX, followUpProblem, templateFollowUps, planInbound } = await import('../js/pure.js');
const { cardFollowUps, followUpQueued, followUpSuggestion } = await import('../js/follow-up-model.js');
const { channelTemplatePlan } = await import('../js/channel-model.js');
const { createAttentionTracker } = await import('../js/attention-model.js');
const { paintFollowUps } = await import('../js/follow-up-bar.js');
const { provider, queueFollowUp } = await import('../js/board.js');
const { initScheduler } = await import('../js/scheduler.js');
const { t } = await import('../js/i18n.js');
listeners.clear();
initScheduler({ provider, pollNow: async () => {}, refreshAttention: () => {}, ruleOf: () => null });

test('a follow-up is the owner\'s own text: normalized, never a placeholder, at most three', () => {
  assert.equal(followUpProblem('  Commit and push.  '), null);
  assert.equal(followUpProblem(' \n '), 'empty');
  assert.equal(followUpProblem('Reply to {{msg.from}}'), 'placeholder');
  assert.equal(followUpProblem('Reply to {{ msg.unknown }}'), 'placeholder');
  assert.equal(followUpProblem('a {{ literal }} brace'), null);
  assert.equal(FOLLOWUPS_MAX, 3);
  assert.deepEqual(templateFollowUps({ followUps: [' Commit\r\nand push ', '', 'Quote {{msg.text}}', 7, 'Stop', 'Wait', 'Fourth'] }),
    ['Commit\nand push', 'Stop', 'Wait']);
  assert.deepEqual(templateFollowUps({ steps: ['s'] }), []);
  assert.deepEqual(templateFollowUps(null), []);
});

test('a run freezes its template\'s follow-ups; nothing fills them', () => {
  const projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }],
    templates: [{ name: 'triage', steps: ['Look at {{msg.text}}'], followUps: ['Commit and push.', 'Quote {{msg.text}}'] }] }];
  const item = { event: { source: 'slack', key: 'k1', badge: 'eyes', text: 'Fix the login test', from: 'U1', where: 'C9', link: '' },
    rule: { projectId: 'P1', columnId: 'C1', template: 'triage', dir: '~/w', cmd: 'claude' } };
  const plan = planInbound(item, { cards: [], projects, home: '/Users/me' });
  assert.equal(plan.outcome, 'create');
  assert.deepEqual(plan.followUps, ['Commit and push.']);
  assert.ok(plan.steps[0].includes('<external_message'), 'the steps are filled, the follow-ups never');
  const channel = channelTemplatePlan({ body: 'Fix it', channelId: 'C1', senderUserId: 'U1',
    target: { cmd: 'claude', template: 'triage' } }, projects[0], 10);
  assert.deepEqual(channel.followUps, ['Commit and push.']);
});

test('a card offers the follow-ups its run froze, from either kind of plan', () => {
  assert.deepEqual(cardFollowUps({ inboundPlan: { followUps: ['A', '', 3, 'B', 'C', 'D'] } }), ['A', 'B', 'C']);
  assert.deepEqual(cardFollowUps({ channelRun: { followUps: ['Reply'] } }), ['Reply']);
  assert.deepEqual(cardFollowUps({ inboundPlan: {} , channelRun: { followUps: ['X'] } }), ['X']);
  assert.deepEqual(cardFollowUps({ inboundPlan: { followUps: 'A' } }), []);
  assert.deepEqual(cardFollowUps({}), []);
  assert.deepEqual(cardFollowUps(null), []);
  const items = [{ session: 'deck-a', text: 'A' }, { session: 'deck-b', text: 'B' }];
  assert.equal(followUpQueued(items, 'deck-a', 'A'), true);
  assert.equal(followUpQueued(items, 'deck-a', 'B'), false, 'another card\'s row is not this one');
  assert.equal(followUpQueued(undefined, 'deck-a', 'A'), false);
});

test('the model\'s pick names one of the card\'s follow-ups, from a current live observation', () => {
  const live = { alive: true, stale: false, followChoice: 2, followReason: 'asks to stop' };
  assert.deepEqual(followUpSuggestion(live, 3), { index: 1, reason: 'asks to stop' });
  assert.deepEqual(followUpSuggestion({ ...live, followReason: null }, 3), { index: 1, reason: '' });
  for (const off of [{ ...live, stale: true }, { ...live, alive: false }, { ...live, followChoice: 4 },
    { ...live, followChoice: 0 }, { ...live, followChoice: null }, { ...live, followChoice: 1.5 }, null]) {
    assert.equal(followUpSuggestion(off, 3), null);
  }
  // the poll's fields reach the snapshot only with an episode
  const tracker = createAttentionTracker();
  const card = { id: 'c', session: 'deck-c' };
  tracker.record([card], [{ name: 'deck-c', alive: true, agent: 'turn-done', episode: 4, follow_choice: 1, follow_reason: ' commit ' }]);
  assert.equal(tracker.get(card).followChoice, 1);
  assert.equal(tracker.get(card).followReason, 'commit');
  tracker.record([card], [{ name: 'deck-c', alive: true, follow_choice: 1, follow_reason: 'x' }]);
  assert.equal(tracker.get(card).followChoice, null);
  assert.equal(tracker.get(card).followReason, null);
});

/* a small element: enough DOM for the bar */
class El {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase(); this.children = []; this.attrs = {}; this.classes = new Set();
    this.hidden = false; this.textContent = ''; this.title = ''; this.parent = null;
  }
  set className(value) { this.classes = new Set(String(value).split(/\s+/).filter(Boolean)); }
  get className() { return [...this.classes].join(' '); }
  get classList() {
    return {
      toggle: (name, on) => (on ? this.classes.add(name) : this.classes.delete(name)),
      contains: name => this.classes.has(name),
    };
  }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  replaceChildren() { this.children = []; }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(child => child !== this); }
  get lastElementChild() { return this.children.at(-1) || null; }
  setAttribute(name, value) { this.attrs[name] = String(value); }
  getAttribute(name) { return this.attrs[name] ?? null; }
  querySelector(selector) {
    const name = selector.replace(/^\./, '');
    for (const child of this.children) {
      if (child.classes.has(name)) return child;
      const deep = child.querySelector(selector);
      if (deep) return deep;
    }
    return null;
  }
}

test('the bar paints the card\'s follow-ups, the model\'s pick, queued marks and a stopped card', async () => {
  const previous = globalThis.document;
  globalThis.document = { createElement: tag => new El(tag) };
  try {
    ctx.attention = createAttentionTracker();
    ctx.queueCache = { items: [], plans: [] };
    const card = { id: 'F1', session: 'deck-f1', title: 'Fix login', status: 'idle',
      inboundPlan: { followUps: ['Commit and push.\nThen reply in the thread.', 'Stop here.'] } };
    const bar = new El();
    const picks = [];
    let release;
    const pick = index => { picks.push(index); return new Promise(resolve => { release = resolve; }); };
    paintFollowUps(bar, card, pick);
    assert.equal(bar.hidden, false);
    assert.equal(bar.getAttribute('aria-label'), t('followUps.group', { name: 'Fix login' }));
    assert.equal(bar.querySelector('.follow-label').textContent, t('followUps.label'));
    const buttons = () => bar.querySelector('.follow-list').children;
    assert.deepEqual(buttons().map(b => b.textContent), ['Commit and push. ⏎1', 'Stop here.']);
    assert.ok(buttons()[0].title.startsWith('Commit and push.\nThen reply in the thread.'), 'the tooltip holds it whole');
    assert.ok(buttons()[0].title.endsWith(t('followUps.sendTitle')));
    assert.equal(buttons()[1].getAttribute('aria-label'), t('followUps.sendNamed', { text: 'Stop here.' }));
    assert.equal(bar.querySelector('.follow-reason').hidden, true);
    // the model's pick marks a button and says why
    ctx.attention.record([card], [{ name: 'deck-f1', alive: true, agent: 'turn-done', episode: 3, follow_choice: 2, follow_reason: 'asks whether to stop' }]);
    paintFollowUps(bar, card, pick);
    assert.deepEqual(buttons().map(b => b.classList.contains('suggested')), [false, true]);
    assert.equal(bar.querySelector('.follow-reason').textContent, t('followUps.reason', { reason: 'asks whether to stop' }));
    assert.ok(buttons()[1].title.includes(t('followUps.suggested')));
    ctx.attention.fail();
    paintFollowUps(bar, card, pick);
    assert.deepEqual(buttons().map(b => b.classList.contains('suggested')), [false, false], 'an old snapshot picks nothing');
    // a click: one at a time, then painted again
    buttons()[1].onclick();
    assert.deepEqual(picks, [1]);
    assert.equal(buttons()[1].getAttribute('aria-disabled'), 'true');
    buttons()[1].onclick();
    assert.deepEqual(picks, [1], 'a second click while the first queues does nothing');
    release(true);
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.equal(buttons()[1].getAttribute('aria-disabled'), 'false');
    // already in this card's queue: marked, and a click does nothing
    ctx.queueCache = { items: [{ session: 'deck-f1', text: 'Stop here.' }], plans: [] };
    paintFollowUps(bar, card, pick);
    assert.equal(buttons()[1].textContent, '✓ Stop here.');
    assert.equal(buttons()[1].getAttribute('aria-disabled'), 'true');
    assert.equal(buttons()[1].getAttribute('aria-label'), t('followUps.queuedNamed', { text: 'Stop here.' }));
    buttons()[1].onclick();
    assert.deepEqual(picks, [1]);
    // a stopped card queues nothing
    paintFollowUps(bar, { ...card, status: 'stopped' }, pick);
    assert.ok(buttons().every(b => b.getAttribute('aria-disabled') === 'true'));
    assert.ok(buttons()[0].title.endsWith(t('followUps.stopped')));
    // fewer follow-ups, then none: the bar follows and hides
    paintFollowUps(bar, { ...card, inboundPlan: { followUps: ['Only'] } }, pick);
    assert.deepEqual(buttons().map(b => b.textContent), ['Only']);
    paintFollowUps(bar, { ...card, inboundPlan: {} }, pick);
    assert.equal(bar.hidden, true);
    assert.equal(bar.children.length, 0);
  } finally {
    globalThis.document = previous;
    ctx.queueCache = { items: [], plans: [] };
  }
});

function board(cards, invoke) {
  store.projects = [{ id: 'P1', name: 'P', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = cards;
  state.view = 'none';   // nothing on screen to repaint around the call
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'queue_list') return { items: [], plans: [] };
    if (cmd === 'ui_event') return;
    return invoke(cmd, args);
  } } };
  return calls;
}

test('a click queues the frozen text for its card through the owner command and wakes the scheduler', async () => {
  ctx.attention = createAttentionTracker();
  const card = { id: 'F2', projectId: 'P1', columnId: 'C1', title: 'Fix', session: 'deck-f2', dir: '/w', cmd: 'claude',
    status: 'idle', inboundPlan: { followUps: ['Commit and push.', 'Stop here.'] } };
  let calls = board([card], async () => {});
  assert.equal(await queueFollowUp('F2', 0), true);
  const add = calls.find(([cmd]) => cmd === 'queue_add')[1].args;
  assert.deepEqual({ ...add, at: typeof add.at }, { session: 'deck-f2', cardId: 'F2', dir: '/w', cmd: 'claude',
    text: 'Commit and push.', mode: 'at', at: 'number', wake: true });
  assert.ok(Math.abs(add.at - Date.now() / 1000) < 5);
  const said = () => calls.filter(([cmd]) => cmd === 'ui_event').map(([, args]) => args.detail);
  assert.deepEqual(said(), ['queued'], 'no pick was shown');
  // with the model's pick shown: whether the click took it
  ctx.attention.record([card], [{ name: 'deck-f2', alive: true, agent: 'turn-done', episode: 1, follow_choice: 2 }]);
  calls = board([card], async () => {});
  await queueFollowUp('F2', 1);
  await queueFollowUp('F2', 0);
  assert.deepEqual(said(), ['queued-suggested', 'queued-other']);
  // nothing to queue: an index the card lacks, an unknown card, a stopped card
  calls = board([{ ...card, status: 'stopped' }], async () => {});
  assert.equal(await queueFollowUp('F2', 0), false);
  assert.equal(await queueFollowUp('F2', 5), false);
  assert.equal(await queueFollowUp('nope', 0), false);
  assert.ok(!calls.some(([cmd]) => cmd === 'queue_add'));
  // a refused add says so and reports false
  calls = board([card], async () => { throw new Error('refused'); });
  assert.equal(await queueFollowUp('F2', 0), false);
  assert.ok(!calls.some(([cmd]) => cmd === 'ui_event'), 'nothing was queued, nothing is logged');
});
