// Pure Connector desktop shapes: task presets and deterministic journal IDs.
const LOCAL_ID = /^[A-Za-z0-9_-]{1,128}$/;
export const PRESET_MAX = 50;

const utf8 = value => new TextEncoder().encode(String(value || '')).byteLength;
export function normalizeTaskPreset(raw, columns = []) {
  if (!raw || typeof raw !== 'object') return null;
  const preset = {
    id: String(raw.id || ''), name: String(raw.name || '').trim(),
    columnId: String(raw.columnId || ''), title: String(raw.title || '').trim(),
    dir: String(raw.dir || '').trim(), cmd: String(raw.cmd || '').trim(),
    steps: (Array.isArray(raw.steps) ? raw.steps : []).map(String).map(value => value.trim()).filter(Boolean),
  };
  const agent = preset.cmd.split(/\s+/, 1)[0].split('/').at(-1);
  if (!LOCAL_ID.test(preset.id) || !preset.name || [...preset.name].length > 120
    || !preset.title || [...preset.title].length > 120 || !preset.dir || utf8(preset.dir) > 1024
    || /[\r\n\0]/.test(preset.dir) || !['codex', 'claude'].includes(agent)
    || utf8(preset.cmd) > 200 || /[\r\n\0]/.test(preset.cmd)
    || !columns.some(column => column.id === preset.columnId) || preset.steps.length > 20
    || preset.steps.some(step => utf8(step) > 2000)) return null;
  return preset;
}

export function normalizeTaskPresets(values, columns = []) {
  const out = []; const seen = new Set();
  for (const raw of Array.isArray(values) ? values : []) {
    const preset = normalizeTaskPreset(raw, columns);
    if (!preset || seen.has(preset.id)) continue;
    seen.add(preset.id); out.push(preset);
    if (out.length === PRESET_MAX) break;
  }
  return out;
}

export async function connectorId(prefix, handle, suffix = '') {
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${handle}/${suffix}`));
  return prefix + [...new Uint8Array(hash)].slice(0, 16).map(byte => byte.toString(16).padStart(2, '0')).join('');
}

export async function connectorBufferOperationId(handle, entryId) {
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`connector-buffer:${handle}:${entryId}`));
  return 'B' + [...new Uint8Array(hash)].slice(0, 16).map(byte => byte.toString(16).padStart(2, '0')).join('');
}

export const unfinishedConnectorPlans = cards => (cards || [])
  .filter(card => card.connectorRun && !card.connectorRun.initialQueued);
