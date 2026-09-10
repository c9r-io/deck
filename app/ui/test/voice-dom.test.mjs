import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { initVoice } = await import('../js/voice.js');
const { ctx } = await import('../js/state.js');
const target = { session: 'deck-voice-a', cardId: 'a', title: 'A' };
const other = { session: 'deck-voice-b', cardId: 'b', title: 'B' };
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
function setup() {
  ids.clear(); const calls = [], events = new Map(), native = new Map();
  let selected = target, generation = 1, serial = 0, outcome = 'submitted', phase = 'ready';
  const bindings = new Map();
  globalThis.window = {
    addEventListener: (name, fn) => events.set(name, fn),
    __TAURI__: {
      event: { listen: async (name, fn) => { native.set(name, fn); return () => {}; } },
      core: { invoke: async (cmd, args) => {
        calls.push([cmd, args]);
        if (cmd === 'voice_bind') { const id = ++serial; bindings.set(id, generation); return { id, process: 'zsh' }; }
        if (cmd === 'voice_start') return ++serial;
        if (cmd === 'voice_snapshot') return { id: args.id, status: phase, text: 'spoken draft' };
        if (cmd === 'voice_deliver') {
          if (bindings.get(args.targetId) !== generation) throw 'target-expired';
          return outcome === 'submitted' && !args.submit ? 'inserted' : outcome;
        }
      } },
    },
  };
  ctx.settings.voice = { languages: ['zh-CN', 'en-US', 'ja-JP'], defaultLanguage: 'en-US' };
  const panel = fakeDocument.getElementById('voice-panel');
  for (const value of ['bottom', 'floating', 'right']) {
    const button = new FakeElement('button'); button.dataset.voiceLayout = value; panel.appendChild(button);
  }
  const model = initVoice({ selectedTarget: () => selected, prepareTarget: async () => {}, afterDelivery() {} });
  return { model, calls, panel, native, element: id => fakeDocument.getElementById(id),
    emit: (name, detail) => events.get(name)?.({ detail }),
    select: value => { selected = value; return events.get('deck-voice-session-changed')(); },
    phase: value => { phase = value; },
    replace: () => { generation++; }, outcome: value => { outcome = value; },
  };
}

test('microphone, editor and placement buttons execute production handlers on one mounted editor', async () => {
  const f = setup(); await f.element('voice-btn').onclick();
  assert.equal(f.model.state.draft, 'spoken draft');
  assert.equal(f.element('voice-draft').readOnly, false);
  const draft = f.element('voice-draft'); draft.focus(); draft.setSelectionRange(1, 4);
  for (const button of f.panel.children) {
    assert.equal(button.fire('mousedown').prevented, 1); button.onclick();
    assert.equal(f.model.state.layout, button.dataset.voiceLayout);
    assert.equal(draft.selectionStart, 1); assert.equal(fakeDocument.activeElement, draft);
  }
  draft.value = 'edited'; draft.fire('input');
  f.element('voice-language').fire('change', { target: { value: 'ja-JP' } });
  assert.equal(f.model.state.draft, 'edited'); assert.equal(f.model.state.language, 'ja-JP');
  await f.element('voice-insert').onclick();
  assert.equal(f.model.state.notice, 'inserted'); assert.equal(f.model.state.draft, '');
  await f.element('voice-close').onclick(); assert.equal(f.element('voice-panel').hidden, true);
});

test('IME and ordinary Enter never send; command Enter dispatches one explicit send', async () => {
  const f = setup(); f.model.show(); await f.model.ensureTarget(target); f.model.edit('typed');
  f.panel.fire('keydown', { key: 'Enter', metaKey: true, isComposing: true });
  f.panel.fire('keydown', { key: 'Enter', metaKey: true, keyCode: 229 });
  f.panel.fire('keydown', { key: 'Enter' });
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 0);
  const sent = f.panel.fire('keydown', { key: 'Enter', metaKey: true }); await tick();
  assert.equal(sent.prevented, 1); assert.equal(sent.stopped, 1);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 1);
  assert.equal(f.model.state.notice, 'submitted');
});

test('expired target only rebinds on the next explicit send, including after session switches', async () => {
  const f = setup(); f.model.show(); await f.model.ensureTarget(target); f.model.edit('keep'); f.replace();
  await f.element('voice-send').onclick();
  assert.equal(f.model.state.error, 'target-expired'); assert.equal(f.model.state.target, null);
  assert.equal(f.model.state.draft, 'keep');
  await f.select(other); await f.select(target);
  assert.equal(f.calls.filter(([cmd, args]) => cmd === 'voice_bind' && args.name === target.session).length, 1);
  await f.element('voice-send').onclick();
  assert.equal(f.model.state.notice, 'submitted');
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 2);
  assert.equal(f.calls.filter(([cmd, args]) => cmd === 'voice_bind' && args.name === target.session).length, 2);
});

test('uncertainty survives expiry and navigation; only clearing permits a fresh binding', async () => {
  const f = setup(); f.model.show(); await f.model.ensureTarget(target); f.model.edit('keep');
  f.outcome('ambiguous'); await f.element('voice-send').onclick();
  assert.equal(f.element('voice-retry').hidden, false); assert.equal(f.element('voice-send').disabled, true);
  f.replace(); await f.element('voice-retry').onclick();
  assert.equal(f.model.state.needsConfirmation, true);
  assert.equal(f.element('voice-retry').disabled, true);
  await f.select(other); await f.select(target); await f.element('voice-send').onclick();
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 2);
  await f.element('voice-clear').onclick(); f.model.edit('new'); f.outcome('submitted');
  await f.element('voice-send').onclick(); assert.equal(f.model.state.notice, 'submitted');
});

test('close, leave, target exit and hidden window release capture; preference changes render the selector', async () => {
  const f = setup(); await f.element('voice-btn').onclick();
  ctx.settings.voice = { languages: ['en-US'], defaultLanguage: 'en-US' };
  f.emit('deck-voice-preferences-changed'); assert.equal(f.element('voice-language').hidden, true);
  assert.equal(f.element('voice-language-label').hidden, true);
  for (const event of ['deck-session-leave', 'deck-voice-target-exit', 'pagehide']) {
    f.phase('recording'); f.model.show(); await f.element('voice-record').onclick();
    assert.equal(f.model.state.phase, 'recording');
    const cancels = f.calls.filter(([cmd]) => cmd === 'voice_cancel').length;
    await f.emit(event, target.session); assert.equal(f.model.state.open, false);
    assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_cancel').length, cancels + 1);
  }
  f.model.show(); await f.element('voice-record').onclick(); await f.native.get('voice-window-hidden')(); assert.equal(f.model.state.open, false);
  f.model.show(); f.panel.fire('keydown', { key: 'Escape' }); await tick(); assert.equal(f.model.state.open, false);
  f.model.show(); await f.element('voice-record').onclick();
  await f.element('voice-stop').onclick();
  assert.equal(f.model.state.phase, 'stopping');
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_stop').length, 1);
  await f.model.close();
});
