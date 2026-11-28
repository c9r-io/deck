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

test('a worker card is labelled with its orchestrator card, and is the user’s once that card is gone', async () => {
  setLocale('en');
  const { workerLabel } = await import('../js/workers.js');
  const worker = { cardId: 'W1', clientName: 'Orchestrator', takenOver: false, orchestratorCardId: 'OA' };
  const cards = [{ id: 'OA', title: 'Orch A' }, { id: 'W1', title: 'Fix' }];
  const driven = workerLabel(worker, cards);
  assert.equal(driven.takenOver, false);
  assert.equal(driven.text, t('workers.driven', { name: 'Orch A' }));
  // no orchestrator card recorded: the client's name, as before
  assert.equal(workerLabel({ ...worker, orchestratorCardId: undefined }, cards).text, t('workers.driven', { name: 'Orchestrator' }));
  // the orchestrator card closed: held, and it says so
  const gone = workerLabel(worker, [{ id: 'W1', title: 'Fix' }]);
  assert.equal(gone.takenOver, true);
  assert.equal(gone.orphaned, true);
  assert.equal(gone.text, t('workers.orphaned', { name: 'Orchestrator' }));
  assert.equal(gone.title, t('workers.orphanedTitle', { name: 'Orchestrator', client: 'Orchestrator' }));
  // and the chip offers the hand-back button
  initWorkers({ render: () => {} });
  backend([worker]);
  await refreshWorkers();
  store.cards = [{ id: 'W1', session: 'deck-w', title: 'Fix' }];
  const chip = workerChip({ id: 'W1' });
  assert.equal(chip.className, 'card-worker taken');
  assert.equal(typeof chip.onclick, 'function');
});

test('a worker card’s driver groups the cards one orchestrator opened, its closed card’s apart', async () => {
  const { workerDriver } = await import('../js/workers.js');
  const worker = { cardId: 'W1', clientName: 'Orchestrator', takenOver: false, orchestratorCardId: 'OA' };
  const cards = [{ id: 'OA', title: 'Orch A' }, { id: 'W1', title: 'Fix' }];
  assert.deepEqual(workerDriver(worker, cards), { key: 'card:OA', name: 'Orch A', orphaned: false });
  // started from no card: its client
  assert.deepEqual(workerDriver({ ...worker, orchestratorCardId: undefined }, cards),
    { key: 'client:Orchestrator', name: 'Orchestrator', orphaned: false });
  // its orchestrator card closed: left behind, under the client's name, a
  // group of its own (not the client's live cards)
  assert.deepEqual(workerDriver(worker, [{ id: 'W1', title: 'Fix' }]),
    { key: 'left:Orchestrator', name: 'Orchestrator', orphaned: true });
});

test('a worker card that reported says so on its label, with the time and never the text', async () => {
  setLocale('en');
  const { workerLabel } = await import('../js/workers.js');
  const { fmtClock } = await import('../js/scheduler-model.js');
  const now = new Date(2026, 10, 19, 15, 0);
  const at = new Date(2026, 10, 19, 14, 5).getTime();
  const worker = { cardId: 'W1', clientName: 'Orchestrator', takenOver: false, orchestratorCardId: 'OA', reportedAt: at };
  const cards = [{ id: 'OA', title: 'Orch A' }, { id: 'W1', title: 'Fix' }];
  const label = workerLabel(worker, cards, now);
  const time = fmtClock(Math.floor(at / 1000), now);
  assert.equal(label.text, `${t('workers.driven', { name: 'Orch A' })} · ${t('workers.reported', { time })}`);
  assert.match(label.text, /05/);
  assert.ok(label.title.endsWith(t('workers.reportedTitle')));
  assert.equal(label.takenOver, false);
  /* held cards say it too */
  assert.match(workerLabel({ ...worker, takenOver: true }, cards, now).text, /reported/);
  assert.doesNotMatch(workerLabel({ ...worker, reportedAt: undefined }, cards, now).text, /reported/);
});

test('a report its orchestrator has not acknowledged says so until it is, then the plain label is back', async () => {
  setLocale('en');
  const { workerLabel } = await import('../js/workers.js');
  const { fmtClock } = await import('../js/scheduler-model.js');
  const now = new Date(2026, 10, 28, 15, 0);
  const at = new Date(2026, 10, 28, 14, 5).getTime();
  const worker = { cardId: 'W1', clientName: 'Orchestrator', takenOver: false, orchestratorCardId: 'OA', reportedAt: at, reportPending: true };
  const cards = [{ id: 'OA', title: 'Orch A' }, { id: 'W1', title: 'Fix' }];
  const time = fmtClock(Math.floor(at / 1000), now);
  const pending = workerLabel(worker, cards, now);
  assert.equal(pending.pending, true);
  assert.equal(pending.text, `${t('workers.driven', { name: 'Orch A' })} · ${t('workers.reportPending', { time })}`);
  assert.ok(pending.title.endsWith(t('workers.reportPendingTitle')));
  // acknowledged: "reported" again
  const acked = workerLabel({ ...worker, reportPending: false }, cards, now);
  assert.equal(acked.pending, false);
  assert.equal(acked.text, `${t('workers.driven', { name: 'Orch A' })} · ${t('workers.reported', { time })}`);
  // an untracked record (no field) and a card the user holds never say "not yet received"
  assert.equal(workerLabel({ ...worker, reportPending: undefined }, cards, now).pending, false);
  assert.equal(workerLabel({ ...worker, takenOver: true }, cards, now).pending, false);
  // the orchestrator card gone: the user's card, no pending label
  assert.equal(workerLabel(worker, [{ id: 'W1', title: 'Fix' }], now).pending, false);
});

test('the receipt label tells an untracked receipt from a received one', async () => {
  setLocale('en');
  const { workerLabel } = await import('../js/workers.js');
  const { fmtClock } = await import('../js/scheduler-model.js');
  const now = new Date(2026, 10, 28, 15, 0);
  const at = new Date(2026, 10, 28, 14, 5).getTime();
  const base = { cardId: 'W1', clientName: 'Orchestrator', takenOver: false, orchestratorCardId: 'OA', reportedAt: at, reportPending: false };
  const cards = [{ id: 'OA', title: 'Orch A' }, { id: 'W1', title: 'Fix' }];
  const time = fmtClock(Math.floor(at / 1000), now);
  const driven = t('workers.driven', { name: 'Orch A' });
  // acknowledged: null on an owned card with reports → "receipt unknown", never "received"
  const unknown = workerLabel({ ...base, receiptState: 'unknown' }, cards, now);
  assert.equal(unknown.text, `${driven} · ${t('workers.reportUnknown', { time })}`);
  assert.ok(unknown.title.endsWith(t('workers.reportUnknownTitle')));
  assert.equal(unknown.pending, false);
  const received = workerLabel({ ...base, receiptState: 'acknowledged' }, cards, now);
  assert.equal(received.text, `${driven} · ${t('workers.reportReceived', { time })}`);
  assert.notEqual(t('workers.reportReceived', { time }), t('workers.reportUnknown', { time }));
  // pending outranks the receipt word; unowned is the plain label
  const pending = workerLabel({ ...base, reportPending: true, receiptState: 'pending' }, cards, now);
  assert.equal(pending.text, `${driven} · ${t('workers.reportPending', { time })}`);
  const unowned = workerLabel({ ...base, receiptState: 'unowned' }, cards, now);
  assert.equal(unowned.text, `${driven} · ${t('workers.reported', { time })}`);
  // a card the user took over says nothing of receipt
  assert.ok(!workerLabel({ ...base, takenOver: true, receiptState: 'unknown' }, cards, now).text.includes(t('workers.reportUnknown', { time })));
});
