// The automations drawer's one settings write that no click asks for: dropping
// the rules whose project is gone (automation.js `pruneOrphans`). The
// production module runs here over the test document; the drawer itself is
// verified in the real WKWebView (SMOKE.md).
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument; globalThis.window = { __TAURI__: null };
const { initAutomation, startOrphanPruning } = await import('../js/automation.js');
const { provider } = await import('../js/board.js');
const { ctx, emit, listeners, store } = await import('../js/state.js');
const { normalizeSettings } = await import('../js/settings-model.js');

const settle = () => new Promise(resolve => setTimeout(resolve, 20));
const rule = (id, source, projectId) => ({ id, source, badge: id, projectId, columnId: 'C1', template: 'triage',
  cmd: source === 'slack' ? 'claude' : '',
  ...(source === 'clock' ? { name: id, schedule: { unit: 'day', minute: 540 }, since: 0 } : {}) });
const channel = (id, projectId) => ({ id, channelIds: ['C1'], senderUserIds: ['U1'], match: { kind: 'contains', value: 'incident' },
  projectId, columnId: 'C1', cmd: 'claude', template: 'triage' });
const ruleIds = inbound => [inbound.rules.map(value => value.id), inbound.channelRules.map(value => value.id)];

test('rules are judged against the user\'s Board only: nothing is dropped until startOrphanPruning', async () => {
  const saves = [], events = [];
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (cmd, args) => {
    if (cmd === 'save_settings') saves.push(JSON.parse(args.data).inbound);
    if (cmd === 'ui_event') events.push(`${args.code} ${args.detail ?? ''}`.trim());
    return null;
  } } };
  ctx.settings = normalizeSettings({ inbound: {
    rules: [rule('deck', 'slack', 'P1'), rule('morning', 'clock', 'P1'), rule('old', 'slack', 'Pgone')],
    channelRules: [channel('incidents', 'P1'), channel('stale', 'Pgone')],
  } });
  const all = [['deck', 'morning', 'old'], ['incidents', 'stale']];
  assert.deepEqual(ruleIds(ctx.settings.inbound), all, 'the fixture survives normalization');
  // only the drawer's subscribers: board.js's own render needs the real document
  listeners.clear();
  initAutomation({ provider, activeProject: () => store.projects[0], newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });

  // the placeholder boot() leaves after a failed load: no project of the
  // user's. A Board operation that changed nothing still announces `projects`
  // (the project defaults dialog confirmed as it was).
  store.projects = [{ id: 'Pplaceholder', name: 'main', columns: [{ id: 'Cplaceholder' }] }]; store.cards = [];
  emit('projects'); await settle();
  assert.deepEqual([saves.length, events, ruleIds(ctx.settings.inbound)], [0, [], all], 'every rule looked orphaned and was dropped');

  // the user's Board is held (it loaded, or the way out was taken): from here
  // on a rule whose project is gone is dropped, whatever its trigger
  startOrphanPruning();
  store.projects = [{ id: 'P1', name: 'work', columns: [{ id: 'C1' }] }, { id: 'Pgone', name: 'old', columns: [{ id: 'C1' }] }];
  emit('projects'); await settle();
  assert.deepEqual([saves.length, ruleIds(ctx.settings.inbound)], [0, all], 'every rule has its project: nothing to save');
  store.projects = [store.projects[0]];
  emit('projects'); await settle();
  assert.deepEqual(events, ['inbound rule-orphaned']);
  assert.equal(saves.length, 1);
  assert.deepEqual(ruleIds(saves[0]), [['deck', 'morning'], ['incidents']]);
  assert.deepEqual(ruleIds(ctx.settings.inbound), [['deck', 'morning'], ['incidents']]);
  // other Board events never prune
  store.projects = [];
  emit('list'); await settle();
  assert.equal(saves.length, 1);
});
