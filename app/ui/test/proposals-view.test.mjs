// proposals.js against the fake document: the review dialog approves,
// rejects and — when the draft stopped waiting while the user answered the
// "replace this template?" question — writes nothing.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { initProposals, openProposal, refreshProposals, pendingNow } = await import('../js/proposals.js');
const { cfmDone } = await import('../js/dialogs.js');
const { setLocale } = await import('../js/i18n.js');

const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const draft = (status = 'pending') => ({
  id: 'prop_1', status, projectId: 'P1', clientName: 'orchestrator',
  template: { name: 'plain', steps: ['Review: {{msg.text}}', 'Run the tests.'], followUps: [] },
});

function world(list) {
  const calls = [];
  let current = list;
  window.__TAURI__ = {
    event: { listen: async () => {} },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'mcp_proposals') return structuredClone(current);
      return null;
    } },
  };
  const saved = [];
  const project = { id: 'P1', columns: [{ id: 'C1', name: 'Doing' }], templates: [{ name: 'plain', steps: ['Hello'] }] };
  initProposals({
    provider: {
      project: id => (id === 'P1' ? project : null),
      saveTemplate: async (...args) => { saved.push(args); },
      saveTemplateFollowUps: async () => {},
    },
    refresh: () => {},
  });
  return { calls, saved, set(next) { current = next; }, decided: () => calls.filter(([cmd]) => cmd === 'mcp_proposal_decide') };
}

test('a draft withdrawn while the user answers the replace question is not written', async () => {
  setLocale('en');
  const w = world([draft()]);
  await refreshProposals();
  openProposal('prop_1');
  assert.equal(fakeDocument.getElementById('prop').style.display, 'flex');
  fakeDocument.getElementById('prop-approve').fire('click');
  await tick();
  assert.equal(fakeDocument.getElementById('cfm').style.display, 'flex', 'the replace question');
  // the client withdraws it before the user says yes
  w.set([draft('withdrawn')]);
  cfmDone(true);
  await tick(); await tick();
  assert.deepEqual(w.saved, [], 'nothing saved');
  assert.deepEqual(w.decided(), [], 'nothing decided');
  assert.equal(fakeDocument.getElementById('prop').style.display, 'none');
  assert.equal(pendingNow().length, 0);
});

test('a draft still waiting is saved as drafted and answered approved', async () => {
  const w = world([draft()]);
  await refreshProposals();
  openProposal('prop_1');
  fakeDocument.getElementById('prop-approve').fire('click');
  await tick();
  cfmDone(true);
  for (let i = 0; i < 6; i++) await tick();
  assert.deepEqual(w.saved, [['P1', 'plain', ['Review: {{msg.text}}', 'Run the tests.']]]);
  assert.deepEqual(w.decided().map(([, args]) => args.decision), ['approved']);
});

test('reject answers rejected and saves nothing', async () => {
  const w = world([draft()]);
  await refreshProposals();
  openProposal('prop_1');
  fakeDocument.getElementById('prop-reject').fire('click');
  for (let i = 0; i < 4; i++) await tick();
  assert.deepEqual(w.saved, []);
  assert.deepEqual(w.decided().map(([, args]) => args.decision), ['rejected']);
});

/* a clock draft: the dialog words its schedule, Approve saves an ordinary
   clock rule through the settings writer, and a draft whose one-time slot
   passed while it waited cannot be approved */
test('a clock draft is saved as a clock rule with its first-send choice; a past one cannot be approved', async () => {
  const { ctx } = await import('../js/state.js');
  const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
  const { refreshChannelAuthority } = await import('../js/settings.js');
  const clock = (anchor, extra = {}) => ({
    id: 'prop_c', status: 'pending', projectId: 'P1', clientName: 'orchestrator', template: null,
    rule: { trigger: 'clock', automationId: '', name: 'Follow up', columnId: 'C1', dir: '/w', cmd: 'claude', template: 'plain',
      finish: 'keep', reviewEach: false, autoContinue: false, external: false, firstSend: true,
      schedule: { unit: 'once', days: [], minute: 570, anchor }, graceMin: null, targetRuleId: null, ...extra },
  });
  const w = world([clock('2999-01-02')]);
  ctx.settings = normalizeSettings({});
  const disk = { data: serializeSettings(ctx.settings), writes: [] };
  const invoke = window.__TAURI__.core.invoke;
  window.__TAURI__.core.invoke = async (cmd, args) => {
    if (cmd === 'load_settings') return { data: disk.data };
    if (cmd === 'save_settings') { disk.data = args.data; disk.writes.push(args.data); return disk.data; }
    return invoke(cmd, args);
  };
  await refreshChannelAuthority();
  await refreshProposals();
  openProposal('prop_c');
  assert.equal(fakeDocument.getElementById('prop-approve').disabled, false);
  fakeDocument.getElementById('prop-approve').fire('click');
  for (let i = 0; i < 12; i++) await tick();
  assert.equal(disk.writes.length, 1, 'one settings write');
  const [rule] = ctx.settings.inbound.rules;
  assert.equal(rule.source, 'clock');
  assert.equal(rule.badge, rule.id);
  assert.deepEqual(rule.schedule, { unit: 'once', days: [], minute: 570, anchor: '2999-01-02' });
  assert.equal(rule.firstSendWithoutReadiness, true);
  assert.equal('autoSend' in rule, false, 'a clock rule carries no approval');
  assert.deepEqual(w.decided().map(([, args]) => [args.decision, args.ruleId]), [['approved', rule.id]]);

  w.set([clock('2020-01-01')]);
  await refreshProposals();
  openProposal('prop_c');
  assert.equal(fakeDocument.getElementById('prop-approve').disabled, true);
  assert.equal(fakeDocument.getElementById('prop-edit').disabled, true);
  assert.notEqual(fakeDocument.getElementById('prop-problem').textContent, '');
});
