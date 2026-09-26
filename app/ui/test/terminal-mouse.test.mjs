import test from 'node:test';
import assert from 'node:assert/strict';
import { keepLocalTerminalMouse } from '../js/terminal-mouse.js';

test('outer tmux mouse requests preserve local selection without swallowing other modes', () => {
  let handler;
  const disposable = { dispose() {} };
  const term = { parser: { registerCsiHandler(id, callback) {
    assert.deepEqual(id, { prefix: '?', final: 'h' });
    handler = callback;
    return disposable;
  } } };
  assert.equal(keepLocalTerminalMouse(term), disposable);
  for (const mode of [9, 1000, 1001, 1002, 1003, 1005, 1006, 1015, 1016]) {
    assert.equal(handler([mode]), true);
  }
  assert.equal(handler([1000, 1006]), true);
  for (const params of [[], [1], [25], [1049], [2004], [1000, 25], [[1000, 1]]]) {
    assert.equal(handler(params), false, JSON.stringify(params));
  }
});
