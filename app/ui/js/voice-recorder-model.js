// One microphone owner. Capture epochs reject late replies independently of
// session binding. Closing cancels promptly; draft actions await native cleanup
// before any paste. Pending start replies must release their ids before reuse.
import { voiceRecording, voiceError, joinVoiceDraft } from './voice-state-model.js';

export function createVoiceRecorder(deps) {
  const { invoke, changed } = deps;
  const schedule = deps.schedule || (fn => setTimeout(fn, 200));
  const unschedule = deps.unschedule || clearTimeout;
  let epoch = 0, timer = null, base = '';
  let pendingStart = Promise.resolve(), pendingCancel = Promise.resolve();
  const cancelNative = id => (pendingCancel = invoke('voice_cancel', { id }).catch(() => {}));
  const clearPoll = () => { if (timer !== null) unschedule(timer); timer = null; };
  async function poll(owner, revision, id) {
    try {
      const result = await invoke('voice_snapshot', { id });
      if (epoch !== revision || owner.recordingId !== id) return;
      if (result.id !== id) throw new Error('recording-expired');
      if (result.status !== 'cancelled') owner.draft = joinVoiceDraft(base, result.text || '');
      owner.phase = result.status;
      if (owner.phase === 'recording' && !owner.startedAt) owner.startedAt = Date.now();
      if (result.code) owner.error = voiceError(result.code);
      if (voiceRecording(owner.phase)) timer = schedule(() => poll(owner, revision, id));
      else { owner.recordingId = null; await cancelNative(id); }
      if (epoch === revision) changed();
    } catch (_) {
      if (epoch !== revision) return;
      owner.phase = 'error'; owner.error = 'operation-failed'; owner.recordingId = null;
      await cancelNative(id); if (epoch === revision) changed();
    }
  }

  async function startOwned(owner) {
    const revision = ++epoch;
    base = owner.draft.trimEnd();
    owner.phase = 'preparing'; owner.error = ''; owner.notice = ''; owner.startedAt = 0; changed();
    try {
      const id = await invoke('voice_start', { targetId: owner.target.id, locale: owner.language });
      if (epoch !== revision) { await cancelNative(id); return; }
      owner.recordingId = id; changed();
      await poll(owner, revision, id);
    } catch (error) {
      if (epoch !== revision) return;
      owner.phase = 'error'; owner.error = voiceError(error); changed();
    }
  }

  async function stopOwned(owner) {
    if (!voiceRecording(owner.phase) || owner.phase === 'stopping') return;
    if (owner.recordingId === null) {
      ++epoch; clearPoll(); owner.phase = 'idle'; changed(); return;
    }
    const revision = epoch;
    owner.phase = 'stopping'; changed();
    try { await invoke('voice_stop', { id: owner.recordingId }); }
    catch (_) {
      if (epoch !== revision) return;
      const cancelledRevision = ++epoch; clearPoll(); const id = owner.recordingId; owner.recordingId = null;
      await cancelNative(id);
      if (epoch !== cancelledRevision) return;
      owner.phase = 'error'; owner.error = 'operation-failed'; changed();
    }
  }

  function interruptCapture(owner) {
    ++epoch; clearPoll();
    const id = owner.recordingId;
    owner.recordingId = null;
    // With an outstanding start, wait for its stale-reply cancellation. With
    // an id, cancel directly without waiting for an in-flight snapshot reply.
    const cancelled = id === null ? pendingStart : invoke('voice_cancel', { id });
    const cleanup = Promise.all([pendingCancel, cancelled]).then(() => {});
    pendingCancel = cleanup.catch(() => {});
    return cleanup;
  }

  async function close(owner) {
    ++epoch; clearPoll();
    const id = owner.recordingId;
    owner.recordingId = null;
    if (id !== null) await cancelNative(id);
  }
  return {
    start(owner) { pendingStart = startOwned(owner); return pendingStart; },
    stop: stopOwned, close, interrupt: interruptCapture,
    settled: () => Promise.all([pendingStart, pendingCancel]),
  };
}
