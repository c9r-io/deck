import test from 'node:test';
import assert from 'node:assert/strict';
import { createVoiceInput, voiceSlice, voiceBusy, voiceRecording, voiceError, voiceSettingsTarget } from '../js/voice-model.js';

const target = { session: 'deck-voice-test', cardId: 'v1', title: 'Target' };
const other = { session: 'deck-voice-other', cardId: 'v2', title: 'Other' };
const settle = () => new Promise(resolve => setTimeout(resolve, 0));
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { resolve, reject, promise }; };
function fixture(overrides = {}) {
  const calls = [], scheduled = [], reports = [], prepared = [], finished = [];
  let snapshot = { id: 2, status: 'recording', text: '你好', preview: '世' };
  const model = createVoiceInput({
    language: 'zh-CN', changed() {}, report: (kind, code) => reports.push(`${kind}:${code}`),
    prepareTarget: async t => { prepared.push(t.session); }, afterDelivery: t => finished.push(t.session),
    schedule(fn) { scheduled.push(fn); return fn; }, unschedule(fn) { const i = scheduled.indexOf(fn); if (i >= 0) scheduled.splice(i, 1); },
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (overrides[cmd]) return overrides[cmd](args);
      if (cmd === 'voice_bind') return { id: 1, process: 'zsh' };
      if (cmd === 'voice_start') return 2;
      if (cmd === 'voice_snapshot') return snapshot;
    }, ...overrides.deps,
  });
  const typed = () => calls.filter(([cmd]) => cmd === 'voice_deliver').map(([, args]) => args.text);
  const tick = async () => { const fn = scheduled.shift(); assert.ok(fn, 'a poll is scheduled'); await fn(); };
  return { model, calls, scheduled, reports, prepared, finished, typed, tick, setSnapshot(value) { snapshot = value; } };
}

test('slices type whole words, carry the separator with the next word and never a line break', () => {
  assert.deepEqual(voiceSlice('hello world', 0, false), { text: 'hello world', next: 11 });
  assert.deepEqual(voiceSlice('hello ', 0, false), { text: 'hello', next: 5 });
  assert.deepEqual(voiceSlice('hello world', 5, false), { text: ' world', next: 11 });
  assert.deepEqual(voiceSlice('a\r\nb\tc\n', 0, true), { text: 'a b c', next: 7 });
  assert.deepEqual(voiceSlice('  ', 0, false), { text: '', next: 0 });
  assert.deepEqual(voiceSlice('  ', 0, true), { text: '', next: 2 });
  assert.deepEqual(voiceSlice('done ', 0, true), { text: 'done', next: 5 });
  assert.deepEqual(voiceSlice('same', 4, true), { text: '', next: 4 });
});

test('committed text is typed as it grows, the remainder lands after stop, and nothing sends Enter', async () => {
  const f = fixture(), m = f.model;
  await m.start(target);
  assert.equal(m.state.phase, 'recording'); assert.equal(m.state.recordingId, 2); assert.ok(m.state.startedAt);
  assert.deepEqual(f.typed(), ['你好']); assert.equal(m.state.typed, 2);
  assert.equal(m.state.preview, '世', 'the volatile tail is a preview, never typed');
  assert.deepEqual(f.prepared, [target.session]); assert.deepEqual(f.finished, [target.session]);
  f.setSnapshot({ id: 2, status: 'recording', text: '你好 世界 ', preview: '再' }); await f.tick();
  assert.deepEqual(f.typed(), ['你好', ' 世界']); assert.equal(m.state.typed, 5);
  assert.equal(m.state.preview, '再', 'committed blanks not yet typed do not show either');
  await f.tick(); assert.deepEqual(f.typed(), ['你好', ' 世界'], 'a trailing blank waits for the next word');
  await m.stop(); assert.equal(m.state.phase, 'stopping'); await m.stop();
  assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_stop').length, 1);
  f.setSnapshot({ id: 2, status: 'recording', text: '你好 世界 再' }); await f.tick();
  assert.equal(m.state.phase, 'stopping', 'a stale recording snapshot cannot undo the stop');
  assert.deepEqual(f.typed(), ['你好', ' 世界', ' 再']);
  f.setSnapshot({ id: 2, status: 'stopping', text: '你好 世界 再见' }); await f.tick();
  assert.deepEqual(f.typed(), ['你好', ' 世界', ' 再', '见']); assert.equal(m.state.phase, 'stopping');
  f.setSnapshot({ id: 2, status: 'ready', text: '你好 世界 再见。 ', preview: 'stale' }); await f.tick();
  assert.deepEqual(f.typed(), ['你好', ' 世界', ' 再', '见', '。']);
  assert.equal(m.state.phase, 'idle'); assert.equal(m.state.recordingId, null); assert.equal(m.state.startedAt, 0);
  assert.equal(m.state.preview, '');
  assert.equal(f.scheduled.length, 0); assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 2));
  assert.ok(f.calls.filter(([cmd]) => cmd === 'voice_deliver').every(([, args]) => !('submit' in args)));
  assert.deepEqual(f.reports, []);
  assert.equal(voiceBusy(m.state.phase), false);
});

test('a recognizer that commits only at the end types everything once, on ready', async () => {
  const f = fixture(), m = f.model;
  f.setSnapshot({ id: 2, status: 'recording', text: '', preview: 'whole utter' }); await m.start(target);
  await f.tick(); await f.tick(); assert.deepEqual(f.typed(), []); assert.equal(m.state.preview, 'whole utter');
  await m.stop(); f.setSnapshot({ id: 2, status: 'ready', text: 'whole utterance\n' }); await f.tick();
  assert.deepEqual(f.typed(), ['whole utterance']); assert.equal(m.state.phase, 'idle');
});

test('the recording language is the configured default, chosen at start', async () => {
  const f = fixture(), m = f.model;
  await m.start(target); await m.cancel();
  m.configure({ languages: ['en-US', 'ja-JP'], defaultLanguage: 'ja-JP' }); await m.start(target); await m.cancel();
  m.configure({ languages: ['en-US', 'ja-JP'], defaultLanguage: 'system' }); await m.start(target);
  assert.deepEqual(f.calls.filter(([cmd]) => cmd === 'voice_start').map(([, args]) => args.locale), ['zh-CN', 'ja-JP', 'en-US']);
  assert.ok(f.calls.every(([cmd, args]) => cmd !== 'voice_start' || args.targetId === 1));
});

test('start while busy is a no-op; toggle starts, stops, and waits during finalization', async () => {
  const f = fixture(), m = f.model;
  await m.toggle(target); const count = f.calls.length;
  await m.start(target); await m.start(other); assert.equal(f.calls.length, count);
  await m.toggle(target); assert.equal(m.state.phase, 'stopping');
  await m.toggle(target); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_stop').length, 1);
  assert.equal(m.state.phase, 'stopping'); await m.cancel();
  assert.equal(await m.toggle(null), undefined); assert.equal(m.state.phase, 'idle');
});

test('switching sessions, pane exit and leaving cancel capture; late snapshots type nothing', async () => {
  const f = fixture(), m = f.model;
  await m.start(target); const reply = deferred(); f.setSnapshot(reply.promise); const poll = f.tick();
  await m.select(target); assert.equal(m.state.phase, 'recording', 'the same session keeps recording');
  await m.select(other); assert.equal(m.state.phase, 'idle'); assert.equal(m.state.preview, '');
  assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 2));
  reply.resolve({ id: 2, status: 'recording', text: '你好 late private text' }); await poll;
  assert.deepEqual(f.typed(), ['你好']); assert.equal(f.scheduled.length, 0);
  await m.start(target); await m.targetExit(other.session); assert.equal(m.state.phase, 'recording');
  await m.targetExit(target.session); assert.equal(m.state.phase, 'idle');
  await m.start(target); await m.cancel(); assert.equal(m.state.phase, 'idle');
  await m.cancel(); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_cancel').length, 3);
});

test('cancelling a pending bind or start releases the eventual native id and binds nothing', async () => {
  const bind = deferred(), f = fixture({ voice_bind: () => bind.promise }), m = f.model;
  const pending = m.start(target); assert.equal(m.state.phase, 'binding');
  await m.cancel(); bind.resolve({ id: 1 }); await pending;
  assert.equal(m.state.target, null); assert.equal(m.state.phase, 'idle');
  assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_start'));
  const start = deferred(), g = fixture({ voice_start: () => start.promise }), n = g.model;
  const pendingStart = n.start(target); await settle();
  assert.equal(n.state.phase, 'preparing');
  await n.stop(); assert.equal(n.state.phase, 'idle');
  start.resolve(99); await pendingStart;
  assert.ok(g.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 99));
  assert.ok(!g.calls.some(([cmd]) => cmd === 'voice_snapshot'));
  // The next start waits for that cancellation before binding again.
  await n.start(target);
  const cancel = g.calls.findIndex(([cmd, args]) => cmd === 'voice_cancel' && args.id === 99);
  const rebind = g.calls.findLastIndex(([cmd]) => cmd === 'voice_bind');
  assert.ok(rebind > cancel);
});

test('cancelling while preparation waits drops the unsent slice', async () => {
  const preparing = deferred(), ready = deferred();
  const f = fixture({ deps: { prepareTarget: async () => { ready.resolve(); await preparing.promise; } } });
  f.setSnapshot({ id: 2, status: 'recording', text: '' });
  await f.model.start(target);
  f.setSnapshot({ id: 2, status: 'recording', text: 'cancelled words' });
  const pending = f.tick(); await ready.promise;
  await f.model.cancel(); preparing.resolve(); await pending;
  assert.deepEqual(f.typed(), []);
  assert.deepEqual(f.finished, [target.session]);
  assert.equal(f.model.state.phase, 'idle');
  assert.equal(f.scheduled.length, 0);
});

test('old preparation cannot send to or clean up a new binding, including A → B → A', async () => {
  for (const destination of [other, target]) {
    for (const rejected of [false, true]) {
      const old = deferred(), next = deferred(), oldReady = deferred(), nextReady = deferred();
      const finished = [];
      let serial = 0;
      const f = fixture({
        voice_bind: async () => ({ id: ++serial }),
        deps: {
          prepareTarget: async bound => {
            (bound.id === 1 ? oldReady : nextReady).resolve();
            await (bound.id === 1 ? old : next).promise;
          },
          afterDelivery: bound => finished.push(bound.id),
        },
      });
      const m = f.model;
      f.setSnapshot({ id: 2, status: 'recording', text: '' }); await m.start(target);
      f.setSnapshot({ id: 2, status: 'recording', text: 'old private words' });
      const pending = f.tick(); await oldReady.promise;
      await m.select(other);
      f.setSnapshot({ id: 2, status: 'recording', text: 'new words' });
      const starting = m.start(destination); await nextReady.promise;
      if (rejected) old.reject('target-not-visible'); else old.resolve();
      await pending;
      assert.deepEqual(f.typed(), [], 'revoked preparation never reaches voice_deliver');
      assert.deepEqual(finished, [1], 'cleanup retains the old binding, even for the same session');
      assert.equal(m.state.target.id, 2);
      assert.equal(m.state.phase, 'recording');
      assert.equal(m.state.typed, 0);
      assert.deepEqual(f.reports, []);
      next.resolve(); await starting;
      assert.deepEqual(f.calls.filter(([cmd]) => cmd === 'voice_deliver'),
        [['voice_deliver', { targetId: 2, text: 'new words' }]]);
      assert.deepEqual(finished, [1, 2]);
      await m.cancel();
    }
  }
});

test('already-submitted delivery stays on its captured binding and late results cannot affect a new recording', async () => {
  for (const rejected of [false, true]) {
    const delivery = deferred(), ready = deferred(), finished = [];
    let serial = 0;
    const f = fixture({
      voice_bind: async () => ({ id: ++serial }),
      voice_deliver: async () => { ready.resolve(); return delivery.promise; },
      deps: { afterDelivery: bound => finished.push(bound.id) },
    });
    f.setSnapshot({ id: 2, status: 'recording', text: '' }); await f.model.start(target);
    f.setSnapshot({ id: 2, status: 'recording', text: 'already submitted' });
    const pending = f.tick(); await ready.promise;
    await f.model.select(other);
    f.setSnapshot({ id: 2, status: 'recording', text: '', preview: 'new preview' }); await f.model.start(other);
    if (rejected) delivery.reject('delivery-unknown'); else delivery.resolve();
    await pending;
    assert.deepEqual(f.calls.filter(([cmd]) => cmd === 'voice_deliver'),
      [['voice_deliver', { targetId: 1, text: 'already submitted' }]]);
    assert.deepEqual(finished, [1]);
    assert.equal(f.model.state.typed, 0);
    assert.equal(f.model.state.preview, 'new preview');
    assert.deepEqual(f.reports, []);
    assert.equal(f.scheduled.length, 1, 'only the new recording continues polling');
    await f.model.cancel();
  }
});

test('a delivery failure awaiting native cancellation cannot reset the next recording', async () => {
  const cancelled = deferred(), ready = deferred();
  let refuse = true;
  const f = fixture({
    voice_cancel: async () => { ready.resolve(); await cancelled.promise; },
    voice_deliver: async () => { if (refuse) throw 'target-expired'; },
  });
  f.setSnapshot({ id: 2, status: 'recording', text: '' }); await f.model.start(target);
  f.setSnapshot({ id: 2, status: 'recording', text: 'old words' });
  const pending = f.tick(); await ready.promise;
  refuse = false;
  const next = f.model.start(other);
  cancelled.resolve(); await Promise.all([pending, next]);
  assert.equal(f.model.state.session, other.session);
  assert.equal(f.model.state.phase, 'recording');
  assert.equal(f.model.state.error, '');
  assert.deepEqual(f.reports, []);
  await f.model.cancel();
});

test('permission and dictation failures report once, open the matching settings once, and end the recording', async () => {
  for (const [code, kind] of [['microphone-denied', 'microphone'], ['speech-denied', 'speech'], ['dictation-disabled', 'dictation']]) {
    const f = fixture(), m = f.model;
    f.setSnapshot({ id: 2, status: 'error', text: '', code }); await m.start(target);
    assert.equal(m.state.phase, 'idle'); assert.equal(m.state.error, code); assert.equal(m.state.recordingId, null);
    assert.deepEqual(f.reports, [`error:${code}`]);
    assert.deepEqual(f.calls.filter(([cmd]) => cmd === 'voice_open_settings'), [['voice_open_settings', { kind }]]);
    assert.ok(f.calls.some(([cmd]) => cmd === 'voice_cancel')); assert.deepEqual(f.typed(), []);
    assert.equal(f.scheduled.length, 0);
    f.setSnapshot({ id: 2, status: 'ready', text: '恢复识别' }); await m.start(target);
    assert.deepEqual(f.typed(), ['恢复识别']); assert.equal(f.calls.filter(([cmd]) => cmd === 'voice_open_settings').length, 1);
  }
  const f = fixture({ voice_open_settings: async () => { throw 'cannot open'; } }), m = f.model;
  f.setSnapshot({ id: 2, status: 'error', text: '', code: 'microphone-denied' }); await m.start(target);
  assert.equal(m.state.error, 'microphone-denied');
  const g = fixture({ voice_start: async () => { throw 'local-unavailable'; } });
  await g.model.start(target); assert.deepEqual(g.reports, ['error:local-unavailable']); assert.equal(g.model.state.phase, 'idle');
  const h = fixture({ voice_bind: async () => { throw 'target-unavailable'; } });
  await h.model.start(target); assert.deepEqual(h.reports, ['error:target-unavailable']); assert.equal(h.model.state.target, null);
});

test('recognition failures type the words committed so far, then report; stale failures never open settings', async () => {
  const f = fixture(), m = f.model;
  await m.start(target);
  f.setSnapshot({ id: 2, status: 'error', text: '你好 部分结果', code: 'audio-overrun' }); await f.tick();
  assert.deepEqual(f.typed(), ['你好', ' 部分结果']); assert.deepEqual(f.reports, ['error:audio-overrun']);
  assert.equal(m.state.phase, 'idle'); assert.ok(!f.calls.some(([cmd]) => cmd === 'voice_open_settings'));
  const reply = deferred(), g = fixture({ voice_snapshot: () => reply.promise });
  const pending = g.model.start(target); await settle();
  await g.model.cancel();
  reply.resolve({ id: 2, status: 'error', text: '', code: 'speech-denied' }); await pending;
  assert.deepEqual(g.reports, []); assert.ok(!g.calls.some(([cmd]) => cmd === 'voice_open_settings'));
  for (const bad of [{ id: 3, status: 'recording', text: '你好' }, { id: 2, status: 'recording', text: 'rewritten' }, { id: 2, status: 'recording' }]) {
    const h = fixture(), n = h.model; await n.start(target); h.setSnapshot(bad); await h.tick();
    assert.equal(n.state.phase, 'idle'); assert.deepEqual(h.reports, ['error:recognition-failed']); assert.deepEqual(h.typed(), ['你好']);
  }
  const j = fixture({ voice_snapshot: async () => { throw 'failed'; } }); await j.model.start(target);
  assert.deepEqual(j.reports, ['error:operation-failed']); assert.ok(j.calls.some(([cmd]) => cmd === 'voice_cancel'));
  const k = fixture({ voice_stop: async () => { throw 'failed'; } }); await k.model.start(target); await k.model.stop();
  assert.deepEqual(k.reports, ['error:operation-failed']); assert.equal(k.model.state.phase, 'idle');
});

test('a lost or replaced target ends the recording with its code and types nothing more', async () => {
  for (const error of ['target-expired', 'target-unavailable', 'text-invalid', 'multiline-unsupported', 'transport contains secret']) {
    const f = fixture({ voice_deliver: async () => { throw error; } }), m = f.model;
    await m.start(target);
    assert.equal(m.state.phase, 'idle'); assert.equal(m.state.typed, 0);
    assert.deepEqual(f.reports, [`error:${error.startsWith('transport') ? 'operation-failed' : error}`]);
    assert.ok(f.calls.some(([cmd, args]) => cmd === 'voice_cancel' && args.id === 2));
    assert.equal(f.scheduled.length, 0); assert.deepEqual(f.finished, [target.session]);
  }
  const f = fixture({ deps: { prepareTarget: async () => { throw 'target-not-visible'; } } }), m = f.model;
  await m.start(target); assert.deepEqual(f.reports, ['error:target-not-visible']); assert.equal(m.state.phase, 'idle');
});

test('an unconfirmed paste counts as typed and is never retransmitted', async () => {
  let attempts = 0;
  const f = fixture({ voice_deliver: async () => { if (++attempts === 1) throw 'delivery-unknown'; } }), m = f.model;
  await m.start(target);
  assert.equal(m.state.phase, 'recording'); assert.equal(m.state.typed, 2); assert.deepEqual(f.reports, ['notice:delivery-unknown']);
  f.setSnapshot({ id: 2, status: 'ready', text: '你好 世界' }); await f.tick();
  assert.deepEqual(f.typed(), ['你好', ' 世界']); assert.equal(attempts, 2);
});

test('transient refusals retry on later polls without duplicates, then give up at the end or after the limit', async () => {
  let refusals = 2;
  const f = fixture({ voice_deliver: async () => { if (refusals-- > 0) throw 'delivery-busy'; } }), m = f.model;
  await m.start(target); assert.equal(m.state.typed, 0); assert.equal(m.state.phase, 'recording');
  await f.tick(); assert.equal(m.state.typed, 0);
  await f.tick(); assert.equal(m.state.typed, 2); assert.deepEqual(f.typed(), ['你好', '你好', '你好']);
  assert.deepEqual(f.reports, []);
  const g = fixture({ voice_deliver: async () => { throw 'target-changed'; } }), n = g.model;
  await n.start(target); for (let i = 0; i < 9; i++) await g.tick();
  assert.equal(n.state.phase, 'recording'); assert.deepEqual(g.reports, []);
  await g.tick(); assert.equal(n.state.phase, 'idle'); assert.deepEqual(g.reports, ['error:target-changed']);
  assert.equal(g.scheduled.length, 0);
  const h = fixture({ voice_deliver: async () => { throw 'delivery-busy'; } }), o = h.model;
  h.setSnapshot({ id: 2, status: 'ready', text: 'final words' }); await o.start(target);
  assert.deepEqual(h.reports, ['error:delivery-busy']); assert.equal(o.state.phase, 'idle');
});

test('the download notice is reported once per recording and the native cancel is also honoured', async () => {
  const f = fixture(), m = f.model;
  f.setSnapshot({ id: 2, status: 'downloading', text: '' }); await m.start(target);
  await f.tick(); assert.deepEqual(f.reports, ['notice:downloading']); assert.equal(m.state.phase, 'downloading');
  assert.equal(m.state.startedAt, 0);
  f.setSnapshot({ id: 2, status: 'cancelled', text: '' }); await f.tick();
  assert.equal(m.state.phase, 'idle'); assert.equal(f.scheduled.length, 0); assert.deepEqual(f.typed(), []);
});

test('typeText binds and types through the same guarded path', async () => {
  const f = fixture(), m = f.model;
  await m.typeText(target, ' literal words');
  assert.deepEqual(f.calls.map(([cmd]) => cmd), ['voice_bind', 'voice_deliver']);
  assert.deepEqual(f.calls[1][1], { targetId: 1, text: ' literal words' });
  assert.deepEqual(f.prepared, [target.session]); assert.deepEqual(f.finished, [target.session]);
  const g = fixture({ voice_deliver: async () => { throw 'target-expired'; } });
  await assert.rejects(g.model.typeText(target, 'x'), /target-expired/); assert.deepEqual(g.finished, [target.session]);
});

test('closed codes and phase guards are deterministic', () => {
  assert.equal(voiceError('private error'), 'operation-failed'); assert.equal(voiceError(), 'operation-failed');
  assert.equal(voiceError('text-limit'), 'text-limit');
  assert.equal(voiceBusy('binding'), true); assert.equal(voiceRecording('binding'), false);
  assert.equal(voiceRecording('downloading'), true); assert.equal(voiceBusy('idle'), false);
  assert.equal(voiceSettingsTarget('dictation-disabled'), 'dictation'); assert.equal(voiceSettingsTarget('text-limit'), null);
});
