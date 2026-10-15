// settings-extensions.js — generic first-party Settings contribution host.
// The generated registry is the only registration path. This host validates
// declarations, mounts each extension once, performs the one shared agent
// status read, and serializes controls which write the same external file.
// Extensions receive narrow UI services; they never import settings.js back.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { EXTENSION_CONTRIBUTIONS } from './extensions-generated.js';
import { agentHooksToastKey } from './notify-model.js';

const controls = Object.freeze(EXTENSION_CONTRIBUTIONS.flatMap(contribution =>
  (contribution.agentControls || []).map(control => Object.freeze({ ...control, extensionId: contribution.id }))));
const byBox = new Map(controls.map(control => [control.boxId, control]));
const sourceAdapters = new Map(EXTENSION_CONTRIBUTIONS.filter(entry => entry.agentSource)
  .map(entry => [entry.agentSource.source, entry.agentSource]));

let services = null;
let mounted = false;
let reading = false;
let generation = 0;
let status = null;
const pendingLocks = new Set();

const details = control => `<div class="set-row"><label for="${control.boxId}" data-i18n="${control.labelKey}">${control.labelKey}</label><input type="checkbox" id="${control.boxId}" aria-describedby="set-${control.item}-read" disabled></div>${control.noteId ? `<div class="set-hint" id="${control.noteId}" hidden></div>` : ''}<details class="set-details"><summary data-i18n="settings.details">Learn more</summary><div class="set-hint" data-i18n="${control.hintKey}">${control.hintKey}</div></details>`;

function agentPanels() {
  const shared = controls.filter(control => control.item === 'agent-status').map(details).join('');
  const separate = controls.filter(control => control.item !== 'agent-status').map(control => `
    <div class="set-group" id="set-item-${control.item}" data-setting-id="${control.item}" tabindex="-1">
      <div class="set-read-feedback"><span id="set-${control.item}-read" role="status" aria-live="polite"></span><button class="btn" id="set-${control.item}-retry" type="button" data-i18n="settings.readRetry" hidden>Retry reading</button></div>
      ${details(control)}
    </div>`).join('');
  return `<div class="set-group" id="set-item-agent-status" data-setting-id="agent-status" tabindex="-1" role="group" aria-labelledby="set-item-agent-status-title">
    <h3 class="set-group-title" id="set-item-agent-status-title" data-i18n="settings.group.agentStatus">Agent status</h3>
    <div class="set-read-feedback"><span id="set-agent-status-read" role="status" aria-live="polite"></span><button class="btn" id="set-agent-status-retry" type="button" data-i18n="settings.readRetry" hidden>Retry reading</button></div>
    ${shared}</div>${separate}`;
}

function mountDeclaredPanels(host) {
  const fragments = [{ slotId: 'set-extension-agents', html: agentPanels() },
    ...EXTENSION_CONTRIBUTIONS.flatMap(contribution => contribution.slots || [])];
  for (const fragment of fragments) {
    const slot = host.$(fragment.slotId);
    if (typeof slot?.insertAdjacentHTML !== 'function') throw new Error(`extension mount unavailable: ${fragment.slotId}`);
    slot.insertAdjacentHTML('beforebegin', fragment.html);
  }
}

export function validateExtensionContributions(contributions = EXTENSION_CONTRIBUTIONS) {
  const ids = new Set(), boxes = new Set();
  for (const contribution of contributions) {
    if (!contribution || typeof contribution.id !== 'string' || ids.has(contribution.id))
      throw new Error(`duplicate or invalid extension contribution: ${contribution?.id}`);
    ids.add(contribution.id);
    for (const control of contribution.agentControls || []) {
      if (!control.boxId || boxes.has(control.boxId)) throw new Error(`duplicate agent control: ${control.boxId}`);
      boxes.add(control.boxId);
    }
  }
  return true;
}

function normalizeStatus(raw) {
  if (!raw || typeof raw !== 'object') return null;
  if (Object.hasOwn(raw, 'agents'))
    return raw.agents && typeof raw.agents === 'object' && !Array.isArray(raw.agents) ? raw.agents : null;
  // Compatibility for old carriers and focused DOM mocks. New native builds
  // always answer the `agents` map.
  return Object.fromEntries([...sourceAdapters].map(([source, adapter]) => [source, adapter.legacy?.(raw)]));
}

const entry = source => status?.[source];
const projection = source => {
  const current = entry(source), adapter = sourceAdapters.get(source);
  return adapter?.project ? adapter.project(current) : {
    signal: current?.signal, lifecycle: current?.lifecycle, usable: true, known: true, note: null,
  };
};
const known = control => projection(control.source).known !== false
  && typeof projection(control.source)[control.capability] === 'boolean';
const lockOf = control => control.lock || `${control.source}:${control.capability}`;
const pending = control => pendingLocks.has(lockOf(control));

function groupUnknown(item) {
  const members = controls.filter(control => control.item === item);
  return reading || members.some(control => !known(control));
}

export function renderAgentContributions() {
  if (!services) return;
  for (const control of controls) {
    const box = services.$(control.boxId);
    const view = projection(control.source);
    const usable = view.usable !== false;
    const isKnown = !reading && known(control);
    if (!pending(control)) box.checked = isKnown ? view[control.capability] : false;
    box.indeterminate = !isKnown;
    box.disabled = pending(control) || !isKnown || !usable;
  }
  const items = new Set(controls.map(control => control.item));
  for (const item of items) {
    const unknown = groupUnknown(item);
    const read = services.$(`set-${item}-read`), retry = services.$(`set-${item}-retry`);
    read.textContent = unknown ? services.t(reading ? 'settings.reading' : 'settings.readFailed') : '';
    retry.hidden = !unknown;
    retry.disabled = reading || pendingLocks.size > 0;
  }
  for (const control of controls.filter(control => control.noteId)) {
    const note = services.$(control.noteId);
    const noteKey = !status ? (reading ? 'settings.reading' : 'settings.readFailed') : projection(control.source).note;
    note.hidden = !noteKey;
    note.textContent = noteKey ? services.t(noteKey) : '';
  }
  const dependency = Object.fromEntries(controls.filter(control => control.notifyDependency)
    .map(control => [control.source, known(control) ? entry(control.source).signal : undefined]));
  services.renderNotifyDependency(dependency);
}

async function readStatus() {
  let timer;
  const inv = services.inv;
  try {
    return await Promise.race([
      inv('agent_hooks_status'),
      new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('hooks-read-timeout')), 10000); }),
    ]);
  } finally { clearTimeout(timer); }
}

export async function refreshAgentContributions() {
  if (!services || pendingLocks.size) return;
  const ownGeneration = ++generation;
  reading = true; status = null;
  renderAgentContributions();
  let answer = null;
  try { answer = await readStatus(); } catch (_) { /* unknown stays visible and retryable */ }
  if (ownGeneration !== generation) return;
  reading = false; status = normalizeStatus(answer);
  renderAgentContributions();
}

export async function persistAgentContribution(boxId) {
  const control = byBox.get(boxId);
  if (!control || reading || pending(control) || !known(control)) return;
  const box = services.$(boxId);
  if (box.disabled || projection(control.source).usable === false) return;
  const previous = entry(control.source)[control.capability];
  const desired = box.checked, ownGeneration = generation;
  if (desired && !(await services.confirmDialog(services.t(control.confirmKey)))) {
    box.checked = false;
    return;
  }
  if (ownGeneration !== generation || reading || pending(control) || !known(control)
    || box.disabled || projection(control.source).usable === false) return;
  const lock = lockOf(control);
  pendingLocks.add(lock); ++generation;
  const feedback = services.beginFeedback(control.feedbackKey,
    [control.item, control.feedbackLabelKey]);
  renderAgentContributions();
  try {
    if (control.write) await control.write(services, desired);
    else { const inv = services.inv; await inv('agent_hooks_set', { agent: control.source, enable: desired }); }
    status = { ...status, [control.source]: { ...entry(control.source), [control.capability]: desired } };
    services.finishFeedback(feedback, 'saved');
    const key = control.toast === 'agent-hooks'
      ? agentHooksToastKey(desired, services.ctx.settings.notifyAway)
      : desired ? control.onKey : control.offKey;
    services.toast(services.t(key));
  } catch (error) {
    status = { ...status, [control.source]: { ...entry(control.source), [control.capability]: previous } };
    const adapter = sourceAdapters.get(control.source);
    const errorKey = adapter?.errorKey
      ? adapter.errorKey(typeof error === 'string' ? error : error?.message) : 'error.agentHooks';
    services.finishFeedback(feedback, 'failed', errorKey);
    services.toast(services.t(errorKey));
    services.uev('settings-save-fail');
  } finally {
    if (sourceAdapters.get(control.source)?.readBackAfterWrite) {
      let answer = null;
      try { answer = normalizeStatus(await readStatus()); } catch (_) { /* this source becomes unknown */ }
      status = { ...status, [control.source]: answer?.[control.source] };
    }
    pendingLocks.delete(lock);
    renderAgentContributions();
  }
}

export function initializeExtensionContributions(host) {
  if (mounted) return;
  validateExtensionContributions();
  services = host;
  mountDeclaredPanels(host);
  for (const control of controls) host.$(control.boxId).onchange = () => persistAgentContribution(control.boxId);
  for (const item of new Set(controls.map(control => control.item)))
    host.$(`set-${item}-retry`).onclick = refreshAgentContributions;
  for (const contribution of EXTENSION_CONTRIBUTIONS) contribution.mount?.(host);
  mounted = true;
}

export const extensionContributions = () => EXTENSION_CONTRIBUTIONS;
