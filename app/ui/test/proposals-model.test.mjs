// proposals-model.js: a drafted template and Agent rule become the rule the
// editor would save, what changes is read against the Board and settings as
// they are now, and a draft that no longer fits is not saved.
import test from 'node:test';
import assert from 'node:assert/strict';
import { agentRuleOf, draftedBy, pendingProposals, proposalChanges, proposalProblem, proposalRule, proposalSummary, proposalTemplate } from '../js/proposals-model.js';

const project = {
  id: 'P1', columns: [{ id: 'C1', name: 'Doing' }],
  templates: [{ name: 'plain', steps: ['Hello', 'Bye'] }],
};
const rule = {
  automationId: 'review', name: 'Review', columnId: 'C1', dir: '/w', cmd: 'claude', template: 'review',
  finish: 'keep', reviewEach: false, autoContinue: true, external: true, firstSend: false,
};
const proposal = (extra = {}) => ({
  id: 'prop_1', status: 'pending', projectId: 'P1', clientName: 'orchestrator',
  template: { name: 'review', steps: ['Review: {{msg.text}}', 'Run the tests.'], followUps: [] },
  rule, ...extra,
});

test('a draft becomes the rule the editor would save; an existing rule keeps its id and pause', () => {
  const made = proposalRule(proposal(), { previous: null, genId: prefix => `${prefix}new` });
  assert.deepEqual(made, {
    id: 'Rnew', source: 'agent', badge: 'review', projectId: 'P1', columnId: 'C1', cmd: 'claude',
    template: 'review', dir: '/w', name: 'Review', enabled: true, finish: 'keep',
  });
  const previous = { id: 'R9', source: 'agent', badge: 'review', enabled: false, autoSend: { digest: 'x' } };
  const kept = proposalRule(proposal({ rule: { ...rule, reviewEach: true, finish: 'close' } }), { previous, genId: () => 'never' });
  assert.equal(kept.id, 'R9');
  assert.equal(kept.enabled, false);
  assert.equal(kept.finish, 'close');
  assert.equal(kept.reviewEach, true);
  // the approval is never carried over: the caller signs a new one or none
  assert.equal('autoSend' in kept, false);
  assert.equal('firstSendWithoutReadiness' in kept, false);
});

test('the template a rule uses is the drafted one, or the project’s own', () => {
  assert.deepEqual(proposalTemplate(proposal(), project).steps, ['Review: {{msg.text}}', 'Run the tests.']);
  const onExisting = proposal({ template: undefined, rule: { ...rule, template: 'plain' } });
  assert.deepEqual(proposalTemplate(onExisting, project).steps, ['Hello', 'Bye']);
  assert.equal(proposalTemplate(proposal({ template: undefined }), project), null);
});

test('a draft that no longer fits is not saved', () => {
  assert.equal(proposalProblem(proposal(), project), null);
  assert.equal(proposalProblem(proposal(), null), 'proposals.stale.project');
  assert.equal(proposalProblem(proposal({ rule: { ...rule, columnId: 'C9' } }), project), 'proposals.stale.column');
  assert.equal(proposalProblem(proposal({ template: undefined }), project), 'proposals.stale.template');
  assert.equal(proposalProblem(proposal({ rule: undefined }), project), null);
});

test('what changes is read against the Board and settings as they are now', () => {
  const changes = proposalChanges(proposal({ template: { name: 'plain', steps: ['Hello', 'Bye now', 'More'] } }), {
    project,
    rules: [{ id: 'R1', source: 'agent', badge: 'review', columnId: 'C1', cmd: 'claude --x', template: 'review', dir: '/w', name: 'Review', finish: 'keep' }],
  });
  assert.deepEqual(changes[0].steps.map(s => s.change), ['same', 'changed', 'new']);
  assert.equal(changes[0].isNew, false);
  assert.equal(changes[1].isNew, false);
  assert.deepEqual(changes[1].fields, [
    { key: 'cmd', before: 'claude --x', after: 'claude' },
    { key: 'autoContinue', before: false, after: true },
  ]);
  const fresh = proposalChanges(proposal(), { project, rules: [] });
  assert.equal(fresh[0].isNew, true);
  assert.deepEqual(fresh[0].steps.map(s => s.change), ['new', 'new']);
  assert.deepEqual(fresh[1], { kind: 'rule', isNew: true, fields: [] });
  const shorter = proposalChanges(proposal({ template: { name: 'plain', steps: ['Hello'] }, rule: undefined }), { project, rules: [] });
  assert.equal(shorter[0].removed, 1);
});

test('pending drafts, their row summary, and who drafted a saved rule', () => {
  const list = [proposal(), proposal({ id: 'prop_2', status: 'approved', ruleId: 'R1', decidedAt: 5 }),
    proposal({ id: 'prop_3', status: 'edited', ruleId: 'R1', decidedAt: 9, clientName: 'later' }),
    proposal({ id: 'prop_4', status: 'rejected', ruleId: null })];
  assert.deepEqual(pendingProposals(list).map(p => p.id), ['prop_1']);
  assert.deepEqual(pendingProposals(null), []);
  assert.equal(draftedBy(list, 'R1'), 'later');
  assert.equal(draftedBy(list, 'R2'), null);
  assert.deepEqual(proposalSummary(proposal()).map(part => part.key), ['proposals.summary.template', 'proposals.summary.rule']);
  assert.equal(agentRuleOf([{ source: 'slack', badge: 'review' }, { source: 'agent', badge: 'review', id: 'R1' }], 'review').id, 'R1');
});
