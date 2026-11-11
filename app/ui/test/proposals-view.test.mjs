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
