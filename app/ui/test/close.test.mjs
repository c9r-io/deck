import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };

const { ctx, listeners, store } = await import('../js/state.js');
const { closeSelectedCards, markSessionsStoppedForServerRestart, provider } = await import('../js/board.js');
const { cfmDone } = await import('../js/dialogs.js');
const { t } = await import('../js/i18n.js');
listeners.clear();

function card(id, status = 'running') {
  return { id, projectId: 'P1', columnId: 'C1', title: id, desc: '', cmd: '', dir: '/tmp',
    session: `deck-${id}`, status, launched: true };
}

function setup(cards, kill = async () => {}) {
  store.projects = [{ id: 'P1', name: 'Project', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = cards;
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'queue_clear_sessions') return;
    if (cmd === 'kill_session') return kill(args);
    if (cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  return calls;
}

test('a stopped card after server restart retires through the ordinary transaction', async () => {
  const calls = setup([card('A'), card('B')]);
  markSessionsStoppedForServerRestart();
  assert.deepEqual(store.cards.map(c => c.status), ['stopped', 'stopped']);
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: true });
  assert.deepEqual(store.cards.map(c => c.id), ['B']);
  assert.deepEqual(calls.map(([cmd]) => cmd), ['queue_clear_sessions', 'kill_session', 'save_board']);
  assert.equal(calls[1][1].name, 'deck-A');
  assert.equal(JSON.parse(calls[2][1].data).cards.length, 1);
});

test('an uncertain runtime failure retains the stopped card and does not write the Board', async () => {
  const calls = setup([card('A', 'stopped')], async () => { throw new Error('tmux timeout'); });
  assert.deepEqual(await provider.close('A', { detail: true, quiet: true }),
    { ok: false, applied: false, stage: 'kill', admitted: false });
  assert.equal(store.cards.length, 1);
  assert.ok(!calls.some(([cmd]) => cmd === 'save_board'));
});

test('live close still kills before retirement and repeated close is a no-op', async () => {
  const calls = setup([card('A')]);
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: true });
  assert.deepEqual(await provider.close('A', { detail: true }), { ok: true, applied: false });
  assert.equal(store.cards.length, 0);
  assert.deepEqual(calls.map(([cmd]) => cmd), ['queue_clear_sessions', 'kill_session', 'save_board']);
});

/* Batch close (Needs attention › Turn ended): one confirmation for the
   ticked cards, each through the ordinary close; a card with a reminder or
   a scratchpad is left for its own question; the list is asked again after
   the confirmation; a declined confirmation closes nothing. */
test('a batch close confirms once, closes through the ordinary path and leaves protected cards', async () => {
  const $ = id => fakeDocument.getElementById(id);
  const withReminder = { ...card('C'), reminder: { id: 'r1', revision: 1, dueAt: Date.now() + 3_600_000 } };
  const withScratchpad = { ...card('D'), buffer: { entries: [{ id: 'e1', text: 'kept' }] } };
  const calls = setup([card('A'), card('B'), withReminder, withScratchpad, card('E')]);
  const lastToast = () => $('toasts').children.at(-1)?.textContent;
  // declined: nothing happens
  let pending = closeSelectedCards(['A', 'B', 'C', 'D']);
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal($('cfm-msg').textContent, t('attention.closePickedConfirm', { count: '2', live: t('attention.closePickedLive') }));
  cfmDone(false);
  assert.deepEqual(await pending, []);
  assert.equal(store.cards.length, 5);
  assert.ok(!calls.some(([cmd]) => cmd === 'kill_session'));
  // confirmed: A and B close through the ordinary transaction; C, D and the unticked E stay
  const asked = [];
  pending = closeSelectedCards(['A', 'B', 'C', 'D'], async id => { asked.push(id); return true; });
  await new Promise(resolve => setTimeout(resolve, 0));
  cfmDone(true);
  assert.deepEqual(await pending, ['A', 'B']);
  assert.deepEqual(asked, ['A', 'B'], 'the list is asked again after the confirmation, for the closable ones');
  assert.deepEqual(store.cards.map(c => c.id), ['C', 'D', 'E']);
  assert.deepEqual(calls.filter(([cmd]) => cmd === 'kill_session').map(([, args]) => args.name), ['deck-A', 'deck-B']);
  assert.equal(lastToast(), `${t('attention.closedPicked', { count: '2' })} ${t('attention.closedKept', { count: '2' })}`);
  assert.equal(ctx.destructiveCards.size, 0);
});

test('a card that left the list during the confirmation stays, and only protected cards ask nothing', async () => {
  const $ = id => fakeDocument.getElementById(id);
  setup([card('A'), card('B'), { ...card('C'), buffer: { collecting: true, entries: [] } }]);
  const pending = closeSelectedCards(['A', 'B'], async id => id !== 'B');
  await new Promise(resolve => setTimeout(resolve, 0));
  cfmDone(true);
  assert.deepEqual(await pending, ['A']);
  assert.deepEqual(store.cards.map(c => c.id), ['B', 'C']);
  assert.equal($('toasts').children.at(-1).textContent,
    `${t('attention.closedPicked', { count: '1' })} ${t('attention.closedMoved', { count: '1' })}`);
  // every ticked card protected: no confirmation, a note, nothing closed
  $('cfm-msg').textContent = 'untouched';
  assert.deepEqual(await closeSelectedCards(['C']), []);
  assert.equal($('cfm-msg').textContent, 'untouched');
  assert.equal($('toasts').children.at(-1).textContent, t('attention.closeKeptAll'));
  assert.deepEqual(await closeSelectedCards(['gone']), [], 'an id no card has is nothing to do');
});
