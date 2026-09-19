import test from 'node:test';
import assert from 'node:assert/strict';
import { createTerminalCopy, writeClipboard } from '../js/terminal-clipboard.js';
import { b64ToU8, strToB64 } from '../js/terminal-bytes.js';

const settle = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { resolve, reject, promise }; };
function fixture() {
  const logs = [], writes = [], notices = [], unavailable = [];
  let deck = true, native = true, text = 'native text', copied = 'deck text', clock = 0;
  let identity = { run: 1, pane: 2, selection: 3 };
  let other = { count: 0, ageMs: -1, context: null };
  const handle = createTerminalCopy({
    selection: { hasSelection: () => deck, traceContext: () => identity, traceUnavailable: trace => unavailable.push(trace) },
    term: { hasSelection: () => native, getSelection: () => text },
    copySelection: () => copied, elsewhere: () => other,
    write: async (text, trace) => { writes.push({ text, trace }); },
    log: (...args) => logs.push(args), notice: key => notices.push(key), now: () => clock,
  });
  return { handle, logs, writes, notices, unavailable,
    select: (d, n) => { deck = d; native = n; }, copy: value => { copied = value; },
    other: value => { other = value; }, identity: value => { identity = value; },
    text: value => { text = value; }, time: value => { clock = value; },
    key: values => handle({ type: 'keydown', metaKey: true, key: 'c', preventDefault() {}, ...values }),
  };
}

test('copy prefers the Deck token, captures identity before awaiting and then supports native selection', async () => {
  const f = fixture(), read = deferred(); f.copy(read.promise);
  assert.equal(f.key(), true); f.identity({ run: 1, pane: 2, selection: 8 });
  read.resolve('exact\nbytes '); await settle();
  assert.deepEqual(f.writes, [{ text: 'exact\nbytes ', trace: { run: 1, pane: 2, selection: 3, attempt: 1 } }]);
  assert.equal(f.logs.at(-1)[1], 'success');
  assert.equal(f.logs.at(-1)[4].selection, 3);
  f.select(false, true); assert.equal(f.key({ key: 'C' }), true); await settle();
  assert.equal(f.writes[1].text, 'native text'); assert.equal(f.writes[1].trace.attempt, 2);
  assert.equal(f.key({ type: 'keyup' }), false); assert.equal(f.key({ metaKey: false }), false);
  assert.equal(f.key({ key: 'v' }), false); assert.deepEqual(f.notices, []);
});

test('empty copy reports elsewhere without copying it and throttles repeated notices', () => {
  const f = fixture(); f.select(false, false); f.key(); f.key();
  assert.deepEqual(f.notices, ['error.copyEmpty']);
  f.other({ count: 1, ageMs: 20, context: { run: 1, pane: 9, selection: 7 } }); f.key();
  assert.deepEqual(f.notices, ['error.copyEmpty', 'error.copyElsewhere']);
  assert.equal(f.logs.at(-1)[1], 'source-elsewhere'); assert.equal(f.logs.at(-1)[4].pane, 9);
  f.time(1600); f.key(); assert.equal(f.notices.length, 3);
  assert.equal(f.unavailable.length, 4); assert.deepEqual(f.writes, []);
});

test('vanished and failed snapshots never write the clipboard and report the captured attempt', async () => {
  for (const reason of [null, 'selection-missing', 'other failure']) {
    const f = fixture(), read = deferred(); f.copy(read.promise); f.key();
    if (reason) read.reject(reason); else read.resolve('');
    await settle(); assert.deepEqual(f.writes, []);
    assert.equal(f.logs.at(-1)[1], reason === null ? 'selection-vanished' : reason === 'selection-missing' ? 'selection-missing' : 'snapshot-failed');
    assert.equal(f.logs.at(-1)[4].attempt, 1);
    assert.deepEqual(f.notices, [reason === null ? 'error.copyEmpty' : 'error.copy']);
  }
});

test('native clipboard failure falls back to web only when available; both retain exact text and closed diagnostics', async () => {
  const text = ' 中文😀\n\n trailing ', trace = { run: 1, pane: 2, attempt: 3 };
  for (const mode of ['native', 'fallback', 'unavailable', 'failed']) {
    const nativeWrites = [], webWrites = [], logs = [];
    globalThis.window = { __TAURI__: { core: { invoke: async (cmd, args) => {
      if (cmd === 'ui_event') { logs.push(args); return; }
      assert.equal(cmd, 'write_clipboard'); nativeWrites.push(args.text);
      if (mode !== 'native') throw new Error('native unavailable');
    } } } };
    Object.defineProperty(globalThis, 'navigator', { configurable: true, value: {
      clipboard: mode === 'unavailable' ? undefined : { writeText: async value => {
        webWrites.push(value); if (mode === 'failed') throw new Error('web unavailable');
      } },
    } });
    if (['failed', 'unavailable'].includes(mode)) await assert.rejects(writeClipboard(text, trace), /unavailable/);
    else assert.equal(await writeClipboard(text, trace), text.length);
    assert.deepEqual(nativeWrites, [text]);
    assert.deepEqual(webWrites, ['fallback', 'failed'].includes(mode) ? [text] : []);
    assert.deepEqual(logs.map(log => log.detail), mode === 'native' ? [] : mode === 'fallback'
      ? ['pbcopy-failed'] : ['pbcopy-failed', mode === 'failed' ? 'web-failed' : 'web-unavailable']);
    assert.ok(logs.every(log => log.context === trace && log.a === text.length));
    assert.ok(!JSON.stringify(logs).includes(text));
  }
});

test('PTY codecs preserve Unicode, control keys and arbitrary output bytes', () => {
  for (const text of ['', '中文😀 é\n\r\x03\x1b[200~', 'large '.repeat(20000)]) {
    assert.deepEqual(b64ToU8(strToB64(text)), new TextEncoder().encode(text));
  }
  assert.deepEqual([...b64ToU8('/wAB')], [255, 0, 1]);
  assert.throws(() => b64ToU8('%%%'));
});
