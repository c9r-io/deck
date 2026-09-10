import test from 'node:test';
import assert from 'node:assert/strict';
import { createVoiceComposer, voiceLanguage, voiceBusy, voiceRecording, voiceCanAct, voiceError, joinVoiceDraft } from '../js/voice-model.js';

const target = { session: 'deck-voice-test', cardId: 'v1', title: 'Target' };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { resolve, reject, promise }; };
function fixture(overrides = {}) {
  const calls = [], scheduled = [];
  let snapshot = { id: 2, status: 'recording', text: '你好' };
  const model = createVoiceComposer({
    language: 'zh-CN', changed() {}, prepareTarget: async () => {}, afterDelivery() {},
    schedule(fn) { scheduled.push(fn); return fn; }, unschedule(fn) { const i = scheduled.indexOf(fn); if (i >= 0) scheduled.splice(i, 1); },
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (overrides[cmd]) return overrides[cmd](args);
      if (cmd === 'voice_bind') return { id: 1, process: 'zsh' };
      if (cmd === 'voice_start') return 2;
      if (cmd === 'voice_snapshot') return snapshot;
      if (cmd === 'voice_deliver') return 'submitted';
    }, ...overrides.deps,
  });
  return { model, calls, scheduled, setSnapshot(value) { snapshot = value; } };
}

test('placements preserve the same draft, recorder and target; partial results replace rather than duplicate', async () => {
  const f = fixture(), m = f.model;
  m.show(); await m.bind(target); m.edit('已有草稿'); await m.start();
  assert.equal(m.state.draft, '已有草稿\n你好');
  const count = f.calls.length;
  for (const layout of ['floating', 'right', 'bottom']) {
    m.layout(layout); assert.equal(m.state.layout, layout); assert.equal(m.state.target.session, target.session);
    assert.equal(m.state.recordingId, 2); assert.equal(m.state.draft, '已有草稿\n你好');
  }
  assert.equal(f.calls.length, count);
  m.layout('window'); assert.equal(m.state.layout, 'bottom');
  m.edit('overwrite'); m.language('ja-JP'); await m.bind({ ...target, session: 'other' }); await m.start();
  assert.equal(f.calls.length, count, 'busy recording rejects editing, rebinding and duplicate start');
  f.setSnapshot({ id: 2, status: 'recording', text: '你好世界' });
  await f.scheduled.shift()(); assert.equal(m.state.draft, '已有草稿\n你好世界');
  await m.stop(); assert.equal(m.state.phase, 'stopping'); await m.stop();
  f.setSnapshot({ id: 2, status: 'ready', text: '你好，世界。' });
  await f.scheduled.shift()(); assert.equal(m.state.phase, 'ready'); assert.equal(m.state.recordingId, null);
  assert.equal(m.state.draft, '已有草稿\n你好，世界。');
  m.edit('修改后'); m.language('ja-JP'); assert.equal(m.state.language, 'ja-JP');
  await m.deliver(true); assert.equal(m.state.draft, ''); assert.equal(m.state.notice, 'submitted');
  assert.equal(m.state.target.session, target.session);
  assert.equal(createVoiceComposer({ invoke() {} }).state.layout, 'bottom', 'no persistence');
});

test('closing a pending bind/start cancels late recording without reopening', async () => {
  const bind = deferred(), f = fixture({ voice_bind: () => bind.promise }), m = f.model;
  m.show(); const pending = m.bind(target); await m.close(); bind.resolve({ id: 1 });
  assert.equal(await pending, false); assert.equal(m.state.target, null);
  const start = deferred(), g = fixture({ voice_start: () => start.promise }), n = g.model;
  n.show(); await n.bind(target); const pendingStart = n.start(); await n.close(); start.resolve(99); await pendingStart;
  assert.equal(n.state.open, false); assert.equal(n.state.phase, 'idle');
  assert.ok(g.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 99));
});

test('late snapshots after close cannot overwrite an edited draft or restart polling', async () => {
  const reply = deferred(), f = fixture({ voice_snapshot: () => reply.promise }), m = f.model;
  await m.bind(target); const start = m.start(); await Promise.resolve();
  await m.close(); m.edit('保留这一句'); reply.resolve({ id: 2, status: 'recording', text: '旧回调' }); await start;
  assert.equal(m.state.draft, '保留这一句'); assert.equal(f.scheduled.length, 0);
});

test('stop during pending start keeps the editor open and cancels the eventual native id', async () => {
  const reply = deferred(), f = fixture({ voice_start: () => reply.promise }), m = f.model;
  m.show(); await m.bind(target); m.edit('keep'); const start = m.start();
  await m.stop(); assert.equal(m.state.open, true); assert.equal(m.state.phase, 'idle');
  reply.resolve(77); await start;
  assert.equal(m.state.draft, 'keep'); assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 77));
});

test('native cancellation does not replace a partial draft with an empty snapshot', async () => {
  const f = fixture(), m = f.model; await m.bind(target); await m.start();
  f.setSnapshot({ id: 2, status: 'cancelled', text: '' }); await f.scheduled.shift()();
  assert.equal(m.state.draft, '你好'); assert.equal(m.state.recordingId, null);
});

test('recognition failure retains partial text, cancels native capture, and does not send', async () => {
  const f = fixture(), m = f.model;
  await m.bind(target); await m.start();
  f.setSnapshot({ id: 2, status: 'error', text: '部分结果', code: 'microphone-unavailable' });
  await f.scheduled.shift()(); assert.equal(m.state.draft, '部分结果'); assert.equal(m.state.error, 'microphone-unavailable');
  assert.ok(f.calls.some(([cmd]) => cmd === 'voice_cancel')); assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_deliver'));
});

test('uncertain delivery keeps the session binding but a second ordinary click never resends', async () => {
  for (const outcome of ['ambiguous', 'enter-refused']) {
    const f = fixture({ voice_deliver: async () => outcome }), m = f.model;
    await m.bind(target); m.edit('不要删除文件'); await m.deliver(true);
    assert.equal(m.state.draft, '不要删除文件'); assert.equal(m.state.notice, outcome); assert.equal(m.state.target.session, target.session); assert.equal(m.state.needsConfirmation, true);
    await m.deliver(true); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 1);
  }
});

test('delivery blocks duplicate clicks and close; insert-only does not submit', async () => {
  const reply = deferred(), f = fixture({ voice_deliver: () => reply.promise }), m = f.model;
  m.show(); await m.bind(target); m.edit('hello'); const sent = m.deliver(false); await Promise.resolve();
  await m.deliver(true); await m.close(); assert.equal(m.state.open, true); assert.equal(m.state.phase, 'sending');
  reply.resolve('inserted'); await sent;
  const deliveries = f.calls.filter(([cmd]) => cmd === 'voice_deliver'); assert.equal(deliveries.length, 1);
  assert.equal(deliveries[0][1].submit, false); assert.equal(m.state.notice, 'inserted');
});

test('closed errors and preflight failures preserve draft without dispatching', async () => {
  const f = fixture({ deps: { prepareTarget: async () => { throw 'target-not-visible'; } } }), m = f.model;
  await m.bind(target); m.edit('keep'); await m.deliver(true);
  assert.equal(m.state.error, 'target-not-visible'); assert.ok(m.state.target); assert.equal(m.state.draft, 'keep');
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_deliver'));
  for (const error of ['target-changed', 'delivery-busy', 'transport contains secret']) {
    const g = fixture({ voice_deliver: async () => { throw error; } });
    await g.model.bind(target); g.model.edit('keep'); await g.model.deliver(true);
    assert.equal(g.model.state.target.session, target.session); assert.equal(g.model.state.draft, 'keep');
    assert.equal(g.model.state.needsConfirmation, error.startsWith('transport'));
    assert.equal(g.model.state.error, error.startsWith('transport') ? 'delivery-unknown' : error);
  }
});

test('language mapping, phase guards and transcript joining are deterministic', () => {
  assert.equal(voiceLanguage('zh-Hant-HK'), 'zh-TW'); assert.equal(voiceLanguage('zh-CN'), 'zh-CN');
  assert.equal(voiceLanguage('ja-JP'), 'ja-JP'); assert.equal(voiceLanguage('xx'), 'en-US'); assert.equal(voiceLanguage(), 'en-US');
  assert.equal(joinVoiceDraft('', 'a'), 'a'); assert.equal(joinVoiceDraft('a', ''), 'a');
  assert.equal(voiceError('private error'), 'operation-failed'); assert.equal(voiceError(), 'operation-failed');
  assert.equal(voiceBusy('sending'), true); assert.equal(voiceRecording('sending'), false);
  assert.equal(voiceRecording('downloading'), true); assert.equal(voiceBusy('ready'), false);
});

test('failed bind/start/poll/stop leave a recoverable composer', async () => {
  const f = fixture({ voice_bind: async () => { throw 'target-unavailable'; } });
  assert.equal(await f.model.bind(target), false); assert.equal(f.model.state.phase, 'error');
  const g = fixture({ voice_start: async () => { throw 'local-unavailable'; } });
  await g.model.bind(target); await g.model.start(); assert.equal(g.model.state.error, 'local-unavailable');
  const h = fixture({ voice_snapshot: async () => { throw 'failed'; } });
  await h.model.bind(target); await h.model.start(); assert.equal(h.model.state.phase, 'error');
  assert.ok(h.calls.some(([cmd]) => cmd === 'voice_cancel'));
  const j = fixture({ voice_stop: async () => { throw 'failed'; } });
  await j.model.bind(target); await j.model.start(); await j.model.stop(); assert.equal(j.model.state.phase, 'error'); await j.model.close();
});

const other = { session: 'deck-voice-other', cardId: 'v2', title: 'Other' };

test('session switches restore independent drafts, languages and bindings without rebinding or recording', async () => {
  const f = fixture(), m = f.model;
  m.show(); await m.ensureTarget(target); m.edit('A draft'); m.language('ja-JP'); m.layout('floating');
  const a = m.state.target;
  await m.select(other); assert.equal(m.state.draft, ''); assert.equal(m.state.language, 'zh-CN');
  assert.equal(m.state.open, true); assert.equal(m.state.layout, 'floating');
  m.edit('B draft'); m.language('en-US'); const b = m.state.target;
  for (let i = 0; i < 3; i++) {
    await m.select(target); assert.equal(m.state.draft, 'A draft'); assert.equal(m.state.language, 'ja-JP'); assert.equal(m.state.target, a);
    await m.select(other); assert.equal(m.state.draft, 'B draft'); assert.equal(m.state.language, 'en-US'); assert.equal(m.state.target, b);
  }
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 2);
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_start'));
  await m.close({ preserveOpen: true }); await m.select(target); assert.equal(m.state.open, true);
  await m.close(); await m.select(other); assert.equal(m.state.draft, 'B draft');
});

test('switching capture keeps the displayed partial in A and ignores late snapshots while editing B', async () => {
  const f = fixture(), m = f.model;
  m.show(); await m.ensureTarget(target); m.edit('A'); await m.start();
  const reply = deferred();
  f.setSnapshot(reply.promise);
  const poll = f.scheduled.shift()();
  await m.select(other); m.edit('B');
  reply.resolve({ id: 2, status: 'recording', text: 'late private text' }); await poll;
  assert.equal(m.state.draft, 'B'); assert.equal(m.state.recordingId, null); assert.equal(f.scheduled.length, 0);
  await m.select(target); assert.equal(m.state.draft, 'A\n你好');
  assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 2));
});

test('rapid switches await late native start cancellation and bind only the final selection', async () => {
  const reply = deferred(), f = fixture({ voice_start: () => reply.promise }), m = f.model;
  m.show(); await m.ensureTarget(target); m.edit('A'); const recording = m.start();
  const b = m.select(other), a = m.select(target), c = m.select({ ...other, session: 'third' });
  m.edit('C'); reply.resolve(77);
  await Promise.all([recording, a, b, c]);
  assert.equal(m.state.session, 'third'); assert.equal(m.state.draft, 'C');
  const cancel = f.calls.findIndex(([cmd, args]) => cmd === 'voice_cancel' && args.id === 77);
  const bind = f.calls.findIndex(([cmd, args]) => cmd === 'voice_bind' && args.name === 'third');
  assert.ok(cancel >= 0 && bind > cancel);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 2);
  await m.select(target); assert.equal(m.state.draft, 'A');
});

test('pending binds cannot overwrite the selected session and closing during a switch stays closed', async () => {
  const reply = deferred(), f = fixture({ voice_bind: ({ name }) => name === target.session ? reply.promise : { id: 3 } }), m = f.model;
  m.show(); const binding = m.bind(target); const switching = m.select(other); await m.close();
  reply.resolve({ id: 1 }); await Promise.all([binding, switching]);
  assert.equal(m.state.open, false); assert.equal(m.state.session, other.session); assert.equal(m.state.target, null);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 1);
});

test('a delivery completes only in its originating session; another session cannot send concurrently', async () => {
  const reply = deferred(), f = fixture({ voice_deliver: () => reply.promise }), m = f.model;
  m.show(); await m.ensureTarget(other); m.edit('B'); await m.select(target); m.edit('A');
  const sent = m.deliver(true); await Promise.resolve(); await m.select(other);
  assert.equal(m.state.deliveryBusy, true); await m.deliver(true); await m.start();
  reply.resolve('submitted'); await sent;
  assert.equal(m.state.draft, 'B'); assert.equal(m.state.notice, ''); assert.equal(m.state.deliveryBusy, false);
  await m.select(target); assert.equal(m.state.draft, ''); assert.equal(m.state.notice, 'submitted');
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 1);
  m.edit('A again'); await m.deliver(false);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 2, 'successful delivery retains the original binding');
});

test('uncertain A delivery is not implicitly reauthorized by visiting B and returning', async () => {
  const f = fixture({ voice_deliver: async () => 'ambiguous' }), m = f.model;
  m.show(); await m.ensureTarget(target); m.edit('A'); await m.deliver(true);
  await m.select(other); m.edit('B'); await m.select(target);
  assert.equal(await m.ensureTarget(target), false); await m.deliver(true);
  assert.equal(m.state.draft, 'A'); assert.equal(m.state.notice, 'ambiguous'); assert.equal(m.state.target.session, target.session);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 1);
  await m.bind(target); assert.equal(m.state.needsConfirmation, true); await m.deliver(true);
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_deliver').length, 1);
});


test('a new session selected during delivery acquires its binding after the original send completes', async () => {
  const reply = deferred(), f = fixture({ voice_deliver: () => reply.promise }), m = f.model;
  m.show(); await m.ensureTarget(target); m.edit('A'); const sent = m.deliver(true);
  await m.select(other); m.edit('B'); assert.equal(m.state.target, null);
  reply.resolve('submitted'); await sent;
  assert.equal(m.state.target.session, other.session); assert.equal(m.state.draft, 'B');
  assert.equal(m.state.deliveryBusy, false);
});

test('a late stop failure for A cannot cancel B recording or change its draft', async () => {
  const reply = deferred(), f = fixture({ voice_stop: () => reply.promise }), m = f.model;
  m.show(); await m.ensureTarget(target); await m.start(); const stop = m.stop();
  await m.select(other); m.edit('B'); await m.start();
  reply.reject('old stop failed'); await stop;
  assert.equal(m.state.phase, 'recording'); assert.equal(m.state.draft, 'B\n你好'); assert.equal(m.state.error, '');
  assert.equal(f.scheduled.length, 1); await m.close();
});


for (const action of ['clear', 'insert', 'send']) {
  const act = m => action === 'clear' ? m.clear() : m.deliver(action === 'send');
  test(`${action} during capture freezes visible text, cancels first and ignores late snapshots`, async () => {
    const cancelled = deferred(), f = fixture({ voice_cancel: () => cancelled.promise }), m = f.model;
    m.show(); await m.ensureTarget(target); await m.start();
    const late = deferred(); f.setSnapshot(late.promise); const poll = f.scheduled.shift()();
    assert.equal(voiceCanAct(m.state), true);
    const operation = act(m);
    assert.equal(m.state.recordingId, null); assert.equal(voiceCanAct(m.state), false);
    await act(m); await m.clear(); await m.deliver(true); await m.start();
    assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_deliver'));
    cancelled.resolve(); await operation;
    late.resolve({ id: 2, status: 'ready', text: 'late text must never be sent' }); await poll;
    assert.equal(m.state.draft, ''); assert.equal(m.state.open, true); assert.equal(f.scheduled.length, 0);
    const deliveries = f.calls.filter(([cmd]) => cmd === 'voice_deliver');
    assert.equal(deliveries.length, action === 'clear' ? 0 : 1);
    if (deliveries.length) {
      assert.equal(deliveries[0][1].text, '你好'); assert.equal(deliveries[0][1].submit, action === 'send');
      assert.ok(f.calls.findIndex(([cmd]) => cmd === 'voice_cancel') < f.calls.findIndex(([cmd]) => cmd === 'voice_deliver'));
    }
  });

  test(`${action} on an empty draft leaves recording and polling untouched`, async () => {
    const f = fixture(), m = f.model; await m.bind(target);
    f.setSnapshot({ id: 2, status: 'recording', text: ' \n\t' }); await m.start();
    const before = structuredClone(m.state), calls = f.calls.length;
    assert.equal(voiceCanAct(m.state), false); await act(m);
    assert.deepEqual(m.state, before); assert.equal(f.calls.length, calls); assert.equal(f.scheduled.length, 1);
    await m.close();
  });

  test(`${action} during pending microphone preparation waits for late start cancellation`, async () => {
    const start = deferred(), f = fixture({ voice_start: () => start.promise }), m = f.model;
    await m.bind(target); m.edit('visible'); const starting = m.start(), operation = act(m);
    assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_deliver'));
    start.resolve(77); await Promise.all([starting, operation]);
    assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 77));
    assert.equal(m.state.draft, ''); assert.equal(m.state.recordingId, null);
    assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_snapshot'));
  });

  test(`${action} is allowed during model download and transcription finalization`, async () => {
    for (const phase of ['downloading', 'stopping']) {
      const f = fixture(), m = f.model; await m.bind(target); m.edit('visible');
      f.setSnapshot({ id: 2, status: phase, text: '' }); await m.start();
      assert.equal(voiceCanAct(m.state), true); await act(m);
      assert.equal(m.state.draft, ''); assert.equal(m.state.recordingId, null); assert.equal(f.scheduled.length, 0);
    }
  });
}

test('failed recording cancellation prevents delivery and keeps the visible draft and target', async () => {
  const f = fixture({ voice_cancel: async () => { throw 'cancel failed'; } }), m = f.model;
  await m.bind(target); await m.start(); await m.deliver(true);
  assert.equal(m.state.draft, '你好'); assert.equal(m.state.error, 'operation-failed'); assert.ok(m.state.target);
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_deliver'));
});

test('switching sessions during clear cannot clear B or leave its first binding disabled', async () => {
  const cancelled = deferred(), f = fixture({ voice_cancel: () => cancelled.promise }), m = f.model;
  m.show(); await m.ensureTarget(target); await m.start(); const cleared = m.clear();
  const switching = m.select(other); m.edit('B draft'); cancelled.resolve(); await Promise.all([cleared, switching]);
  assert.equal(m.state.draft, 'B draft'); assert.equal(m.state.target.session, other.session); assert.equal(m.state.deliveryBusy, false);
  await m.select(target); assert.equal(m.state.draft, '');
});


test('known refusals keep the session binding and allow an ordinary corrected send without confirmation', async () => {
  for (const error of ['multiline-unsupported', 'target-changed', 'target-unavailable', 'delivery-busy', 'text-invalid']) {
    let attempts = 0;
    const f = fixture({ voice_deliver: async () => { if (++attempts === 1) throw error; return 'submitted'; } }), m = f.model;
    await m.bind(target); const binding = m.state.target; m.edit('first\nsecond'); await m.deliver(true);
    assert.equal(m.state.target, binding); assert.equal(m.state.needsConfirmation, false); assert.equal(m.state.draft, 'first\nsecond');
    m.edit('first second'); await m.deliver(true);
    assert.equal(m.state.draft, ''); assert.equal(m.state.notice, 'submitted'); assert.equal(attempts, 2);
    assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 1);
  }
});

test('explicit uncertain retry repeats the original insert/send once without rebinding', async () => {
  for (const submit of [false, true]) {
    const retry = deferred(); let attempts = 0;
    const f = fixture({ voice_deliver: async () => ++attempts === 1 ? 'ambiguous' : retry.promise }), m = f.model;
    await m.bind(target); m.edit('review this'); await m.deliver(submit);
    const pending = m.retry(); await m.retry(); await m.deliver(submit);
    retry.resolve(submit ? 'submitted' : 'inserted'); await pending;
    const deliveries = f.calls.filter(([cmd]) => cmd === 'voice_deliver');
    assert.equal(deliveries.length, 2); assert.equal(deliveries[1][1].submit, submit);
    assert.equal(m.state.needsConfirmation, false); assert.equal(m.state.draft, '');
    assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 1);
  }
});

test('clearing an uncertain draft allows a new prompt on the same session without rebinding', async () => {
  let attempts = 0;
  const f = fixture({ voice_deliver: async () => ++attempts === 1 ? 'ambiguous' : 'submitted' }), m = f.model;
  await m.bind(target); m.edit('old'); await m.deliver(true); await m.clear();
  assert.equal(m.state.needsConfirmation, false); m.edit('new'); await m.deliver(true);
  assert.equal(m.state.notice, 'submitted'); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_bind').length, 1);
});

test('preference changes preserve enabled session choices, apply defaults to new sessions, and never start capture', async () => {
  const f = fixture(), m = f.model;
  await m.select(target); m.language('ja-JP'); m.edit('keep A');
  m.configure({ languages: ['en-US', 'ja-JP'], defaultLanguage: 'en-US' });
  assert.equal(m.state.language, 'ja-JP'); assert.equal(m.state.draft, 'keep A');
  await m.select(other); assert.equal(m.state.language, 'en-US');
  m.configure({ languages: ['en-US'], defaultLanguage: 'en-US' });
  await m.select(target); assert.equal(m.state.language, 'en-US'); assert.deepEqual(m.state.languages, ['en-US']);
  assert.equal(m.state.draft, 'keep A'); m.language('zh-CN'); assert.equal(m.state.language, 'en-US');
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_start' || cmd === 'voice_cancel'));
});

test('removing the recording language defers replacement until final text arrives', async () => {
  const f = fixture(), m = f.model; m.show(); await m.ensureTarget(target); await m.start();
  const calls = f.calls.length;
  m.configure({ languages: ['en-US'], defaultLanguage: 'en-US' });
  assert.equal(m.state.language, 'zh-CN'); assert.deepEqual(m.state.languages, ['zh-CN', 'en-US']);
  assert.equal(m.state.recordingId, 2); assert.equal(f.calls.length, calls);
  f.setSnapshot({ id: 2, status: 'ready', text: '完整结果' }); await f.scheduled.shift()();
  assert.equal(m.state.draft, '完整结果'); assert.equal(m.state.language, 'en-US'); assert.deepEqual(m.state.languages, ['en-US']);
});

test('changing preferences during a pending start does not invalidate or restart the microphone request', async () => {
  const ready = deferred(), f = fixture({ voice_start: () => ready.promise }), m = f.model;
  await m.bind(target); const starting = m.start();
  m.configure({ languages: ['ja-JP'], defaultLanguage: 'ja-JP' });
  ready.resolve(2); await starting;
  assert.equal(m.state.phase, 'recording'); assert.equal(m.state.language, 'zh-CN');
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_start').length, 1);
  await m.close(); assert.equal(m.state.language, 'ja-JP'); assert.equal(m.state.draft, '你好');
});
