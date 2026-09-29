// Coordinator contract through the Lens's own DOM controls and window
// events, with a controlled backend (L1). The same file is run against the
// pre-fix sources as the negative control of scripts/translation-lens-verify.py,
// so it drives only user-level controls that existed before and after.
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids, documentListeners } from './fixtures/dom-fixture.mjs';

class CountingElement extends FakeElement {
  writes = 0;
  set textContent(value) { this.writes++; super.textContent = value; }
  get textContent() { return super.textContent; }
}
const windowListeners = new Map();
let focused = true;
globalThis.document = Object.assign(fakeDocument, {
  hidden: false, hasFocus: () => focused, getSelection: () => ({ isCollapsed: true }),
});
globalThis.window = {
  __TAURI__: null,
  addEventListener(type, fn) { const list = windowListeners.get(type) || []; list.push(fn); windowListeners.set(type, list); },
  dispatchEvent(event) { for (const fn of windowListeners.get(event.type) || []) fn(event); },
  setTimeout: (...args) => setTimeout(...args), clearTimeout: id => clearTimeout(id),
};
globalThis.CustomEvent ??= class extends Event { constructor(type, init = {}) { super(type); this.detail = init.detail; } };
globalThis.ResizeObserver = class { observe() {} };
globalThis.requestAnimationFrame = fn => setTimeout(fn, 16);
globalThis.__DECK_SMOKE_TRANSLATION = true;
mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'] });

const { ctx, state } = await import('../js/state.js');
const { t } = await import('../js/i18n.js');
const intelligence = await import('../js/local-intelligence.js');
const lens = await import('../js/translation-lens.js');

const flush = async () => { for (let i = 0; i < 6; i++) await new Promise(resolve => setImmediate(resolve)); };
const tick = async ms => { for (let left = ms; left > 0; left -= 10) { mock.timers.tick(Math.min(10, left)); await flush(); } };
const fire = (type, detail) => window.dispatchEvent(Object.assign(new Event(type), { detail }));
const $ = id => fakeDocument.getElementById(id);
const status = () => $('translation-status').textContent;

// Backend: every translate call is controlled by the test.
const calls = [];
let clipboard = { version: 0, baseline: 0, armed: false, text: '' };
let armGate = null, outstanding = 0, maxOutstanding = 0, copied = [];
intelligence.installTranslationSmokeBackend({
  capability: async () => ({ available: true, enabled: true, installed: true, loaded: false }),
  unload: async () => {},
  translate: (id, text, target, strategy) => new Promise((resolve, reject) => {
    outstanding++; maxOutstanding = Math.max(maxOutstanding, outstanding);
    calls.push({ id, text, strategy, done: false,
      ok(result) { if (this.done) return; this.done = true; outstanding--; resolve({ requestId: id, text: result }); },
      fail(code) { if (this.done) return; this.done = true; outstanding--; reject(new Error(code)); } });
  }),
  cancel: async () => {},
  arm: async () => { if (armGate) await armGate.promise; if (!focused) throw new Error('clipboard-not-focused');
    clipboard.armed = true; clipboard.baseline = clipboard.version; clipboard.arms = (clipboard.arms || 0) + 1; },
  disarm: async () => { clipboard.armed = false; },
  poll: async () => { if (!focused || !clipboard.armed || clipboard.version <= clipboard.baseline) return null;
    clipboard.baseline = clipboard.version; clipboard.reads = (clipboard.reads || 0) + 1; return clipboard.text; },
  copy: async text => { copied.push(text); clipboard.text = text; clipboard.version++; },
});
const copyExternally = text => { clipboard.text = text; clipboard.version++; };

// Two panes whose viewport text the test controls.
const viewports = { a: ['The build completed successfully.'], b: ['Beta pane output.'] };
const pane = name => ({ term: { rows: 4, buffer: { active: { viewportY: 0,
  getLine: row => ({ translateToString: () => viewports[name][row] || '' }) } } } });
const panes = new Map([['a', pane('a')], ['b', pane('b')]]);
ctx.settings.localIntelligence = { translation: { enabled: true, documentLimitBytes: 16384 } };
state.view = 'session'; ctx.attachedName = 'a';
ids.set('translation-result', new CountingElement());
lens.initTranslationLens({ panes, closeBuffer() {} });
await flush();

const openLens = async () => { $('translation-btn').onclick(); await flush(); };
const chooseTab = async mode => {
  const tab = $(`translation-tab-${mode}`);
  if (typeof tab.onclick === 'function') tab.onclick();
  else { $('translation-mode').value = mode; $('translation-mode').onchange?.({ target: { value: mode } }); }
  await flush();
};
const pending = () => calls.filter(call => !call.done);
async function reset() {
  lens.closeTranslationLens(); await tick(50);
  for (const call of pending()) call.fail('request-cancelled');
  await flush(); calls.length = 0; outstanding = 0; maxOutstanding = 0; copied = [];
  clipboard = { version: 0, baseline: 0, armed: false, text: '' }; armGate = null; focused = true;
  viewports.a = ['The build completed successfully.']; viewports.b = ['Beta pane output.'];
  ctx.attachedName = 'a';
}

test('[A01] opening on already-displayed output translates it with no further output, scroll or resize', async () => {
  await reset(); await openLens();
  await tick(400);
  assert.equal(calls.length, 1); assert.equal(calls[0].text, 'The build completed successfully.');
  calls[0].ok('构建已成功完成。'); await flush();
  assert.equal($('translation-result').textContent, '构建已成功完成。');
  assert.equal(status(), t('translation.ready'));
});

test('[A02] pointer, wheel and key interaction on the result before the first frame never pause it', async () => {
  await reset(); await openLens();
  const result = $('translation-result');
  result.fire('pointerdown'); result.fire('wheel'); result.fire('keydown', { key: 'Tab' });
  fire('pointerup');
  await tick(400);
  assert.equal(calls.length, 1, 'the first request still starts');
  calls[0].ok('构建已成功完成。'); await flush();
  assert.equal(result.textContent, '构建已成功完成。');
  viewports.a = ['The next step is ready.']; fire('deck-terminal-changed', 'a');
  await tick(700);
  assert.equal(calls.length, 2, 'later output is still followed');
});

test('[D01] a first transient failure is retried automatically for the same unchanged viewport', async () => {
  await reset(); await openLens(); await tick(400);
  calls[0].fail('translation-failed'); await flush();
  assert.equal(status(), t('translation.error.translation-failed'));
  await tick(1500);
  assert.equal(calls.length, 2, 'retried'); assert.equal(calls[1].text, calls[0].text);
  calls[1].ok('构建已成功完成。'); await flush();
  assert.equal($('translation-result').textContent, '构建已成功完成。');
  assert.equal(status(), t('translation.ready'));
});

test('[D02] an update failure with an older result shows the error instead of a permanent "updating"', async () => {
  await reset(); await openLens(); await tick(400);
  calls[0].ok('第一版'); await flush();
  viewports.a = ['A second view with more text.']; fire('deck-terminal-changed', 'a'); await tick(700);
  assert.equal(calls.length, 2);
  calls[1].fail('protected-restoration-failed'); await flush(); await tick(3000);
  assert.equal($('translation-result').textContent, '第一版');
  assert.equal(status(), t('translation.error.protected-restoration-failed'));
  assert.equal(pending().length, 0);
});

test('[B02] a terminal scroll burst submits nothing for intermediate views, then the final view once', async () => {
  await reset(); await openLens(); await tick(400);
  calls[0].ok('第一版'); await flush();
  const before = calls.length;
  for (let i = 0; i < 125; i++) { // 2 s of wheel frames with tmux redraws
    viewports.a = [`history line ${i}`];
    fire('deck-terminal-scroll', 'a'); fire('deck-terminal-changed', 'a');
    await tick(16);
  }
  assert.equal(calls.length, before, 'no request during the gesture');
  await tick(300);
  assert.equal(calls.length, before + 1);
  assert.equal(calls.at(-1).text, 'history line 124', 'the final view, not a stale one');
});

test('[B06] repeated changes and renders with an unchanged translation do not rewrite the result DOM', async () => {
  await reset(); await openLens(); await tick(400);
  calls[0].ok('构建已成功完成。'); await flush();
  const result = $('translation-result'), writes = result.writes;
  for (let i = 0; i < 20; i++) { fire('deck-terminal-changed', 'a'); fire('deck-pane-focused-noop'); await tick(100); }
  await chooseTab('live');
  assert.equal(result.writes, writes, 'no textContent write for identical text');
  assert.equal(calls.length, 1, 'the unchanged viewport is not re-sent');
});

test('[B05] wheel, click and Tab on the result leave Live following the terminal', async () => {
  await reset(); await openLens(); await tick(400);
  calls[0].ok('第一版'); await flush();
  const result = $('translation-result');
  result.fire('wheel'); result.fire('pointerdown'); fire('pointerup'); result.fire('keydown', { key: 'Tab' });
  viewports.a = ['Output after reading.']; fire('deck-terminal-changed', 'a'); await tick(700);
  assert.equal(calls.length, 2); calls[1].ok('阅读后的输出'); await flush();
  assert.equal(result.textContent, '阅读后的输出');
  assert.notEqual(status(), t('translation.paused'));
});

test('[C01] entering Copied text never reads earlier clipboard text and is not "ready" before the baseline', async () => {
  await reset(); copyExternally('unrelated earlier copy');
  await openLens(); await tick(400); calls[0]?.ok('第一版'); await flush();
  armGate = Promise.withResolvers();
  await chooseTab('clipboard');
  assert.notEqual(status(), t('translation.waiting'), 'not ready until the baseline exists');
  armGate.resolve(); armGate = null; await flush(); await tick(1000);
  assert.equal(status(), t('translation.waiting'));
  assert.ok(!calls.some(call => call.text === 'unrelated earlier copy'));
  assert.equal(clipboard.reads || 0, 0);
});

test('[C02] a copy right after the baseline is translated automatically with no further click', async () => {
  await reset(); await openLens(); await tick(400); calls[0]?.ok('第一版'); await flush();
  await chooseTab('clipboard'); await flush();
  copyExternally('The copied answer.');
  await tick(400);
  const request = calls.find(call => call.text === 'The copied answer.');
  assert.ok(request, 'translated automatically'); assert.equal(request.strategy, 'clipboard');
  request.ok('复制的回答。'); await flush();
  assert.equal($('translation-result').textContent, '复制的回答。');
});

test('[C05][C08] copying the translation never feeds back; re-choosing the current tab does not re-arm', async () => {
  await reset(); await openLens(); await tick(400); calls[0]?.ok('第一版'); await flush();
  await chooseTab('clipboard'); await tick(50);
  const arms = clipboard.arms;
  await chooseTab('clipboard'); await tick(50);
  assert.equal(clipboard.arms, arms, 'the selected tab is not re-armed');
  copyExternally('The copied answer.'); await tick(400);
  calls.find(call => call.text === 'The copied answer.').ok('复制的回答。'); await flush();
  const count = calls.length;
  $('translation-copy').onclick(); await tick(800);
  $('translation-copy-source').onclick(); await tick(800);
  assert.equal(calls.length, count, 'no feedback');
  copyExternally('A genuinely new answer.'); await tick(400);
  assert.ok(calls.some(call => call.text === 'A genuinely new answer.'), 'the next real copy still works');
  assert.equal(typeof intelligence.clipboardCurrent, 'undefined', 'no explicit read of the current clipboard');
});

test('[A04] rapid Live ↔ Copied text round trips keep at most one outstanding request and end translated', async () => {
  await reset(); await openLens(); await tick(40);
  for (let i = 0; i < 10; i++) { await chooseTab('clipboard'); await tick(5); await chooseTab('live'); await tick(40); }
  assert.ok(maxOutstanding <= 1, `outstanding ${maxOutstanding}`);
  while (pending().length) { pending()[0].ok('构建已成功完成。'); await flush(); await tick(40); }
  assert.equal($('translation-result').textContent, '构建已成功完成。');
  assert.equal(status(), t('translation.ready'));
});

test('[A05] switching panes never shows pane A results for pane B', async () => {
  await reset(); await openLens(); await tick(400);
  const a = calls[0];
  ctx.attachedName = 'b'; fire('deck-pane-focused'); await tick(400);
  a.ok('甲的译文'); await flush(); await tick(100);
  assert.notEqual($('translation-result').textContent, '甲的译文');
  const b = calls.find(call => call.text === 'Beta pane output.');
  assert.ok(b, 'B is translated'); b.ok('乙的译文'); await flush();
  assert.equal($('translation-result').textContent, '乙的译文');
});

test('[A06] after focus loss and return the static view is translated again without any action', async () => {
  await reset(); await openLens(); await tick(40);
  focused = false; fire('blur'); await tick(400);
  for (const call of pending()) call.ok('失焦期间'); await flush();
  focused = true; fire('focus'); await tick(400);
  const latest = calls.at(-1);
  assert.ok(latest && !latest.done, 'a fresh request after focus returns');
  latest.ok('构建已成功完成。'); await flush();
  assert.equal($('translation-result').textContent, '构建已成功完成。');
});

test('[C04] text copied while Deck was away is not read after returning; a later copy is', async () => {
  await reset(); await openLens(); await tick(400); calls[0]?.ok('第一版'); await flush();
  await chooseTab('clipboard'); await tick(50);
  focused = false; fire('blur'); copyExternally('away copy'); await tick(400);
  focused = true; fire('focus'); await tick(800);
  assert.ok(!calls.some(call => call.text === 'away copy'));
  copyExternally('after return'); await tick(400);
  assert.ok(calls.some(call => call.text === 'after return'));
});

test('[D03] a request still running when the Lens closes cannot publish into the reopened Lens', async () => {
  await reset(); await openLens(); await tick(400);
  const late = calls[0];
  lens.closeTranslationLens(); await openLens();
  late.ok('迟到的译文'); await flush(); await tick(100);
  assert.notEqual($('translation-result').textContent, '迟到的译文');
});
