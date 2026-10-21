// The find bar (⌘F): the pure decisions (terminal-find-model.js) and the
// controller's request order and presentation (terminal-find.js) over the
// shared fake DOM and a fake backend. tmux's own search is exercised by the
// real-tmux test in `src-tauri/src/terminal_search.rs`.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, FakeElement } from './fixtures/dom-fixture.mjs';
import { FIND_DEBOUNCE_MS, FIND_QUERY_MAX_BYTES, findQueryState, findStatus, findStep } from '../js/terminal-find-model.js';
import { createTerminalFind } from '../js/terminal-find.js';

test('queries follow the backend rules', () => {
  assert.equal(findQueryState(''), 'empty');
  assert.equal(findQueryState(undefined), 'empty');
  assert.equal(findQueryState('needle'), 'ok');
  assert.equal(findQueryState('中'.repeat(85)), 'ok', '255 bytes');
  assert.equal(findQueryState('中'.repeat(86)), 'long', '258 bytes');
  assert.equal(findQueryState('a'.repeat(FIND_QUERY_MAX_BYTES)), 'ok');
  for (const bad of ['a\nb', 'a\tb', '\u001b[0m', 'a\u007f', 'a\u0085']) assert.equal(findQueryState(bad), 'invalid');
});

test('steps start on a new query and turn once on a change of direction', () => {
  const start = findStep({ query: 'q', direction: 'older' });
  assert.deepEqual(start, { step: 'start', turn: false, last: 'older' }, 'nothing searched yet');
  assert.deepEqual(findStep({ last: 'older', lastQuery: 'q', query: 'q', direction: 'older' }),
    { step: 'older', turn: false, last: 'older' });
  assert.deepEqual(findStep({ last: 'older', lastQuery: 'q', query: 'q', direction: 'newer' }),
    { step: 'newer', turn: true, last: 'newer' });
  assert.deepEqual(findStep({ last: 'newer', lastQuery: 'q', query: 'q', direction: 'newer' }),
    { step: 'newer', turn: false, last: 'newer' });
  assert.equal(findStep({ last: 'older', lastQuery: 'q', query: 'qq', direction: 'older' }).step, 'start');
  assert.equal(findStep({ last: 'older', lastQuery: 'q', query: 'q', direction: 'start' }).step, 'start');
});

test('the label is a count, never a position', () => {
  assert.deepEqual(findStatus({ result: { active: true, present: true, count: 3 } }),
    { key: 'find.count', count: 3, screenOnly: false });
  assert.equal(findStatus({ result: { active: true, present: true, count: 1000, partial: true } }).key, 'find.countMore');
  assert.deepEqual(findStatus({ result: { active: true, present: false, alternate: true } }),
    { key: 'find.none', screenOnly: true });
  assert.equal(findStatus({ error: true }).key, 'find.unavailable');
  assert.equal(findStatus({ unavailable: true }).key, 'find.unavailable');
  assert.equal(findStatus({ state: 'long' }).key, 'find.invalid');
  assert.equal(findStatus({ state: 'empty' }).key, '');
  assert.equal(findStatus().key, '');
});

const flush = () => new Promise(resolve => setImmediate(resolve));

function harness({ reply, available = () => true } = {}) {
  const calls = [], results = [], timers = new Map();
  let nextTimer = 1, lives = 0, focused = 0;
  const pending = [];
  const mount = new FakeElement('div');
  const find = createTerminalFind({
    doc: fakeDocument,
    mount,
    run: request => {
      calls.push(request);
      if (reply) return reply(request);
      return new Promise((resolve, reject) => pending.push({ resolve, reject }));
    },
    live: () => { lives++; },
    onResult: result => results.push(result),
    available,
    focusTerminal: () => { focused++; },
    t: (key, params) => params?.count != null ? `${key}:${params.count}` : key,
    setTimer: (fn, ms) => { const id = nextTimer++; timers.set(id, { fn, ms }); return id; },
    clearTimer: id => timers.delete(id),
  });
  const bar = () => mount.children[0];
  const input = () => bar().children[0];
  const button = cls => bar().children.find(node => node.className === cls);
  const fireTimers = () => {
    for (const [id, timer] of [...timers]) {
      timers.delete(id);
      assert.equal(timer.ms, FIND_DEBOUNCE_MS);
      timer.fn();
    }
  };
  const type = value => { input().value = value; input().fire('input'); };
  const key = (k, extra = {}) => input().fire('keydown', { key: k, ...extra });
  return { find, calls, results, timers, pending, bar, input, button, fireTimers, type, key,
    lives: () => lives, focused: () => focused };
}

const found = count => ({ active: true, present: true, count, partial: false, alternate: false });

test('typing searches after the debounce and never mid-composition', async () => {
  const h = harness({ reply: () => found(2) });
  h.find.open();
  assert.equal(h.find.isOpen(), true);
  assert.equal(fakeDocument.activeElement, h.input());
  h.input().value = 'nee';
  h.input().fire('input', { isComposing: true });
  assert.equal(h.timers.size, 0, 'a preedit is not a query');
  h.input().fire('compositionend');
  h.type('needle');
  assert.equal(h.timers.size, 1, 'one debounce timer, restarted');
  h.fireTimers();
  await flush();
  assert.deepEqual(h.calls, [{ query: 'needle', step: 'start', turn: false }]);
  assert.equal(h.find.view().label, 'find.count:2');
  assert.equal(h.results.length, 1);
});

test('Enter, Shift+Enter and Command-G step through matches with one turn', async () => {
  const h = harness({ reply: () => found(3) });
  h.find.open();
  h.type('q');
  h.fireTimers();
  await flush();
  const enter = h.key('Enter');
  assert.equal(enter.prevented, 1);
  assert.equal(enter.stopped, 1);
  await flush();
  h.key('Enter', { shiftKey: true });
  await flush();
  h.key('g', { metaKey: true, shiftKey: true });
  await flush();
  h.key('g', { metaKey: true });
  await flush();
  h.button('tfind-older').onclick();
  await flush();
  h.button('tfind-newer').onclick();
  await flush();
  assert.deepEqual(h.calls.map(call => `${call.step}${call.turn ? '!' : ''}`),
    ['start', 'older', 'newer!', 'newer', 'older!', 'older', 'newer!']);
  assert.equal(h.key('Enter', { isComposing: true }).prevented, 0, 'an IME confirm is not a step');
  assert.equal(h.key('a').prevented, 0);
});

test('a step before the debounce fires searches the new query from the start', async () => {
  const h = harness({ reply: () => found(1) });
  h.find.open();
  h.type('q');
  h.key('Enter');
  await flush();
  assert.equal(h.timers.size, 0);
  assert.deepEqual(h.calls, [{ query: 'q', step: 'start', turn: false }]);
});

test('one request in flight; the latest waiting request runs on the reply', async () => {
  const h = harness();
  h.find.open();
  h.type('a');
  h.fireTimers();
  await flush();
  assert.equal(h.calls.length, 1);
  h.type('ab');
  h.fireTimers();
  h.key('Enter');
  h.key('Enter');
  assert.equal(h.calls.length, 1, 'nothing overlaps the request in flight');
  h.pending.shift().resolve(found(4));
  await flush();
  await flush();
  assert.deepEqual(h.calls.map(call => [call.query, call.step]), [['a', 'start'], ['ab', 'start']],
    'a waiting start is never downgraded to a step of the old query');
  h.pending.shift().resolve(found(1));
  await flush();
  assert.equal(h.find.view().label, 'find.count:1');
});

test('no results, refusals and an unattached pane are said, never retried', async () => {
  let attached = true;
  const replies = [() => ({ active: true, present: false, alternate: true }), () => Promise.reject(new Error('x'))];
  const h = harness({ reply: () => replies.shift()(), available: () => attached });
  h.find.open();
  h.type('zzz');
  h.fireTimers();
  await flush();
  assert.equal(h.find.view().label, 'find.none · find.screenOnly');
  assert.equal(h.find.view().state, 'miss');
  h.key('Enter');
  await flush();
  assert.equal(h.find.view().label, 'find.unavailable');
  assert.equal(h.calls.length, 2);
  h.type('a\tb');
  h.fireTimers();
  await flush();
  assert.equal(h.find.view().label, 'find.invalid');
  assert.equal(h.calls.length, 2, 'refused before the backend');
  attached = false;
  h.find.close('replace');
  h.find.open();
  assert.equal(h.find.view().label, 'find.unavailable');
  h.type('q');
  h.fireTimers();
  await flush();
  assert.equal(h.calls.length, 2);
  attached = true;
  h.find.close('replace');
  h.find.open();
  assert.equal(h.find.view().label, '', 'reopening an attached pane clears a stale unavailable');
});

test('Esc keeps the view and the query; clearing a searched query goes live', async () => {
  const h = harness({ reply: () => found(1) });
  h.find.open();
  h.type('q');
  h.fireTimers();
  await flush();
  h.key('Escape');
  assert.equal(h.find.isOpen(), false);
  assert.equal(h.focused(), 1, 'the user\'s close returns focus to the terminal');
  assert.equal(h.lives(), 0, 'closing never leaves the match');
  h.key('Enter');
  await flush();
  assert.equal(h.calls.length, 1, 'a closed bar sends nothing');
  h.find.open();
  assert.equal(h.find.view().query, 'q');
  assert.equal(h.input().selected, true);
  h.type('');
  h.fireTimers();
  await flush();
  assert.equal(h.lives(), 1);
  assert.equal(h.find.view().label, '');
  h.type('');
  h.fireTimers();
  assert.equal(h.lives(), 1, 'only a bar that searched goes live');
  h.button('tfind-x').onclick({ stopPropagation() {} });
  assert.equal(h.find.isOpen(), false);
  assert.equal(h.focused(), 2);
  h.find.close();
  assert.equal(h.focused(), 2, 'close is idempotent');
  h.find.open();
  h.type('pending');
  h.find.dispose();
  assert.equal(h.timers.size, 0, 'dispose drops the debounce');
  assert.equal(h.bar().isConnected, false);
  assert.equal(h.find.isOpen(), false);
  const down = h.bar().fire('mousedown');
  assert.equal(down.stopped, 1, 'the bar keeps its own focus');
  assert.equal(h.button('tfind-older').fire('mousedown').prevented, 1);
});
