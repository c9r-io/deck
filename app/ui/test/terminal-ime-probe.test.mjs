import test from 'node:test';
import assert from 'node:assert/strict';
import {
  IME_FLAGS, IME_PROBE_CAP, createImeProbe, imeCommitVerdict, wireImeProbe,
} from '../js/terminal-ime-probe.js';

// A textarea and the part of xterm 5.5's CompositionHelper that decides what
// is sent: the length at compositionstart, then `value.substring(start)`
// from a timer after compositionend. The IME inserts its commit at the caret.
function harness({ value = '', caret = value.length } = {}) {
  const textarea = { value, selectionStart: caret, selectionEnd: caret };
  const logs = [], timers = [];
  const probe = createImeProbe({ textarea, log: (...args) => logs.push(args),
    setTimer: fn => { timers.push(fn); return timers.length; }, clearTimer: () => {} });
  let xtermStart = 0;
  const h = {
    textarea, logs, probe,
    start() { xtermStart = textarea.value.length; probe.compositionstart(); },
    commit(text) {
      const at = textarea.selectionStart;
      textarea.value = textarea.value.slice(0, at) + text + textarea.value.slice(textarea.selectionEnd);
      textarea.selectionStart = textarea.selectionEnd = at + text.length;
      probe.compositionend({ data: text });
    },
    emit() { const sent = textarea.value.substring(xtermStart); if (sent) probe.data(sent); },
    flush() { for (const fn of timers.splice(0)) fn(); },
    type(text) { h.start(); h.commit(text); h.emit(); h.flush(); },
    lines: code => logs.filter(([c]) => c === code),
  };
  return h;
}

test('verdicts compare emitted with committed text', () => {
  assert.equal(imeCommitVerdict('中文', '中文'), 'ok');
  assert.equal(imeCommitVerdict('中文', '中文\r'), 'ok');
  assert.equal(imeCommitVerdict('中文', ''), 'missing');
  assert.equal(imeCommitVerdict('', ''), 'ok');
  assert.equal(imeCommitVerdict('', 'x'), 'extra');
  assert.equal(imeCommitVerdict('中文', '中文中文'), 'doubled');
  assert.equal(imeCommitVerdict('中文', '中'), 'partial');
  assert.equal(imeCommitVerdict('中文', '测试'), 'differs');
});

test('a matching commit logs nothing but the summary at dispose', () => {
  const h = harness();
  h.type('中文'); h.type('测试');
  assert.deepEqual(h.logs, []);
  h.probe.dispose();
  assert.deepEqual(h.logs, [['ime-summary', null, 2, 0]]);
  h.probe.dispose();
  assert.equal(h.logs.length, 1, 'dispose logs once');
});

test('a caret inside the textarea makes xterm send other text, and the probe says why', () => {
  const h = harness({ value: 'abcdef', caret: 2 });
  h.type('中文');
  assert.deepEqual(h.lines('ime-textarea'), [['ime-textarea', 'caret-inside', 4, 6]]);
  assert.deepEqual(h.lines('ime-commit'), [['ime-commit', 'differs', 2, 2]]);
  assert.deepEqual(h.lines('ime-context'),
    [['ime-context', 'caret-inside', IME_FLAGS.displaced | IME_FLAGS.earlier, 6]]);
  // it stays wrong for every later commit in the same textarea
  h.type('测试');
  assert.equal(h.lines('ime-commit').length, 2);
  h.probe.dispose();
  assert.deepEqual(h.lines('ime-summary'), [['ime-summary', null, 2, 2]]);
});

test('a range selection in the textarea is named at start and on the commit', () => {
  const h = harness({ value: 'abcdef' });
  h.textarea.selectionStart = 1; h.textarea.selectionEnd = 3;
  h.type('中');
  assert.deepEqual(h.lines('ime-textarea'), [['ime-textarea', 'range', 3, 6]]);
  assert.equal(h.lines('ime-context')[0][1], 'range');
});

test('missing, doubled and previous-commit emissions are told apart', () => {
  const h = harness();
  h.start(); h.commit('中文'); h.flush();
  assert.deepEqual(h.lines('ime-commit').at(-1), ['ime-commit', 'missing', 2, 0]);
  assert.deepEqual(h.lines('ime-context').at(-1), ['ime-context', 'caret-end', 0, 0]);
  h.start(); h.commit('测试'); h.probe.data('测试测试'); h.flush();
  assert.deepEqual(h.lines('ime-commit').at(-1), ['ime-commit', 'doubled', 2, 4]);
  h.start(); h.commit('你好'); h.probe.data('测试'); h.flush();
  assert.deepEqual(h.lines('ime-commit').at(-1), ['ime-commit', 'differs', 2, 2]);
  assert.equal(h.lines('ime-context').at(-1)[2] & IME_FLAGS.previous, IME_FLAGS.previous);
});

test('input right after the commit belongs to the commit window without a false mismatch', () => {
  const h = harness();
  h.start(); h.commit('中文'); h.emit(); h.probe.data('\r'); h.flush();
  assert.deepEqual(h.lines('ime-commit'), []);
});

test('an overlapping next composition is flagged and each commit takes its own text', () => {
  const h = harness();
  h.start(); h.commit('中文');
  h.start();                       // next composition before the first is judged
  h.probe.data('中文');            // the first commit's text (cut at the new start)
  h.commit('测试'); h.probe.data('测试'); h.flush();
  assert.deepEqual(h.lines('ime-commit'), []);
  const h2 = harness();
  h2.start(); h2.commit('中文'); h2.start(); h2.probe.data('中文zh'); h2.commit('测试'); h2.flush();
  assert.deepEqual(h2.lines('ime-commit')[0], ['ime-commit', 'missing', 2, 0]);
  assert.equal(h2.lines('ime-context')[0][2] & IME_FLAGS.overlapped, 0, 'the first matched; only the second is reported');
});

test('order anomalies, blur and shrink are recorded', () => {
  const h = harness({ value: 'abc' });
  h.probe.compositionend({ data: '' });
  h.start(); h.start();
  h.probe.blur();
  h.textarea.value = ''; h.textarea.selectionStart = h.textarea.selectionEnd = 0;
  h.probe.compositionend({ data: '中' }); h.flush();
  assert.deepEqual(h.lines('ime-state').map(l => l[1]),
    ['end-without-start', 'start-while-composing', 'blur-composing']);
  const flags = h.lines('ime-context').at(-1)[2];
  assert.equal(flags & IME_FLAGS.blurred, IME_FLAGS.blurred);
  assert.equal(flags & IME_FLAGS.shrank, IME_FLAGS.shrank);
  h.probe.blur();
  assert.equal(h.lines('ime-state').length, 3, 'blur outside a composition is silent');
});

test('terminal replies and non-strings are ignored; text after dispose is ignored', () => {
  const h = harness();
  h.start(); h.commit('中文'); h.probe.data('\x1b[0n'); h.probe.data(null); h.emit(); h.flush();
  assert.deepEqual(h.lines('ime-commit'), []);
  h.start(); h.commit('测试');
  h.probe.dispose();               // a commit still waiting is judged at dispose
  assert.deepEqual(h.lines('ime-commit'), [['ime-commit', 'missing', 2, 0]]);
  h.probe.compositionstart(); h.probe.compositionend({ data: 'x' }); h.probe.data('x'); h.probe.blur();
  assert.equal(h.logs.length, 3);
});

test('an empty commit followed by text is extra; an empty commit alone is fine', () => {
  const h = harness();
  h.start(); h.probe.compositionend({ data: '' }); h.flush();
  h.start(); h.probe.compositionend({}); h.probe.data('a'); h.flush();
  assert.deepEqual(h.lines('ime-commit'), [['ime-commit', 'extra', 0, 1]]);
});

test('each anomaly label is capped per pane; the summary still counts all', () => {
  const h = harness();
  for (let i = 0; i < IME_PROBE_CAP + 5; i++) { h.start(); h.commit('中'); h.flush(); }
  assert.equal(h.lines('ime-commit').length, IME_PROBE_CAP);
  h.probe.dispose();
  assert.deepEqual(h.lines('ime-summary'), [['ime-summary', null, IME_PROBE_CAP + 5, IME_PROBE_CAP + 5]]);
});

test('no logged value carries the committed or emitted text', () => {
  const h = harness({ value: 'secret-before', caret: 3 });
  h.type('机密内容'); h.start(); h.commit('另一段'); h.probe.data('完全不同'); h.flush(); h.probe.dispose();
  assert.ok(h.logs.length > 3);
  const allowed = new Set([null, 'missing', 'partial', 'differs', 'doubled', 'extra',
    'caret-end', 'caret-inside', 'range', 'unknown', 'blur-composing', 'start-while-composing', 'end-without-start']);
  for (const [code, detail, a, b] of h.logs) {
    assert.match(code, /^ime-(commit|context|textarea|state|summary)$/);
    assert.ok(allowed.has(detail), `closed label: ${detail}`);
    assert.ok(Number.isInteger(a) && Number.isInteger(b));
  }
});

test('a headless textarea gives unknown context; wiring listens on the textarea', () => {
  const listeners = {};
  const surface = { addEventListener: (type, fn, capture) => { assert.equal(capture, true); listeners[type] = fn; } };
  const logs = [];
  const probe = wireImeProbe(surface, (...args) => logs.push(args));
  listeners.compositionstart(); listeners.blur(); listeners.compositionend({ data: '中' });
  probe.dispose();
  assert.deepEqual(logs, [
    ['ime-state', 'blur-composing', 0, 0],
    ['ime-commit', 'missing', 1, 0],
    ['ime-context', 'unknown', IME_FLAGS.blurred, 0],
    ['ime-summary', null, 1, 1],
  ]);
  assert.equal(typeof wireImeProbe(null, () => {}).data, 'function');
});
