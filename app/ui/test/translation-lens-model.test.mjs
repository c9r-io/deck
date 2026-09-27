import test from 'node:test';
import assert from 'node:assert/strict';
import { TranslationLensModel, LiveCadence, translationShortcutAction } from '../js/translation-lens-model.js';
import { visibleViewport } from '../js/translation-lens.js';
import { closedCode } from '../js/local-intelligence.js';

test('visible capture uses public viewport rows and drops blank trailing rows', () => {
  const rows = ['above', 'first', 'second', '', '  '];
  const pane = { term: { rows: 4, buffer: { active: { viewportY: 1,
    getLine: row => ({ translateToString: () => rows[row] }) } } } };
  assert.equal(visibleViewport(pane), 'first\nsecond');
  assert.equal(closedCode('view-too-large'), 'view-too-large');
  assert.equal(closedCode('foreign private text'), 'translation-failed');
});

class FakeClock {
  time = 0; next = 0; timers = new Map();
  now = () => this.time;
  setTimeout = (callback, delay) => { const id = ++this.next;
    this.timers.set(id, { at: this.time + delay, callback }); return id; };
  clearTimeout = id => this.timers.delete(id);
  advance(ms) {
    const end = this.time + ms;
    while (true) {
      const due = [...this.timers].filter(([, item]) => item.at <= end)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (!due) break;
      this.time = due[1].at; this.timers.delete(due[0]); due[1].callback();
    }
    this.time = end;
  }
}

test('continuous writes capture at bounded cadence while one running and one latest pending', () => {
  const clock = new FakeClock(), model = new TranslationLensModel();
  model.show(); const initial = model.observeLive('initial', 'pane');
  model.finish(initial, 'previous translation');
  let viewport = 'initial', captures = 0;
  const cadence = new LiveCadence({ clock, onCapture: () => {
    captures++; model.observeLive(viewport, 'pane');
  } });
  for (let elapsed = 0; elapsed < 3500; elapsed += 70) {
    viewport = `viewport ${elapsed}`; cadence.dirty();
    if (elapsed === 0) assert.equal(model.result, 'previous translation');
    clock.advance(70);
  }
  assert.ok(captures >= 4 && captures <= 7, `bounded captures: ${captures}`);
  assert.equal(model.result, 'previous translation');
  assert.ok(model.running && model.pending);
  clock.advance(650);
  assert.equal(model.pending.text, viewport);
  cadence.stop();
});

test('stale completed snapshot is readable and Copy Source matches displayed revision', () => {
  const model = new TranslationLensModel(); model.show();
  const first = model.observeLive('source one', 'pane');
  model.observeLive('source two', 'pane');
  model.observeLive('source three', 'pane');
  assert.equal(model.pending.text, 'source three');
  const finished = model.finish(first, 'translation one');
  assert.equal(model.result, 'translation one');
  assert.equal(model.sourceForCopy(), 'source one');
  assert.equal(model.isUpdating(), true);
  assert.equal(finished.next.text, 'source three');
  model.observeLive('source four', 'pane');
  model.finish(finished.next, 'translation three');
  assert.equal(model.result, 'translation three');
  assert.equal(model.sourceForCopy(), 'source three');
  assert.equal(model.isUpdating(), true);
  const newest = model.running;
  model.finish(newest, 'translation four');
  assert.equal(model.isUpdating(), false);
  assert.equal(model.sourceForCopy(), 'source four');
});

test('pause freezes result and resume captures current viewport', () => {
  const model = new TranslationLensModel(); model.show();
  const first = model.observeLive('first', 'pane'); model.finish(first, '译文');
  const second = model.observeLive('second', 'pane');
  assert.equal(model.pause(), true);
  assert.equal(model.observeLive('third', 'pane'), null);
  model.finish(second, 'late');
  assert.equal(model.result, '译文'); assert.equal(model.sourceForCopy(), 'first');
  model.resume(); const current = model.observeLive('fourth', 'pane');
  assert.equal(current.text, 'fourth'); assert.equal(model.result, '译文');
});

test('oversize does not truncate or erase previous readable result', () => {
  const model = new TranslationLensModel(); model.show();
  const first = model.observeLive('first', 'pane'); model.finish(first, '译文');
  assert.equal(model.observeLive('x'.repeat(4097), 'pane'), null);
  assert.equal(model.error, 'view-too-large');
  assert.equal(model.result, '译文'); assert.equal(model.sourceForCopy(), 'first');
});

test('selection and clipboard Copy Source use accepted snapshot; close rejects late result', () => {
  const model = new TranslationLensModel(); model.show();
  const selected = model.snapshot('selected original', 'selection');
  model.finish(selected, 'selected translation');
  assert.equal(model.sourceForCopy(), 'selected original');
  model.modeTo('clipboard');
  const copied = model.snapshot('copied original', 'clipboard');
  model.finish(copied, 'copied translation');
  assert.equal(model.sourceForCopy(), 'copied original');
  const late = model.snapshot('later', 'clipboard'); model.close();
  assert.equal(model.finish(late, 'late translation').accepted, false);
  assert.equal(model.sourceForCopy(), '');
});

test('primary shortcut is context sensitive', () => {
  assert.equal(translationShortcutAction(false, false), 'live');
  assert.equal(translationShortcutAction(true, false), 'selection');
  assert.equal(translationShortcutAction(true, true), 'selection');
  assert.equal(translationShortcutAction(false, true), 'close');
});
