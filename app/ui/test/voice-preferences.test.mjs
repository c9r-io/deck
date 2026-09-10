import test from 'node:test';
import assert from 'node:assert/strict';
import { defaultVoiceLanguage, normalizeVoicePreferences, DEFAULT_VOICE_PREFERENCES } from '../js/voice-preferences-model.js';
import { parseSettings, serializeSettings } from '../js/settings-model.js';

test('old settings gain three voice languages and round-trip custom choices plus extension fields', () => {
  assert.deepEqual(parseSettings('{}').voice, DEFAULT_VOICE_PREFERENCES);
  const settings = parseSettings(JSON.stringify({ future: 7, voice: { languages: ['ja-JP', 'en-US'], defaultLanguage: 'ja-JP', futureVoice: true } }));
  const saved = JSON.parse(serializeSettings(settings));
  assert.equal(saved.future, 7); assert.equal(saved.voice.futureVoice, true);
  assert.deepEqual(saved.voice.languages, ['en-US', 'ja-JP']); assert.equal(saved.voice.defaultLanguage, 'ja-JP');
});

test('language preferences reject empty/unknown choices through normalization and repair removed defaults', () => {
  for (const value of [null, [], false, { languages: [] }, { languages: ['invalid'] }]) {
    assert.deepEqual(normalizeVoicePreferences(value).languages, DEFAULT_VOICE_PREFERENCES.languages);
  }
  assert.deepEqual(normalizeVoicePreferences({ languages: ['ja-JP', 'ja-JP', 'bad'], defaultLanguage: 'en-US' }),
    { languages: ['ja-JP'], defaultLanguage: 'ja-JP' });
});

test('system default picks only enabled languages and never detects speech', () => {
  const p = normalizeVoicePreferences();
  assert.equal(defaultVoiceLanguage(p, 'ja-JP'), 'ja-JP');
  assert.equal(defaultVoiceLanguage(p, 'zh-Hant-TW'), 'zh-CN');
  assert.equal(defaultVoiceLanguage(p, 'de-DE'), 'zh-CN');
  assert.equal(defaultVoiceLanguage(normalizeVoicePreferences({ languages: ['fr-FR'], defaultLanguage: 'system' }), 'en-US'), 'fr-FR');
  assert.equal(defaultVoiceLanguage(normalizeVoicePreferences({ defaultLanguage: 'ja-JP' }), 'en-US'), 'ja-JP');
});
