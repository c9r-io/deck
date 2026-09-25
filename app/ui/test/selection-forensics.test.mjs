import test from 'node:test';
import assert from 'node:assert/strict';
import { createSelectionForensics, COPY_NO_SELECTION_REASONS } from '../js/selection-forensics.js';

test('gesture observations retain only bounded relative cells and a closed promotion source', () => {
  const f = createSelectionForensics(() => 100);
  f.begin({ id: 7, cell: { row: 50, col: 40 }, detail: 1, native: false });
  f.observe('pointer', { row: 9999, col: -9999 });
  f.observe('compat', { row: 50, col: 41 });
  f.observe('up', { row: 50, col: 40 });
  f.upCoordinates(true);
  f.promote('compat'); f.end(false);
  const g = f.snapshot().gesture;
  assert.deepEqual([g.pointer, g.compat, g.up], [
    { row: 99, col: -999 }, { row: 0, col: 1 }, { row: 0, col: 0 },
  ]);
  assert.equal(g.pointerCrossed, true);
  assert.equal(g.compatCrossed, true);
  assert.equal(g.upCrossed, false);
  assert.equal(g.upCoordinatesPresent, true);
  assert.equal(g.promotionSource, 'compat');
  assert.ok(!JSON.stringify(g).includes('9999'));
});

test('a later ordinary click cannot erase the cause of a revoked selection', () => {
  const f = createSelectionForensics(() => 100);
  f.begin({ id: 1, cell: { row: 0, col: 0 }, detail: 1 });
  f.observe('pointer', { row: 0, col: 2 }); f.promote('pointer'); f.end(false);
  f.gestureOutcome(1, 'finished-and-live');
  f.selectionOutcome('finished-and-live', 17, 1);
  f.selectionOutcome('revoked-pointer', 17, 1, 2);
  f.begin({ id: 2, cell: { row: 0, col: 0 }, detail: 1 });
  f.observe('up', { row: 0, col: 0 }); f.end(false);
  assert.equal(f.snapshot().gesture.outcome, 'same-cell');
  assert.equal(f.reason(), 'selection-revoked-pointer');
  assert.deepEqual([f.snapshot().selection.token, f.snapshot().selection.revokerGestureId], [17, 2]);
});

test('empty finish, native end and active gesture have distinct outcomes', () => {
  const f = createSelectionForensics(() => 200);
  assert.equal(f.reason(), 'no-gesture');
  f.begin({ id: 3, cell: { row: 0, col: 0 }, native: false });
  assert.equal(f.reason(), 'gesture-active');
  f.promote('up'); f.end(false);
  f.gestureOutcome(3, 'promoted-empty'); f.selectionOutcome('promoted-empty', 9, 3);
  assert.equal(f.reason(), 'promoted-empty');
  f.nativeOutcome('native-end-pointer', 10, 3);
  assert.equal(f.reason(), 'native-range-ended');
  assert.ok(COPY_NO_SELECTION_REASONS.includes(f.reason()));
});

test('late compatibility mousedown updates native ownership without erasing observed movement', () => {
  const f = createSelectionForensics(() => 200);
  f.begin({ id: 11, cell: { row: 2, col: 3 }, detail: 0 });
  f.observe('pointer', { row: 2, col: 3 });
  f.nativeDetail(2, true);
  f.end(false);
  const g = f.snapshot().gesture;
  assert.equal(g.sawPointerMove, true);
  assert.equal(g.native, true);
  assert.equal(g.detail, 2);
});

test('a cancelled pending gesture is distinguishable from one still held', () => {
  const f = createSelectionForensics(() => 300);
  f.begin({ id: 12, cell: { row: 1, col: 1 }, detail: 1 });
  assert.equal(f.reason(), 'gesture-active');
  f.abort();
  assert.equal(f.reason(), 'gesture-cancelled');
});
