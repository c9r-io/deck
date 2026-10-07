// Exercise the real editor save handler without invoking native services.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { FakeElement, fakeDocument } from './fixtures/dom-fixture.mjs';
fakeDocument.querySelectorAll = () => [];
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { initAutomation, openEditor } = await import('../js/automation.js');
const { provider } = await import('../js/board.js');
const { ctx, store } = await import('../js/state.js');
const { normalizeSettings } = await import('../js/settings-model.js');
const get = id => fakeDocument.getElementById(id);
for (const [id, values] of [['auto-trigger', ['channel', 'clock', 'slack']], ['auto-finish', ['keep', 'close']]]) {
  const buttons = values.map(value => { const button = new FakeElement('button'); button.dataset.v = value; return button; });
  get(id).querySelectorAll = () => buttons;
  get(id).querySelector = () => buttons.find(button => button['aria-pressed'] === 'true');
}
get('auto-drawer').hidden = true;

test('one channel checkbox and save authorize once; ordinary saves preserve and semantic saves update', async () => {
  const rule = { id: 'R1', source: 'channel', enabled: true, connectionId: 'default', channelIds: ['C1', 'C2'],
    senderUserIds: ['U1'], senderBotIds: [], match: { kind: 'contains', value: 'incident' }, includeThreads: true,
    projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 };
  const project = { id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}', 'Later'] }] };
  store.projects = [project]; store.cards = [];
  ctx.settings = normalizeSettings({ inbound: { channelConnection: { enabled: true }, channelRules: [rule] } });
  const saves = [];
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
    if (command === 'slack_channel_prepare') return { identity: 'verified-identity' };
    if (command !== 'save_settings') return null;
    saves.push(structuredClone(args));
    const saved = JSON.parse(args.data);
    if (args.channelFirstSendRequests?.length) saved.inbound.channelRules[0].firstSendGrant = {
      id: `native-${saves.length}`, digest: 'native-digest',
      stepHash: createHash('sha256').update('Inspect {{msg.text}}').digest('hex'),
    };
    return JSON.stringify(saved);
  } } };
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  openEditor(rule);
  assert.equal(get('auto-channel-first-send').checked, false);
  get('auto-channel-first-send').checked = true;
  get('auto-channel-first-send').fire('change');
  await get('auto-save').onclick();
  assert.equal(saves.length, 1);
  assert.deepEqual(saves[0].channelFirstSendRequests, [{ ruleId: 'R1', external: true, identity: 'verified-identity' }]);
  const granted = () => ({ ...ctx.settings.inbound.channelRules[0], source: 'channel' });
  const firstGrant = granted().firstSendGrant.id;
  openEditor(granted());
  get('auto-channel-ids').value = 'C2, C1, C1';
  project.templates[0].steps[1] = 'Different follow-up';
  await get('auto-save').onclick();
  assert.equal(saves[1].channelFirstSendRequests, undefined);
  assert.equal(granted().firstSendGrant.id, firstGrant);
  openEditor(granted());
  get('auto-idle').value = '5';
  await get('auto-save').onclick();
  assert.deepEqual(saves[2].channelFirstSendRequests, [{ ruleId: 'R1', external: true, identity: 'verified-identity' }]);
  assert.notEqual(granted().firstSendGrant.id, firstGrant);
  openEditor(granted());
  get('auto-channel-first-send').checked = false;
  get('auto-channel-first-send').fire('change');
  await get('auto-save').onclick();
  assert.equal(saves[3].channelFirstSendRequests, undefined);
  assert.equal(granted().firstSendGrant, undefined);
  assert.equal(granted().firstSend, undefined);
});

/* ---------- the first rule, from zero channel rules (F3) ---------- */

const project = { id: 'P1', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['Inspect {{msg.text}}', 'Later'] }] };
const deferred = () => { let settle; const promise = new Promise((resolve, reject) => { settle = { resolve, reject }; }); return { promise, ...settle }; };
const turn = () => new Promise(resolve => setImmediate(resolve));

/* A native side with no verified identity until the transport connects:
   `prepare` answers are scripted, `save_settings` grants only for the
   identity the request names. */
function zeroRules(answers) {
  store.projects = [project]; store.cards = [];
  ctx.settings = normalizeSettings({ inbound: { channelConnection: { enabled: true }, channelRules: [] } });
  const calls = []; const saves = [];
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
    calls.push(command);
    if (command === 'slack_channel_prepare') {
      const next = answers.shift();
      if (!next) throw 'pending';
      return next();
    }
    if (command !== 'save_settings') return null;
    saves.push(structuredClone(args));
    const saved = JSON.parse(args.data);
    const request = args.channelFirstSendRequests?.[0];
    if (request) {
      if (request.identity !== 'verified-identity') throw new Error('verified Slack identity changed');
      saved.inbound.channelRules.find(rule => rule.id === request.ruleId).firstSendGrant = { id: 'native-1', digest: 'native-digest',
        stepHash: createHash('sha256').update('Inspect {{msg.text}}').digest('hex') };
    }
    return JSON.stringify(saved);
  } } };
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  openEditor(null);
  get('auto-trigger').querySelectorAll().forEach(button => { button['aria-pressed'] = button.dataset.v === 'channel' ? 'true' : 'false'; });
  get('auto-channel-ids').value = 'C1'; get('auto-sender-users').value = 'U1'; get('auto-sender-bots').value = '';
  get('auto-match-kind').value = 'contains'; get('auto-match-value').value = 'incident';
  get('auto-threads').checked = true; get('auto-idle').value = '30';
  get('auto-dir').value = '/tmp'; get('auto-cmd').value = 'claude';
  get('auto-template').value = 'triage'; get('auto-column').value = 'C1';
  get('auto-channel-first-send').checked = true;
  get('auto-channel-first-send').fire('change');
  return { calls, saves };
}
const rules = () => ctx.settings.inbound.channelRules;
const count = (calls, command) => calls.filter(value => value === command).length;

test('the first channel rule with automatic first steps is one save: the wait is shown, retried and never asked twice', async () => {
  const outage = deferred();
  const { calls, saves } = zeroRules([
    () => outage.promise,                              // the transport is not connected yet
    () => { throw 'pending'; },                        // a short outage: asked again
    () => ({ identity: 'verified-identity' }),         // connected
  ]);
  const saving = get('auto-save').onclick();
  await turn();
  /* waiting: said in the editor, the read fields held still, nothing saved */
  assert.equal(get('auto-channel-verify').hidden, false);
  assert.match(get('auto-channel-verify').textContent, /not saved yet/);
  assert.equal(get('auto-save').disabled, true);
  assert.equal(get('auto-channel-ids').disabled, true);
  assert.equal(get('auto-cancel').disabled, false, 'the way out stays');
  assert.equal(saves.length, 0);
  await get('auto-save').onclick();                    // a second click starts nothing
  assert.equal(count(calls, 'slack_channel_prepare'), 1);
  outage.reject('pending');
  await saving;
  /* one click, one save, the rule and its permission together */
  assert.equal(count(calls, 'slack_channel_prepare'), 3);
  assert.equal(saves.length, 1);
  assert.deepEqual(saves[0].channelFirstSendRequests.map(({ external, identity }) => ({ external, identity })),
    [{ external: true, identity: 'verified-identity' }]);
  assert.equal(rules().length, 1);
  assert.equal(rules()[0].firstSend, true);
  assert.equal(rules()[0].firstSendGrant.id, 'native-1');
  assert.equal(get('auto-editor').hidden, true);
  assert.equal(get('auto-channel-verify').hidden, true);
  assert.equal(get('auto-save').disabled, false);
  assert.equal(get('auto-channel-ids').disabled, false);
  assert.ok(calls.indexOf('slack_channel_prepare_cancel') > calls.indexOf('save_settings'), 'the hold is released after the save');
});

test('a wait that ends without an identity saves nothing and says so; the same button retries', async () => {
  const { calls, saves } = zeroRules([]);              // Slack never answers
  await get('auto-save').onclick();
  assert.equal(count(calls, 'slack_channel_prepare'), 6, 'bounded');
  assert.equal(saves.length, 0);
  assert.equal(rules().length, 0);
  assert.equal(get('auto-editor').hidden, false, 'the editor and its fields stay');
  assert.equal(get('auto-channel-ids').value, 'C1');
  assert.equal(get('auto-channel-first-send').checked, true, 'the choice is kept, not asked again');
  assert.equal(get('auto-channel-verify').hidden, false);
  assert.match(get('auto-channel-verify').textContent, /was not saved and no permission was given/);
  assert.equal(get('auto-save').disabled, false);

  for (const [code, text] of [['disabled', /monitoring is off/], ['no-token', /monitoring is off/], ['worker', /could not be checked/]]) {
    const blocked = zeroRules([() => { throw code; }]);
    await get('auto-save').onclick();
    assert.equal(count(blocked.calls, 'slack_channel_prepare'), 1, `${code} is not retried`);
    assert.equal(blocked.saves.length, 0);
    assert.match(get('auto-channel-verify').textContent, text);
  }
  /* a native answer without an identity never becomes a grant request */
  const empty = zeroRules([() => ({})]);
  await get('auto-save').onclick();
  assert.equal(empty.saves.length, 0);

  /* Slack is back: pressing the same button completes the save */
  const back = zeroRules([() => ({ identity: 'verified-identity' })]);
  await get('auto-save').onclick();
  assert.equal(back.saves.length, 1);
  assert.equal(rules()[0].firstSendGrant.id, 'native-1');
});

test('giving the waiting save up grants nothing, even when the identity arrives afterwards', async () => {
  for (const leave of [() => get('auto-cancel').onclick(), () => get('auto-close').onclick()]) {
    const late = deferred();
    const { calls, saves } = zeroRules([() => late.promise]);
    const saving = get('auto-save').onclick();
    await turn();
    leave();
    assert.equal(count(calls, 'slack_channel_prepare_cancel'), 1, 'the native hold is released');
    assert.equal(get('auto-editor').hidden, true);
    late.resolve({ identity: 'verified-identity' });   // the old result arrives now
    await saving;
    assert.equal(saves.length, 0);
    assert.equal(rules().length, 0);
    assert.equal(count(calls, 'slack_channel_prepare'), 1);
    assert.equal(get('auto-save').disabled, false);
  }
  /* an identity that changed after the choice is refused by the native save */
  const { saves } = zeroRules([() => ({ identity: 'another-workspace' })]);
  await get('auto-save').onclick();
  assert.equal(saves.length, 1);
  assert.equal(rules().length, 0, 'the refused save left no rule');
  assert.equal(get('auto-editor').hidden, false);
});

test('a save that does not turn first steps on never asks for the identity', async () => {
  const { calls, saves } = zeroRules([]);
  get('auto-channel-first-send').checked = false;
  get('auto-channel-first-send').fire('change');
  await get('auto-save').onclick();
  assert.equal(count(calls, 'slack_channel_prepare'), 0);
  assert.equal(count(calls, 'slack_channel_prepare_cancel'), 0);
  assert.equal(saves.length, 1);
  assert.equal(saves[0].channelFirstSendRequests, undefined);
  assert.equal(rules().length, 1);
});
