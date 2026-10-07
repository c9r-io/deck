// A badge rule's approval as the drawer shows it (R1): what the rule holds
// now comes before anything computed earlier. The production save handler,
// the painted rule list, the editor and the settings writer run here over
// the test document; only the native side and the moment a hash completes
// are scripted.
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';
fakeDocument.querySelectorAll = () => [];
FakeElement.prototype.querySelector = function querySelector(selector) {
  if (selector === '.ar-pause' && !String(this.innerHTML || '').includes('ar-pause')) return null;
  this.parts ||= new Map();
  if (!this.parts.has(selector)) this.parts.set(selector, new FakeElement());
  return this.parts.get(selector);
};
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, dispatchEvent() {} };
const { initAutomation, openAutomations } = await import('../js/automation.js');
const { provider } = await import('../js/board.js');
const { ctx, emit, listeners, state, store } = await import('../js/state.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const { loadSettings } = await import('../js/settings.js');
const { approveRule } = await import('../js/automation-model.js');
const { t } = await import('../js/i18n.js');
const get = id => fakeDocument.getElementById(id);
Object.defineProperty(get('auto-list'), 'innerHTML', { get: () => '', set(value) { if (!value) this.children = []; } });
for (const [id, values] of [['auto-trigger', ['channel', 'clock', 'slack']], ['auto-finish', ['keep', 'close']]]) {
  const buttons = values.map(value => { const button = new FakeElement('button'); button.dataset.v = value; return button; });
  get(id).querySelectorAll = () => buttons;
  get(id).querySelector = () => buttons.find(button => button['aria-pressed'] === 'true');
}
const deferred = () => { let settle; const promise = new Promise((resolve, reject) => { settle = { resolve, reject }; }); return { promise, ...settle }; };
const turn = async (times = 4) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
/* hashes complete when the test lets them: `hold` keeps every hash asked
   for from now on waiting; `hashing` counts the ones not done yet */
let hold = null; let hashing = 0;
const digest = crypto.subtle.digest.bind(crypto.subtle);
crypto.subtle.digest = async (...args) => {
  hashing += 1;
  try { if (hold) await hold.promise; return await digest(...args); } finally { hashing -= 1; }
};
const hashed = async () => { do { await turn(); } while (hashing); await turn(); };
const failures = [];
process.on('unhandledRejection', error => failures.push(error));

const FIXED = { name: 'fixed', steps: ['Run the checks', 'Later'] };
const MESSAGE = { name: 'message', steps: ['Inspect {{msg.text}}'] };
const badge = (id, over = {}) => ({ id, source: 'slack', badge: id, name: '', projectId: 'P1', columnId: 'C1', template: 'fixed',
  cmd: 'claude', dir: '/tmp', enabled: true, finish: 'keep', ...over });
const CLOCK = { id: 'rb', source: 'clock', badge: 'rb', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'fixed', dir: '/tmp',
  name: 'Morning', enabled: true, schedule: { unit: 'day', days: [], minute: 540 }, finish: 'keep', since: 0 };
const ON = t('automation.autoSend.on'); const OFF = t('automation.autoSend.off');

/* The file holds `rules`; the launch loads it and the drawer is open with
   every approval computed. `gates` hold back or refuse `save_settings`. */
async function scene(rules, { gates = [], templates = [FIXED, MESSAGE] } = {}) {
  hold?.resolve(); hold = null;
  if (get('auto-cancel').onclick) get('auto-cancel').onclick();
  for (const element of ids.values()) element.disabled = false;
  get('auto-drawer').hidden = true;
  const project = { id: 'P1', columns: [{ id: 'C1' }], templates: structuredClone(templates) };
  store.projects = [project]; store.cards = []; state.projectId = 'P1';
  const disk = { data: serializeSettings(normalizeSettings({ inbound: { rules } })), writes: [] };
  const saves = [];
  gates = [...gates];
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
    if (command === 'inbound_runs') return [];
    if (command === 'load_settings') return { data: disk.data, source: 'main', warning: null };
    if (command !== 'save_settings') return null;
    saves.push(structuredClone(args));
    const gate = gates.shift();
    if (gate) await gate.promise;
    disk.data = JSON.stringify(JSON.parse(args.data));
    disk.writes.push(disk.data);
    return disk.data;
  } } };
  await loadSettings();
  listeners.clear();                                   // only the drawer's subscribers
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  await openAutomations();
  await hashed();
  failures.length = 0;
  return { saves, disk, project, toasts: get('toasts').children.length };
}
const row = id => get('auto-list').children[ctx.settings.inbound.rules.findIndex(rule => rule.id === id)];
/* the approval line of a painted rule row */
const shown = id => {
  const cells = row(id).querySelector('.ar-kv').children;
  return cells[cells.findIndex(cell => cell.textContent === t('automation.kv.autoSend')) + 1].textContent;
};
const edit = id => row(id).querySelector('.ar-edit').onclick();
const boxes = () => [get('auto-send').checked, get('auto-send-external').checked];
const stored = id => ctx.settings.inbound.rules.find(rule => rule.id === id);
const onDisk = (disk, id) => JSON.parse(disk.data).inbound.rules.find(rule => rule.id === id);
const said = (made, key) => get('toasts').children.slice(made.toasts).filter(toast => toast.textContent === t(key)).length;
const revoke = id => { edit(id); get('auto-send').checked = false; get('auto-send').fire('change'); return get('auto-save').onclick(); };

test('taking a badge rule\'s approval away is one save: the list, the editor and the file agree', async () => {
  const made = await scene([await approveRule(badge('rs'), FIXED), CLOCK]);
  assert.equal(shown('rs'), ON, 'the list shows the approval that is in the file');
  edit('rs');
  assert.deepEqual(boxes(), [true, false]);
  get('auto-send').checked = false; get('auto-send').fire('change');
  await get('auto-save').onclick();                    // rejects if anything in the save or the repaint throws
  await hashed();
  assert.equal(made.saves.length, 1, 'one save');
  assert.equal(JSON.parse(made.saves[0].data).inbound.rules.find(rule => rule.id === 'rs').autoSend, undefined);
  assert.equal(onDisk(made.disk, 'rs').autoSend, undefined);
  assert.equal(stored('rs').autoSend, undefined);
  assert.equal(shown('rs'), OFF, 'the list says it is off');
  assert.equal(get('auto-editor').hidden, true, 'its editor closed');
  assert.equal(said(made, 'automation.saved'), 1, 'saved is said once');
  assert.deepEqual(failures, []);
  edit('rs');
  assert.deepEqual(boxes(), [false, false], 'and it opens unapproved');
});

test('an approval computed earlier is not shown or ticked for a rule that no longer has one', async () => {
  /* ra is listed first: while its hash waits, nothing newer is computed for rs */
  const made = await scene([await approveRule(badge('ra'), FIXED), await approveRule(badge('rs'), FIXED), CLOCK]);
  assert.equal(shown('rs'), ON);
  hold = deferred();
  await revoke('rs');
  assert.ok(hashing > 0, 'the approvals are still being computed');
  assert.equal(made.saves.length, 1);
  assert.equal(shown('rs'), OFF, 'off at once, without waiting for a hash');
  assert.equal(get('auto-editor').hidden, true);
  edit('rs');
  assert.deepEqual(boxes(), [false, false], 'the editor does not tick it either');
  get('auto-cancel').onclick();
  hold.resolve(); hold = null;
  await hashed();
  assert.equal(shown('rs'), OFF);
  assert.equal(shown('ra'), ON, 'the rule that is approved is shown so');
  assert.deepEqual(failures, []);
});

test('an approval result that arrives late, for the rule as it was, changes nothing', async () => {
  const made = await scene([await approveRule(badge('rs'), FIXED), CLOCK]);
  hold = deferred();
  emit('projects');                                    // a repaint starts computing rs as it is now: approved
  await turn();
  assert.ok(hashing > 0, 'the old computation is held');
  await revoke('rs');                                  // lands; its own refresh has nothing to hash for rs
  await turn();
  assert.equal(shown('rs'), OFF);
  const late = hold; hold = null;
  late.resolve();                                      // the old answer, valid for the old rule, arrives now
  await hashed();
  assert.equal(shown('rs'), OFF, 'the list is not turned back on');
  edit('rs');
  assert.deepEqual(boxes(), [false, false], 'nor the editor');
  get('auto-cancel').onclick();
  emit('projects');                                    // whatever reads the cache next
  assert.equal(shown('rs'), OFF);
  await hashed();
  assert.equal(shown('rs'), OFF);
  assert.equal(made.saves.length, 1);
  assert.deepEqual(failures, []);
});

test('a result that arrives late, for the rule before it was approved, does not hide the approval', async () => {
  /* ra is listed first: the old computation waits on its hash before it reaches rs */
  const made = await scene([await approveRule(badge('ra'), FIXED), badge('rs'), CLOCK]);
  assert.equal(shown('rs'), OFF);
  const late = hold = deferred();
  emit('projects');                                    // computes rs as it is now: not approved
  await turn();
  assert.ok(hashing > 0, 'the old computation is held');
  hold = null;                                         // what is asked for from here on is computed at once
  edit('rs');
  get('auto-send').checked = true; get('auto-send').fire('change');
  await get('auto-save').onclick();
  for (let i = 0; i < 20 && shown('rs') !== ON; i += 1) await turn();
  assert.equal(shown('rs'), ON, 'the approval that landed is shown');
  late.resolve();                                      // the old answer arrives now
  await hashed();
  edit('rs');
  assert.deepEqual(boxes(), [true, false], 'the editor still opens approved: saving it would not take the approval away');
  get('auto-cancel').onclick();
  emit('projects');
  assert.equal(shown('rs'), ON, 'and the next paint still shows it');
  await hashed();
  assert.equal(made.saves.length, 1);
  assert.deepEqual(failures, []);
});

test('approved, approved with message content, changed since, and never approved read as before', async () => {
  const external = await approveRule(badge('rx', { template: 'message' }), MESSAGE, { external: true });
  const made = await scene([
    await approveRule(badge('rf'), FIXED), external,
    { ...await approveRule(badge('rr'), FIXED), dir: '/elsewhere' },
    await approveRule(badge('rt', { template: 'gone' }), FIXED), badge('rn'), CLOCK]);
  assert.deepEqual(['rf', 'rx', 'rr', 'rt', 'rn'].map(shown), [ON, t('automation.autoSend.onExternal'),
    t('automation.autoSend.staleRule'), t('automation.autoSend.staleTemplate'), OFF]);
  const ticked = id => { edit(id); const value = boxes(); get('auto-cancel').onclick(); return value; };
  assert.deepEqual(['rf', 'rx', 'rr', 'rt', 'rn'].map(ticked), [[true, false], [true, true], [false, false], [false, false], [false, false]]);
  /* the template changes under an approval: stale after the next repaint */
  made.project.templates[0].steps[0] = 'Run other checks';
  emit('projects');
  assert.notEqual(shown('rf'), ON, 'not called approved before it was checked against the new steps');
  await hashed();
  assert.equal(shown('rf'), t('automation.autoSend.staleTemplate'));
  assert.deepEqual(ticked('rf'), [false, false]);
  assert.equal(made.saves.length, 0, 'showing saves nothing');
  assert.deepEqual(failures, []);
});

test('a refused save of the same change leaves the approval that is in the file, and says the save failed', async () => {
  const refused = deferred();
  const made = await scene([await approveRule(badge('rs'), FIXED), CLOCK], { gates: [refused] });
  const approval = structuredClone(stored('rs').autoSend);
  const saving = revoke('rs');
  await turn();
  refused.reject('io');
  await saving; await hashed();
  assert.equal(made.disk.writes.length, 0);
  assert.deepEqual(stored('rs').autoSend, approval, 'the approval is still the rule\'s');
  assert.deepEqual(onDisk(made.disk, 'rs').autoSend, approval);
  assert.equal(shown('rs'), ON, 'and still shown');
  assert.equal(get('auto-editor').hidden, false, 'the editor stays for another try');
  assert.deepEqual(boxes(), [false, false], 'with the choice as the user left it');
  assert.equal(said(made, 'automation.saved'), 0);
  assert.equal(said(made, 'error.inboundSave'), 1);
  assert.deepEqual(failures, []);
});

test('after taking one approval away, saving another rule and approving again all work once each', async () => {
  const made = await scene([await approveRule(badge('rs'), FIXED), CLOCK]);
  await revoke('rs');
  edit('rb');
  get('auto-dir').value = '/var';
  await get('auto-save').onclick();
  await hashed();
  assert.equal(JSON.parse(made.saves[1].data).inbound.rules.find(rule => rule.id === 'rs').autoSend, undefined, 'saving rb brings nothing back');
  assert.equal(shown('rs'), OFF);
  assert.equal(stored('rb').dir, '/var');
  edit('rs');
  assert.deepEqual(boxes(), [false, false]);
  get('auto-send').checked = true; get('auto-send').fire('change');
  await get('auto-save').onclick();
  await hashed();
  assert.equal(made.saves.length, 3, 'three saves for three presses');
  assert.ok(onDisk(made.disk, 'rs').autoSend.digest, 'the new approval is in the file');
  assert.equal(shown('rs'), ON);
  assert.equal(said(made, 'automation.saved'), 3);
  assert.equal(onDisk(made.disk, 'rb').dir, '/var');
  edit('rs');
  assert.deepEqual(boxes(), [true, false]);
  get('auto-cancel').onclick();
  assert.deepEqual(failures, []);
});
