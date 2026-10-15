// Voice Settings contribution. Preference changes use the host transaction before
// notifying the recorder; registration never captures audio or downloads assets.
import { normalizeVoicePreferences } from './voice-preferences-model.js';
import { createVoiceSettings } from './voice-settings.js';

export const CONTRIBUTION = Object.freeze({
  id: 'voice',
  feedback: { voice: ['voice', 'settings.voice'] },
  slots: [{ slotId: 'set-extension-voice', html: `          <fieldset class="set-group voice-preferences" id="set-item-voice" data-setting-id="voice" tabindex="-1">
            <legend data-i18n="settings.voice">Voice input</legend>
            <div id="set-voice-languages" class="voice-language-choices" role="group" data-i18n-aria-label="settings.voiceLanguages"></div>
            <div class="set-row">
              <label for="set-voice-default" data-i18n="settings.voiceDefault">Default recognition language</label>
              <select id="set-voice-default" class="col-select"></select>
            </div>
            <div class="set-hint" data-i18n="settings.voiceHint">Choose at least one language. Follow system language picks among the chosen ones. Recognition files download only when a recording needs them.</div>
          </fieldset>
` }],
  settings: [{ id: 'voice', section: 'terminal', order: 0,
    labelKeys: ['settings.voice', 'settings.voiceDefault'],
    keywords: ['speech', 'dictation', 'microphone', 'mic', '听写', '麦克风', '识别'] }],
  create(host) {
    const { $, ctx, commitSettings, toast, t, uev } = host;
    const voiceSettings = createVoiceSettings({
      drain: () => host.drainWrites(),
      save: voice => commitSettings({ key: 'voice', preview: false,
        edit: settings => { settings.voice = structuredClone(voice); }, errorKey: 'settings.voiceSaveFailed' }),
      failed: () => { toast(t('settings.voiceSaveFailed')); uev('settings-save-fail'); },
    });
    const { render: renderVoicePreferences, save: persistVoicePreferences, announce: announceVoicePreferences } = voiceSettings;
    return {
      refresh: renderVoicePreferences, localeChanged: renderVoicePreferences,
      loaded: announceVoicePreferences, isPending: voiceSettings.isPending,
      renderVoicePreferences, persistVoicePreferences,
      mount() {
        $('set-voice-default').onchange = () => persistVoicePreferences({
          ...normalizeVoicePreferences(ctx.settings.voice), defaultLanguage: $('set-voice-default').value,
        });
        $('set-voice-languages').onchange = () => persistVoicePreferences({
          ...normalizeVoicePreferences(ctx.settings.voice),
          languages: [...$('set-voice-languages').querySelectorAll('input:checked')].map(input => input.value),
        });

      },
    };
  },
});
