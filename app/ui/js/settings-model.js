const THEMES = new Set(['deck-dark', 'light', 'system', 'high-contrast']);
const ACCENTS = new Set(['teal', 'blue', 'purple', 'orange']);
const UPDATE_CHANNELS = new Set(['stable', 'nightly']);

export const DEFAULT_SETTINGS = Object.freeze({
  editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
  updateChannel: 'stable', sessionRestore: true,
});

export function normalizeSettings(value) {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const merged = { ...DEFAULT_SETTINGS, ...raw };
  /* `debug` was a user setting before verbose diagnostics moved to the
     --debug-logging launch flag. Retire this known field while continuing to
     round-trip genuinely unknown extension fields. */
  delete merged.debug;
  if (!THEMES.has(merged.theme)) merged.theme = DEFAULT_SETTINGS.theme;
  if (!ACCENTS.has(merged.accent)) merged.accent = DEFAULT_SETTINGS.accent;
  if (!UPDATE_CHANNELS.has(merged.updateChannel)) merged.updateChannel = DEFAULT_SETTINGS.updateChannel;
  if (typeof merged.sessionRestore !== 'boolean') merged.sessionRestore = DEFAULT_SETTINGS.sessionRestore;
  return merged;
}

export function parseSettings(json) {
  return normalizeSettings(JSON.parse(json));
}

export function serializeSettings(value) {
  return JSON.stringify(normalizeSettings(value), null, 2);
}
