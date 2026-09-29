import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { isTerminalAutoReply } from '../js/pure.js';
import { createTerminalDataHandler, terminalInputDiagnostic } from '../js/terminal-input.js';
import { keepLocalTerminalMouse } from '../js/terminal-mouse.js';
const { Terminal } = createRequire(import.meta.url)('../vendor/xterm.js');
const parse = (term, data) => new Promise(resolve => term.write(data, resolve));

test('vendored xterm status reply passes the production input boundary exactly once', async () => {
  const term = new Terminal({ allowProposedApi: true });
  const pane = {}, replies = [], sent = [];
  const handler = createTerminalDataHandler({ pane,
    blocked: () => true,
    onInput: () => assert.fail('Replies must not update the input mirror'),
    hasSelection: () => true,
    cancelSelection: () => assert.fail('Replies must not revoke selection'),
    scrolled: () => true,
    goLive: () => assert.fail('Replies must not leave the frozen view'),
    write: data => sent.push(data),
  });
  term.onData(data => { replies.push(data); handler(data); });
  try {
    await parse(term, '\x1b[5n');
    assert.deepEqual(replies, ['\x1b[0n']);
    assert.equal(isTerminalAutoReply(replies[0]), true, 'Generated DSR status must be recognized');
    assert.deepEqual(sent, replies, 'Each reply must be forwarded unchanged exactly once');
  } finally { term.dispose(); }
});

test('real input cleanup queues replies and subsequent input in arrival order', async () => {
  const pane = {}, events = [];
  let release, selected = true;
  const cleanup = new Promise(resolve => { release = resolve; });
  const handle = createTerminalDataHandler({ pane, blocked: () => false,
    onInput: data => events.push(['input', data]), hasSelection: () => selected,
    cancelSelection: diagnostic => { selected = false; events.push(['cancel', diagnostic]); return cleanup; },
    scrolled: () => false, goLive: () => assert.fail('No scroll to leave'),
    write: data => events.push(['write', data]),
  });
  handle('a'); handle('\x1b[0n'); handle('b');
  assert.deepEqual(events, [['input', 'a'], ['cancel', { category: 1, length: 1 }], ['input', 'b']]);
  release(); await pane.liveQ;
  assert.deepEqual(events.slice(3), [['write', 'a'], ['write', '\x1b[0n'], ['write', 'b']]);
  assert.equal(pane.liveQ, null);
});

test('input shapes are bounded and contain no input content', () => {
  for (const [data, category] of [['', 0], ['secret /private/path 中文', 1], ['\r', 2],
    ['\x1b[A', 3], ['\x1b[200~secret\x1b[201~', 4]]) {
    assert.deepEqual(terminalInputDiagnostic(data), { category, length: data.length });
  }
  assert.deepEqual(terminalInputDiagnostic('s'.repeat(100000)), { category: 1, length: 99999 });
});

test('blocked input stays blocked and ordinary scroll cleanup precedes input', async () => {
  const pane = {}, events = [];
  let blocked = true;
  const handle = createTerminalDataHandler({ pane, blocked: () => blocked,
    onInput: () => events.push('input'), hasSelection: () => false,
    cancelSelection: () => assert.fail('No selection'), scrolled: () => true,
    goLive: () => { events.push('live'); return Promise.resolve(); },
    write: () => events.push('write'),
  });
  handle('x'); assert.deepEqual(events, []);
  blocked = false; handle('x'); await pane.liveQ;
  assert.deepEqual(events, ['input', 'live', 'write']);
});

test('vendored parser preserves the mouse-only and mixed-mode contract', async () => {
  const term = new Terminal({ allowProposedApi: true });
  keepLocalTerminalMouse(term);
  try {
    await parse(term, '\x1b[?1000;1006h');
    assert.equal(term.modes.mouseTrackingMode, 'none');
    await parse(term, '\x1b[?2004h');
    assert.equal(term.modes.bracketedPasteMode, true);
    await parse(term, '\x1b[?1000;2004h');
    assert.equal(term.modes.mouseTrackingMode, 'vt200', 'Mixed requests fall through intact');
    await parse(term, '\x1b[?1000;2004l');
    assert.equal(term.modes.mouseTrackingMode, 'none');
    assert.equal(term.modes.bracketedPasteMode, false);
  } finally { term.dispose(); }
});

test('keyboard controls and bracketed paste retain input effects and cleanup ordering', async () => {
  for (const data of ['\x1b[A', '\x1b', '\x1b[200~pasted 中文\x1b[201~', 'text\x1b[0n']) {
    const pane = {}, events = [];
    const handle = createTerminalDataHandler({ pane, blocked: () => false,
      onInput: value => events.push(['input', value]), hasSelection: () => true,
      cancelSelection: () => { events.push(['cancel']); return Promise.resolve(); },
      scrolled: () => false, goLive: () => assert.fail('No scroll transition'),
      write: value => events.push(['write', value]),
    });
    handle(data);
    assert.deepEqual(events, [['input', data], ['cancel']]);
    await pane.liveQ;
    assert.deepEqual(events, [['input', data], ['cancel'], ['write', data]]);
  }
});
