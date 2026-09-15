import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { initVoice } = await import('../js/voice.js');
const { ctx } = await import('../js/state.js');
const { t } = await import('../js/i18n.js');
const target = { session: 'deck-voice-a', cardId: 'a', title: 'A' };
const other = { session: 'deck-voice-b', cardId: 'b', title: 'B' };
const settle = () => new Promise(resolve => setTimeout(resolve, 0));
function setup() {
  ids.clear(); const calls = [], events = new Map(), native = new Map(), toasts = [];
  let selected = target, phase = 'recording', code = '', text = 'spoken words', preview = 'still listening', focused = 0;
  globalThis.window = {
    addEventListener: (name, fn) => events.set(name, fn),
    __TAURI__: {
      event: { listen: async (name, fn) => { native.set(name, fn); return () => {}; } },
      core: { invoke: async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'voice_bind') return { id: 7, process: 'zsh' };
        if (cmd === 'voice_start') return 2;
        if (cmd === 'voice_snapshot') return { id: args.id, status: phase, text, preview, code };
      } },
    },
  };
  ctx.settings.voice = { languages: ['zh-CN', 'en-US', 'ja-JP'], defaultLanguage: 'en-US' };
  const model = initVoice({ selectedTarget: () => selected, prepareTarget: async () => {}, afterDelivery() {},
    focusTerminal: () => { focused++; }, toast: message => toasts.push(message) });
  return { model, calls, native, toasts, element: id => fakeDocument.getElementById(id),
    focused: () => focused, typed: () => calls.filter(([cmd]) => cmd === 'voice_deliver').map(([, args]) => args.text),
    emit: (name, detail) => events.get(name)?.({ detail }),
    select: value => { selected = value; return events.get('deck-voice-session-changed')(); },
    phase: value => { phase = value; }, text: value => { text = value; },
    error: value => { phase = 'error'; code = value; },
  };
}

test('the header button starts a recording for the focused pane, keeps terminal focus, and types committed words', async () => {
  const f = setup(), button = f.element('voice-btn'), label = f.element('voice-time');
  assert.equal(button.dataset.phase, 'idle'); assert.equal(button.title, t('voice.phase.idle'));
  assert.equal(button['aria-pressed'], 'false'); assert.equal(label.hidden, true);
  assert.equal(button.fire('mousedown').prevented, 1, 'the button never takes focus from the terminal');
  await button.onclick(); await settle();
  assert.equal(f.focused(), 1);
  assert.deepEqual(f.calls.slice(0, 2).map(([cmd]) => cmd), ['voice_bind', 'voice_start']);
  assert.equal(f.calls[1][1].locale, 'en-US'); assert.equal(f.calls[1][1].targetId, 7);
  assert.equal(button.dataset.phase, 'recording'); assert.equal(button.title, t('voice.phase.recording'));
  assert.equal(button['aria-pressed'], 'true'); assert.equal(button.disabled, false);
  assert.equal(label.hidden, false); assert.match(label.textContent, /^\d\d:\d\d$/);
  assert.deepEqual(f.typed(), ['spoken words']); assert.deepEqual(f.toasts, []);
  const caption = f.element('voice-caption');
  assert.equal(caption.hidden, false); assert.equal(caption.textContent, 'still listening');
  await button.onclick();
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_stop').length, 1);
  assert.equal(button.dataset.phase, 'stopping'); assert.equal(button.disabled, true);
  assert.equal(button.title, t('voice.phase.stopping'));
  await button.onclick(); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_stop').length, 1, 'stopping waits');
  await f.model.cancel(); assert.equal(button.dataset.phase, 'idle'); assert.equal(label.hidden, true);
  assert.equal(caption.hidden, true); assert.equal(caption.textContent, '');
});

test('errors and notices are toasts; a permission failure opens its settings pane once', async () => {
  for (const [code, kind] of [['microphone-denied', 'microphone'], ['speech-denied', 'speech'], ['dictation-disabled', 'dictation']]) {
    const f = setup(); f.error(code); f.text('');
    await f.element('voice-btn').onclick(); await settle();
    assert.deepEqual(f.toasts, [t(`voice.error.${code}`)]);
    assert.deepEqual(f.calls.filter(([cmd]) => cmd === 'voice_open_settings'), [['voice_open_settings', { kind }]]);
    assert.equal(f.element('voice-btn').dataset.phase, 'idle'); assert.deepEqual(f.typed(), []);
  }
  const f = setup(); f.phase('downloading'); f.text('');
  await f.element('voice-btn').onclick(); await settle();
  assert.deepEqual(f.toasts, [t('voice.notice.downloading')]);
  assert.equal(f.element('voice-btn').dataset.phase, 'downloading'); assert.equal(f.element('voice-time').textContent, '…');
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_open_settings'));
  await f.model.cancel();
});

test('switching sessions, leaving, pane exit, page hide and a hidden window end the recording', async () => {
  const f = setup(), button = f.element('voice-btn');
  await button.onclick(); await settle(); assert.equal(f.model.state.phase, 'recording');
  await f.select(target); assert.equal(f.model.state.phase, 'recording');
  await f.select(other); assert.equal(f.model.state.phase, 'idle');
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_cancel').length, 1);
  await f.select(target);
  for (const [event, detail] of [['deck-session-leave', undefined], ['deck-voice-target-exit', target.session], ['pagehide', undefined]]) {
    await button.onclick(); await settle(); assert.equal(f.model.state.phase, 'recording');
    const cancels = f.calls.filter(([cmd]) => cmd === 'voice_cancel').length;
    await f.emit(event, detail); assert.equal(f.model.state.phase, 'idle');
    assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_cancel').length, cancels + 1);
  }
  await button.onclick(); await settle();
  await f.emit('deck-voice-target-exit', other.session); assert.equal(f.model.state.phase, 'recording');
  await f.native.get('voice-window-hidden')(); assert.equal(f.model.state.phase, 'idle');
});

test('preference changes reach the recorder and the next recording uses the new default language', async () => {
  const f = setup();
  ctx.settings.voice = { languages: ['ja-JP'], defaultLanguage: 'ja-JP' };
  f.emit('deck-voice-preferences-changed');
  await f.element('voice-btn').onclick(); await settle();
  assert.equal(f.calls.find(([cmd]) => cmd === 'voice_start')[1].locale, 'ja-JP');
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_open_settings'));
  await f.model.cancel();
});
