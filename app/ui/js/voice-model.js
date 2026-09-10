// One mounted editor with volatile drafts, languages and bindings per session.
// Switching releases capture and keeps the last displayed partial in its owner.
// Epochs suppress late replies; sends finish against their captured session.
// Clear/insert/send freeze the visible draft and release capture immediately;
// empty drafts are no-ops, and late finalization cannot change the action text.
// Preference changes preserve enabled session choices and defer replacing a
// removed language until its recording/delivery finishes.
// Session bindings survive refusals. Only uncertain delivery requires the
// separate retry action; it never gets reauthorized by switching sessions.
import { defaultVoiceLanguage, normalizeVoicePreferences, VOICE_LANGUAGES, voiceLanguage } from './voice-preferences-model.js';
export { VOICE_LANGUAGES, voiceLanguage };
export const VOICE_LAYOUTS = ['bottom', 'floating', 'right'];
export const voiceBusy = phase => ['binding', 'preparing', 'downloading', 'recording', 'stopping', 'sending'].includes(phase);
export const voiceRecording = phase => ['preparing', 'downloading', 'recording', 'stopping'].includes(phase);
export const voiceCanAct = s => !!s.draft.trim() && !s.deliveryBusy && !s.switching
  && !['binding', 'sending'].includes(s.phase);
export function joinVoiceDraft(base, transcript) {
  return base && transcript ? `${base}\n${transcript}` : base || transcript;
}
const ERROR_CODES = new Set(['microphone-denied', 'speech-denied', 'microphone-unavailable', 'local-unavailable',
  'recognition-failed', 'finalize-timeout', 'audio-overrun', 'text-limit', 'target-unavailable', 'target-changed',
  'delivery-busy', 'delivery-unknown', 'text-invalid', 'multiline-unsupported', 'target-not-visible']);
export function voiceError(error) {
  const text = String(error || '');
  return ERROR_CODES.has(text) ? text : 'operation-failed';
}

export function createVoiceComposer(deps) {
  const invoke = deps.invoke;
  const schedule = deps.schedule || (fn => setTimeout(fn, 200));
  const unschedule = deps.unschedule || clearTimeout;
  let preferences = normalizeVoicePreferences(deps.preferences);
  const fresh = () => ({ open: false, layout: 'bottom', phase: 'idle', draft: '', target: null,
    language: defaultVoiceLanguage(preferences, deps.language), error: '', notice: '', recordingId: null, startedAt: 0,
    languages: [...preferences.languages],
    session: null, needsConfirmation: false, deliveryBusy: false });
  let s = fresh();
  const sessions = new Map();
  let releasing = false, resumeOpen = false;
  let placement = 'bottom', selection = 0, deliveryBusy = false;
  let release = Promise.resolve(), pendingBind = Promise.resolve(), pendingStart = Promise.resolve(), pendingCancel = Promise.resolve();
  let epoch = 0, timer = null, base = '';
  const reconcileLanguage = state => {
    if (!voiceBusy(state.phase) && !preferences.languages.includes(state.language)) {
      state.language = defaultVoiceLanguage(preferences, deps.language);
    }
    state.languages = preferences.languages.includes(state.language)
      ? [...preferences.languages] : [state.language, ...preferences.languages];
  };
  const changed = () => {
    reconcileLanguage(s);
    s.deliveryBusy = deliveryBusy; s.switching = releasing; deps.changed?.(s);
  };
  const cancelNative = id => (pendingCancel = invoke('voice_cancel', { id }).catch(() => {}));
  const clearPoll = () => { if (timer !== null) unschedule(timer); timer = null; };

  function bind(target) {
    if (voiceBusy(s.phase) || deliveryBusy) return Promise.resolve(false);
    const owner = s, revision = epoch;
    pendingBind = releasing ? release.then(() => owner === s && epoch === revision && bindOwned(owner, target)) : bindOwned(owner, target);
    return pendingBind;
  }
  async function bindOwned(s, target) {
    if (deliveryBusy || voiceBusy(s.phase) || !target || (s.session && target.session !== s.session)) return false;
    s.session = target.session; sessions.set(s.session, s);
    const revision = ++epoch;
    s.phase = 'binding'; s.error = ''; s.notice = ''; changed();
    try {
      const result = await invoke('voice_bind', { name: target.session });
      if (epoch !== revision) return false;
      s.target = { ...target, ...result }; s.phase = 'idle'; changed(); return true;
    } catch (error) {
      if (epoch !== revision) return false;
      s.target = null; s.phase = 'error'; s.error = voiceError(error); changed(); return false;
    }
  }

  async function poll(s, revision, id) {
    try {
      const result = await invoke('voice_snapshot', { id });
      if (epoch !== revision || s.recordingId !== id) return;
      if (result.id !== id) throw new Error('recording-expired');
      if (result.status !== 'cancelled') s.draft = joinVoiceDraft(base, result.text || '');
      s.phase = result.status;
      if (s.phase === 'recording' && !s.startedAt) s.startedAt = Date.now();
      if (result.code) s.error = voiceError(result.code);
      if (voiceRecording(s.phase)) timer = schedule(() => poll(s, revision, id));
      else { s.recordingId = null; await cancelNative(id); }
      if (epoch === revision) changed();
    } catch (_) {
      if (epoch !== revision) return;
      s.phase = 'error'; s.error = 'operation-failed'; s.recordingId = null;
      await cancelNative(id); changed();
    }
  }

  function start() {
    if (voiceBusy(s.phase) || deliveryBusy) return Promise.resolve();
    const owner = s, revision = epoch;
    pendingStart = releasing ? release.then(() => owner === s && epoch === revision && startOwned(owner)) : startOwned(owner);
    return pendingStart;
  }
  async function startOwned(s) {
    if (deliveryBusy || voiceBusy(s.phase) || !s.target) return;
    const revision = ++epoch;
    base = s.draft.trimEnd();
    s.phase = 'preparing'; s.error = ''; s.notice = ''; s.startedAt = 0; changed();
    try {
      const id = await invoke('voice_start', { targetId: s.target.id, locale: s.language });
      if (epoch !== revision) { await cancelNative(id); return; }
      s.recordingId = id; changed();
      await poll(s, revision, id);
    } catch (error) {
      if (epoch !== revision) return;
      s.phase = 'error'; s.error = voiceError(error); changed();
    }
  }

  async function stop() {
    const owner = s;
    return stopOwned(owner);
  }
  async function stopOwned(s) {
    if (!voiceRecording(s.phase) || s.phase === 'stopping') return;
    if (s.recordingId === null) {
      ++epoch; clearPoll(); s.phase = 'idle'; changed(); return;
    }
    const revision = epoch;
    s.phase = 'stopping'; changed();
    try { await invoke('voice_stop', { id: s.recordingId }); }
    catch (_) {
      if (epoch !== revision) return;
      ++epoch; clearPoll(); const id = s.recordingId; s.recordingId = null;
      await cancelNative(id); s.phase = 'error'; s.error = 'operation-failed'; changed();
    }
  }

  async function close({ preserveOpen = false } = {}) {
    const s = currentState();
    resumeOpen = preserveOpen && s.open;
    if (s.phase === 'sending') return;
    ++epoch; clearPoll();
    const id = s.recordingId;
    s.recordingId = null; s.open = false; s.phase = 'idle'; changed();
    if (id !== null) await cancelNative(id);
  }

  function interruptCapture(s) {
    ++epoch; clearPoll();
    const id = s.recordingId;
    s.recordingId = null;
    // With an outstanding start, wait for its stale-reply cancellation. With
    // an id, cancel directly without waiting for an in-flight snapshot reply.
    const cancelled = id === null ? pendingStart : invoke('voice_cancel', { id });
    const cleanup = Promise.all([pendingCancel, cancelled]).then(() => {});
    pendingCancel = cleanup.catch(() => {});
    return cleanup;
  }

  async function clear() {
    const owner = s;
    if (!voiceCanAct(owner)) return;
    const cleanup = interruptCapture(owner);
    deliveryBusy = true; // One draft action owns microphone cleanup at a time.
    owner.draft = ''; owner.needsConfirmation = false; owner.phase = 'stopping'; owner.error = ''; owner.notice = ''; changed();
    try { await cleanup; owner.phase = 'idle'; }
    catch (_) { owner.phase = 'error'; owner.error = 'operation-failed'; }
    finally { await finishAction(owner); }
  }

  async function deliver(submit, { retry = false } = {}) {
    const s = currentState();
    if (!voiceCanAct(s) || !s.target || (s.needsConfirmation && !retry)) return;
    const target = s.target, text = s.draft;
    const cleanup = interruptCapture(s);
    deliveryBusy = true;
    s.lastSubmit = submit;
    s.phase = 'sending'; s.error = ''; s.notice = ''; changed();
    let attempted = false;
    try {
      await cleanup;
      await deps.prepareTarget(target);
      attempted = true;
      const result = await invoke('voice_deliver', { targetId: target.id, text, submit });
      if (result === 'submitted' || result === 'inserted') {
        s.draft = ''; s.needsConfirmation = false; s.phase = 'idle'; s.notice = result;
      } else {
        s.needsConfirmation = true;
        s.phase = 'error'; s.notice = result === 'enter-refused' ? 'enter-refused' : 'ambiguous';
      }
    } catch (error) {
      s.phase = 'error'; s.error = attempted && voiceError(error) === 'operation-failed' ? 'delivery-unknown' : voiceError(error);
      if (attempted) s.needsConfirmation = s.error === 'delivery-unknown';
    } finally {
      deps.afterDelivery?.(target); await finishAction(s);
    }
  }

  async function finishAction(owner) {
    deliveryBusy = false; changed();
    if (s !== owner && s.open && !s.target && !s.needsConfirmation) await bind(s.destination);
  }

  const currentState = () => s;
  async function select(target) {
    if (!target) return;
    if (s.session === target.session) {
      s.open ||= resumeOpen; resumeOpen = false;
      if (s.target) { s.target.title = target.title; changed(); }
      return;
    }
    const revision = ++selection, previous = s, wasOpen = previous.open || resumeOpen;
    // Capture all outstanding IPC before selecting the next owner. A late
    // start must be cancelled before another session can acquire the mic.
    const closing = close();
    releasing = true;
    release = Promise.all([release, closing, pendingBind, pendingStart, pendingCancel]).then(() => {});
    s = sessions.get(target.session) || fresh();
    s.session = target.session; s.destination = target; sessions.set(target.session, s);
    previous.open = wasOpen;
    s.layout = placement; s.open ||= wasOpen;
    if (s.target) s.target.title = target.title;
    changed();
    await release;
    if (selection === revision) { releasing = false; changed(); }
    if (selection === revision && s.open && !s.target && !s.needsConfirmation) await bind(target);
  }
  async function ensureTarget(target) {
    await select(target);
    if (!target || s.session !== target.session) return false;
    if (s.needsConfirmation) return false;
    if (s.target) return true;
    return !s.needsConfirmation && bind(target);
  }

  const controller = {
    select, ensureTarget, bind, start, stop, close, clear, deliver,
    configure(value) {
      preferences = normalizeVoicePreferences(value);
      sessions.forEach(reconcileLanguage); changed();
    },
    retry() { if (s.needsConfirmation) return deliver(s.lastSubmit, { retry: true }); },
    show() { s.open = true; changed(); },
    layout(value) { if (VOICE_LAYOUTS.includes(value)) { placement = value; s.layout = value; changed(); } },
    edit(text) { if (!voiceBusy(s.phase)) { s.draft = text; changed(); } },
    language(value) { if (!voiceBusy(s.phase) && preferences.languages.includes(value)) { s.language = value; changed(); } },
  };
  Object.defineProperty(controller, 'state', { get: currentState });
  return controller;
}
