// Exercise the production card renderer and its delegated gestures. This
// minimal DOM checks routing/labels, not browser layout or focus retention.
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument } from './fixtures/dom-fixture.mjs';

class Element extends FakeElement {
  constructor(tag) {
    super(tag);
    this.classes = new Set();
    this.classList = {
      add: name => this.classes.add(name),
      remove: name => this.classes.delete(name),
      contains: name => this.classes.has(name),
      toggle: (name, on) => on ? this.classes.add(name) : this.classes.delete(name),
    };
  }
  set innerHTML(markup) {
    this.markup = markup;
    this.parts = new Map();
    for (const [, tag, names] of markup.matchAll(/<(\w+) class="([^"]+)"/g)) {
      const part = new Element(tag);
      part.parts = this.parts;
      for (const name of names.split(' ')) this.parts.set('.' + name, part);
    }
  }
  get innerHTML() { return this.markup; }
  querySelector(selector) {
    assert.ok(this.parts.has(selector), `rendered selector ${selector}`);
    return this.parts.get(selector);
  }
  getAttribute(name) { return this[name] ?? null; }
  removeAttribute(name) { delete this[name]; }
}
globalThis.document = { ...fakeDocument, createElement: tag => new Element(tag) };
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };
const { createBoardCard } = await import('../js/board-card.js');
const { ctx } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');
const { createAttentionTracker } = await import('../js/attention-model.js');

function fixture(extra = {}, overrides = {}) {
  setLocale('en');
  ctx.attention = createAttentionTracker();
  ctx.queueCache = { items: [], last_fired: {} };
  ctx.settings.inbound.rules = [];
  const card = { id: 'C1', session: 'fixture', title: '<img src=x>', status: 'stopped', dir: '/tmp/<work>', ...extra };
  const calls = [];
  const reminder = new Element('button');
  const actions = {
    open: id => calls.push(['open', id]),
    close: id => calls.push(['close', id]),
    togglePinned: async id => { calls.push(['pin', id]); },
    context: (event, id) => calls.push(['context', id, event]),
    reminderChip: value => { assert.equal(value, card); return reminder; },
    ...overrides,
  };
  return { card, calls, actions, reminder };
}
const click = target => ({ target, stopped: 0, stopPropagation() { this.stopped++; } });

test('fixed card shape paints data as text and keeps independent status, reminder and queue slots', () => {
  for (const status of ['stopped', 'waiting', 'attention']) {
    const f = fixture({ status, pinned: status === 'attention', cmd: '<script>', desc: '<b>description</b>', mem: 2000, reminder: { revision: 1 } });
    const el = createBoardCard(f.card, f.actions);
    assert.equal(el.dataset.sid, f.card.id);
    assert.equal(el.draggable, true);
    assert.equal(el.className, 'card' + (status === 'stopped' ? '' : ' ' + status));
    assert.equal(el.querySelector('.card-title').textContent, f.card.title);
    assert.equal(el.querySelector('.cmd').textContent, '$ ' + f.card.cmd);
    assert.equal(el.querySelector('.dir').textContent, f.card.dir);
    assert.equal(el.querySelector('.card-desc').textContent, f.card.desc);
    assert.equal(el.querySelector('.card-desc').title, f.card.desc);
    assert.ok(!el.innerHTML.includes(f.card.title) && !el.innerHTML.includes(f.card.desc));
    assert.deepEqual(el.querySelector('.card-status').children, [f.reminder]);
    assert.equal(el.querySelector('.card-attention-badge').hidden, true);
    assert.equal(el.querySelector('.auto-chip').hidden, true);
    assert.equal(el.querySelector('.q-chip').textContent, '');
    assert.equal(el.querySelector('.mem-chip').classes.has('high'), true);
    const pin = el.querySelector('.card-pin');
    assert.equal(pin['aria-pressed'], String(f.card.pinned));
    assert.equal(pin.textContent, f.card.pinned ? '★' : '☆');
    assert.equal(pin.title, t(f.card.pinned ? 'card.unmarkImportant' : 'card.markImportant'));
    assert.deepEqual(f.calls, [], 'rendering never starts, closes, moves or saves a card');
  }
  const f = fixture();
  const el = createBoardCard(f.card, f.actions);
  assert.equal(el.querySelector('.card-desc').textContent, '');
  assert.equal(el.querySelector('.cmd').textContent, '');
  assert.deepEqual(el.querySelector('.card-status').children, []);
});

test('card delegates only the named gesture; child controls never open a session', () => {
  const f = fixture();
  const el = createBoardCard(f.card, f.actions);
  for (const selector of ['.card-x', '.card-pin', '.card-signal-help', '.card-reminder']) {
    const e = click({ closest: query => query.split(', ').includes(selector) });
    el.fire('click', e);
    assert.deepEqual(f.calls, []);
  }
  const body = click({ closest: () => null });
  const opened = el.fire('click', body);
  assert.equal(opened.stopped, 1, 'Board column selection does not see the click');
  const closed = click(null);
  el.querySelector('.card-x').onclick(closed);
  assert.equal(closed.stopped, 1);
  const menu = {};
  el.oncontextmenu(menu);
  assert.deepEqual(f.calls, [['open', 'C1'], ['close', 'C1'], ['context', 'C1', menu]]);
  const data = new Map();
  el.fire('dragstart', { dataTransfer: { setData: (key, value) => data.set(key, value) } });
  assert.equal(data.get('text/deck-session'), 'C1');
  assert.equal(el.classes.has('dragging'), true);
  el.fire('dragend');
  assert.equal(el.classes.has('dragging'), false);
  assert.equal(f.calls.length, 3, 'dragging itself grants no placement operation');
});

test('pin action stays focusable, ignores overlap, and clears busy after success or failure', async () => {
  for (const fail of [false, true]) {
    let settle;
    const pending = new Promise((resolve, reject) => { settle = fail ? reject : resolve; });
    const calls = [];
    const f = fixture({}, { togglePinned: id => { calls.push(id); return pending; } });
    const pin = createBoardCard(f.card, f.actions).querySelector('.card-pin');
    const first = pin.onclick(click(null));
    assert.equal(pin.getAttribute('aria-disabled'), 'true');
    assert.equal(pin.disabled, false);
    await pin.onclick(click(null));
    assert.deepEqual(calls, ['C1']);
    settle(fail ? new Error('fixture save failed') : undefined);
    if (fail) await assert.rejects(first, /fixture save failed/);
    else await first;
    assert.equal(pin.getAttribute('aria-disabled'), null);
    assert.equal(f.card.pinned, undefined, 'the renderer does not commit optimistic Board state');
  }
});

test('existing painters still supply automation, queue and attention information', () => {
  const f = fixture({ origin: { source: 'clock', badge: 'rule1' } });
  ctx.settings.inbound.rules = [{ id: 'rule1', source: 'clock', name: 'Morning' }];
  ctx.queueCache.items = [{ session: 'fixture', mode: 'at', at: 1, text: 'fixture prompt' }];
  ctx.attention.record([f.card], [{ name: 'fixture', alive: true, agent: 'needs-input' }]);
  const el = createBoardCard(f.card, f.actions);
  assert.equal(el.querySelector('.auto-chip').textContent, '↻ Morning');
  assert.equal(el.querySelector('.auto-chip').hidden, false);
  assert.equal(el.querySelector('.q-chip').textContent, '⏰1');
  assert.equal(el.querySelector('.card-attention-badge').hidden, false);
  assert.equal(el.querySelector('.card-attention-badge').dataset.kind, 'input');
});
