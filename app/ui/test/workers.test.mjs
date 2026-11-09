// Worker cards (workers.js) against the fake document: the label a worker
// card carries, the one take-over call a user's input makes, the hand-back
// button, and that a card that is no worker card gets nothing.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { handBack, initWorkers, noteHumanInput, refreshWorkers, workerChip, workerOf } = await import('../js/workers.js');
const { store } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');

function backend(list) {
  const calls = [];
  let current = list;
  window.__TAURI__ = {
    event: { listen: async () => {} },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'mcp_workers_ui') return structuredClone(current);
      if (cmd === 'mcp_worker_take_over') { current = current.map(w => w.cardId === args.cardId ? { ...w, takenOver: true } : w); return true; }
      if (cmd === 'mcp_worker_hand_back') { current = current.map(w => w.cardId === args.cardId ? { ...w, takenOver: false } : w); return true; }
      return null;
    } },
  };
  return { calls, named: name => calls.filter(([cmd]) => cmd === name) };
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));

test('a worker card is labelled with its client and taken over once by the user’s input', async () => {
  setLocale('en');
  let renders = 0;
  initWorkers({ render: () => { renders++; } });
  store.cards = [
    { id: 'S1', session: 'deck-w1', title: 'Fix A' },
    { id: 'S2', session: 'deck-plain', title: 'Mine' },
  ];
  const be = backend([{ cardId: 'S1', clientName: 'Orchestrator', takenOver: false }]);
  await refreshWorkers();
  assert.ok(renders > 0, 'a new list repaints the Board');
  assert.equal(workerOf('S2'), null);
  assert.equal(workerChip({ id: 'S2' }), null, 'an ordinary card has no label');
  const chip = workerChip({ id: 'S1' });
  assert.equal(chip.tagName?.toLowerCase?.() ?? 'span', 'span');
  assert.equal(chip.textContent, t('workers.driven', { name: 'Orchestrator' }));
  assert.equal(chip.className, 'card-worker');
  // input into an ordinary card's pane: nothing
  noteHumanInput('deck-plain');
  await settle();
  assert.equal(be.named('mcp_worker_take_over').length, 0);
  // input into the worker card's pane: one call, the label turns into a button
  noteHumanInput('deck-w1');
  noteHumanInput('deck-w1');
  await settle(); await settle();
  assert.deepEqual(be.named('mcp_worker_take_over'), [['mcp_worker_take_over', { cardId: 'S1' }]]);
  assert.equal(workerOf('S1').takenOver, true);
  const taken = workerChip({ id: 'S1' });
  assert.equal(taken.className, 'card-worker taken');
  assert.equal(taken.textContent, t('workers.takenOver', { name: 'Orchestrator' }));
  assert.equal(typeof taken.onclick, 'function');
  // the hand-back is the user's click, one call
  await handBack('S1');
  assert.deepEqual(be.named('mcp_worker_hand_back'), [['mcp_worker_hand_back', { cardId: 'S1' }]]);
  assert.equal(workerOf('S1').takenOver, false);
});

test('a worker list that cannot be read changes nothing', async () => {
  initWorkers({ render: () => {} });
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async () => { throw new Error('down'); } } };
  const before = workerOf('S1');
  await refreshWorkers();
  assert.deepEqual(workerOf('S1'), before);
});
