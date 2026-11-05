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

test('a forwarded click answers one closed word and logs it', async () => {
  const { forwardTerminalClick, terminalClickOutcome } = await import('../js/terminal-mouse.js');
  for (const [reply, word] of [[{ outcome: 'sent' }, 'sent'], [{ outcome: 'local' }, 'local'],
    [{ outcome: 'stale' }, 'stale'], [{ outcome: 'refused' }, 'refused'],
    [{ outcome: 'other' }, 'failed'], [null, 'failed'], [{}, 'failed']]) {
    assert.equal(terminalClickOutcome(reply), word);
  }
  const run = async ({ reply, sync = true, throws = false }) => {
    const calls = [], logs = [], debug = [], said = [];
    const outcome = await forwardTerminalClick({
      pane: { session: 'deck-card', syncSize: async () => sync },
      cell: { row: 7, col: 4 }, grid: { cols: 80, rows: 24 },
      invoke: async (name, args) => { calls.push({ name, args }); if (throws) throw new Error('tmux'); return reply; },
      log: (...entry) => logs.push(entry), debugLog: (...entry) => debug.push(entry),
      announce: word => said.push(word),
    });
    return { outcome, calls, logs, debug, said };
  };
  const sent = await run({ reply: { outcome: 'sent' } });
  assert.equal(sent.outcome, 'sent');
  assert.deepEqual(sent.calls, [{ name: 'click_session',
    args: { name: 'deck-card', column: 4, row: 7, cols: 80, rows: 24 } }]);
  assert.deepEqual(sent.logs, [['terminal-click', 'sent']]);
  assert.deepEqual(sent.said, ['sent']);
  const local = await run({ reply: { outcome: 'local' } });
  assert.deepEqual([local.logs, local.debug], [[], [['terminal-click', 'local']]]);
  // a grid the pane has not confirmed is never sent: the click is dropped
  const stale = await run({ reply: { outcome: 'sent' }, sync: false });
  assert.deepEqual([stale.outcome, stale.calls.length, stale.said], ['stale', 0, ['stale']]);
  const failed = await run({ throws: true });
  assert.deepEqual([failed.outcome, failed.logs], ['failed', [['terminal-click', 'failed']]]);
});
