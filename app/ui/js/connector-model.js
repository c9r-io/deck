// Pure Connector desktop shapes: task presets use the same bare codex/claude
// rule as Slack channels; this module also owns deterministic journal IDs and
// pairing detection.
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
  if (!LOCAL_ID.test(preset.id) || !preset.name || [...preset.name].length > 120
    || !preset.title || [...preset.title].length > 120 || !preset.dir || utf8(preset.dir) > 1024
    || /[\r\n\0]/.test(preset.dir) || !['codex', 'claude'].includes(preset.cmd)
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

// The device that a pairing added: present and active now, absent before.
// A pairing is shown to the user at once so a device paired by someone who
// saw the QR code cannot appear unnoticed.
export function newlyPairedDevice(before, after) {
  const known = new Set((before || []).map(device => device.id));
  return (after || []).find(device => !device.revoked && !known.has(device.id)) || null;
}

export const unfinishedConnectorPlans = cards => (cards || [])
  .filter(card => card.connectorRun && !card.connectorRun.initialQueued);
