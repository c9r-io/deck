// Pure Connector desktop shapes: task presets use the same codex/claude
// rule as Slack channels; this module also owns deterministic journal IDs and
// pairing detection.
//
// A preset may carry `firstSend: true`: the user's acceptance, for this
// preset, that the first step of a phone task goes to the freshly started
// agent without waiting for readiness (scheduler/first_send.rs). It is set
// in the project defaults dialog on this Mac only, stored as absent when
// off, and frozen into the run (`connectorRun.firstSend`) as a claim the
// native admission checks against the current Board. The phone never sends
// or sees it.
import { channelAgentCommand } from './channel-model.js';
import { LOCAL_ID_RE } from './pure.js';
export const PRESET_MAX = 50;

const utf8 = value => new TextEncoder().encode(String(value || '')).byteLength;
export function normalizeTaskPreset(raw, columns = []) {
  if (!raw || typeof raw !== 'object') return null;
  const preset = {
    id: String(raw.id || ''), name: String(raw.name || '').trim(),
    columnId: String(raw.columnId || ''), title: String(raw.title || '').trim(),
    dir: String(raw.dir || '').trim(), cmd: String(raw.cmd || '').trim(),
    steps: (Array.isArray(raw.steps) ? raw.steps : []).map(String).map(value => value.trim()).filter(Boolean),
    ...(raw.firstSend === true ? { firstSend: true } : {}),
  };
  if (!LOCAL_ID_RE.test(preset.id) || !preset.name || [...preset.name].length > 120
    || !preset.title || [...preset.title].length > 120 || !preset.dir || utf8(preset.dir) > 1024
    || /[\r\n\0]/.test(preset.dir) || !channelAgentCommand(preset.cmd)
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

/* the frozen run a phone task card carries until its rows are queued: one
   step per preset step under its deterministic operation id, the first one
   timed, and the preset's first-send choice when there is a first step */
export function connectorRunPlan(handle, preset, operationIds, at) {
  const initialSteps = preset.steps.map((text, index) => ({ operationId: operationIds[index], text,
    mode: index ? 'chain' : 'at', at: index ? null : at, tpl: preset.id,
    tplIdx: index + 1, tplTotal: preset.steps.length }));
  return { handle, presetId: preset.id, initialSteps, initialQueued: initialSteps.length === 0,
    ...(preset.firstSend === true && initialSteps.length ? { firstSend: true } : {}) };
}

/* the head row's first-send claim for a frozen phone task run: the preset,
   its project and the command handle; nothing when the run did not freeze
   the choice */
export const connectorFirstSendClaim = card => (card?.connectorRun?.firstSend === true
  ? { firstSend: { rule: card.connectorRun.presetId, event: card.connectorRun.handle, presetProject: card.projectId } }
  : {});

export const unfinishedConnectorPlans = cards => (cards || [])
  .filter(card => card.connectorRun && !card.connectorRun.initialQueued);
