// Closed composer phases and content-free errors, shared by capture and delivery.
export const VOICE_LAYOUTS = ['bottom', 'floating', 'right'];
export const voiceBusy = phase => ['binding', 'preparing', 'downloading', 'recording', 'stopping', 'sending'].includes(phase);
export const voiceRecording = phase => ['preparing', 'downloading', 'recording', 'stopping'].includes(phase);
export const voiceCanAct = s => !!s.draft.trim() && !s.deliveryBusy && !s.switching
  && !['binding', 'sending'].includes(s.phase);
export function joinVoiceDraft(base, transcript) {
  return base && transcript ? `${base}\n${transcript}` : base || transcript;
}
const ERROR_CODES = new Set(['microphone-denied', 'speech-denied', 'dictation-disabled', 'microphone-unavailable', 'local-unavailable',
  'recognition-failed', 'finalize-timeout', 'audio-overrun', 'text-limit', 'target-unavailable', 'target-changed', 'target-expired',
  'delivery-busy', 'delivery-unknown', 'text-invalid', 'multiline-unsupported', 'target-not-visible']);
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
