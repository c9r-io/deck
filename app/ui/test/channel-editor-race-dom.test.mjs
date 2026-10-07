// A save belongs to the edit it was started from (F3.1). The real editor
// handlers, the real rule list buttons and the real settings writer run
// here; only the native side is scripted, with answers the test holds back.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';
fakeDocument.querySelectorAll = () => [];
/* a painted rule row: one stable element per selector, a pause button only
   where the row's markup has one */
FakeElement.prototype.querySelector = function querySelector(selector) {
  if (selector === '.ar-pause' && !String(this.innerHTML || '').includes('ar-pause')) return null;
  this.parts ||= new Map();
  if (!this.parts.has(selector)) this.parts.set(selector, new FakeElement());
  return this.parts.get(selector);
};
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { initAutomation, openAutomations, openEditor } = await import('../js/automation.js');
const { provider } = await import('../js/board.js');
const { ctx, state, store } = await import('../js/state.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const { persistInbound, persistSettings, refreshChannelAuthority } = await import('../js/settings.js');
const get = id => fakeDocument.getElementById(id);
/* the list is repainted from nothing, as in a browser */
Object.defineProperty(get('auto-list'), 'innerHTML', { get: () => '', set(value) { if (!value) this.children = []; } });
for (const [id, values] of [['auto-trigger', ['channel', 'clock', 'slack']], ['auto-finish', ['keep', 'close']]]) {
  const buttons = values.map(value => { const button = new FakeElement('button'); button.dataset.v = value; return button; });
  get(id).querySelectorAll = () => buttons;
  get(id).querySelector = () => buttons.find(button => button['aria-pressed'] === 'true');
}

const project = { id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}', 'Later'] }] };
const channel = (id, over = {}) => ({ id, source: 'channel', enabled: true, connectionId: 'default', channelIds: ['C1'],
  senderUserIds: ['U1'], senderBotIds: [], match: { kind: 'contains', value: 'incident' }, includeThreads: true,
  projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30, ...over });
const OTHERS = {
  channel: channel('rb', { channelIds: ['C9'], match: { kind: 'contains', value: 'deploy' }, idleMinutes: 45 }),
  clock: { id: 'rb', source: 'clock', badge: 'rb', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage', dir: '/tmp',
    name: 'Morning', enabled: true, schedule: { unit: 'day', days: [], minute: 540 }, finish: 'keep', since: 0 },
  slack: { id: 'rb', source: 'slack', badge: 'deck', name: '', projectId: 'P1', columnId: 'C1', template: 'triage', cmd: 'claude',
    dir: '/tmp', enabled: true, finish: 'keep' },
};
const deferred = () => { let settle; const promise = new Promise((resolve, reject) => { settle = { resolve, reject }; }); return { promise, ...settle }; };
const turn = async (times = 4) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
const count = (calls, command) => calls.filter(value => value === command).length;
const inbound = () => structuredClone({ rules: ctx.settings.inbound.rules, channelRules: ctx.settings.inbound.channelRules });
const VERIFIED = { identity: 'verified-identity' };

/* Settings holding channel rule ra and `others`, the drawer open with its
   list painted. `prepare` answers are taken in order; `gates` hold back the
   matching `save_settings` call until the test settles them. */
async function scene(others, { prepare = [], gates = [] } = {}) {
  /* whatever an earlier test left waiting is given up, and its locks with it */
  if (get('auto-cancel').onclick) get('auto-cancel').onclick();
  for (const element of ids.values()) element.disabled = false;
  get('auto-drawer').hidden = true;
  store.projects = [project]; store.cards = []; state.projectId = 'P1';
  const rules = [channel('ra'), ...others];
  ctx.settings = normalizeSettings({ inbound: { channelConnection: { enabled: true },
    channelRules: rules.filter(rule => rule.source === 'channel').map(({ source, ...rule }) => rule),
    rules: rules.filter(rule => rule.source !== 'channel') } });
  const calls = []; const saves = [];
  const onDisk = serializeSettings(ctx.settings);
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
    calls.push(command);
    if (command === 'inbound_runs') return [];
    if (command === 'load_settings') return { data: onDisk };
    if (command === 'slack_channel_prepare') {
      const next = prepare.shift();
      if (!next) throw 'pending';
      return next();
    }
    if (command !== 'save_settings') return null;
    saves.push(structuredClone(args));
    const gate = gates.shift();
    if (gate) await gate.promise;
    const saved = JSON.parse(args.data);
    for (const request of args.channelFirstSendRequests || []) {
      saved.inbound.channelRules.find(rule => rule.id === request.ruleId).firstSendGrant = { id: `native-${saves.length}`,
        digest: 'native-digest', stepHash: createHash('sha256').update('Inspect {{msg.text}}').digest('hex') };
    }
    return JSON.stringify(saved);
  } } };
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  await refreshChannelAuthority();                     // the writer's view of the native rules is this scene's
  await openAutomations();
  await turn();
  calls.length = 0;
  return { calls, saves, before: inbound() };
}
/* the painted row of a rule, by its position in the project's list */
const row = id => {
  const all = [...ctx.settings.inbound.rules, ...ctx.settings.inbound.channelRules];
  return get('auto-list').children[all.findIndex(rule => rule.id === id)];
};
const edit = id => row(id).querySelector('.ar-edit').onclick();
const stored = id => [...ctx.settings.inbound.rules, ...ctx.settings.inbound.channelRules.map(rule => ({ ...rule, source: 'channel' }))]
  .find(rule => rule.id === id);
/* edit ra, turn automatic first steps on and press Save: the save waits */
async function saveWaiting(id = 'ra') {
  edit(id);
  get('auto-channel-first-send').checked = true;
  get('auto-channel-first-send').fire('change');
  const saving = get('auto-save').onclick();
  await turn();
  return { done: saving };          // wrapped: an async function would wait for it
}
/* every toast shown so far */
const said = () => get('toasts').children.map(toast => toast.textContent);
const requestsOf = saves => saves.flatMap(save => save.channelFirstSendRequests || []);
const waitingShown = () => !get('auto-channel-verify').hidden && get('auto-save').disabled && get('auto-channel-ids').disabled;
const editorFree = () => get('auto-channel-verify').hidden && !get('auto-save').disabled && !get('auto-channel-ids').disabled
  && !get('auto-editor').hidden;

for (const kind of ['channel', 'clock', 'slack']) {
  test(`a save waiting for Slack is dropped when another rule (${kind}) is opened: that rule is untouched`, async () => {
    const late = deferred();
    const { calls, saves, before } = await scene([OTHERS[kind]], { prepare: [() => late.promise] });
    const { done: saving } = await saveWaiting();
    assert.ok(waitingShown());
    edit('rb');
    const freeAtOnce = editorFree();
    const releasedAtOnce = count(calls, 'slack_channel_prepare_cancel');
    const shown = { ids: get('auto-channel-ids').value, name: get('auto-name').value, badge: get('auto-badge').value };
    late.resolve(VERIFIED);                            // the old answer arrives now, uncanceled
    await saving; await turn();
    assert.equal(saves.length, 0, 'nothing was saved');
    assert.deepEqual(inbound(), before, 'every rule is as it was');
    assert.ok(stored('rb'), 'the other rule is still there');
    /* the other rule's editor was usable at once, with its own values */
    assert.ok(freeAtOnce, 'the new editor is not held by the old wait');
    assert.equal(releasedAtOnce, 1, 'the old save released its own hold');
    assert.ok(editorFree(), 'the other rule\'s editor stays open and usable');
    assert.deepEqual({ ids: get('auto-channel-ids').value, name: get('auto-name').value, badge: get('auto-badge').value }, shown);
    assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1, 'a late answer releases nothing again');
    /* the editor that is open belongs to rb: saving it changes rb alone */
    get(kind === 'channel' ? 'auto-idle' : 'auto-dir').value = kind === 'channel' ? '50' : '/var';
    await get('auto-save').onclick();
    assert.equal(saves.length, 1);
    assert.deepEqual(requestsOf(saves), []);
    assert.equal(kind === 'channel' ? stored('rb').idleMinutes : stored('rb').dir, kind === 'channel' ? 50 : '/var');
    assert.deepEqual(stored('ra'), { ...before.channelRules.find(rule => rule.id === 'ra'), source: 'channel' });
  });
}

test('a late failure of a dropped save says nothing in the editor that replaced it', async () => {
  const late = deferred();
  const { saves, before } = await scene([OTHERS.channel], { prepare: [() => late.promise] });
  const { done: saving } = await saveWaiting();
  edit('rb');
  late.reject('worker');
  await saving; await turn();
  assert.ok(editorFree());
  assert.equal(get('auto-channel-verify').textContent, '');
  assert.equal(saves.length, 0);
  assert.deepEqual(inbound(), before);
});

test('a save waiting for Slack is dropped when the editor for a new rule is opened', async () => {
  const late = deferred();
  const { calls, saves, before } = await scene([OTHERS.clock], { prepare: [() => late.promise] });
  const { done: saving } = await saveWaiting();
  const toasts = said().length;
  openEditor(null);                                    // what the New session menu's entries call
  assert.ok(editorFree());
  assert.match(said().slice(toasts).join('|'), /^Canceled\. The rule was not saved and no permission was given\.$/,
    'the dropped save is said once, without a question');
  assert.equal(get('auto-channel-ids').value, '');
  late.resolve(VERIFIED);
  await saving; await turn();
  assert.equal(saves.length, 0);
  assert.deepEqual(inbound(), before, 'no rule was added, removed or changed');
  assert.ok(editorFree());
  assert.equal(get('auto-channel-ids').value, '', 'the new editor keeps its own (empty) fields');
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1);
});

for (const [outcome, settle] of [['succeeds', late => late.resolve(VERIFIED)], ['fails', late => late.reject('worker')]]) {
  test(`a canceled save that ${outcome} late leaves the next save's wait alone`, async () => {
    const lateA = deferred(); const lateC = deferred();
    const { calls, saves, before } = await scene([OTHERS.channel], { prepare: [() => lateA.promise, () => lateC.promise] });
    const { done: savingA } = await saveWaiting('ra');
    get('auto-cancel').onclick();                      // A is given up
    assert.equal(get('auto-editor').hidden, true);
    const { done: savingC } = await saveWaiting('rb');           // a new save, waiting on its own answer
    assert.ok(waitingShown());
    const waitingText = get('auto-channel-verify').textContent;
    settle(lateA);                                     // A's answer arrives while C waits
    await savingA; await turn();
    assert.ok(waitingShown(), 'the new save is still held and shown as waiting');
    assert.equal(get('auto-channel-verify').textContent, waitingText, 'its message is not replaced');
    assert.equal(saves.length, 0);
    assert.deepEqual(inbound(), before);
    assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1, 'only A\'s own hold was released, when A was canceled');
    /* the new save can still be canceled: its handle was not cleared */
    const again = get('auto-save').onclick();          // and a second click still starts nothing
    await again;
    assert.equal(count(calls, 'slack_channel_prepare'), 2);
    lateC.resolve(VERIFIED);
    await savingC; await turn();
    assert.equal(saves.length, 1, 'the new save went through once');
    assert.deepEqual(requestsOf(saves), [{ ruleId: 'rb', external: true, identity: 'verified-identity' }]);
    assert.equal(stored('rb').firstSendGrant.id, 'native-1');
    assert.equal(stored('ra').firstSend, undefined, 'the canceled rule got nothing');
    assert.equal(get('auto-editor').hidden, true);
  });
}

test('the next save is still cancelable after an older save\'s answer arrived', async () => {
  const lateA = deferred(); const lateC = deferred();
  const { calls, saves, before } = await scene([OTHERS.channel], { prepare: [() => lateA.promise, () => lateC.promise] });
  const { done: savingA } = await saveWaiting('ra');
  get('auto-cancel').onclick();
  const { done: savingC } = await saveWaiting('rb');
  lateA.resolve(VERIFIED);
  await savingA; await turn();
  get('auto-cancel').onclick();                        // C's own handle is still the live one
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 2);
  lateC.resolve(VERIFIED);
  await savingC; await turn();
  assert.equal(saves.length, 0);
  assert.deepEqual(inbound(), before);
});

test('a save queued behind another settings write is not written once its edit is gone', async () => {
  const held = deferred();
  const { calls, saves, before } = await scene([OTHERS.clock], { prepare: [() => VERIFIED], gates: [held] });
  const unrelated = persistSettings();                 // an unrelated write occupies the writer
  await turn();
  assert.equal(saves.length, 1);
  const { done: saving } = await saveWaiting();                  // the identity answers at once: A is queued behind it
  assert.equal(saves.length, 1, 'A has not been written yet');
  edit('rb');                                          // A's edit is replaced while it waits in the queue
  held.resolve();
  await unrelated; await saving; await turn();
  assert.equal(saves.length, 1, 'the queued save was not written');
  assert.deepEqual(requestsOf(saves), [], 'no permission was requested');
  assert.deepEqual(inbound(), before, 'the settings in memory are the saved ones');
  assert.equal(stored('ra').firstSend, undefined);
  assert.ok(editorFree());
  assert.equal(get('auto-name').value, 'Morning', 'the open editor is still rb\'s');
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1);
});

test('a waiting save that goes ahead keeps what was saved meanwhile and never revives a deleted rule', async () => {
  const late = deferred();
  const { saves } = await scene([OTHERS.clock], { prepare: [() => late.promise] });
  const { done: saving } = await saveWaiting();
  /* another rule is saved while A waits (the editor is not touched) */
  const added = { ...OTHERS.clock, id: 'rn', badge: 'rn', name: 'Added meanwhile' };
  assert.equal(await persistInbound({ ...ctx.settings.inbound, rules: [...ctx.settings.inbound.rules, added] }), true);
  assert.ok(waitingShown(), 'A still waits');
  late.resolve(VERIFIED);
  await saving; await turn();
  assert.equal(saves.length, 2);
  assert.deepEqual(JSON.parse(saves[1].data).inbound.rules.map(rule => rule.id), ['rb', 'rn'], 'the rule saved meanwhile is kept');
  assert.deepEqual(saves[1].channelFirstSendRequests, [{ ruleId: 'ra', external: true, identity: 'verified-identity' }]);
  assert.equal(stored('ra').firstSendGrant.id, 'native-2');
  assert.equal(stored('rn').name, 'Added meanwhile');
  assert.equal(get('auto-editor').hidden, true);

  /* the rule being saved is deleted while its save waits: it stays deleted */
  const gone = deferred();
  const second = await scene([OTHERS.clock], { prepare: [() => gone.promise] });
  const { done: waiting } = await saveWaiting();
  assert.equal(await persistInbound({ ...ctx.settings.inbound, channelRules: [] }), true);
  const afterDelete = inbound();
  gone.resolve(VERIFIED);
  await waiting; await turn();
  assert.equal(second.saves.length, 1, 'only the deletion was written');
  assert.deepEqual(inbound(), afterDelete);
  assert.equal(stored('ra'), undefined);
  assert.equal(get('auto-save').disabled, false);
});

test('losing focus or being covered does not give a waiting save up', async () => {
  const late = deferred();
  const { calls, saves } = await scene([OTHERS.clock], { prepare: [() => late.promise] });
  const { done: saving } = await saveWaiting();
  for (const type of ['blur', 'focusout', 'visibilitychange', 'pagehide']) {
    fakeDocument.fire(type);
    for (const id of ['auto-drawer', 'auto-editor', 'auto-save', 'auto-channel-ids']) get(id).fire(type);
  }
  get('auto-drawer').fire('keydown', { key: 'Tab' });
  assert.ok(waitingShown());
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 0);
  late.resolve(VERIFIED);
  await saving; await turn();
  assert.equal(saves.length, 1);
  assert.deepEqual(requestsOf(saves), [{ ruleId: 'ra', external: true, identity: 'verified-identity' }]);
  assert.equal(get('auto-editor').hidden, true);
});

test('a save whose approval is still being computed is dropped when another rule is opened', async () => {
  /* no Slack wait here: a badge rule's approval is hashed before the save */
  const { saves, before } = await scene([OTHERS.slack, { ...OTHERS.clock, id: 'rc', badge: 'rc', name: 'Evening' }]);
  edit('rb');
  get('auto-send').checked = true;
  const saving = get('auto-save').onclick();
  edit('rc');                                          // before the approval hash settled
  await saving; await turn();
  assert.equal(saves.length, 0);
  assert.deepEqual(inbound(), before);
  assert.ok(stored('rc'), 'the rule opened meanwhile is still there');
  assert.equal(get('auto-name').value, 'Evening');
});

test('pausing a rule from the list never replaces the rule open in the editor', async () => {
  const { saves, before } = await scene([OTHERS.clock, { ...OTHERS.slack, id: 'rs' }]);
  edit('rs');
  await row('rb').querySelector('.ar-pause').onclick();
  await turn();
  assert.equal(saves.length, 1);
  assert.ok(stored('rs'), 'the rule being edited was not removed');
  assert.equal(stored('rb').enabled, false);
  assert.deepEqual(inbound().channelRules, before.channelRules);
  assert.equal(get('auto-editor').hidden, false);
});

test('a save already sent finishes as its own: it closes and reports to no other editor', async () => {
  const held = deferred();
  const { calls, saves, before } = await scene([OTHERS.clock], { prepare: [() => VERIFIED], gates: [held] });
  const { done: saving } = await saveWaiting();        // the native save is under way, not answered yet
  assert.equal(saves.length, 1);
  edit('rb');
  const toasts = said().length;
  assert.ok(editorFree());
  held.resolve();
  await saving; await turn();
  assert.equal(saves.length, 1, 'the write that had started is the only one');
  assert.deepEqual(requestsOf(saves), [{ ruleId: 'ra', external: true, identity: 'verified-identity' }]);
  assert.equal(stored('ra').firstSendGrant.id, 'native-1', 'it is not pretended to be undone');
  assert.deepEqual(stored('rb'), before.rules.find(rule => rule.id === 'rb'));
  assert.ok(editorFree(), 'the other editor is not closed by it');
  assert.equal(get('auto-name').value, 'Morning');
  assert.equal(said().length, toasts, 'nothing is announced in the other editor');
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1, 'its hold is released once, after its write');
});
