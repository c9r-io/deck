// One scene of settings-first-save-dom.test.mjs, in a process of its own:
// `node first-save-scenes.mjs <scene>`. Nothing here was saved, refreshed or
// loaded before the scene's own production `loadSettings()`, so the settings
// writer knows only what that load told it. Exit 0 when the scene holds.
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './dom-fixture.mjs';
fakeDocument.querySelectorAll = () => [];
FakeElement.prototype.querySelector = function querySelector(selector) {
  if (selector === '.ar-pause' && !String(this.innerHTML || '').includes('ar-pause')) return null;
  this.parts ||= new Map();
  if (!this.parts.has(selector)) this.parts.set(selector, new FakeElement());
  return this.parts.get(selector);
};
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, dispatchEvent() {} };
const { initAutomation, openAutomations } = await import('../../js/automation.js');
const { provider } = await import('../../js/board.js');
const { ctx, state, store } = await import('../../js/state.js');
const { normalizeSettings, serializeSettings } = await import('../../js/settings-model.js');
const { loadSettings, setFontScale, setShortcut } = await import('../../js/settings.js');
const get = id => fakeDocument.getElementById(id);
Object.defineProperty(get('auto-list'), 'innerHTML', { get: () => '', set(value) { if (!value) this.children = []; } });
for (const [id, values] of [['auto-trigger', ['channel', 'clock', 'slack']], ['auto-finish', ['keep', 'close']]]) {
  const buttons = values.map(value => { const button = new FakeElement('button'); button.dataset.v = value; return button; });
  get(id).querySelectorAll = () => buttons;
  get(id).querySelector = () => buttons.find(button => button['aria-pressed'] === 'true');
}

const project = { id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}', 'Later'] }] };
const GRANT = { id: 'native-0', digest: 'native-digest', stepHash: 'a'.repeat(64) };
const FILE = normalizeSettings({ inbound: { channelConnection: { enabled: true },
  channelRules: [{ id: 'ra', enabled: true, connectionId: 'default', channelIds: ['C1'], senderUserIds: ['U1'], senderBotIds: [],
    match: { kind: 'contains', value: 'incident' }, includeThreads: true, projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude',
    template: 'triage', idleMinutes: 30, firstSend: true, firstSendGrant: GRANT }],
  rules: [{ id: 'rb', source: 'clock', badge: 'rb', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage', dir: '/tmp',
    name: 'Morning', enabled: true, schedule: { unit: 'day', days: [], minute: 540 }, finish: 'keep', since: 0 },
  { id: 'rs', source: 'slack', badge: 'deck', name: '', projectId: 'P1', columnId: 'C1', template: 'triage', cmd: 'claude',
    dir: '/tmp', enabled: true, finish: 'keep' }] } });
const deferred = () => { let settle; const promise = new Promise((resolve, reject) => { settle = { resolve, reject }; }); return { promise, ...settle }; };
const turn = async (times = 4) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
let hashing = 0;
const digest = crypto.subtle.digest.bind(crypto.subtle);
crypto.subtle.digest = (...args) => { hashing += 1; return digest(...args).finally(() => { hashing -= 1; }); };

/* The native side. `load` is what `load_settings` does: 'main' answers with
   the file, 'backup' with the file as native hands a backup over (no channel
   choice, no grant), 'none' with a first run, 'unreadable' fails. A
   `save_settings` call is three moments: sent (recorded in `saves`), the file
   replaced (`disk.writes`), answered. A gate's `promise` holds the call back
   before the file is touched (rejecting it is a refusal that wrote nothing);
   its `answer` holds back only the reply. */
const disk = { data: serializeSettings(FILE), writes: [] };
const saves = []; const calls = []; let gates = []; let load = 'main';
window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
  calls.push(command);
  if (command === 'inbound_runs') return [];
  if (command === 'load_settings') {
    if (load === 'unreadable') throw 'io';
    if (load === 'none') return { data: '', source: 'none', warning: null };
    if (load === 'main') return { data: disk.data, source: 'main', warning: null };
    const handed = JSON.parse(disk.data);
    for (const rule of handed.inbound.channelRules) { delete rule.firstSend; delete rule.firstSendGrant; }
    return { data: JSON.stringify(handed), source: 'backup', warning: 'recovered' };
  }
  if (command !== 'save_settings') return null;
  saves.push(structuredClone(args));
  const gate = gates.shift();
  if (gate) await gate.promise;
  disk.data = JSON.stringify(JSON.parse(args.data));
  disk.writes.push(disk.data);
  if (gate?.answer) await gate.answer.promise;
  return disk.data;
} } };

/* what production does at launch, and nothing more: load, then the drawer */
async function launch(how = 'main', held = []) {
  load = how; gates = [...held];
  store.projects = [project]; store.cards = []; state.projectId = 'P1';
  await loadSettings();
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  await openAutomations();
  await turn();
  assert.equal(saves.length, 0, 'loading writes nothing');
}
const written = data => JSON.parse(data).inbound;
const rule = (data, id) => written(data).rules.find(value => value.id === id);
const granted = data => written(data).channelRules.find(value => value.id === 'ra');
const BEFORE = JSON.parse(serializeSettings(FILE)).inbound;
const memory = () => JSON.parse(serializeSettings(ctx.settings)).inbound;
/* the editor's change to a clock rule (rb) or a badge rule (rs), or `change` */
const open = id => {
  const all = [...ctx.settings.inbound.rules, ...ctx.settings.inbound.channelRules];
  get('auto-list').children[all.findIndex(value => value.id === id)].querySelector('.ar-edit').onclick();
};
async function editRule(id, change = null) {
  open(id);
  if (change) change();
  else {
    get('auto-dir').value = '/var';
    if (id === 'rb') { get('auto-time').value = '18:30'; get('auto-first-send').checked = true; }
    else { get('auto-send').checked = true; get('auto-send').fire('change'); }
  }
  const done = get('auto-save').onclick();
  do { await turn(); } while (hashing);
  await turn();
  return { done };          // wrapped: an async function would wait for it
}
const wanted = (value, id) => value.dir === '/var' || (id === 'rb'
  ? value.schedule.minute === 1110 || value.firstSendWithoutReadiness === true : !!value.autoSend);
/* A is the first save_settings of the run and is not answered; B (font
   size) and C (a shortcut) are made while it is under way */
async function firstSaveUnderWay(id, held) {
  await launch('main', held);
  const { done: a } = await editRule(id);
  assert.equal(saves.length, 1, 'A is the first save of the run, sent and not answered');
  assert.equal(wanted(rule(saves[0].data, id), id), true, 'and it carries the edit');
  const b = setFontScale(1.2);
  const c = setShortcut('splitRight', 'Meta+KeyK');
  await turn();
  return { a, b, c };
}

const scenes = {
  /* A is refused with nothing written; B is in the file and C not sent yet */
  async failed(id) {
    const held = [deferred(), { answer: deferred() }, null];
    const { a, b, c } = await firstSaveUnderWay(id, held);
    held[0].reject('io');
    await a; await turn();
    assert.equal(saves.length, 2, 'B was sent');
    assert.equal(disk.writes.length, 1, 'and is the only write the file was given');
    assert.equal(wanted(rule(saves[1].data, id), id), false, 'B\'s write does not carry what the refused save wanted');
    assert.deepEqual(written(saves[1].data), BEFORE, 'the automations B writes are the ones in the file');
    assert.deepEqual(written(disk.data), BEFORE, 'the file is right without waiting for C');
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
    held[1].answer.resolve();
    await b; await c; await turn();
    assert.equal(saves.length, 3);
    assert.deepEqual(written(saves[2].data), BEFORE, 'C\'s write does not carry it either: B made nothing of it current');
    assert.deepEqual(written(disk.data), BEFORE);
    assert.deepEqual(saves.flatMap(save => save.channelFirstSendRequests || []), []);
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
    assert.equal(JSON.parse(disk.data).shortcuts.splitRight, 'Meta+KeyK');
    assert.deepEqual(memory(), BEFORE, 'the rules in memory are the ones in the file');
    assert.equal(ctx.settings.fontScale, 1.2);
    assert.equal(ctx.settings.shortcuts.splitRight, 'Meta+KeyK');
    assert.equal(get('auto-editor').hidden, false, 'the refused save\'s editor stays open');
    /* and the next save of another setting still writes the file's automations */
    await setFontScale(1.3);
    assert.deepEqual(written(saves[3].data), BEFORE);
  },
  'clock-first-save-fails': () => scenes.failed('rb'),
  'badge-first-save-fails': () => scenes.failed('rs'),
  /* the last queued save never reaches the file: nothing is left to put it right */
  async 'first-save-fails-then-last-fails'() {
    const held = [deferred(), null, deferred()];
    const { a, b, c } = await firstSaveUnderWay('rb', held);
    held[0].reject('io');
    await a; await b; await turn();
    held[2].reject('io');
    await c; await turn();
    assert.equal(saves.length, 3);
    assert.equal(disk.writes.length, 1, 'B alone reached the file');
    assert.deepEqual(written(disk.data), BEFORE);
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
    assert.deepEqual(memory(), BEFORE);
  },
  async 'first-save-succeeds'() {
    const held = [deferred(), null, null];
    const { a, b, c } = await firstSaveUnderWay('rb', held);
    held[0].resolve();
    await a; await b; await c; await turn();
    assert.equal(disk.writes.length, 3);
    for (const data of disk.writes) {
      const saved = rule(data, 'rb');
      assert.deepEqual([saved.dir, saved.schedule.minute, saved.firstSendWithoutReadiness], ['/var', 1110, true]);
      assert.deepEqual(rule(data, 'rs'), BEFORE.rules.find(value => value.id === 'rs'));
      assert.equal(granted(data).firstSendGrant.id, 'native-0');
    }
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
    assert.equal(JSON.parse(disk.data).shortcuts.splitRight, 'Meta+KeyK');
    assert.equal(memory().rules.find(value => value.id === 'rb').dir, '/var');
    assert.equal(get('auto-editor').hidden, true);
  },
  /* A waits behind a first save that is itself refused, and is given up there */
  async 'given-up-before-its-turn'() {
    const held = [deferred(), null, null];
    await launch('main', held);
    const w0 = setShortcut('toggleSidebar', 'Meta+KeyJ');
    await turn();
    const { done: a } = await editRule('rb');
    assert.equal(saves.length, 1, 'A waits behind W0');
    const b = setFontScale(1.2);
    const c = setShortcut('splitRight', 'Meta+KeyK');
    await turn();
    get('auto-cancel').onclick();
    held[0].reject('io');                              // no save has succeeded in this run
    await w0; await a; await b; await c; await turn();
    assert.equal(saves.length, 3, 'W0, B and C: A sent nothing');
    assert.equal(disk.writes.length, 2);
    for (const data of disk.writes) assert.deepEqual(written(data), BEFORE);
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
    assert.equal(JSON.parse(disk.data).shortcuts.splitRight, 'Meta+KeyK');
    assert.deepEqual(memory(), BEFORE);
  },
  async 'grant-kept'() {
    await launch('main');
    await setFontScale(1.2);
    await setShortcut('splitRight', 'Meta+KeyK');
    assert.equal(saves.length, 2);
    assert.deepEqual(saves.flatMap(save => save.channelFirstSendRequests || []), [], 'nothing is asked for again');
    for (const data of disk.writes) {
      assert.deepEqual(granted(data).firstSendGrant, GRANT);
      assert.equal(granted(data).firstSend, true);
      assert.deepEqual(written(data), BEFORE);
    }
  },
  /* the file was set aside: what is handed over has no channel choice */
  async 'backup'() {
    const main = disk.data;
    await launch('backup');
    assert.equal(disk.data, main, 'loading a backup writes nothing');
    const withdrawn = value => assert.deepEqual([value.firstSend, value.firstSendGrant], [undefined, undefined]);
    withdrawn(memory().channelRules[0]);
    assert.deepEqual(memory().rules, BEFORE.rules, 'the rest of the backup is there to be used');
    await setFontScale(1.2);
    assert.equal(saves.length, 1, 'the owner\'s save goes ahead');
    assert.deepEqual(saves[0].channelFirstSendRequests, undefined);
    withdrawn(granted(saves[0].data));
    assert.deepEqual(written(saves[0].data).rules, BEFORE.rules);
    /* a rule save refused next is in no later write, as after a main load */
    const held = [deferred(), null];
    gates = [...held];
    const { done: a } = await editRule('rb');
    const b = setFontScale(1.3);
    await turn();
    held[0].reject('io');
    await a; await b; await turn();
    assert.equal(saves.length, 3);
    assert.equal(wanted(rule(saves[2].data, 'rb'), 'rb'), false);
    withdrawn(granted(saves[2].data));
    /* loaded again, now from the file the owner's saves wrote: still withdrawn */
    load = 'main';
    await loadSettings();
    withdrawn(memory().channelRules[0]);
    await setFontScale(1.4);
    withdrawn(granted(saves.at(-1).data));
    assert.deepEqual(saves.flatMap(save => save.channelFirstSendRequests || []), []);
  },
  async 'unreadable'() {
    await launch('unreadable');
    assert.equal(calls.filter(command => command === 'save_settings').length, 0, 'an unreadable file is not written over by loading');
    assert.match(get('toasts').children.map(toast => toast.textContent).join('|'), /settings/i, 'and the failure is said');
    assert.deepEqual(memory(), JSON.parse(serializeSettings(normalizeSettings({}))).inbound, 'defaults are in memory only');
  },
  async 'first-run'() {
    disk.data = '';
    await launch('none');
    assert.equal(get('toasts').children.length, 0, 'a first run is not a failure');
    await setFontScale(1.2);
    assert.equal(saves.length, 1, 'the first setting is saved by one save');
    assert.deepEqual(written(saves[0].data), JSON.parse(serializeSettings(normalizeSettings({}))).inbound);
    assert.equal(JSON.parse(disk.data).fontScale, 1.2);
  },
};
/* ---- F3.4: a rule save that landed is not undone by another setting's
   failed save, in memory or in the next rule save's write ---- */
const OTHER = {
  font: { save: () => setFontScale(1.2), own: () => ctx.settings.fontScale, before: 1, wanted: 1.2 },
  shortcut: { save: () => setShortcut('splitRight', 'Meta+KeyK'), own: () => ctx.settings.shortcuts.splitRight,
    before: 'Meta+KeyD', wanted: 'Meta+KeyK' },
};
const said = () => get('toasts').children.length;
/* A (the rule edit) is in the file and not answered when B (another
   setting) is made; A is answered, then B is refused with nothing written */
async function landedThenRefused(edit, other, { refused = true } = {}) {
  const held = [{ answer: deferred() }, refused ? deferred() : null];
  await launch('main', held);
  const { done: a } = await edit();
  assert.deepEqual([saves.length, disk.writes.length], [1, 1], 'A is in the file and not answered');
  const b = OTHER[other].save();
  await turn();
  assert.equal(saves.length, 1, 'B waits behind A');
  held[0].answer.resolve();
  await a; await turn();
  assert.equal(saves.length, 2, 'B was sent after A was answered');
  const landed = memory();
  assert.deepEqual(landed, written(disk.data), 'A is in memory as it is in the file');
  const toasts = said();
  if (refused) held[1].reject('io');
  await b; await turn();
  assert.equal(disk.writes.length, refused ? 1 : 2);
  if (refused) {
    assert.equal(said(), toasts + 1, 'B\'s failure is said once');
    assert.equal(OTHER[other].own(), OTHER[other].before, 'B\'s own change is taken back');
  } else assert.equal(OTHER[other].own(), OTHER[other].wanted);
  return landed;
}
/* D: another rule saved once B has settled; `kept` is asked of every place */
async function thenAnotherRule(id, landed, kept) {
  assert.deepEqual(memory(), landed, 'the automations in memory are still the ones in the file');
  kept(memory(), 'memory after B');
  const sent = saves.length;
  const { done } = await editRule(id, () => { get('auto-dir').value = '/opt'; });
  await done; await turn();
  assert.equal(saves.length, sent + 1, 'D is one save');
  const request = written(saves.at(-1).data);
  kept(request, 'D\'s request');
  kept(written(disk.data), 'the file after D');
  kept(memory(), 'memory after D');
  assert.equal(request.rules.find(value => value.id === id).dir, '/opt', 'D\'s own change is saved');
  assert.equal(written(disk.data).rules.find(value => value.id === id).dir, '/opt');
  assert.deepEqual(saves.flatMap(save => save.channelFirstSendRequests || []), []);
}
const clockKept = (inbound, where) => {
  const saved = inbound.rules.find(value => value.id === 'rb');
  assert.deepEqual([saved.dir, saved.schedule.minute, saved.firstSendWithoutReadiness], ['/var', 1110, true], where);
};
Object.assign(scenes, {
  async landedClock(other, refused = true) {
    const landed = await landedThenRefused(() => editRule('rb'), other, { refused });
    clockKept(landed, 'after A');
    open('rb');
    assert.equal(get('auto-dir').value, '/var', 'the rule opens as it was saved');
    get('auto-cancel').onclick();
    await thenAnotherRule('rs', landed, clockKept);
    assert.deepEqual(granted(disk.data).firstSendGrant, GRANT);
    if (!refused) assert.equal(JSON.parse(disk.data).fontScale, 1.2);
  },
  'landed-clock-then-font-fails': () => scenes.landedClock('font'),
  'landed-clock-then-shortcut-fails': () => scenes.landedClock('shortcut'),
  'landed-clock-then-font-lands': () => scenes.landedClock('font', false),
  /* A approves a badge rule: the approval that landed stays */
  async 'landed-approval-then-font-fails'() {
    const approved = (inbound, where) => {
      const saved = inbound.rules.find(value => value.id === 'rs');
      assert.deepEqual([saved.dir, Object.keys(saved.autoSend || {}).sort()], ['/var', ['classes', 'digest', 'external', 'steps']], where);
    };
    const landed = await landedThenRefused(() => editRule('rs'), 'font');
    approved(landed, 'after A');
    await thenAnotherRule('rb', landed, approved);
  },
  /* A withdraws the channel permission: nothing brings it back */
  async 'withdrawn-channel-permission-then-font-fails'() {
    const withdrawn = (inbound, where) => assert.deepEqual(
      [inbound.channelRules[0].firstSend, inbound.channelRules[0].firstSendGrant], [undefined, undefined], where);
    const landed = await landedThenRefused(() => editRule('ra', () => {
      assert.equal(get('auto-channel-first-send').checked, true);
      get('auto-channel-first-send').checked = false; get('auto-channel-first-send').fire('change');
    }), 'font');
    withdrawn(landed, 'after A');
    await thenAnotherRule('rb', landed, withdrawn);
  },
  /* no rule save at all: B's failure is B's own, and a permission in the file stays */
  async 'font-fails-alone'() {
    const held = [deferred()];
    await launch('main', held);
    const toasts = said();
    const b = setFontScale(1.2);
    await turn();
    held[0].reject('io');
    await b; await turn();
    assert.equal(said(), toasts + 1);
    assert.equal(ctx.settings.fontScale, 1);
    assert.equal(disk.writes.length, 0);
    const kept = (inbound, where) => {
      assert.deepEqual(inbound.channelRules, BEFORE.channelRules, where);
      assert.deepEqual(inbound.rules.find(value => value.id === 'rb'), BEFORE.rules.find(value => value.id === 'rb'), where);
    };
    await thenAnotherRule('rs', BEFORE, kept);
    await setFontScale(1.2);
    assert.equal(JSON.parse(disk.data).fontScale, 1.2, 'the same setting saves on the next try');
  },
});
const scene = scenes[process.argv[2]];
if (!scene) { console.error('unknown scene'); process.exit(2); }
await scene();
