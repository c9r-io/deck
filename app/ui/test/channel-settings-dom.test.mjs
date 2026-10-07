import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { ctx } = await import('../js/state.js');
const { normalizeSettings } = await import('../js/settings-model.js');
const { persistInbound, persistSettings, refreshChannelAuthority } = await import('../js/settings.js');
const rule = { id: 'R1', enabled: true, connectionId: 'default', channelIds: ['C1'], senderUserIds: ['U1'], senderBotIds: [],
  match: { kind: 'contains', value: 'incident' }, includeThreads: true, projectId: 'P1', columnId: 'C1',
  dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 };

test('explicit channel save accepts native canonical grant; unrelated save carries no approval request', async () => {
  ctx.settings = normalizeSettings({ inbound: { channelConnection: { enabled: true }, channelRules: [rule] } });
  const calls = [];
  window.__TAURI__ = { core: { invoke: async (command, args) => {
    if (command !== 'save_settings') return null;
    calls.push(structuredClone(args));
    const saved = JSON.parse(args.data);
    if (args.channelFirstSendRequests?.length) saved.inbound.channelRules[0].firstSendGrant = { id: 'native-new', digest: 'digest', stepHash: 'hash' };
    return JSON.stringify(saved);
  } } };
  assert.equal(await persistInbound({ ...ctx.settings.inbound, channelRules: [{ ...rule, firstSend: true }] }, [{ ruleId: 'R1', external: true }]), true);
  assert.equal(ctx.settings.inbound.channelRules[0].firstSendGrant.id, 'native-new');
  await persistSettings();
  assert.deepEqual(calls[0].channelFirstSendRequests, [{ ruleId: 'R1', external: true }]);
  assert.equal(calls[1].channelFirstSendRequests, undefined);
  assert.equal(JSON.parse(calls[1].data).inbound.channelRules[0].firstSendGrant.id, 'native-new');
});

test('a failed channel save preserves the previous grant and a native retirement refresh removes it', async () => {
  const before = structuredClone(ctx.settings.inbound);
  window.__TAURI__ = { core: { invoke: async command => {
    if (command === 'save_settings') throw Error('temporary storage failure');
    return null;
  } } };
  assert.equal(await persistInbound({ ...before, channelRules: [rule] }), false);
  assert.deepEqual(ctx.settings.inbound, before);
  const retired = normalizeSettings({ ...ctx.settings, inbound: { ...before, channelRules: [rule] } });
  window.__TAURI__ = { core: { invoke: async command => command === 'load_settings'
    ? { data: JSON.stringify(retired), source: 'main' } : null } };
  await refreshChannelAuthority();
  assert.equal(ctx.settings.inbound.channelRules[0].firstSendGrant, undefined);
  assert.equal(ctx.settings.inbound.channelRules[0].firstSend, undefined);
});
