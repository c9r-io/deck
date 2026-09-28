// Verified shell exit (shell_exit.rs → poll `exited_normally`) is the one
// liveness fact that retires a card, through the ordinary durable close.
// Absence, poll failure, an MCP-origin card and a retained buffer never do.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';
globalThis.document = { ...fakeDocument, querySelectorAll: () => [], querySelector: () => null };
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {}, dispatchEvent() {} };
const { ctx, listeners, state, store } = await import('../js/state.js');
const { pollNow, stopPolling, markSessionsStoppedForServerRestart } = await import('../js/board.js');
await import('../js/layout.js');
const { t } = await import('../js/i18n.js');
// DOM rendering is not under test: the Board's render listeners are dropped
// and no project tab exists to draw.
listeners.clear();

function card(id, extra = {}) {
  return { id, projectId: 'P1', columnId: 'C1', title: id, desc: '', cmd: '', dir: '/tmp',
    session: `deck-${id}`, status: 'running', launched: true, ...extra };
}

const toasts = () => (ids.get('toasts')?.children || []).map(el => el.textContent);

/* The Board and runtime seams one poll touches. `infos` answers poll_sessions;
   `fail` names native commands that reject. */
function setup(cards, infos, fail = new Set()) {
  state.view = 'board';
  store.projects = [];
  store.cards = cards;
  ctx.queueCache = { items: cards.map(c => ({ id: `q-${c.id}`, session: c.session })), last_fired: {} };
  ids.get('toasts')?.replaceChildren();
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'ui_event' || cmd === 'notify_cards' || cmd === 'notify_dismiss') return;
    if (cmd === 'poll_sessions') {
      if (fail.has(cmd)) throw new Error('tmux control recovering');
      return typeof infos === 'function' ? infos() : infos;
    }
    if (['queue_clear_sessions', 'kill_session', 'save_board'].includes(cmd)) {
      if (fail.has(cmd)) throw new Error(`${cmd} failed`);
      return;
    }
    throw new Error(`unexpected ${cmd}`);
  } } };
  return calls;
}
const count = (calls, cmd) => calls.filter(([c]) => c === cmd).length;
const exited = (c, extra = {}) => ({ name: c.session, alive: false, exited_normally: true, agent: null, ...extra });
const missing = c => ({ name: c.session, alive: false, exited_normally: false, agent: null });

test('a verified exit retires exactly once through the durable close, however polls race', async () => {
  const a = card('a'); const b = card('b');
  const calls = setup([a, b], [exited(a), missing(b)]);
  try {
    // pty-exit, the interval tick and an attention refresh all call pollNow
    await Promise.all([pollNow(), pollNow(), pollNow()]);
    await pollNow();
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['b', 'stopped']]);
    assert.equal(count(calls, 'queue_clear_sessions'), 1);
    assert.deepEqual(calls.find(([c]) => c === 'queue_clear_sessions')[1].sessions, ['deck-a']);
    assert.equal(count(calls, 'kill_session'), 1);
    assert.equal(count(calls, 'save_board'), 1);
    assert.deepEqual(toasts(), [t('session.closedExited', { name: 'a' })]);
  } finally { stopPolling(); }
});

test('absence, poll failure, signal death and service restart keep every card and queue row', async () => {
  const cards = ['a', 'b'].map(id => card(id));
  let failPoll = true;
  const calls = setup(cards, () => cards.map(missing));
  window.__TAURI__.core.invoke = (inner => async (cmd, args) => {
    if (cmd === 'poll_sessions' && failPoll) { calls.push([cmd]); throw new Error('tmux unavailable'); }
    return inner(cmd, args);
  })(window.__TAURI__.core.invoke);
  try {
    assert.equal(await pollNow(), false);
    failPoll = false;
    for (let i = 0; i < 3; i++) assert.equal(await pollNow(), true);
    markSessionsStoppedForServerRestart();
    assert.equal(await pollNow(), true);
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['a', 'stopped'], ['b', 'stopped']]);
    assert.equal(ctx.queueCache.items.length, 2);
    for (const cmd of ['queue_clear_sessions', 'kill_session', 'save_board']) assert.equal(count(calls, cmd), 0, cmd);
    assert.deepEqual(toasts(), []);
  } finally { stopPolling(); }
});

test('an MCP-origin card and a card with a retained buffer stay stopped on a verified exit', async () => {
  const mcp = card('m', { origin: { source: 'mcp' } });
  const kept = card('k', { buffer: { entries: [{ id: 'e1', text: 'keep me' }] } });
  const calls = setup([mcp, kept], [exited(mcp), exited(kept)]);
  try {
    await pollNow(); await pollNow();
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['m', 'stopped'], ['k', 'stopped']]);
    for (const cmd of ['queue_clear_sessions', 'kill_session', 'save_board']) assert.equal(count(calls, cmd), 0, cmd);
  } finally { stopPolling(); }
});

for (const failing of ['queue_clear_sessions', 'save_board']) {
  test(`a ${failing} failure keeps the card and its queue, toasts once, and a later poll retries`, async () => {
    const a = card('a');
    const fail = new Set([failing]);
    const calls = setup([a], [exited(a)], fail);
    try {
      await pollNow(); await pollNow(); await pollNow();
      assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['a', 'stopped']]);
      assert.equal(ctx.queueCache.items.length, 1);
      assert.deepEqual(toasts(), [t('error.retire')]);
      assert.ok(count(calls, failing) >= 2, 'each poll retries while the evidence stands');
      fail.clear();
      await pollNow();
      assert.equal(store.cards.length, 0);
      assert.deepEqual(toasts(), [t('error.retire'), t('session.closedExited', { name: 'a' })]);
    } finally { stopPolling(); }
  });
}

test('evidence is re-proven each poll: a card reopened after a failed retirement is never closed', async () => {
  const a = card('a');
  let infos = [exited(a)];
  const fail = new Set(['save_board']);
  const calls = setup([a], () => infos, fail);
  try {
    await pollNow();
    assert.equal(store.cards.length, 1);
    // the user reopened the stopped card: a new, live session of the same name
    infos = [{ name: a.session, alive: true, exited_normally: false, agent: null, fg: 'zsh', idle_secs: 1 }];
    fail.clear();
    await pollNow(); await pollNow();
    assert.equal(store.cards.length, 1);
    assert.notEqual(store.cards[0].status, 'stopped');
    assert.equal(count(calls, 'save_board'), 1, 'no retirement ran after the evidence ended');
    // the webview lost its state (reload / Deck restart): the backend's answer
    // alone decides, and without evidence the card only stops
    stopPolling();
    infos = [missing(a)];
    await pollNow();
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['a', 'stopped']]);
    assert.equal(count(calls, 'kill_session'), 1);
  } finally { stopPolling(); }
});
