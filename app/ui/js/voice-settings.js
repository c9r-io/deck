// Voice preference controls own their render/save lock, while the shared
// settings writer remains in dialogs.js. Commit before announcing changes;
// failed saves leave both the recorder and committed settings untouched.
import { $, ctx } from './state.js';
import { t } from './i18n.js';
import { normalizeSettings } from './settings-model.js';
import { normalizeVoicePreferences, VOICE_LANGUAGES, VOICE_LANGUAGE_NAMES } from './voice-preferences-model.js';

export function createVoiceSettings(deps) {
  let voiceSavePending = false;
  function announceVoicePreferences() {
    if (typeof window.dispatchEvent === 'function' && typeof Event === 'function') {
      window.dispatchEvent(new Event('deck-voice-preferences-changed'));
    }
  }
  function renderVoicePreferences() {
    const preferences = normalizeVoicePreferences(ctx.settings.voice);
    const list = $('set-voice-languages');
    list.replaceChildren();
    for (const code of VOICE_LANGUAGES) {
      const label = document.createElement('label'); label.className = 'voice-language-choice';
      const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.value = code;
      checkbox.checked = preferences.languages.includes(code);
      checkbox.disabled = voiceSavePending || (checkbox.checked && preferences.languages.length === 1);
      const name = document.createElement('span'); name.textContent = VOICE_LANGUAGE_NAMES[code];
      label.appendChild(checkbox); label.appendChild(name); list.appendChild(label);
    }
    const selector = $('set-voice-default');
    selector.replaceChildren();
    for (const code of ['system', ...preferences.languages]) {
      const option = document.createElement('option'); option.value = code;
      option.textContent = code === 'system' ? t('settings.voiceSystem') : VOICE_LANGUAGE_NAMES[code];
      selector.appendChild(option);
    }
    selector.value = preferences.defaultLanguage; selector.disabled = voiceSavePending;
  }
  async function persistVoicePreferences(value) {
    if (voiceSavePending || !Array.isArray(value?.languages) || !value.languages.length) return false;
    voiceSavePending = true;
    // Drain earlier writes before constructing this patch. Lock mutation controls
    // while saving; on failure the committed settings and recorder stay intact.
    const controls = [...$('settings-modal').querySelectorAll('input, select, button')].filter(control => !control.disabled);
    controls.forEach(control => { control.disabled = true; });
    try {
      await deps.drain();
      controls.forEach(control => { control.disabled = true; });
      const candidate = normalizeSettings({ ...ctx.settings, voice: value });
      await deps.save(candidate);
      ctx.settings = { ...ctx.settings, voice: candidate.voice };
      announceVoicePreferences();
      return true;
    } catch (_) {
      deps.failed(); return false;
    } finally {
      voiceSavePending = false;
      controls.forEach(control => { control.disabled = false; });
      renderVoicePreferences();
    }
  }
  return { render: renderVoicePreferences, save: persistVoicePreferences,
    announce: announceVoicePreferences, isPending: () => voiceSavePending };
}
