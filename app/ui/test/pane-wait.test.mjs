import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { attachFailure, paneNotice, retryAllowed, startFailure, startFailureToastKey, TERMINAL_WAITS } from '../js/pane-wait-model.js';
import { createTerminalDataHandler } from '../js/terminal-input.js';

const tick = () => new Promise(resolve => setImmediate(resolve));
const limits = JSON.parse(readFileSync(new URL('./fixtures/limits.json', import.meta.url), 'utf8'));

test('the waits are the backend list', () => {
  const waits = limits.terminal_waits;
  assert.equal(TERMINAL_WAITS.attachBudgetMs, waits.attach_budget_ms);
  assert.equal(TERMINAL_WAITS.firstFrameWaitMs, waits.first_frame_wait_ms);
  assert.equal(TERMINAL_WAITS.startBudgetMs, waits.start_budget_ms);
  assert.equal(TERMINAL_WAITS.connectingNoticeMs, waits.connecting_notice_ms);
  assert.ok(TERMINAL_WAITS.connectingNoticeMs < TERMINAL_WAITS.attachBudgetMs);
});

test('each wait says what Deck observed and offers only its own action', () => {
  assert.deepEqual(paneNotice('connecting'), { key: 'pane.connecting', params: { seconds: 10 }, action: null });
  assert.deepEqual(paneNotice('starting').action, null);
  assert.equal(paneNotice('timeout').key, 'pane.attachTimeout');
  assert.equal(paneNotice('timeout').params.seconds, TERMINAL_WAITS.attachBudgetMs / 1000);
  assert.equal(paneNotice('timeout').action, 'retry');
  assert.equal(paneNotice('busy').action, 'retry');
  assert.equal(paneNotice('startTimeout').action, 'retry');
  assert.deepEqual(paneNotice('noFrame'), { key: 'pane.noFrame', params: { seconds: 10 }, action: 'reconnect' });
  assert.equal(paneNotice('other'), null);
});

test('backend answers are read by their wire message', () => {
  assert.equal(attachFailure('terminal-timeout'), 'timeout');
  assert.equal(attachFailure('terminal-busy'), 'busy');
  assert.equal(attachFailure('attach-superseded'), 'superseded');
  assert.equal(attachFailure('not attached'), null);
  assert.equal(startFailure('start-unknown'), 'unknown');
  assert.equal(startFailure('terminal-timeout'), 'timeout');
  assert.equal(startFailure('not a directory: x'), null);
  assert.equal(startFailureToastKey('start-unknown'), 'session.startTimeout');
  assert.equal(startFailureToastKey('terminal-timeout'), 'session.startTimeout');
  assert.equal(startFailureToastKey('terminal-busy'), 'session.startBusy');
  assert.equal(startFailureToastKey('tmux-server-restart-in-progress'), null);
});

test('a retry is never offered for a dead session, a card being closed, a replaced pane or a restart', () => {
  const live = { status: 'idle' };
  const base = { kind: 'timeout', card: live, current: true, restarting: false, closing: false, exited: false };
  assert.equal(retryAllowed(base), true);
  assert.equal(retryAllowed({ ...base, kind: 'noFrame' }), true);
  assert.equal(retryAllowed({ ...base, card: null }), false);
  assert.equal(retryAllowed({ ...base, current: false }), false, 'the pane was replaced');
  assert.equal(retryAllowed({ ...base, restarting: true }), false);
  assert.equal(retryAllowed({ ...base, closing: true }), false, 'a card being retired');
  assert.equal(retryAllowed({ ...base, exited: true }), false, 'its stream ended');
  assert.equal(retryAllowed({ ...base, card: { status: 'stopped' } }), false, 'an attach retry never revives a dead session');
  // a start that did not finish is retried only while the card is still stopped
  assert.equal(retryAllowed({ ...base, kind: 'startTimeout', card: { status: 'stopped' } }), true);
  assert.equal(retryAllowed({ ...base, kind: 'startTimeout', card: live }), false);
  assert.equal(retryAllowed({ ...base, kind: 'startTimeout', card: { status: 'stopped' }, closing: true }), false);
});

/* layout.js binds input exactly like this: to the pane's generation, or,
   while an attach request is open, to that request */
function paneInput(pane, events) {
  return createTerminalDataHandler({
    pane, blocked: () => false, onInput: () => {},
    hasSelection: () => false, cancelSelection: () => assert.fail('No selection'),
    scrolled: () => false, goLive: () => assert.fail('No scroll'),
    bind: () => {
      if (pane.inputGen != null) return { gen: pane.inputGen };
      const request = pane.attachRequest;
      return request && !request.settled ? { request, wait: request.ready } : { gen: null };
    },
    stale: bound => {
      const gen = bound.request ? bound.request.gen : bound.gen;
      if (gen == null) return 'attachment';
      return pane.inputGen !== gen ? 'attachment' : null;
    },
    cancelled: reason => events.push(['cancel', reason]),
    write: (data, bound) => events.push(['write', data, bound.request ? bound.request.gen : bound.gen]),
  });
}

function openRequest(pane) {
  const request = { gen: null, settled: false };
  request.ready = new Promise(resolve => { request.settle = resolve; });
  pane.attachRequest = request;
  pane.inputGen = null;
  return request;
}

test('keys typed while connecting are dropped when the attach times out, and never reach the retry', async () => {
  const pane = {}, events = [];
  const handle = paneInput(pane, events);
  const timedOut = openRequest(pane);
  handle('typed while connecting');
  await tick();
  assert.deepEqual(events, [], 'the input waits for its own request');
  // the attach answered terminal-timeout: the request settles naming nothing
  timedOut.settled = true; timedOut.settle();
  await tick(); await tick();
  assert.deepEqual(events, [['cancel', 'attachment']]);
  // the pane's retry is a new request that delivers generation 8
  const retry = openRequest(pane);
  handle('typed during the retry');
  retry.gen = 8; pane.inputGen = 8; retry.settled = true; retry.settle();
  await tick(); await tick();
  handle('after');
  await tick();
  assert.deepEqual(events, [
    ['cancel', 'attachment'],
    ['write', 'typed during the retry', 8],
    ['write', 'after', 8],
  ], 'only what was bound to the retry reaches it');
});
