// Headless tests for the DOM-free frontend logic (ui/js/pure.js).
// Node built-ins only — no npm, no bundler:  node --test app/ui/test/
process.env.TZ = 'UTC'; // date math below assumes a fixed zone

import test from 'node:test';
import assert from 'node:assert/strict';
import {
  sessionName, fmtMem, fmtEvery, minToHM, hmToMin, winHas, hasWindow,
  nextFire, groupQueue, groupSteps, itemDead, blockedBy,
  chainQuietHint, CHAIN_QUIET_SECS, shQuote, quickBarBottom,
} from '../js/pure.js';

test('the completion bar never covers the line being typed', () => {
  const view = { viewH: 600, cellH: 20, barH: 40 };
  // prompt high up in the pane: the bar keeps its bottom-edge home
  assert.equal(quickBarBottom({ ...view, cursorTop: 100 }), 0);
  assert.equal(quickBarBottom({ ...view, cursorTop: 534 }), 0, 'exactly fits below');
  // prompt at the bottom (the reported bug): the bar hops above the row
  assert.equal(quickBarBottom({ ...view, cursorTop: 560 }), 46);
  assert.equal(quickBarBottom({ ...view, cursorTop: 580 }), 26, 'last line');
  // …and stays fully inside the view even for an absurd cursor/bar combo
  assert.equal(quickBarBottom({ ...view, cursorTop: 0 }), 0);
  assert.equal(quickBarBottom({ viewH: 60, cellH: 20, barH: 40, cursorTop: 30 }), 20);
  assert.equal(quickBarBottom({ viewH: 30, cellH: 20, barH: 40, cursorTop: 10 }), 0);
  // degenerate inputs (pane mid-teardown, bar not laid out yet) → bottom
  for (const bad of [{ viewH: 0 }, { barH: 0 }, { cursorTop: null }]) {
    assert.equal(quickBarBottom({ ...view, cursorTop: 580, ...bad }), 0);
  }
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
  assert.equal(chainQuietHint(null, false), ' · ready', 'dead session counts as quiet');
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
