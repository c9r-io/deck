// Typed Local Intelligence translation IPC. The backend is authoritative for
// input bounds, native focus, clipboard baselines and closed errors.
import { inv } from './state.js';
import { writeClipboard } from './terminal-clipboard.js';
export const MAX_TRANSLATION_BYTES = 16 * 1024;
export const MAX_LIVE_TRANSLATION_BYTES = 4 * 1024;
let smoke = null;
// The isolated WKWebView smoke sets this only after main.rs marks its debug
// translation mode. Production translation IPC remains the default path.
export function installTranslationSmokeBackend(backend) {
  if (globalThis.__DECK_SMOKE_TRANSLATION === true) smoke = backend;
}
export const capability = () => smoke ? smoke.capability() : inv('translation_capability');
export const packStatus = () => smoke?.packStatus ? smoke.packStatus() : inv('translation_pack_status');
export const packInstall = () => smoke?.packInstall ? smoke.packInstall() : inv('translation_pack_install');
export const packDelete = () => smoke?.packDelete ? smoke.packDelete() : inv('translation_pack_delete');
export const unload = () => smoke?.unload ? smoke.unload() : inv('translation_unload');
export const translate = (requestId, text, targetLanguage, strategy) =>
  smoke ? smoke.translate(requestId, text, targetLanguage, strategy)
    : inv('translation_translate', { requestId, text, targetLanguage, strategy });
export const cancel = requestId => smoke ? smoke.cancel(requestId) : inv('translation_cancel', { requestId }).catch(() => {});
export const clipboardArm = () => smoke?.arm ? smoke.arm() : inv('translation_clipboard_arm');
export const clipboardDisarm = () => smoke?.disarm ? smoke.disarm() : inv('translation_clipboard_disarm').catch(() => {});
export const clipboardPoll = () => smoke?.poll ? smoke.poll() : inv('translation_clipboard_poll');
export const copyTranslation = text => smoke?.copy ? smoke.copy(text) : writeClipboard(text);
const CODES = Object.freeze(['translation-disabled', 'translation-model-missing', 'translation-model-corrupt',
  'translation-model-download-failed', 'translation-model-delete-failed', 'source-language-unsupported',
  'view-too-large', 'text-empty', 'text-too-large', 'request-cancelled', 'protected-restoration-failed',
  'translation-failed', 'clipboard-unavailable', 'clipboard-not-text', 'clipboard-not-focused']);
export function closedCode(error) {
  const value = String(error || '');
  return CODES.find(code => value.includes(code)) || 'translation-failed';
}
