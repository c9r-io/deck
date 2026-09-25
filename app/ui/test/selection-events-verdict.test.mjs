import test from 'node:test';
import assert from 'node:assert/strict';
import { judgeSelectionEvents } from '../../../scripts/selection-events-verdict.mjs';

test('manual physical-input verdict requires all five gesture/copy paths with matching IDs', () => {
  const line = (code, detail, a, b, gesture) =>
    `[ui] ${code} ${detail} a=${a} b=${b} run=1 pane=2 selection=3 gesture=${gesture} attempt=1`;
  const log = [
    '[ui] smoke-check selection-events-ready a=1',
    line('terminal-selection', 'event-end', 0, 0, 1),
    line('terminal-selection', 'event-pointer', 0, 2, 2),
    line('terminal-selection', 'event-end', 1, 0, 2),
    line('terminal-selection', 'event-pointer', 4, 0, 3),
    line('terminal-selection', 'event-end', 1, 0, 3),
    line('terminal-selection', 'event-end', 1, 0, 4),
    line('terminal-copy', 'keydown-deck', 0, 0, 4),
    line('terminal-copy', 'success', 0, 0, 4),
    line('terminal-selection', 'event-mousedown', 2, 1, 5),
    line('terminal-copy', 'keydown-native', 0, 0, 5),
    line('terminal-copy', 'success', 0, 0, 5),
  ].join('\n');
  assert.deepEqual(judgeSelectionEvents(log), { ok: true, missing: [] });
  assert.deepEqual(judgeSelectionEvents(log.replace('keydown-native', 'keydown-none')).missing,
    ['doubleCopy']);
  assert.deepEqual(judgeSelectionEvents(''), { ok: false, missing: ['ready'] });
});
