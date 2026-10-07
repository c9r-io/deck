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
  assert.deepEqual(saves[0].channelFirstSendRequests, [{ ruleId: 'R1', external: true }]);
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
  assert.deepEqual(saves[2].channelFirstSendRequests, [{ ruleId: 'R1', external: true }]);
  assert.notEqual(granted().firstSendGrant.id, firstGrant);
  openEditor(granted());
  get('auto-channel-first-send').checked = false;
  get('auto-channel-first-send').fire('change');
  await get('auto-save').onclick();
  assert.equal(saves[3].channelFirstSendRequests, undefined);
  assert.equal(granted().firstSendGrant, undefined);
  assert.equal(granted().firstSend, undefined);
});
