const THEMES = new Set(['deck-dark', 'light', 'system', 'high-contrast']);
const ACCENTS = new Set(['teal', 'blue', 'purple', 'orange']);
const UPDATE_CHANNELS = new Set(['stable', 'nightly']);

export const FONT_SCALE_MIN = 0.5;
export const FONT_SCALE_MAX = 1.6;
export const FONT_SCALE_STEP = 0.1;

export const SHORTCUT_ACTIONS = Object.freeze([
  Object.freeze({ id: 'newSession', defaultBinding: 'Meta+KeyN', customizable: true }),
  Object.freeze({ id: 'toggleSidebar', defaultBinding: 'Meta+KeyB', customizable: true }),
  Object.freeze({ id: 'splitRight', defaultBinding: 'Meta+KeyD', customizable: true }),
  Object.freeze({ id: 'splitDown', defaultBinding: 'Meta+Shift+KeyD', customizable: true }),
  Object.freeze({ id: 'fontIncrease', defaultBinding: 'Meta+Equal', customizable: false }),
  Object.freeze({ id: 'fontDecrease', defaultBinding: 'Meta+Minus', customizable: false }),
  Object.freeze({ id: 'fontReset', defaultBinding: 'Meta+Digit0', customizable: false }),
]);

export const CUSTOMIZABLE_SHORTCUT_ACTIONS = Object.freeze(
  SHORTCUT_ACTIONS.filter(action => action.customizable),
);

export const DEFAULT_SHORTCUTS = Object.freeze(Object.fromEntries(
  SHORTCUT_ACTIONS.map(action => [action.id, action.defaultBinding]),
));

const MODIFIER_ORDER = Object.freeze(['Meta', 'Control', 'Alt', 'Shift']);
const KEY_CODE = /^(?:Key[A-Z]|Digit[0-9]|F(?:[1-9]|1[0-2])|Equal|Minus|Bracket(?:Left|Right)|Backslash|Semicolon|Quote|Comma|Period|Slash|Backquote|Arrow(?:Up|Down|Left|Right)|Home|End|Page(?:Up|Down)|Space|Enter|Tab|Backspace|Delete)$/;

export function normalizeShortcutBinding(value, fallback = '') {
  if (value === '') return '';
  if (typeof value !== 'string' || value.length > 64) return fallback;
  const parts = value.split('+');
  const code = parts.pop();
  if (!KEY_CODE.test(code || '')) return fallback;
  const seen = new Set();
  for (const modifier of parts) {
    if (!MODIFIER_ORDER.includes(modifier) || seen.has(modifier)) return fallback;
    seen.add(modifier);
  }
  // The runtime intentionally treats Command+= and Command++ as one binding.
  // A separately stored Shift+Equal binding could therefore never match.
  if (code === 'Equal' && seen.has('Shift')) return fallback;
  const canonical = [...MODIFIER_ORDER.filter(modifier => seen.has(modifier)), code].join('+');
  return canonical === value ? canonical : fallback;
}

export function normalizeFontScale(value) {
  const number = Number(value);
  if (!Number.isFinite(number) || number < FONT_SCALE_MIN || number > FONT_SCALE_MAX) return 1;
  return Number((Math.round(number / FONT_SCALE_STEP) * FONT_SCALE_STEP).toFixed(1));
}

export const INBOUND_SOURCES = Object.freeze(['slack', 'clock']);
/* sources with an on/off switch of their own; the clock is on whenever one
   of its rules is enabled */
const INBOUND_TOGGLES = Object.freeze(['slack']);
export const SCHEDULE_UNITS = Object.freeze(['day', 'week', 'month']);
export const FINISH_MODES = Object.freeze(['keep', 'close']);
/* how long after its slot a clock rule may still start a run, in minutes;
   the editor's choices, 1440 = the rest of the day (mirrors inbound.rs) */
export const DEFAULT_GRACE_MIN = 15;
export const MAX_GRACE_MIN = 1440;
export const GRACE_CHOICES = Object.freeze([0, 5, 15, 30, 60, 180, 1440]);
export const normalizeGrace = raw => (Number.isInteger(raw) && raw >= 0 && raw <= MAX_GRACE_MIN ? raw : DEFAULT_GRACE_MIN);
const INBOUND_BADGE = /^[a-z0-9_+-]{1,64}$/;
const INBOUND_ID = /^[A-Za-z0-9_-]{1,128}$/;
const MAX_INBOUND_RULES = 32;

export const DEFAULT_INBOUND = Object.freeze({
  sources: Object.freeze({ slack: Object.freeze({ enabled: false }) }),
  rules: Object.freeze([]),
});

/* A clock rule's schedule, or null when it is not one the backend accepts:
   a minute of the day on every day / listed ISO weekdays / listed days of
   month, days sorted and unique. */
export function normalizeSchedule(raw) {
  if (!raw || typeof raw !== 'object') return null;
  const unit = String(raw.unit ?? '');
  const minute = Number(raw.minute);
  if (!SCHEDULE_UNITS.includes(unit) || !Number.isInteger(minute) || minute < 0 || minute >= 1440) return null;
  if (unit === 'day') return { unit, days: [], minute };
  const limit = unit === 'week' ? 7 : 31;
  const days = [...new Set((Array.isArray(raw.days) ? raw.days : []).map(Number))]
    .filter(d => Number.isInteger(d) && d >= 1 && d <= limit)
    .sort((a, b) => a - b);
  return days.length ? { unit, days, minute } : null;
}

/* Mirrors the backend's structural validation (inbound::validate_settings):
   a rule that would be refused on save is dropped here so the UI never
   shows a rule the poller will not honor. Referential checks against the
   Board happen at dispatch time. A clock rule's badge is its id, so its id
   must also be spelled like a badge (lowercase). */
export function normalizeInbound(value) {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value : {};
  const sources = {};
  for (const name of INBOUND_TOGGLES) {
    const src = raw.sources && typeof raw.sources === 'object' ? raw.sources[name] : null;
    sources[name] = { enabled: !!(src && src.enabled === true) };
  }
  const seenPair = new Set(), seenId = new Set(), rules = [];
  for (const r of Array.isArray(raw.rules) ? raw.rules : []) {
    if (!r || typeof r !== 'object') continue;
    const rule = {
      id: String(r.id ?? ''), source: String(r.source ?? ''), badge: String(r.badge ?? ''),
      projectId: String(r.projectId ?? ''), columnId: String(r.columnId ?? ''),
      cmd: String(r.cmd ?? ''), template: String(r.template ?? ''), dir: String(r.dir ?? ''),
      name: String(r.name ?? ''), enabled: r.enabled !== false,
      finish: FINISH_MODES.includes(r.finish) ? r.finish : 'keep',
      since: Number.isInteger(r.since) && r.since >= 0 ? r.since : 0,
    };
    if (!INBOUND_ID.test(rule.id) || rule.id.length > 64) continue;
    if (rule.source === 'clock') {
      rule.badge = rule.id;
      rule.schedule = normalizeSchedule(r.schedule);
      if (!rule.schedule) continue;
      rule.graceMin = normalizeGrace(r.graceMin);
    }
    if (rule.name.length > 120 || /[\r\n]/.test(rule.name)) continue;
    if (!INBOUND_SOURCES.includes(rule.source) || !INBOUND_BADGE.test(rule.badge)) continue;
    if (!INBOUND_ID.test(rule.projectId) || !INBOUND_ID.test(rule.columnId)) continue;
    if (rule.cmd.length > 200 || /[\r\n]/.test(rule.cmd)) continue;
    if (!rule.template || rule.template.length > 120) continue;
    if (rule.dir.length > 1024 || /[\r\n\0]/.test(rule.dir)) continue;
    const pair = rule.source + '/' + rule.badge;
    if (seenPair.has(pair) || seenId.has(rule.id)) continue;
    seenPair.add(pair); seenId.add(rule.id);
    rules.push(rule);
    if (rules.length >= MAX_INBOUND_RULES) break;
  }
  return { sources, rules };
}

export const DEFAULT_SETTINGS = Object.freeze({
  editor: '', locale: 'system', theme: 'deck-dark', accent: 'teal',
  updateChannel: 'stable', sessionRestore: false, fontScale: 1,
  shortcuts: DEFAULT_SHORTCUTS, inbound: DEFAULT_INBOUND,
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
  merged.fontScale = normalizeFontScale(merged.fontScale);
  const rawShortcuts = raw.shortcuts && typeof raw.shortcuts === 'object' && !Array.isArray(raw.shortcuts)
    ? raw.shortcuts : {};
  const shortcuts = Object.fromEntries(Object.entries(rawShortcuts)
    .filter(([key, value]) => key.length <= 64 && typeof value === 'string' && value.length <= 64));
  for (const action of SHORTCUT_ACTIONS) {
    shortcuts[action.id] = action.customizable
      ? normalizeShortcutBinding(rawShortcuts[action.id], action.defaultBinding)
      : action.defaultBinding;
  }
  merged.shortcuts = shortcuts;
  merged.inbound = normalizeInbound(raw.inbound);
  return merged;
}

export function parseSettings(json) {
  return normalizeSettings(JSON.parse(json));
}

export function serializeSettings(value) {
  return JSON.stringify(normalizeSettings(value), null, 2);
}
