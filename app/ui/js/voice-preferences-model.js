// Persisted language choices only; no model download or recording starts here.
// Missing settings retain the default Chinese/English/Japanese menu. A system
// default selects among enabled languages; it does not detect spoken language.
export const VOICE_LANGUAGES = Object.freeze(['zh-CN', 'zh-TW', 'en-US', 'ja-JP', 'ko-KR', 'de-DE', 'fr-FR', 'es-ES']);
export const VOICE_LANGUAGE_NAMES = Object.freeze({
  'zh-CN': '中文（简体）', 'zh-TW': '中文（繁體）', 'en-US': 'English', 'ja-JP': '日本語',
  'ko-KR': '한국어', 'de-DE': 'Deutsch', 'fr-FR': 'Français', 'es-ES': 'Español',
});
export const DEFAULT_VOICE_PREFERENCES = Object.freeze({
  languages: Object.freeze(['zh-CN', 'en-US', 'ja-JP']), defaultLanguage: 'system',
});
export function normalizeVoicePreferences(value) {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const selected = Array.isArray(raw.languages) ? raw.languages : DEFAULT_VOICE_PREFERENCES.languages;
  let languages = VOICE_LANGUAGES.filter(code => selected.includes(code));
  if (!languages.length) languages = [...DEFAULT_VOICE_PREFERENCES.languages];
  const desired = raw.defaultLanguage ?? 'system';
  const defaultLanguage = desired === 'system' || languages.includes(desired) ? desired : languages[0];
  return { ...raw, languages, defaultLanguage };
}
export function voiceLanguage(language = '') {
  if (/^zh.*(?:TW|HK|Hant)/i.test(language)) return 'zh-TW';
  return VOICE_LANGUAGES.find(code => code.split('-')[0] === language.split('-')[0]) || 'en-US';
}
export function defaultVoiceLanguage(preferences, systemLanguage = '') {
  if (preferences.defaultLanguage !== 'system') return preferences.defaultLanguage;
  const preferred = voiceLanguage(systemLanguage);
  return preferences.languages.find(code => code === preferred)
    || preferences.languages.find(code => code.split('-')[0] === preferred.split('-')[0])
    || preferences.languages[0];
}
