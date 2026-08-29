// Headless tests for the DOM-free frontend logic (ui/js/pure.js).
// Node built-ins only — no npm, no bundler:  node --test app/ui/test/
process.env.TZ = 'UTC'; // date math below assumes a fixed zone

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  sessionName, fmtMem, fmtEvery, minToHM, hmToMin, winHas, hasWindow,
  nextFire, groupQueue, groupSteps, itemDead, blockedBy,
  chainQuietHint, contextStatusKey, CHAIN_QUIET_SECS, shQuote, quickBarLayout, rectsOverlap,
  createExitRetirementTracker, createSerialTransactionQueue, deleteSessionsTransaction, sidebarGroups,
  copyExact, createTerminalSelectionModel, terminalSelectionEdgeLines,
  isComposingKeyEvent, isPlainShiftKeydown, shouldRouteImeKeydownThroughInput,
  terminalSelectionOverlayRows,
  terminalLinkMatches,
  createTerminalWheelAccumulator, terminalWheelLines,
  linkMenuItems,
  inlineRenameValue, persistOptimistically,
} from '../js/pure.js';

test('the completion bar reserves a non-overlapping row in only its pane', () => {
  const shown = quickBarLayout({ width: 800, height: 600, barHeight: 42, visible: true });
  assert.deepEqual(shown.terminal, { left: 0, top: 0, right: 800, bottom: 558 });
  assert.deepEqual(shown.bar, { left: 0, top: 558, right: 800, bottom: 600 });
  assert.equal(rectsOverlap(shown.terminal, shown.bar), false);
  const hidden = quickBarLayout({ width: 800, height: 600, barHeight: 42, visible: false });
  assert.equal(hidden.terminal.bottom, 600);
  assert.equal(rectsOverlap(hidden.terminal, hidden.bar), false);
  const adjacentPane = { left: 800, top: 0, right: 1200, bottom: 600 };
  assert.equal(rectsOverlap(shown.bar, adjacentPane), false);
});

test('shQuote leaves safe paths bare and single-quotes the rest', () => {
  assert.equal(shQuote('/Users/x/shot.png'), '/Users/x/shot.png');
  assert.equal(shQuote('~/.deck/drops/a-b_c.1.png'), '~/.deck/drops/a-b_c.1.png');
  assert.equal(shQuote('/tmp/my shot.png'), "'/tmp/my shot.png'");
  assert.equal(shQuote("/tmp/o'brien.png"), "'/tmp/o'\\''brien.png'");
  assert.equal(shQuote('/tmp/$HOME`x`;rm.png'), "'/tmp/$HOME`x`;rm.png'");
});

test('sessionName derives a safe slug + id suffix', () => {
  assert.equal(sessionName('My API Server!', 'Cw741'), 'deck-my-api-server-w741');
  assert.equal(sessionName('***', 'C1234'), 'deck-card-1234', 'all-symbol titles fall back');
  const long = sessionName('a'.repeat(60), 'Cabcd');
  assert.ok(long.length <= 'deck-'.length + 24 + 5, 'slug capped: ' + long);
  assert.ok(/^deck-[a-z0-9-]+-abcd$/.test(long));
  assert.ok(!sessionName('trail---', 'C0000').includes('--'), 'no dangling dashes');
});

test('fmtMem switches to gigabytes at 1024', () => {
  assert.equal(fmtMem(512), '512M');
  assert.equal(fmtMem(1023.4), '1023M');
  assert.equal(fmtMem(1024), '1.0G');
  assert.equal(fmtMem(1536), '1.5G');
});

test('fmtEvery prefers whole hours', () => {
  assert.equal(fmtEvery(3600), '1 h');
  assert.equal(fmtEvery(7200), '2 h');
  assert.equal(fmtEvery(300), '5 min');
});

test('minToHM / hmToMin round-trip', () => {
  assert.equal(minToHM(0), '00:00');
  assert.equal(minToHM(485), '08:05');
  assert.equal(hmToMin('08:05'), 485);
  assert.equal(hmToMin(minToHM(1439)), 1439);
});

test('winHas covers plain and midnight-wrapping windows', () => {
  assert.ok(winHas(9 * 60, 480, 1080));
  assert.ok(!winHas(18 * 60, 480, 1080), 'end exclusive');
  assert.ok(winHas(23 * 60, 1200, 480), 'wraps midnight');
  assert.ok(winHas(2 * 60, 1200, 480));
  assert.ok(!winHas(12 * 60, 1200, 480));
});

test('hasWindow rejects half-open and degenerate windows', () => {
  assert.ok(hasWindow({ win_from: 480, win_to: 1080 }));
  assert.ok(!hasWindow({ win_from: 480, win_to: null }));
  assert.ok(!hasWindow({ win_from: 600, win_to: 600 }));
  assert.ok(!hasWindow({}));
});

test('nextFire: cadence from last fire, never in the past', () => {
  const now = 1_000_000;
  assert.equal(nextFire({ last: now - 100, every: 300 }, now), now + 200);
  assert.equal(nextFire({ last: now - 900, every: 300 }, now), now, 'overdue fires now');
  assert.equal(nextFire({ every: 300 }, now), now, 'never fired = due now');
});

test('nextFire defers to the window opening (UTC)', () => {
  // 1970-01-02 02:00 UTC, window 08:00–18:00 → next fire 08:00 that day
  const now = 24 * 3600 + 2 * 3600;
  const t = nextFire({ every: 300, win_from: 480, win_to: 1080 }, now);
  assert.equal(t, 24 * 3600 + 8 * 3600);
  // inside the window: unchanged
  const noon = 24 * 3600 + 12 * 3600;
  assert.equal(nextFire({ every: 300, win_from: 480, win_to: 1080 }, noon), noon);
  // after close (20:00) → tomorrow 08:00
  const evening = 24 * 3600 + 20 * 3600;
  assert.equal(
    nextFire({ every: 300, win_from: 480, win_to: 1080 }, evening),
    48 * 3600 + 8 * 3600
  );
});

test('chainQuietHint tracks the quiet window', () => {
  assert.equal(CHAIN_QUIET_SECS, 180, 'must match scheduler.rs');
  assert.equal(chainQuietHint(42, true), ' · quiet 42s/180s');
  assert.equal(chainQuietHint(0, true), ' · quiet 0s/180s', 'fresh activity resets to zero');
  assert.equal(chainQuietHint(180, true), ' · quiet ✓');
  assert.equal(chainQuietHint(9999, true), ' · quiet ✓', 'capped, no runaway counter');
  assert.equal(chainQuietHint(null, true), '', 'no poll data yet — no hint');
  assert.equal(chainQuietHint(null, false), ' · session stopped', 'dead is not mislabeled ready');
});

test('scheduler context states stay closed and UI-localized', () => {
  assert.equal(contextStatusKey('ready'), 'queue.context.ready');
  assert.equal(contextStatusKey('foreground-different'), 'queue.context.differentProcess');
  assert.equal(contextStatusKey('session-replaced'), 'queue.context.replaced');
  assert.equal(contextStatusKey('unexpected-future-value'), 'queue.context.unknown');
});

const item = (id, mode, extra = {}) =>
  ({ id, mode, text: 't-' + id, state: 'pending', attempts: 0, ...extra });

test('groupQueue groups by explicit group id; rules stand alone', () => {
  const a = item('a', 'at', { group: 'a', seq: 1 });
  const c = item('c', 'chain', { group: 'a', seq: 2 });
  const r = item('r', 'every');
  const x = item('x', 'chain', { group: 'x', seq: 1 });
  const gs = groupQueue([a, c, r, x]);
  assert.equal(gs.length, 3);
  assert.deepEqual(gs[0].rows.map(i => i.id), ['a', 'c']);
  assert.deepEqual(gs[1].rows.map(i => i.id), ['r']);
  assert.deepEqual(gs[2].rows.map(i => i.id), ['x']);
});

test('groupSteps flattens embedded template steps in order', () => {
  const g = { rows: [item('r', 'every', { steps: ['s2', 's3'] }), item('c', 'chain')] };
  assert.deepEqual(groupSteps(g), ['t-r', 's2', 's3', 't-c']);
});

test('itemDead / blockedBy mirror the backend blocking rule', () => {
  const dead = item('h', 'chain', { group: 'g', seq: 1, state: 'failed', attempts: 8 });
  const retrying = item('h2', 'chain', { group: 'g2', seq: 1, state: 'failed', attempts: 3 });
  const tail = item('t', 'chain', { group: 'g', seq: 2 });
  assert.ok(itemDead(dead));
  assert.ok(!itemDead(retrying), 'still retrying ≠ dead');
  assert.equal(blockedBy(tail, [dead, tail]).id, 'h', 'dead head blocks the tail');
  assert.equal(blockedBy(dead, [dead, tail]), null, 'the head itself is not "blocked"');
  const t2 = item('t2', 'chain', { group: 'g2', seq: 2 });
  assert.equal(blockedBy(t2, [retrying, t2]), null, 'retrying head does not block');
  assert.equal(blockedBy(item('solo', 'chain'), []), null, 'ungrouped never blocks');
});

test('sidebar groups follow board order, count, and stable status order', () => {
  const project = { columns: [
    { id: 'working', name: 'Working' },
    { id: 'attention', name: 'A very long Attention board name' },
    { id: 'empty', name: 'Empty' },
  ] };
  const cards = [
    { id: 'a', columnId: 'attention', status: 'running', title: 'same' },
    { id: 'b', columnId: 'working', status: 'stopped', title: 'same' },
    { id: 'c', columnId: 'working', status: 'waiting', title: 'c' },
    { id: 'd', columnId: 'working', status: 'running', title: 'd' },
    { id: 'e', columnId: 'working', status: 'waiting', title: 'e' },
  ];
  const groups = sidebarGroups(project, cards);
  assert.deepEqual(groups.map(g => [g.column.id, g.count]), [['working', 4], ['attention', 1]]);
  assert.deepEqual(groups[0].sessions.map(c => c.id), ['c', 'e', 'd', 'b']);
  // moving a session changes groups immediately without relying on its title
  cards[1].columnId = 'attention';
  const moved = sidebarGroups(project, cards);
  assert.deepEqual(moved.map(g => [g.column.id, g.count]), [['working', 3], ['attention', 2]]);
  assert.deepEqual(moved[1].sessions.map(c => c.id), ['a', 'b']);
});

test('delete transaction keeps all cards on partial kill or board-save failure', async () => {
  const cards = [{ id: 'a' }, { id: 'b' }];
  let commits = 0;
  let persisted = 0;
  const partial = await deleteSessionsTransaction(cards, {
    cancel: async () => true,
    kill: async c => { if (c.id === 'b') throw new Error('kill refused'); },
    persist: async () => { persisted++; },
    commit: () => { commits++; },
  });
  assert.equal(partial.stage, 'kill');
  assert.deepEqual(partial.failed.map(x => x.card.id), ['b']);
  assert.equal(persisted, 0);
  assert.equal(commits, 0);

  const saveFail = await deleteSessionsTransaction(cards, {
    cancel: async () => true,
    kill: async () => {}, // already-missing sessions are idempotent success
    persist: async () => { throw new Error('disk full'); },
    commit: () => { commits++; },
  });
  assert.equal(saveFail.stage, 'persist');
  assert.equal(commits, 0);

  const retry = await deleteSessionsTransaction(cards, {
    cancel: async () => true,
    kill: async () => {},
    persist: async () => { persisted++; },
    commit: () => { commits++; },
  });
  assert.equal(retry.ok, true);
  assert.equal(commits, 1, 'only the successful retry commits deletion');
});

function boardHarness({ failWrites = 0 } = {}) {
  let state = {
    projects: [
      { id: 'p1', name: 'one', columns: [{ id: 'c1', name: 'Working' }] },
      { id: 'p2', name: 'two', columns: [{ id: 'c2', name: 'Working' }] },
    ],
    cards: [
      { id: 'a', projectId: 'p1', columnId: 'c1', title: 'A' },
      { id: 'b', projectId: 'p1', columnId: 'c1', title: 'B' },
    ],
  };
  let disk = JSON.stringify(state);
  let failures = failWrites;
  const writes = [];
  const queue = createSerialTransactionQueue({
    snapshot: () => state,
    persist: async (_candidate, json) => {
      writes.push(json);
      if (failures-- > 0) throw new Error('disk full');
      disk = json;
    },
    commit: candidate => { state = candidate; },
  });
  return { queue, get state() { return state; }, get disk() { return disk; }, writes };
}

test('all overlapping Board mutations serialize from the latest committed JSON', async () => {
  const h = boardHarness();
  const closeA = h.queue.enqueue(async draft => {
    await new Promise(resolve => setTimeout(resolve, 8));
    draft.cards = draft.cards.filter(c => c.id !== 'a');
  });
  const closeB = h.queue.enqueue(draft => { draft.cards = draft.cards.filter(c => c.id !== 'b'); });
  await Promise.all([closeA, closeB]);
  assert.deepEqual(h.state.cards, [], 'rapid close A+B deletes both');
  assert.equal(h.disk, JSON.stringify(h.state), 'memory and serialized JSON are identical');

  const h2 = boardHarness();
  await Promise.all([
    h2.queue.enqueue(draft => { draft.cards = draft.cards.filter(c => c.id !== 'a'); }),
    h2.queue.enqueue(draft => {
      const b = draft.cards.find(c => c.id === 'b');
      b.title = 'renamed'; b.columnId = 'c1';
    }),
  ]);
  assert.deepEqual(h2.state.cards.map(c => [c.id, c.title]), [['b', 'renamed']]);
  assert.equal(h2.disk, JSON.stringify(h2.state), 'close+rename/move loses nothing and revives nothing');

  const h3 = boardHarness();
  await Promise.all([
    h3.queue.enqueue(draft => {
      draft.projects = draft.projects.filter(p => p.id !== 'p1');
      draft.cards = draft.cards.filter(c => c.projectId !== 'p1');
    }),
    h3.queue.enqueue(draft => {
      draft.projects.find(p => p.id === 'p2').name = 'two-renamed';
      draft.cards.push({ id: 'x', projectId: 'p2', columnId: 'c2', title: 'new' });
    }),
  ]);
  assert.deepEqual(h3.state.projects.map(p => p.name), ['two-renamed']);
  assert.deepEqual(h3.state.cards.map(c => c.id), ['x']);
  assert.equal(h3.disk, JSON.stringify(h3.state));
});

test('a failed Board persist does not poison the next transaction or write an old snapshot', async () => {
  const h = boardHarness({ failWrites: 1 });
  await assert.rejects(h.queue.enqueue(draft => { draft.cards[0].title = 'not committed'; }));
  await h.queue.enqueue(draft => { draft.cards[1].title = 'second succeeds'; });
  assert.equal(h.state.cards[0].title, 'A');
  assert.equal(h.state.cards[1].title, 'second succeeds');
  assert.equal(h.disk, JSON.stringify(h.state));
  assert.ok(!h.disk.includes('not committed'));

  // A pending debounced edit is inserted before an immediate destructive
  // barrier by persistence.js; the queue core then applies both in order.
  await h.queue.enqueue(draft => { draft.projects[0].selected = 'c1'; });
  await h.queue.enqueue(draft => { draft.cards = draft.cards.filter(c => c.id !== 'a'); });
  await h.queue.enqueue(draft => {
    if (!draft.cards.some(c => c.id === 'a')) return { noop: true }; // duplicate close
    draft.cards = draft.cards.filter(c => c.id !== 'a');
  });
  assert.equal(h.state.projects[0].selected, 'c1');
  assert.deepEqual(h.state.cards.map(c => c.id), ['b']);
  assert.equal(h.disk, JSON.stringify(h.state));
});

test('natural shell exit keeps the pane/card on failures, retries, and never spams', async () => {
  const tracker = createExitRetirementTracker();
  const card = { id: 'a', status: 'running' };
  const outcomes = [false, false, true]; // cancel fail, Board persist fail, success
  let warnings = 0, paneCloses = 0, successes = 0;
  const hooks = {
    get: sid => sid === 'a' ? card : null,
    markStopped: c => { c.status = 'stopped'; },
    close: async () => outcomes.shift(),
    failed: () => { warnings++; },
    succeeded: () => { paneCloses++; successes++; },
  };
  tracker.observe('a');
  await tracker.drain(hooks);
  assert.equal(card.status, 'stopped');
  assert.equal(paneCloses, 0, 'cancel failure cannot close the pane');
  await tracker.drain(hooks);
  assert.equal(paneCloses, 0, 'Board save failure still keeps the pane');
  assert.equal(warnings, 1, 'same automatic error is reported once');
  await tracker.drain(hooks);
  assert.equal(successes, 1);
  assert.equal(paneCloses, 1, 'pane closes only after durable Board commit');
  assert.equal(tracker.pending('a'), false);
});

test('natural retirement is single-flight per sid while other sessions keep progressing', async () => {
  const tracker = createExitRetirementTracker();
  const cards = new Map([['a', { id: 'a' }], ['b', { id: 'b' }]]);
  let releaseA;
  const blockedA = new Promise(resolve => { releaseA = resolve; });
  const calls = new Map();
  const succeeded = [];
  const hooks = {
    get: sid => cards.get(sid), markStopped() {}, failed() {},
    close: async card => {
      calls.set(card.id, (calls.get(card.id) || 0) + 1);
      if (card.id === 'a') await blockedA;
      return { ok: true, applied: true };
    },
    succeeded: card => succeeded.push(card.id),
  };
  tracker.observe('a'); tracker.observe('b');
  const first = tracker.drain(hooks);
  await Promise.resolve();
  await tracker.drain(hooks);
  assert.equal(calls.get('a'), 1, 'overlapping drains cannot re-enter A');
  assert.equal(calls.get('b'), 1, 'B starts while A remains blocked');
  assert.deepEqual(succeeded, ['b']);
  releaseA();
  await first;
  assert.deepEqual(succeeded.sort(), ['a', 'b']);
  assert.equal(tracker.inFlight('a'), false);
});

test('retirement joiners and disposal cannot duplicate success callbacks', async () => {
  const tracker = createExitRetirementTracker();
  const card = { id: 'a' };
  let successes = 0;
  tracker.observe('a');
  await tracker.drain({
    get: () => card, markStopped() {}, failed() {},
    close: async () => ({ ok: true, applied: false }),
    succeeded: () => { successes++; },
  });
  assert.equal(successes, 0, 'a close-operation joiner must not own UI cleanup');

  let release;
  const blocked = new Promise(resolve => { release = resolve; });
  tracker.observe('a');
  const draining = tracker.drain({
    get: () => card, markStopped() {}, failed() {},
    close: async () => { await blocked; return { ok: true, applied: true }; },
    succeeded: () => { successes++; },
  });
  tracker.clear();
  release();
  await draining;
  assert.equal(successes, 0, 'disposed tracker suppresses late callbacks');
  assert.equal(tracker.pending('a'), false);
});

test('terminal selection model keeps anchor, reverses, clamps boundaries, and rejects stale replies', () => {
  assert.equal(terminalSelectionEdgeLines({ pointerY: 300, top: 0, bottom: 600 }), 0);
  assert.ok(terminalSelectionEdgeLines({ pointerY: 599, top: 0, bottom: 600 }) > 0);
  assert.ok(terminalSelectionEdgeLines({ pointerY: 1, top: 0, bottom: 600 }) < 0);

  const model = createTerminalSelectionModel();
  const first = model.begin({ row: 20, col: 4 });
  model.move({ row: 24, col: 12 });
  assert.equal(model.apply(first, { absolute_row: 100, at_top: false, at_bottom: false }), true);
  model.move({ row: 0, col: 2 });
  model.apply(first, { absolute_row: 20, at_top: false, at_bottom: false });
  model.move({ row: 24, col: 8 }); // reverse and shrink again
  const snap = model.snapshot();
  assert.deepEqual(snap.anchor, { row: 20, col: 4 }, 'anchor never moves');
  assert.deepEqual(snap.active, { row: 24, col: 8 });
  assert.equal(model.apply(first, { absolute_row: 0, at_top: true, at_bottom: false }), true);
  model.finish();
  assert.equal(model.snapshot().phase, 'selected');
  model.apply(first, { absolute_row: 1, at_top: false, at_bottom: false });
  assert.equal(model.snapshot().phase, 'selected', 'late status cannot reopen a completed drag');
  model.cancel();
  assert.equal(model.apply(first, { absolute_row: 999 }), false, 'late response is stale after cancel');
  const second = model.begin({ row: 2, col: 1 });
  assert.notEqual(second, first);
  assert.equal(model.apply(first, { absolute_row: 5 }), false, 'old gesture cannot mutate new anchor');
  model.finish();
  assert.equal(model.snapshot().phase, 'selected');
});

test('frozen selection overlay follows content coordinates, not viewport pixels', () => {
  const base = {
    startRow: 100, startCol: 4, endRow: 102, endCol: 7,
    rows: 10, cols: 80,
  };
  const before = terminalSelectionOverlayRows({ ...base, viewportTop: 96 });
  assert.deepEqual(before.map(x => [x.row, x.col, x.width, x.absoluteRow]), [
    [4, 4, 76, 100], [5, 0, 80, 101], [6, 0, 7, 102],
  ]);
  const after = terminalSelectionOverlayRows({ ...base, viewportTop: 99 });
  assert.deepEqual(after.map(x => [x.row, x.absoluteRow]), [[1, 100], [2, 101], [3, 102]]);
  assert.deepEqual(terminalSelectionOverlayRows({ ...base, viewportTop: 103 }), [],
    'highlight disappears when its content is outside the viewport');
  const reverse = terminalSelectionOverlayRows({
    startRow: 102, startCol: 7, endRow: 100, endCol: 4,
    viewportTop: 99, rows: 10, cols: 80,
  });
  assert.deepEqual(reverse, after, 'reverse endpoints normalize to the same content spans');
});

test('IME/dead-key events bypass every Deck keyboard shortcut', () => {
  for (const event of [
    { key: 'Process' }, { key: 'Dead' }, { key: 'Compose' },
    { key: '[', isComposing: true }, { key: '?', keyCode: 229 },
  ]) assert.equal(isComposingKeyEvent(event), true, JSON.stringify(event));
  assert.equal(isComposingKeyEvent({ key: '[', isComposing: false, keyCode: 219 }), false);
});

test('printable IME 229 keydowns defer to final InputEvent data', () => {
  for (const event of [
    { key: '?', code: 'Slash', keyCode: 229 },
    { key: '？', code: 'Slash', keyCode: 229 },
    { key: 'Process', code: 'Slash', keyCode: 229 },
    { key: 'Unidentified', code: 'Slash', keyCode: 229 },
    { key: 'Backspace', keyCode: 229, isComposing: true },
  ]) assert.equal(shouldRouteImeKeydownThroughInput(event), true, JSON.stringify(event));
  assert.equal(shouldRouteImeKeydownThroughInput({ key: '?', keyCode: 191 }), false);
  assert.equal(shouldRouteImeKeydownThroughInput({ key: 'Backspace', keyCode: 229 }), false,
    'non-composing control keys retain xterm handling');
  assert.equal(shouldRouteImeKeydownThroughInput({ key: 'Dead', keyCode: 0 }), false,
    'ordinary dead keys retain xterm dead-key handling');
  assert.equal(isPlainShiftKeydown({ key: 'Shift', code: 'ShiftLeft' }), true);
  assert.equal(isPlainShiftKeydown({ key: 'Shift', code: 'ShiftRight', ctrlKey: true }), false);
  assert.equal(isPlainShiftKeydown({ key: '?', code: 'Slash', shiftKey: true }), false,
    'the actual chord retains its shift modifier');
});

test('terminal wheel input preserves fractions and normalizes browser delta modes', () => {
  assert.equal(terminalWheelLines(28, 0, 24), 2);
  assert.equal(terminalWheelLines(3, 1, 24), 3);
  assert.equal(terminalWheelLines(1, 2, 24), 24);
  assert.equal(terminalWheelLines(1, 99, 24), 1 / 14, 'unknown modes stay pixel-like');
  assert.equal(terminalWheelLines(Number.NaN, 0, 24), 0);

  const wheel = createTerminalWheelAccumulator(3);
  wheel.add(0.3);
  assert.equal(wheel.ready(), false);
  wheel.add(0.3);
  assert.equal(wheel.take(), 1);
  assert.ok(Math.abs(wheel.pending() + 0.4) < 1e-9, 'rounding error is retained');
  wheel.add(4.4);
  assert.equal(wheel.take(), 3, 'one frame is capped without dropping excess');
  assert.equal(wheel.take(), 1);
  wheel.add(-1.6);
  assert.equal(wheel.take(), -2, 'reverse inertial deltas remain directional');
  const reverseHalf = createTerminalWheelAccumulator();
  reverseHalf.add(-0.5);
  assert.equal(reverseHalf.take(), -1, 'negative half-lines round symmetrically');
});

test('terminal clipboard payload remains exact for 2,500 deterministic Unicode rows', async () => {

  const text = '\n中文 English 😀 e\u0301\n```rust\nfn main() {}\n```\n' + 'x'.repeat(10000) + '\n\n';
  let received = null;
  assert.equal(await copyExact(text, async value => { received = value; }), text.length);
  assert.equal(received, text, 'no trimming, normalization, or chunk loss');
  await assert.rejects(copyExact(text, async () => { throw new Error('denied'); }));

  const long = Array.from({ length: 2500 }, (_, i) => {
    if (i === 20) return '```rust';
    if (i === 24) return '```';
    if (i === 25) return '\ttrailing spaces   ';
    if (i === 26) return '';
    if (i === 100) return '路径/' + '无空格'.repeat(300) + '/file.rs';
    return `${String(i + 1).padStart(4, '0')} 中文 😀 e\u0301 👩‍💻️`;
  }).join('\n');
  let pasted = '';
  await copyExact(long, async value => { pasted = value; });
  assert.equal(pasted, long);
  assert.equal(pasted.split('\n').length, 2500);
  assert.equal(pasted.length, long.length, 'Unicode/code blocks/long lines remain byte-for-byte ordered');
});

test('path menu adds parent actions while the URL menu remains unchanged', () => {
  assert.deepEqual(linkMenuItems('url').map(x => x.action), ['url', 'copy']);
  assert.deepEqual(linkMenuItems('path').map(x => x.action), [
    'editor', 'editor-parent', 'session-parent', 'reveal', 'copy',
  ]);
});

test('terminal link grammar covers relative, quoted, Unicode and line suffix paths', () => {
  const line = [
    '/tmp/code.rs', './file.rs:42', '../src/main.rs:42:7', '~/.deck/settings.json',
    'file.rs:9', '目录/组合é/😀.txt', '"/tmp/space name.rs":12:3',
    "'../空 格/emoji😀.md':5", 'https://example.com/a',
  ].join(' | ');
  const links = terminalLinkMatches(line);
  assert.deepEqual(links.map(link => link.value), [
    '/tmp/code.rs', './file.rs:42', '../src/main.rs:42:7', '~/.deck/settings.json',
    'file.rs:9', '目录/组合é/😀.txt', '"/tmp/space name.rs":12:3',
    "'../空 格/emoji😀.md':5", 'https://example.com/a',
  ]);
  assert.equal(links.at(-1).kind, 'url');
  assert.deepEqual(terminalLinkMatches('missing.rs), ordinary_word, foo.').map(x => x.value),
    ['missing.rs'], 'line punctuation is excluded and plain words are ignored');
  assert.deepEqual(terminalLinkMatches([
    'connect 192.168.31.120:6443 failed',
    'localhost 127.0.0.1:8080',
    'version 1.2.3 and v2.4',
    'numeric 12.34',
  ].join(' | ')), [], 'IPv4, ports and dotted versions are not file paths');
  assert.deepEqual(terminalLinkMatches('http://192.168.31.120:6443/a file.123 report.rs'), [
    { kind: 'url', value: 'http://192.168.31.120:6443/a', index: 0 },
    { kind: 'path', value: 'file.123', index: 29 },
    { kind: 'path', value: 'report.rs', index: 38 },
  ], 'URLs and plausible filenames retain their existing ownership');
});

test('rename Enter/Escape/empty semantics and persistence rollback are deterministic', async () => {
  assert.equal(inlineRenameValue('old', ' new ', true), 'new');
  assert.equal(inlineRenameValue('old', 'old', true), null);
  assert.equal(inlineRenameValue('old', '   ', true), null, 'empty cannot overwrite title');
  assert.equal(inlineRenameValue('old', 'new', false), null, 'Escape cancels');
  assert.equal(inlineRenameValue('old', '   ', true, true), '', 'descriptions may opt into empty');

  let value = 'old';
  const failed = await persistOptimistically({
    apply: () => { value = 'new'; },
    persist: async () => { throw new Error('disk full'); },
    rollback: () => { value = 'old'; },
  });
  assert.equal(failed, false);
  assert.equal(value, 'old', 'persistence failure restores the visible title');
  const ok = await persistOptimistically({
    apply: () => { value = 'new'; }, persist: async () => {}, rollback: () => { value = 'old'; },
  });
  assert.equal(ok, true);
  assert.equal(value, 'new');
});
