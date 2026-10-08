import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { isTerminalAutoReply } from '../js/pure.js';
import { appendInputCleanup, createTerminalDataHandler, terminalInputDiagnostic } from '../js/terminal-input.js';
import { keepLocalTerminalMouse } from '../js/terminal-mouse.js';
const { Terminal } = createRequire(import.meta.url)('../vendor/xterm.js');
const tick = () => new Promise(resolve => setImmediate(resolve));
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

test('a new cleanup that rejects first does not let input pass the still pending tail', async () => {
  const pane = {}, writes = [], unhandled = [];
  const onUnhandled = error => unhandled.push(error);
  process.on('unhandledRejection', onUnhandled);
  let release, selected = true, scrolled = false;
  const first = new Promise(resolve => { release = resolve; });
  const handle = createTerminalDataHandler({ pane, blocked: () => false, onInput: () => {},
    hasSelection: () => selected, cancelSelection: () => { selected = false; return first; },
    scrolled: () => scrolled,
    goLive: () => { scrolled = false; return Promise.reject(new Error('cleanup failed')); },
    write: data => { writes.push(data); return Promise.resolve(); },
  });
  try {
    handle('a'); await tick();
    scrolled = true; handle('b'); await tick();
    handle('c'); await tick();
    assert.deepEqual(writes, [], 'The early rejection must not bypass the pending tail');
    release(); await tick(); await tick();
    assert.deepEqual(writes, ['a', 'b', 'c'], 'Each input is attempted once, in order');
    assert.equal(pane.liveQ, null);
    assert.deepEqual(unhandled, []);
  } finally { process.off('unhandledRejection', onUnhandled); }
});

test('an appended cleanup keeps the tail, and a finished link never clears a newer tail', async () => {
  const pane = {}, writes = [];
  let releaseOld, releaseNew;
  const handle = createTerminalDataHandler({ pane, blocked: () => false, onInput: () => {},
    hasSelection: () => false, cancelSelection: () => assert.fail('No selection'),
    scrolled: () => false, goLive: () => assert.fail('No scroll'),
    write: data => { writes.push(data); },
  });
  appendInputCleanup(pane, new Promise(resolve => { releaseOld = resolve; }));
  handle('a');
  appendInputCleanup(pane, new Promise(resolve => { releaseNew = resolve; }));
  const tail = pane.liveQ;
  handle('b');
  releaseNew(); await tick();
  assert.deepEqual(writes, [], 'The new cleanup finishing first releases nothing');
  assert.notEqual(pane.liveQ, null);
  assert.notEqual(pane.liveQ, tail);
  releaseOld(); await tick(); await tick();
  assert.deepEqual(writes, ['a', 'b']);
  assert.equal(pane.liveQ, null);
  appendInputCleanup(pane, undefined);
  await tick();
  assert.equal(pane.liveQ, null, 'A cleanup with nothing to wait for leaves no tail behind');
});

test('an input keeps the identity it was accepted with and a stale one is cancelled once, never written', async () => {
  const pane = {}, events = [];
  let identity = 1, release, scrolled = true;
  const handle = createTerminalDataHandler({ pane, blocked: () => false, onInput: () => {},
    hasSelection: () => false, cancelSelection: () => assert.fail('No selection'),
    scrolled: () => scrolled,
    goLive: () => { scrolled = false; return new Promise(resolve => { release = resolve; }); },
    bind: () => identity,
    stale: bound => bound === identity ? null : 'attachment',
    cancelled: reason => events.push(['cancel', reason]),
    write: (data, bound) => { events.push(['write', data, bound]); },
  });
  handle('old'); identity = 2; handle('new');
  release(); await tick(); await tick();
  assert.deepEqual(events, [['cancel', 'attachment'], ['write', 'new', 2]]);
  assert.equal(pane.liveQ, null);
});

test('an identity that is still being named joins the tail: the input and what follows keep their order', async () => {
  const pane = {}, events = [];
  let named;
  const request = { gen: null, wait: new Promise(resolve => { named = resolve; }) };
  const handle = createTerminalDataHandler({ pane, blocked: () => false, onInput: () => {},
    hasSelection: () => false, cancelSelection: () => assert.fail('No selection'),
    scrolled: () => false, goLive: () => assert.fail('No scroll'),
    bind: reply => reply ? { gen: 7 } : request.gen == null ? request : { gen: request.gen },
    stale: bound => bound.gen == null ? 'attachment' : null,
    cancelled: reason => events.push(['cancel', reason]),
    write: (data, bound) => { events.push([data, bound.gen]); },
  });
  handle('x'); handle('\x1b[0n');
  await tick();
  assert.deepEqual(events, [], 'The reply queues behind the waiting input');
  request.gen = 7; named(); await tick(); await tick();
  handle('y');
  assert.deepEqual(events, [['x', 7], ['\x1b[0n', 7], ['y', 7]]);
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
