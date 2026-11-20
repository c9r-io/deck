// project-env-model.js — the project defaults dialog's environment
// variables, DOM-free. The backend (`project_env.rs`, contract in its
// header) owns them: values live only in the Keychain, the webview receives
// NAMES only and sends a value only when the user typed one. This file holds
// the frontend copy of the backend's shape check (mirrored through
// `ui/test/fixtures/limits.json`) so the dialog never offers what the save
// refuses, and builds the entries `project_env_set` takes: a row whose value
// field was left empty keeps the stored value of its ORIGINAL name (`value:
// null`); a new or renamed row needs a value typed now.

export const ENV_MAX_VARS = 64;
export const ENV_MAX_NAME_BYTES = 128;
export const ENV_MAX_VALUE_BYTES = 8 * 1024;
export const ENV_MAX_TOTAL_BYTES = 32 * 1024;
export const ENV_NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;
/* names that move an Agent away from the hooks Deck installed: allowed,
   with a warning (its status would stop reaching Deck) */
export const ENV_WARN_NAMES = ['CLAUDE_CONFIG_DIR', 'CODEX_HOME'];

const bytes = text => new TextEncoder().encode(text).length;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;

/* why a name is refused: 'name', 'reserved' or null */
export function envNameProblem(name) {
  if (!ENV_NAME_RE.test(name) || bytes(name) > ENV_MAX_NAME_BYTES) return 'name';
  const upper = name.toUpperCase();
  if (upper.startsWith('DECK_') || upper.startsWith('TMUX')) return 'reserved';
  return null;
}

/* Rows as the dialog holds them: { name, value, stored } where `stored` is
   the name the row was loaded with ('' for a new row) and `value` what the
   user typed ('' = nothing typed). Rows with an empty name and nothing
   typed are dropped. Returns { entries } or { problem, index }. */
export function envEntries(rows) {
  const kept = rows
    .map((row, index) => ({ name: String(row.name || '').trim(), value: String(row.value || ''), stored: row.stored || '', index }))
    .filter(row => row.name || row.value);
  if (kept.length > ENV_MAX_VARS) return { problem: 'count', index: kept[ENV_MAX_VARS].index };
  const seen = new Set();
  let total = 0;
  const entries = [];
  for (const row of kept) {
    const problem = envNameProblem(row.name);
    if (problem) return { problem, index: row.index };
    if (seen.has(row.name)) return { problem: 'duplicate', index: row.index };
    seen.add(row.name);
    if (!row.value) {
      // nothing typed: keep the stored value, which only the row's own,
      // unrenamed name has
      if (row.stored !== row.name) return { problem: 'value', index: row.index };
      entries.push({ name: row.name, value: null });
      total += bytes(row.name) + 2;
      continue;
    }
    if (CONTROL.test(row.value) || bytes(row.value) > ENV_MAX_VALUE_BYTES) return { problem: 'control', index: row.index };
    total += bytes(row.name) + bytes(row.value) + 2;
    entries.push({ name: row.name, value: row.value });
  }
  // kept values count by name only here; the backend counts them in full
  if (total > ENV_MAX_TOTAL_BYTES) return { problem: 'size', index: kept[kept.length - 1].index };
  return { entries };
}

/* whether the rows say anything other than the names loaded */
export function envChanged(rows, loaded) {
  const live = rows.filter(row => String(row.name || '').trim() || row.value);
  if (live.some(row => row.value)) return true;
  return JSON.stringify(live.map(row => String(row.name).trim())) !== JSON.stringify(loaded);
}

/* names in the list that deserve the hooks warning */
export const envWarnings = rows => rows
  .map(row => String(row.name || '').trim())
  .filter(name => ENV_WARN_NAMES.includes(name));
