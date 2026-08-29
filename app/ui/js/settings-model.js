export const DEFAULT_SETTINGS = Object.freeze({ editor: '', debug: false, locale: 'system' });

export function normalizeSettings(value) {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  return { ...DEFAULT_SETTINGS, ...raw };
}

export function parseSettings(json) {
  return normalizeSettings(JSON.parse(json));
}

export function serializeSettings(value) {
  return JSON.stringify(normalizeSettings(value), null, 2);
}
