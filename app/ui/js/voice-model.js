// One mounted editor with volatile drafts, languages and bindings per session.
// Switching releases capture and keeps the last displayed partial in its owner.
// Epochs suppress late replies; sends finish against their captured session.
// Clear/insert/send freeze the visible draft and release capture immediately;
// empty drafts are no-ops, and late finalization cannot change the action text.
// Preference changes preserve enabled session choices and defer replacing a
// removed language until its recording/delivery finishes.
// Transient refusals retain bindings. Expired generations rebind only on the
// next explicit action, never by resending. Uncertain delivery requires the
// separate retry action; it never gets reauthorized by switching sessions.
import { defaultVoiceLanguage, normalizeVoicePreferences, VOICE_LANGUAGES, voiceLanguage } from './voice-preferences-model.js';
export { VOICE_LANGUAGES, voiceLanguage };
import { VOICE_LAYOUTS, voiceBusy, voiceRecording, voiceCanAct, voiceError, joinVoiceDraft } from './voice-state-model.js';
export { VOICE_LAYOUTS, voiceBusy, voiceRecording, voiceCanAct, voiceError, joinVoiceDraft };
import { createVoiceRecorder } from './voice-recorder-model.js';
export function createVoiceComposer(deps) {
  const invoke = deps.invoke;
  let preferences = normalizeVoicePreferences(deps.preferences);
  const fresh = () => ({ open: false, layout: 'bottom', phase: 'idle', draft: '', target: null,
    language: defaultVoiceLanguage(preferences, deps.language), error: '', notice: '', recordingId: null, startedAt: 0,
    languages: [...preferences.languages],
    session: null, bindingExpired: false, needsConfirmation: false, deliveryBusy: false });
  let s = fresh();
  const sessions = new Map();
  let releasing = false, resumeOpen = false;
  let placement = 'bottom', selection = 0, deliveryBusy = false;
  let release = Promise.resolve(), pendingBind = Promise.resolve(), pendingStart = Promise.resolve();
  let epoch = 0;
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
  const recorder = createVoiceRecorder({ ...deps, changed });

  function bind(target) {
    if (voiceBusy(s.phase) || deliveryBusy) return Promise.resolve(false);
    const owner = s, revision = epoch;
    pendingBind = releasing ? release.then(() => owner === s && epoch === revision && bindOwned(owner, target)) : bindOwned(owner, target);
    return pendingBind;
  }
  async function bindOwned(owner, target) {
    if (deliveryBusy || voiceBusy(owner.phase) || !target || (owner.session && target.session !== owner.session)) return false;
    owner.session = target.session; sessions.set(owner.session, owner);
    const revision = ++epoch;
    owner.phase = 'binding'; owner.error = ''; owner.notice = ''; changed();
    try {
      const result = await invoke('voice_bind', { name: target.session });
      if (epoch !== revision) return false;
      owner.target = { ...target, ...result }; owner.bindingExpired = false; owner.phase = 'idle'; changed(); return true;
    } catch (error) {
      if (epoch !== revision) return false;
      owner.target = null; owner.phase = 'error'; owner.error = voiceError(error); changed(); return false;
    }
  }

  function start() {
    if (voiceBusy(s.phase) || deliveryBusy || !s.target) return Promise.resolve();
    const owner = s, revision = epoch;
    pendingStart = releasing ? release.then(() => owner === s && epoch === revision && startOwned(owner)) : startOwned(owner);
    return pendingStart;
  }
  function startOwned(owner) {
    if (deliveryBusy || voiceBusy(owner.phase) || !owner.target) return;
    return recorder.start(owner);
  }
  const stop = () => recorder.stop(s);

  async function close({ preserveOpen = false } = {}) {
    const owner = currentState();
    resumeOpen = preserveOpen && owner.open;
    if (owner.phase === 'sending') return;
    ++epoch;
    const cleanup = recorder.close(owner);
    owner.open = false; owner.phase = 'idle'; changed();
    await cleanup;
  }

  async function clear() {
    const owner = s;
    if (!voiceCanAct(owner)) return;
    const cleanup = recorder.interrupt(owner);
    deliveryBusy = true; // One draft action owns microphone cleanup at a time.
    if (owner.bindingExpired) owner.target = null;
    owner.draft = ''; owner.needsConfirmation = false; owner.phase = 'stopping'; owner.error = ''; owner.notice = ''; changed();
    try { await cleanup; owner.phase = 'idle'; }
    catch (_) { owner.phase = 'error'; owner.error = 'operation-failed'; }
    finally { await finishAction(owner); }
  }

  async function deliver(submit, { retry = false } = {}) {
    const owner = currentState();
    if (!voiceCanAct(owner) || !owner.target || (owner.needsConfirmation && !retry)) return;
    const target = owner.target, text = owner.draft;
    const cleanup = recorder.interrupt(owner);
    deliveryBusy = true;
    owner.lastSubmit = submit;
    owner.phase = 'sending'; owner.error = ''; owner.notice = ''; changed();
    let attempted = false;
    try {
      await cleanup;
      await deps.prepareTarget(target);
      attempted = true;
      const result = await invoke('voice_deliver', { targetId: target.id, text, submit });
      if (result === 'submitted' || result === 'inserted') {
        owner.draft = ''; owner.needsConfirmation = false; owner.phase = 'idle'; owner.notice = result;
      } else {
        owner.needsConfirmation = true;
        owner.phase = 'error'; owner.notice = result === 'enter-refused' ? 'enter-refused' : 'ambiguous';
      }
    } catch (error) {
      owner.phase = 'error'; owner.error = attempted && voiceError(error) === 'operation-failed' ? 'delivery-unknown' : voiceError(error);
      if (attempted) owner.needsConfirmation ||= owner.error === 'delivery-unknown';
      if (owner.error === 'target-expired') {
        owner.bindingExpired = true;
        if (!owner.needsConfirmation) owner.target = null;
      }
    } finally {
      deps.afterDelivery?.(target); await finishAction(owner);
    }
  }

  async function finishAction(owner) {
    deliveryBusy = false; changed();
    if (s !== owner && s.open && !s.target && !s.bindingExpired && !s.needsConfirmation) await bind(s.destination);
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
    release = Promise.all([release, closing, pendingBind, pendingStart, recorder.settled()]).then(() => {});
    s = sessions.get(target.session) || fresh();
    s.session = target.session; s.destination = target; sessions.set(target.session, s);
    previous.open = wasOpen;
    s.layout = placement; s.open ||= wasOpen;
    if (s.target) s.target.title = target.title;
    changed();
    await release;
    if (selection === revision) { releasing = false; changed(); }
    if (selection === revision && s.open && !s.target && !s.bindingExpired && !s.needsConfirmation) await bind(target);
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
