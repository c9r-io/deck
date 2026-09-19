// One microphone, one bound pane, no draft. The committed transcript arrives
// as a growing prefix; every new slice is typed into the bound pane while
// capture continues, cut at the last non-blank so a word separator travels
// with the word that follows it. Line breaks become spaces: voice never
// produces an Enter. Stop finishes finalization and types the remainder;
// leaving, replacing or hiding the pane cancels and drops what was not yet
// committed. A delivery failure ends the recording with a closed code, after
// a short bounded retry for transient refusals; an unconfirmed paste counts as
// typed and is never retransmitted. The volatile tail is only a preview for
// the caption: what is heard but not yet typed. Capture epochs reject late
// native replies and revoke prepared-but-unsent slices. Each delivery keeps
// its original bound target through preparation, IPC and cleanup; a late
// completion may never change the next recording. A setup failure opens its
// System Settings pane once.
import { defaultVoiceLanguage, normalizeVoicePreferences } from './voice-preferences-model.js';

export const voiceBusy = phase => ['binding', 'preparing', 'downloading', 'recording', 'stopping'].includes(phase);
export const voiceRecording = phase => ['preparing', 'downloading', 'recording', 'stopping'].includes(phase);
const ERROR_CODES = new Set(['microphone-denied', 'speech-denied', 'dictation-disabled', 'microphone-unavailable', 'local-unavailable',
  'recognition-failed', 'finalize-timeout', 'audio-overrun', 'text-limit', 'target-unavailable', 'target-changed', 'target-expired',
  'delivery-busy', 'delivery-unknown', 'text-invalid', 'multiline-unsupported', 'target-not-visible']);
const TRANSIENT = new Set(['target-changed', 'delivery-busy']);
const RETRY_LIMIT = 10;
export function voiceError(error) {
  const text = String(error || '');
  return ERROR_CODES.has(text) ? text : 'operation-failed';
}
export function voiceSettingsTarget(error) {
  switch (error) {
    case 'microphone-denied': return 'microphone';
    case 'speech-denied': return 'speech';
    case 'dictation-disabled': return 'dictation';
    default: return null;
  }
}
// The slice of `committed` to type now, given how much was typed already.
// While recording, trailing blanks wait for the word that follows them; at
// the end everything is typed with trailing blanks dropped. `next` is in
// committed's own coordinates so no replacement can shift the bookkeeping.
export function voiceSlice(committed, typed, final) {
  const pending = committed.slice(typed);
  const trailing = pending.search(/\s+$/);
  const cut = final || trailing < 0 ? pending.length : trailing;
  const text = pending.slice(0, cut).replace(/[\r\n\t]+/g, ' ').replace(/\s+$/, '');
  return { text: text.trim() ? text : '', next: typed + (final ? pending.length : cut) };
}

export function createVoiceInput(deps) {
  const { invoke } = deps;
  const schedule = deps.schedule || (fn => setTimeout(fn, 200));
  const unschedule = deps.unschedule || clearTimeout;
  let preferences = normalizeVoicePreferences(deps.preferences);
  const s = { phase: 'idle', session: null, target: null, recordingId: null, startedAt: 0, error: '', notice: '', typed: 0, preview: '' };
  let epoch = 0, timer = null, seen = '', retries = 0;
  let pendingStart = Promise.resolve(), pendingCancel = Promise.resolve();
  const changed = () => deps.changed?.(s);
  const report = (kind, code) => deps.report?.(kind, code);
  const clearPoll = () => { if (timer !== null) unschedule(timer); timer = null; };
  const cancelNative = id => (pendingCancel = invoke('voice_cancel', { id }).catch(() => {}));
  const rest = () => { s.recordingId = null; s.startedAt = 0; s.preview = ''; s.phase = 'idle'; changed(); };
  async function fail(code) {
    s.error = code; rest(); report('error', code);
    const kind = voiceSettingsTarget(code);
    // The toast keeps the manual settings path if opening fails.
    if (kind) await invoke('voice_open_settings', { kind }).catch(() => {});
  }
  async function cancelFailed(revision, code) {
    if (epoch !== revision) return;
    const pending = cancel();
    const cancelled = epoch;
    await pending;
    if (epoch === cancelled) await fail(code);
  }

  // true: typed; 'retry': nothing typed, ask again next poll; false: ended.
  async function type(revision, text, final) {
    const target = s.target;
    try {
      await deps.prepareTarget(target);
      if (epoch !== revision) return false;
      await invoke('voice_deliver', { targetId: target.id, text });
      if (epoch !== revision) return false;
      retries = 0; return true;
    } catch (error) {
      if (epoch !== revision) return false;
      const code = voiceError(error);
      if (code === 'delivery-unknown') { s.notice = code; report('notice', code); return true; }
      if (TRANSIENT.has(code) && !final && retries < RETRY_LIMIT) { retries++; return 'retry'; }
      await cancelFailed(revision, code); return false;
    } finally {
      deps.afterDelivery?.(target);
    }
  }

  async function poll(revision, id) {
    let result;
    try { result = await invoke('voice_snapshot', { id }); } catch (_) { result = null; }
    if (epoch !== revision) return;
    if (!result || result.id !== id || typeof result.text !== 'string' || !result.text.startsWith(seen)) {
      await cancelFailed(revision, result ? 'recognition-failed' : 'operation-failed'); return;
    }
    if (result.status === 'cancelled') { rest(); return; }
    if (result.status === 'downloading' && s.phase !== 'downloading') report('notice', 'downloading');
    // A stop already shown never regresses to a capture phase on a stale snapshot.
    if (voiceRecording(result.status) && s.phase !== 'stopping') s.phase = result.status;
    if (result.status === 'recording' && !s.startedAt) s.startedAt = Date.now();
    const final = result.status === 'ready' || result.status === 'error';
    seen = result.text;
    const slice = voiceSlice(result.text, s.typed, final);
    const outcome = slice.text ? await type(revision, slice.text, final) : true;
    if (outcome === false || epoch !== revision) return;
    if (outcome === true) s.typed = slice.next;
    s.preview = final ? '' : (result.text.slice(s.typed) + (typeof result.preview === 'string' ? result.preview : '')).trim();
    if (final) {
      s.recordingId = null; cancelNative(id);
      if (result.status === 'error') await fail(voiceError(result.code)); else rest();
      return;
    }
    timer = schedule(() => poll(revision, id));
    changed();
  }

  async function startOwned(target, revision) {
    try {
      await Promise.all([pendingStart, pendingCancel]);
      if (epoch !== revision) return;
      const binding = await invoke('voice_bind', { name: target.session });
      if (epoch !== revision) return;
      s.target = { ...target, ...binding }; s.phase = 'preparing'; changed();
      const id = await invoke('voice_start', { targetId: s.target.id, locale: defaultVoiceLanguage(preferences, deps.language) });
      if (epoch !== revision) { await cancelNative(id); return; }
      s.recordingId = id; changed();
      await poll(revision, id);
    } catch (error) {
      if (epoch !== revision) return;
      await fail(voiceError(error));
    }
  }
  function start(target) {
    if (!target || voiceBusy(s.phase)) return Promise.resolve();
    const revision = ++epoch;
    clearPoll();
    Object.assign(s, { session: target.session, target: null, recordingId: null, startedAt: 0, error: '', notice: '', typed: 0, phase: 'binding' });
    seen = ''; retries = 0; changed();
    pendingStart = startOwned(target, revision);
    return pendingStart;
  }

  async function stop() {
    if (!voiceBusy(s.phase) || s.phase === 'stopping') return;
    if (s.recordingId === null) return cancel();
    const revision = epoch;
    s.phase = 'stopping'; changed();
    try { await invoke('voice_stop', { id: s.recordingId }); }
    catch (_) { await cancelFailed(revision, 'operation-failed'); }
  }

  async function cancel() {
    if (!voiceBusy(s.phase)) return;
    ++epoch; clearPoll();
    const id = s.recordingId;
    rest();
    if (id !== null) await cancelNative(id);
  }

  const controller = {
    start, stop, cancel,
    toggle(target) {
      if (s.phase === 'idle') return start(target);
      if (s.phase === 'stopping') return Promise.resolve();
      return stop();
    },
    select(target) {
      if (voiceBusy(s.phase) && target?.session !== s.session) return cancel();
      return Promise.resolve();
    },
    targetExit(session) {
      if (voiceBusy(s.phase) && session === s.session) return cancel();
      return Promise.resolve();
    },
    configure(value) { preferences = normalizeVoicePreferences(value); },
    // The literal-typing path on its own, without capture (smoke coverage).
    async typeText(target, text) {
      const binding = await invoke('voice_bind', { name: target.session });
      const bound = { ...target, ...binding };
      try { await deps.prepareTarget(bound); await invoke('voice_deliver', { targetId: bound.id, text }); }
      finally { deps.afterDelivery?.(bound); }
    },
  };
  Object.defineProperty(controller, 'state', { get: () => s });
  return controller;
}
