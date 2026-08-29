const THEMES = new Set(['deck-dark', 'light', 'system', 'high-contrast']);
const ACCENTS = new Set(['teal', 'blue', 'purple', 'orange']);

export const DEFAULT_SETTINGS = Object.freeze({
  editor: '', debug: false, locale: 'system', theme: 'deck-dark', accent: 'teal',
});

export function normalizeSettings(value) {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const merged = { ...DEFAULT_SETTINGS, ...raw };
  if (!THEMES.has(merged.theme)) merged.theme = DEFAULT_SETTINGS.theme;
  if (!ACCENTS.has(merged.accent)) merged.accent = DEFAULT_SETTINGS.accent;
  return merged;
}

export function parseSettings(json) {
  return normalizeSettings(JSON.parse(json));
}

export function serializeSettings(value) {
  return JSON.stringify(normalizeSettings(value), null, 2);
}
