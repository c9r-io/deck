// The finish rule of an automation run ("close the card"), driven through
// the real Board poll: pollNow → observeRunFinish → the retirement tracker →
// provider.close, against a fake `invoke`. A finished run closes on its
// third consecutive reading. A close that failed is tried again only by a
// poll that reads the same thing again; a pane showing the card, an agent or
// a program back in front, a queued prompt, a changed rule or a session that
// is gone drops it. The close itself looks for a pane once more inside its
// transaction. A verified shell exit keeps its own rule (shell-exit.test.mjs).
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';
globalThis.document = { ...fakeDocument, querySelectorAll: () => [], querySelector: () => null };
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {}, dispatchEvent() {} };
const { ctx, listeners, state, store } = await import('../js/state.js');
const { panes, pollNow, prepareCardsForServerRestart, stopPolling } = await import('../js/board.js');
await import('../js/layout.js');
const { t } = await import('../js/i18n.js');
// DOM rendering is not under test: the Board's render listeners are dropped
// and no project tab exists to draw.
listeners.clear();

const SESSION = 'deck-run';
const RULE = { id: 'r1', source: 'clock', badge: 'r1', projectId: 'P1', columnId: 'C1', finish: 'close', enabled: true };
const runCard = (extra = {}) => ({ id: 'run', projectId: 'P1', columnId: 'C1', title: 'run', desc: '', cmd: '', dir: '/tmp',
  session: SESSION, status: 'running', launched: true, origin: { source: 'clock', badge: 'r1', key: 'k1' }, ...extra });
/* the agent program exited: no agent state, a shell in the pane's foreground */
const FINISHED = { name: SESSION, alive: true, exited_normally: false, agent: null, fg: 'zsh', finish_fg: 'zsh', idle_secs: 30 };
const GONE = { name: SESSION, alive: false, exited_normally: false, agent: null };
const FAILED = t('automation.runCloseFailed');
const CLOSED = t('automation.runClosed', { name: 'run' });
const toasts = () => (ids.get('toasts')?.children || []).map(el => el.textContent);

/* One run and the seams a poll touches. `infos` answers poll_sessions and
   `fail` names the native commands that reject. A schedule cancellation that
   lands erases the session's final-review fact, as scheduler/ops.rs
   `clear_session_items` does (the app's queue cache follows through the
   queue-changed event), and a kill that lands leaves the session gone. */
function world(card = runCard(), rules = [RULE]) {
  stopPolling();
  panes.clear();
  state.view = 'board';
  store.projects = [];
  store.cards = [card];
  ctx.settings.inbound = { ...ctx.settings.inbound, rules, channelRules: [] };
  ctx.queueCache = { items: [], last_fired: {}, review_completed: [] };
  ids.get('toasts')?.replaceChildren();
  const w = { infos: [FINISHED], fail: new Set(), calls: [], saveGate: null };
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    w.calls.push(cmd);
    if (['ui_event', 'notify_cards', 'notify_dismiss', 'inbound_run_ended'].includes(cmd)) return;
    if (cmd === 'poll_sessions') return w.infos;
    if (w.fail.has(cmd)) throw new Error(`${cmd} failed`);
    if (cmd === 'queue_clear_sessions') {
      ctx.queueCache = { ...ctx.queueCache,
        items: ctx.queueCache.items.filter(i => !args.sessions.includes(i.session)),
        review_completed: ctx.queueCache.review_completed.filter(s => !args.sessions.includes(s)) };
      return;
    }
    if (cmd === 'kill_session') { w.infos = [GONE]; return; }
    if (cmd === 'save_board') { if (w.saveGate) await w.saveGate; return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  return w;
}
const count = (w, cmd) => w.calls.filter(c => c === cmd).length;
const closes = w => ['queue_clear_sessions', 'kill_session', 'save_board'].map(cmd => count(w, cmd));
const polls = async n => { for (let i = 0; i < n; i++) assert.equal(await pollNow(), true); };
/* a pane shows the run's session (layout.js `hasPane`); it is not the run's
   own pane entry, so a close that does happen has no terminal to dispose */
const show = () => panes.set(SESSION, { sid: 'viewer', session: SESSION, attached: true, el: { querySelector: () => null } });
const leave = () => panes.delete(SESSION);
const kept = () => store.cards.map(c => c.id);
/* three readings queue the close; its first attempt fails at `command` */
async function failedClose(w, command = 'queue_clear_sessions') {
  w.fail.add(command);
  await polls(3);
  assert.deepEqual(kept(), ['run'], 'a failed close keeps the card');
  assert.deepEqual(toasts(), [FAILED]);
  w.fail.clear();
}

test('a finished run closes on its third consecutive reading, through the durable close', async () => {
  const w = world();
  try {
    await polls(2);
    assert.deepEqual(kept(), ['run']);
    assert.deepEqual(closes(w), [0, 0, 0], 'two readings touch nothing');
    await polls(1);
    assert.deepEqual(kept(), []);
    assert.deepEqual(w.calls.filter(c => ['queue_clear_sessions', 'kill_session', 'save_board'].includes(c)),
      ['queue_clear_sessions', 'kill_session', 'save_board']);
    assert.equal(count(w, 'inbound_run_ended'), 1);
    assert.deepEqual(toasts(), [CLOSED]);
  } finally { stopPolling(); }
});

test('a pane showing the run holds the close, and the count starts again from zero once it is gone', async () => {
  const w = world();
  try {
    await polls(2);   // two readings held before anyone looked
    show();
    await polls(5);
    assert.deepEqual(kept(), ['run']);
    assert.deepEqual(closes(w), [0, 0, 0]);
    leave();
    await polls(2);
    assert.deepEqual(kept(), ['run'], 'three consecutive readings, counted again from zero');
    await polls(1);
    assert.deepEqual(kept(), []);
    assert.deepEqual(toasts(), [CLOSED]);
  } finally { stopPolling(); }
});

test('after a failed close, a card someone opened is not closed by the next poll', async () => {
  const w = world();
  try {
    await failedClose(w);
    show();
    await polls(2);
    assert.deepEqual(kept(), ['run']);
    assert.notEqual(store.cards[0].status, 'stopped', 'the live session is shown as what it is');
    assert.deepEqual(closes(w), [1, 0, 0], 'no attempt was made while the pane shows it');
    assert.deepEqual(toasts(), [FAILED]);
    // the user leaves: the run qualifies again from zero and is closed
    leave();
    await polls(2);
    assert.deepEqual(kept(), ['run']);
    await polls(1);
    assert.deepEqual(kept(), []);
    assert.deepEqual(toasts(), [FAILED, CLOSED]);
  } finally { stopPolling(); }
});

for (const [what, change] of [
  ['an agent is working in the session again', w => { w.infos = [{ ...FINISHED, agent: 'working', fg: 'claude', finish_fg: 'claude' }]; }],
  ['a program other than a shell is in front', w => { w.infos = [{ ...FINISHED, fg: 'vim', finish_fg: 'vim' }]; }],
  ['a prompt is queued for the session', () => { ctx.queueCache = { ...ctx.queueCache, items: [{ id: 'q1', session: SESSION }] }; }],
  ['the rule now keeps the card', () => { ctx.settings.inbound = { ...ctx.settings.inbound, rules: [{ ...RULE, finish: 'keep' }] }; }],
  ['the rule is gone', () => { ctx.settings.inbound = { ...ctx.settings.inbound, rules: [] }; }],
]) {
  test(`after a failed close, the queued close is dropped when ${what}`, async () => {
    const w = world();
    try {
      await failedClose(w);
      change(w);
      await polls(3);
      assert.deepEqual(kept(), ['run']);
      assert.deepEqual(closes(w), [1, 0, 0], 'the session is not killed and nothing more is cancelled');
      assert.equal(ctx.queueCache.items.length, what.includes('queued') ? 1 : 0, 'a queued prompt stays queued');
      assert.deepEqual(toasts(), [FAILED]);
    } finally { stopPolling(); }
  });
}

test('after a failed close, a session that is gone keeps its card stopped', async () => {
  const w = world();
  try {
    await failedClose(w);
    w.infos = [GONE];   // the tmux server was lost, or the session was killed elsewhere
    await polls(3);
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['run', 'stopped']]);
    assert.deepEqual(closes(w), [1, 0, 0], 'absence is not deletion authority');
    assert.deepEqual(toasts(), [FAILED]);
  } finally { stopPolling(); }
});

for (const failing of ['queue_clear_sessions', 'kill_session']) {
  test(`a close that failed at ${failing} is tried again by every poll that reads the same, and toasts once`, async () => {
    const w = world();
    try {
      w.fail.add(failing);
      await polls(5);
      assert.deepEqual(kept(), ['run']);
      assert.equal(count(w, failing), 3, 'the third, fourth and fifth poll each tried');
      assert.deepEqual(toasts(), [FAILED]);
      w.fail.clear();
      await polls(1);
      assert.deepEqual(kept(), []);
      assert.deepEqual(toasts(), [FAILED, CLOSED]);
    } finally { stopPolling(); }
  });
}

test('a run that must be reviewed qualifies only after its final review, and a retry does not ask for it twice', async () => {
  const reviewed = () => runCard({ origin: { source: 'clock', badge: 'r1', key: 'k1', reviewEach: true } });
  let w = world(reviewed());
  try {
    await polls(5);
    assert.deepEqual(kept(), ['run'], 'no final review: the run never qualifies');
    assert.deepEqual(closes(w), [0, 0, 0]);
    // reviewed, then the close fails AFTER its schedule cancellation landed,
    // which erased the final-review fact
    w = world(reviewed());
    ctx.queueCache = { ...ctx.queueCache, review_completed: [SESSION] };
    await failedClose(w, 'kill_session');
    assert.deepEqual(ctx.queueCache.review_completed, []);
    await polls(1);
    assert.deepEqual(kept(), [], 'the retry closes the live session it left behind');
    assert.deepEqual(toasts(), [FAILED, CLOSED]);
  } finally { stopPolling(); }
});

test('a close that had already killed the session when the Board write failed is not tried again: the card stays stopped', async () => {
  const w = world();
  try {
    await failedClose(w, 'save_board');
    assert.deepEqual(closes(w), [1, 1, 1]);
    await polls(3);
    assert.deepEqual(store.cards.map(c => [c.id, c.status]), [['run', 'stopped']]);
    assert.deepEqual(closes(w), [1, 1, 1], 'a session that is gone is never closed by the finish rule');
    assert.deepEqual(toasts(), [FAILED]);
  } finally { stopPolling(); }
});

test('a pane that opens while the close waits its turn keeps the card: nothing is cancelled or killed', async () => {
  const w = world();
  try {
    await polls(2);
    // another Board transaction is ahead of the close and its save is slow
    let release;
    w.saveGate = new Promise(resolve => { release = resolve; });
    const ahead = prepareCardsForServerRestart([]);
    const third = pollNow();   // reads no pane, queues the close behind `ahead`
    await new Promise(resolve => setTimeout(resolve, 20));
    show();
    w.saveGate = null;
    release();
    await ahead;
    assert.equal(await third, true);
    assert.deepEqual(kept(), ['run']);
    assert.deepEqual(closes(w), [0, 0, 1], 'only the other transaction was saved');
    assert.deepEqual(toasts(), [], 'giving the close up is not a failure');
    // the queued close is gone: later polls hold while the pane is there …
    await polls(4);
    assert.deepEqual(kept(), ['run']);
    assert.deepEqual(closes(w), [0, 0, 1]);
    // … and the run closes three readings after it is gone
    leave();
    await polls(3);
    assert.deepEqual(kept(), []);
    assert.deepEqual(toasts(), [CLOSED]);
  } finally { stopPolling(); }
});

test('a verified shell exit still retires a card whose pane is open', async () => {
  const card = { id: 'shell', projectId: 'P1', columnId: 'C1', title: 'shell', desc: '', cmd: '', dir: '/tmp',
    session: SESSION, status: 'running', launched: true };
  const w = world(card, []);
  try {
    show();   // Ctrl+D is pressed in the pane itself
    w.infos = [{ ...GONE, exited_normally: true }];
    await polls(1);
    assert.deepEqual(kept(), []);
    assert.deepEqual(closes(w), [1, 1, 1]);
    assert.deepEqual(toasts(), [t('session.closedExited', { name: 'shell' })]);
  } finally { stopPolling(); panes.clear(); }
});
