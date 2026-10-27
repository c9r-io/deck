// Six UI languages. `system` takes the first macOS preferred language Deck has
// (Traditional Chinese has no dictionary and is skipped); none → English.
import { en } from './i18n/en.js';
import { zhHans } from './i18n/zh-Hans.js';
import { ja } from './i18n/ja.js';
import { ko } from './i18n/ko.js';
import { es } from './i18n/es.js';
import { de } from './i18n/de.js';

export const dictionaries = Object.freeze({ en, 'zh-Hans': zhHans, ja, ko, es, de });
export const LOCALES = Object.freeze(['en', 'zh-Hans', 'ja', 'ko', 'es', 'de']);
export const LOCALE_CHOICES = Object.freeze(['system', ...LOCALES]);
let preference = 'system';
let locale = 'en';
const subscribers = new Set();

export function resolveSystemLocale(languages = (globalThis.navigator?.languages || [globalThis.navigator?.language])) {
  for (const tag of (languages || []).filter(Boolean).map(String)) {
    if (/^zh(?:-Hans(?:-|$)|-(?:CN|SG)(?:-|$)|$)/i.test(tag)) return 'zh-Hans';
    const base = tag.split('-')[0].toLowerCase();
    if (base !== 'zh' && LOCALES.includes(base)) return base;
  }
  return 'en';
}

export function resolveLocale(value, languages) {
  return LOCALES.includes(value) ? value : resolveSystemLocale(languages);
}

export function getLocale() { return locale; }
export function getLocalePreference() { return preference; }

export function t(key, params = {}) {
  const template = dictionaries[locale]?.[key] ?? en[key];
  if (template == null) {
    if (globalThis.__DECK_TEST__ || globalThis.location?.hostname === 'localhost') console.warn(`[i18n] missing key: ${key}`);
    return '';
  }
  return template.replace(/\{([A-Za-z][A-Za-z0-9_]*)\}/g, (match, name) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match);
}

export function applyTranslations(root = document) {
  if (!root || typeof root.querySelectorAll !== 'function') return;
  root.querySelectorAll('[data-i18n]').forEach(el => { el.textContent = t(el.dataset.i18n); });
  root.querySelectorAll('[data-i18n-title]').forEach(el => { el.title = t(el.dataset.i18nTitle); });
  root.querySelectorAll('[data-i18n-placeholder]').forEach(el => { el.placeholder = t(el.dataset.i18nPlaceholder); });
  root.querySelectorAll('[data-i18n-aria-label]').forEach(el => { el.setAttribute('aria-label', t(el.dataset.i18nAriaLabel)); });
  if (globalThis.document?.documentElement) globalThis.document.documentElement.lang = locale;
}

export function setLocale(value = 'system', languages) {
  preference = LOCALE_CHOICES.includes(value) ? value : 'system';
  const next = resolveLocale(preference, languages);
  const changed = next !== locale;
  locale = next;
  if (typeof globalThis.document?.querySelectorAll === 'function') applyTranslations();
  if (changed) subscribers.forEach(fn => fn(locale));
  return locale;
}

export function onLocaleChange(fn) { subscribers.add(fn); return () => subscribers.delete(fn); }

export function formatNumber(value, options) {
  return new Intl.NumberFormat(locale, options).format(value);
}

export function formatDateTime(value, options = { hour: '2-digit', minute: '2-digit' }) {
  return new Intl.DateTimeFormat(locale, options).format(value);
}

export function formatInterval(seconds) {
  if (seconds < 60) return t('queue.intervalSeconds', { count: formatNumber(seconds) });
  const hours = seconds / 3600;
  return seconds % 3600 === 0
    ? t('queue.intervalHours', { count: formatNumber(hours) })
    : t('queue.intervalMinutes', { count: formatNumber(seconds / 60) });
}

export function dictionaryAudit() {
  const canonical = Object.keys(en).sort();
  const result = {};
  for (const [name, dict] of Object.entries(dictionaries)) {
    const keys = Object.keys(dict).sort();
    result[name] = {
      missing: canonical.filter(k => !Object.hasOwn(dict, k)),
      orphan: keys.filter(k => !Object.hasOwn(en, k)),
    };
  }
  return result;
}

export function translateNotice(notice) {
  if (!notice || typeof notice !== 'object' || typeof notice.code !== 'string') {
    return t('notice.storage.recovered');
  }
  return t(`notice.${notice.code}`) || t('notice.storage.recovered');
}
