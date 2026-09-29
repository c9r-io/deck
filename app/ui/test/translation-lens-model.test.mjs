import test from 'node:test';
import assert from 'node:assert/strict';
import { TranslationLensModel, LiveCadence, translationShortcutAction,
  FIRST_OUTPUT_MS, OUTPUT_INTERVAL_MS, SCROLL_SETTLE_MS, RETRY_DELAYS_MS } from '../js/translation-lens-model.js';
import { visibleViewport } from '../js/translation-lens.js';
import { closedCode } from '../js/local-intelligence.js';

export class FakeClock {
  time = 0; next = 0; timers = new Map();
  now = () => this.time;
  setTimeout = (callback, delay) => { const id = ++this.next;
    this.timers.set(id, { at: this.time + delay, callback }); return id; };
  clearTimeout = id => this.timers.delete(id);
  frame = callback => this.setTimeout(callback, 16);
  advance(ms) {
    const end = this.time + ms;
    while (true) {
      const due = [...this.timers].filter(([, item]) => item.at <= end)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!due) break;
      this.time = due[1].at; this.timers.delete(due[0]); due[1].callback();
    }
    this.time = end;
  }
}
const live = (model, text, pane = 1, options) => model.observeLive(text, pane, options);

test('visible capture uses public viewport rows and drops blank trailing rows', () => {
  const rows = ['above', 'first', 'second', '', '  '];
  const pane = { term: { rows: 4, buffer: { active: { viewportY: 1,
    getLine: row => ({ translateToString: () => rows[row] }) } } } };
  assert.equal(visibleViewport(pane), 'first\nsecond');
  assert.equal(closedCode('view-too-large'), 'view-too-large');
  assert.equal(closedCode('foreign private text'), 'translation-failed');
});

test('[A01] an intent captures a static viewport after two layout frames, with no output at all', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: info => captures.push({ ...info, at: clock.now() }) });
  cadence.intent();
  clock.advance(31); assert.equal(captures.length, 0, 'waits for layout frames');
  clock.advance(1); assert.equal(captures.length, 1);
  assert.deepEqual([captures[0].intent, captures[0].kind, captures[0].at], [true, 'intent', 32]);
  clock.advance(5000); assert.equal(captures.length, 1, 'no polling without a change');
});

test('[A01] output during the intent frames does not add a second timer or delay the first capture', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: info => captures.push(clock.now()) });
  cadence.intent(); cadence.output(); cadence.output();
  clock.advance(32); assert.deepEqual(captures, [32]);
  cadence.output(); clock.advance(OUTPUT_INTERVAL_MS - 1); assert.equal(captures.length, 1);
  clock.advance(1); assert.deepEqual(captures, [32, 32 + OUTPUT_INTERVAL_MS]);
});

test('[B01] continuous output is throttled with an upper bound and never starves', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: () => captures.push(clock.now()) });
  for (let t = 0; t < 5000; t += 70) { cadence.output(); clock.advance(70); }
  assert.equal(captures[0], FIRST_OUTPUT_MS);
  for (let i = 1; i < captures.length; i++) {
    assert.ok(captures[i] - captures[i - 1] >= OUTPUT_INTERVAL_MS && captures[i] - captures[i - 1] <= OUTPUT_INTERVAL_MS + 70);
  }
  assert.ok(captures.length >= 6 && captures.length <= 9, `bounded: ${captures.length}`);
});

test('[B02] a scroll burst captures nothing until it settles, then captures once', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: info => captures.push({ at: clock.now(), kind: info.kind }) });
  for (let t = 0; t < 5000; t += 16) { cadence.scroll(); clock.advance(16); }
  assert.equal(captures.length, 0, 'no capture of intermediate positions');
  clock.advance(SCROLL_SETTLE_MS - 17); assert.equal(captures.length, 0);
  clock.advance(1); assert.equal(captures.length, 1);
  assert.equal(captures[0].kind, 'scroll');
  assert.equal(captures[0].at - 4992, SCROLL_SETTLE_MS, 'trailing debounce from the LAST wheel event');
});

test('[B03] output during a scroll gesture cannot bypass the debounce; normal throttle resumes after', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: info => captures.push({ at: clock.now(), kind: info.kind }) });
  cadence.output(); clock.advance(100); // output timer armed at 350
  for (let t = 0; t < 1000; t += 20) { cadence.scroll(); cadence.output(); clock.advance(20); }
  assert.equal(captures.length, 0, 'the pending output capture was superseded by the gesture');
  clock.advance(SCROLL_SETTLE_MS); assert.deepEqual(captures.map(c => c.kind), ['scroll']);
  const settled = captures[0].at;
  cadence.output(); clock.advance(OUTPUT_INTERVAL_MS);
  assert.deepEqual(captures.map(c => c.kind), ['scroll', 'output']);
  assert.equal(captures[1].at - settled, OUTPUT_INTERVAL_MS, 'late redraw after the gesture is still captured');
});

test('[B03] stop cancels timers, frames and a gesture; an intent during a gesture is carried to its end', () => {
  const clock = new FakeClock(); const captures = [];
  const cadence = new LiveCadence({ clock, onCapture: info => captures.push(info) });
  cadence.intent(); cadence.stop(); clock.advance(1000); assert.equal(captures.length, 0);
  cadence.scroll(); cadence.stop(); clock.advance(1000); assert.equal(captures.length, 0);
  cadence.output(); cadence.stop(); clock.advance(1000); assert.equal(captures.length, 0);
  cadence.scroll(); cadence.intent(); clock.advance(SCROLL_SETTLE_MS);
  assert.deepEqual(captures, [{ intent: true, kind: 'scroll' }]);
  cadence.intent(); cadence.scroll(); clock.advance(SCROLL_SETTLE_MS + 40);
  assert.equal(captures.length, 2); assert.equal(captures[1].intent, true);
  assert.equal(clock.timers.size, 0, 'no stray timers');
});

test('[B01] one running and one latest replaceable pending; stale snapshot remains readable', () => {
  const model = new TranslationLensModel(); model.show();
  const first = live(model, 'source one');
  assert.equal(live(model, 'source two'), null);
  assert.equal(live(model, 'source three'), null);
  assert.equal(model.pending.text, 'source three');
  const done = model.finish(first, 'translation one');
  assert.equal(model.present().text, 'translation one');
  assert.equal(model.sourceForCopy(), 'source one');
  assert.equal(model.status(), 'translation.updating');
  assert.equal(done.next.text, 'source three');
  model.finish(done.next, 'translation three');
  assert.equal(model.status(), 'translation.ready');
  assert.equal(model.present().source, 'source three');
});

test('[A01] same viewport is not re-queued by background changes; an intent re-attempts only uncovered work', () => {
  const model = new TranslationLensModel(); model.show();
  const ticket = live(model, 'hello');
  assert.equal(live(model, 'hello'), null);
  assert.equal(live(model, 'hello', 1, { intent: true }), null, 'running covers it');
  model.finish(ticket, '你好');
  assert.equal(live(model, 'hello', 1, { intent: true }), null, 'displayed covers it');
  model.interrupt();
  assert.equal(model.status(), 'translation.ready');
});

test('[A06] focus loss interrupts queued work; the focus-return intent re-attempts the same source', () => {
  const model = new TranslationLensModel(); model.show();
  const ticket = live(model, 'hello');
  model.interrupt();
  assert.equal(model.finish(ticket, '晚到').accepted, false, 'a dropped request cannot publish');
  assert.equal(model.status({ active: false }), 'translation.backgroundEmpty');
  assert.equal(live(model, 'hello'), null, 'background output alone does not re-queue');
  const again = live(model, 'hello', 1, { intent: true });
  assert.equal(again.text, 'hello');
});

test('[D01] a transient failure retries with a bounded schedule and is not locked by observation dedupe', () => {
  const model = new TranslationLensModel(); model.show();
  let ticket = live(model, 'hello');
  let out = model.finish(ticket, null, 'translation-failed');
  assert.equal(out.retry, RETRY_DELAYS_MS[0]);
  assert.equal(model.status(), 'translation.error.translation-failed');
  ticket = model.retry(); assert.equal(ticket.text, 'hello'); assert.equal(model.status(), 'translation.translating');
  out = model.finish(ticket, null, 'translation-failed'); assert.equal(out.retry, RETRY_DELAYS_MS[1]);
  ticket = model.retry();
  out = model.finish(ticket, null, 'translation-failed'); assert.equal(out.retry, null, 'bounded');
  assert.equal(model.retry(), null);
  assert.equal(model.working(), false);
  assert.equal(model.status(), 'translation.error.translation-failed');
  const fresh = live(model, 'hello', 1, { intent: true });
  assert.equal(fresh.text, 'hello', 'a fresh intent re-attempts after exhausted retries');
  model.finish(fresh, '你好');
  assert.equal(model.status(), 'translation.ready');
});

test('[D01] definite failures never retry automatically', () => {
  for (const code of ['source-language-unsupported', 'translation-model-corrupt', 'translation-model-missing',
    'protected-restoration-failed', 'translation-disabled', 'text-too-large']) {
    const model = new TranslationLensModel(); model.show();
    const out = model.finish(live(model, 'x'), null, code);
    assert.equal(out.retry, null, code);
    assert.equal(model.status(), `translation.error.${code}`);
    assert.equal(live(model, 'x', 1, { intent: true }), null, `${code} covers its own source`);
  }
});

test('[D02] an update failure keeps the old readable result and shows the error; no endless updating', () => {
  const model = new TranslationLensModel(); model.show();
  model.finish(live(model, 'one'), '一');
  const second = live(model, 'two');
  assert.equal(model.status(), 'translation.updating');
  model.finish(second, null, 'protected-restoration-failed');
  assert.equal(model.present().text, '一'); assert.equal(model.sourceForCopy(), 'one');
  assert.equal(model.status(), 'translation.error.protected-restoration-failed');
  assert.equal(model.working(), false);
  const third = live(model, 'three');
  model.finish(third, '三'); assert.equal(model.status(), 'translation.ready');
});

test('[D02] stale display without work is reported as stale, never as updating', () => {
  const model = new TranslationLensModel(); model.show();
  model.finish(live(model, 'one'), '一');
  live(model, 'two'); model.interrupt();
  assert.equal(model.isUpdating(), false);
  assert.equal(model.status(), 'translation.stale');
});

test('[D03] cancelled, closed, reopened and mode-switched contexts reject late results', () => {
  const model = new TranslationLensModel(); model.show();
  const ticket = live(model, 'hello');
  model.close(); model.show();
  assert.equal(model.finish(ticket, 'late').accepted, false);
  assert.equal(model.present(), null);
  const again = live(model, 'hello');
  model.modeTo('clipboard');
  assert.equal(model.finish(again, 'late').accepted, false);
  assert.equal(model.finish(again, null, 'translation-failed').retry, null, 'a stale failure never retries');
  assert.equal(model.status(), 'translation.preparing');
});

test('[A05] a different pane resets the context; pane A results never display for pane B', () => {
  const model = new TranslationLensModel(); model.show();
  model.finish(live(model, 'alpha', 1), '甲');
  const late = live(model, 'alpha two', 1);
  const b = live(model, 'beta', 2);
  assert.equal(model.present(), null, 'A text is cleared when B is observed');
  assert.equal(model.finish(late, '甲二').accepted, false);
  model.finish(b, '乙'); assert.equal(model.present().text, '乙');
  assert.equal(model.retarget(), true);
  assert.equal(model.present(), null);
  assert.equal(live(model, 'beta', 3).text, 'beta', 'same text in a rebuilt pane is a new source');
});

test('[D04] Live 4 KiB and snapshot limits refuse without truncation or erasing the old result', () => {
  const model = new TranslationLensModel(); model.show();
  model.finish(live(model, 'first'), '译文');
  assert.ok(live(model, 'x'.repeat(4096)), 'exactly 4096 bytes is allowed');
  model.interrupt();
  assert.equal(live(model, 'x'.repeat(4097)), null);
  assert.equal(model.status(), 'translation.error.view-too-large');
  assert.equal(model.present().text, '译文'); assert.equal(model.sourceForCopy(), 'first');
  assert.equal(live(model, '好'.repeat(1366)), null, 'UTF-8 bytes, not characters: 4098 bytes');
  const clip = new TranslationLensModel(); clip.show('clipboard');
  assert.ok(clip.snapshot('y'.repeat(8192), 'clipboard', 8192));
  assert.equal(clip.snapshot('y'.repeat(8193), 'clipboard', 8192), null);
  assert.equal(clip.status(), 'translation.error.text-too-large');
  assert.ok(clip.snapshot('y'.repeat(16384), 'selection', 16384));
  assert.equal(clip.snapshot('y'.repeat(16385), 'selection', 16384), null);
  assert.equal(clip.snapshot('  \n', 'selection'), null);
  assert.equal(clip.status(), 'translation.error.text-empty');
});

test('[C07] selection and copied snapshots stay static and Copy Source pairs with the shown snapshot', () => {
  const model = new TranslationLensModel(); model.show();
  const selected = model.snapshot('selected original', 'selection');
  model.finish(selected, 'selected translation');
  assert.equal(live(model, 'terminal changed'), null, 'a snapshot mode ignores the terminal');
  assert.equal(model.retarget(), false, 'pane switch keeps the snapshot');
  model.present();
  assert.equal(model.sourceForCopy(), 'selected original');
  assert.equal(model.status(), 'translation.selected');
  assert.equal(model.modeTo('clipboard'), true);
  const copied = model.snapshot('copied original', 'clipboard');
  model.finish(copied, 'copied translation'); model.present();
  assert.equal(model.sourceForCopy(), 'copied original');
  const late = model.snapshot('later', 'clipboard'); model.close();
  assert.equal(model.finish(late, 'late translation').accepted, false);
  assert.equal(model.sourceForCopy(), '');
});

test('[C03] a newer copy supersedes an older running one; same text is not re-translated unless it failed', () => {
  const model = new TranslationLensModel(); model.show('clipboard');
  model.clipboardReady = true;
  assert.equal(model.status(), 'translation.waiting');
  const one = model.snapshot('one', 'clipboard');
  const two = model.snapshot('two', 'clipboard');
  assert.equal(model.finish(one, '一').accepted, false, 'superseded copy cannot publish');
  model.finish(two, '二'); model.present();
  assert.equal(model.newCopy('two'), false, 'same text again is already shown');
  assert.equal(model.newCopy('three'), true);
  const three = model.snapshot('three', 'clipboard');
  model.finish(three, null, 'translation-model-corrupt');
  assert.equal(model.newCopy('three'), true, 'a failed text copied again is a new attempt');
});

test('[C05] Deck-owned text (translation, source, a fragment of the translation) never feeds back', () => {
  const model = new TranslationLensModel(); model.show('clipboard');
  model.finish(model.snapshot('The build passed.', 'clipboard'), '构建已通过。'); model.present();
  assert.equal(model.newCopy('构建已通过。'), false);
  assert.equal(model.newCopy('The build passed.'), false);
  assert.equal(model.newCopy('已通过'), false, '⌘C of part of the translation');
  assert.equal(model.newCopy('The next answer.'), true);
});

test('[C01] entering Copied text shows preparing until armed, then the waiting hint; nothing is observed', () => {
  const model = new TranslationLensModel(); model.show();
  assert.equal(model.modeTo('clipboard'), true);
  assert.equal(model.modeTo('clipboard'), false, 're-choosing the current tab does nothing');
  assert.equal(model.status(), 'translation.preparing');
  model.clipboardReady = true;
  assert.equal(model.status(), 'translation.waiting');
  model.fail('clipboard-not-text');
  assert.equal(model.status(), 'translation.error.clipboard-not-text');
});

test('[B05] a live selection holds the shown snapshot; release shows the newer one; Copy Source follows shown', () => {
  const model = new TranslationLensModel(); model.show();
  model.finish(live(model, 'one'), '一'); model.present();
  model.finish(live(model, 'two'), '二');
  assert.equal(model.present(true).text, '一', 'held while selecting');
  assert.equal(model.sourceForCopy(), 'one');
  assert.equal(model.status(), 'translation.held');
  assert.equal(model.present(false).text, '二', 'released at the end of the selection');
  assert.equal(model.sourceForCopy(), 'two');
  assert.equal(model.status(), 'translation.ready');
});

test('[A03] a blank viewport is an honest empty state that later output replaces', () => {
  const model = new TranslationLensModel(); model.show();
  assert.equal(model.status(), 'translation.preparing');
  assert.equal(live(model, '   \n  '), null);
  assert.equal(model.status(), 'translation.empty');
  const ticket = live(model, 'late output');
  assert.equal(model.status(), 'translation.translating');
  model.finish(ticket, '晚到输出'); assert.equal(model.status(), 'translation.ready');
});

test('[E01] primary shortcut is context sensitive', () => {
  assert.equal(translationShortcutAction(false, false), 'live');
  assert.equal(translationShortcutAction(true, false), 'selection');
  assert.equal(translationShortcutAction(true, true), 'selection');
  assert.equal(translationShortcutAction(false, true), 'close');
});
