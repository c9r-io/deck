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

/* ---------- clock drafts ---------- */
import { clockDraftProblem, draftPrevious, isClockDraft } from '../js/proposals-model.js';

const clockRule = {
  trigger: 'clock', automationId: '', name: 'Follow up', columnId: 'C1', dir: '/w', cmd: 'claude', template: 'plain',
  finish: 'keep', reviewEach: false, autoContinue: false, external: false, firstSend: true,
  schedule: { unit: 'once', days: [], minute: 570, anchor: '2999-01-02' }, graceMin: null, targetRuleId: null,
};
const clockDraft = (extra = {}) => proposal({ template: null, rule: { ...clockRule, ...extra } });

test('a clock draft becomes an ordinary clock rule; one that changes a rule keeps its id', () => {
  assert.ok(isClockDraft(clockDraft()) && !isClockDraft(proposal()));
  const made = proposalRule(clockDraft(), { previous: null, genId: prefix => `${prefix}new`, nowSecs: 1000 });
  assert.deepEqual(made, {
    id: 'anew', source: 'clock', badge: 'anew', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'plain',
    dir: '/w', name: 'Follow up', enabled: true, finish: 'keep',
    schedule: { unit: 'once', days: [], minute: 570, anchor: '2999-01-02' }, graceMin: 15, since: 1000,
  });
  const saved = { ...made, id: 'a7', badge: 'a7', enabled: false, graceMin: 60, since: 5, ranAt: 9 };
  const rules = [{ id: 'a7', source: 'agent', badge: 'x' }, saved];
  assert.equal(draftPrevious(rules, clockDraft({ targetRuleId: 'a7' })), saved);
  assert.equal(draftPrevious(rules, clockDraft()), null);
  assert.equal(draftPrevious(null, clockDraft({ targetRuleId: 'a7' })), null);
  /* the same schedule: pause, since, grace and the run record stay */
  const same = proposalRule(clockDraft({ targetRuleId: 'a7', name: 'Renamed' }), { previous: saved, genId: () => 'never', nowSecs: 2000 });
  assert.deepEqual([same.id, same.badge, same.enabled, same.since, same.graceMin, same.ranAt], ['a7', 'a7', false, 5, 60, 9]);
  /* a new schedule starts from now, on, without the record */
  const moved = proposalRule(clockDraft({ targetRuleId: 'a7', graceMin: 30, schedule: { unit: 'day', days: [], minute: 60 } }),
    { previous: saved, genId: () => 'never', nowSecs: 2000 });
  assert.deepEqual([moved.enabled, moved.since, moved.graceMin, 'ranAt' in moved], [true, 2000, 30, false]);
});

test('a clock draft lists its schedule change and goes stale when its rule is gone or its time passed', () => {
  const saved = { id: 'a7', source: 'clock', name: 'Follow up', columnId: 'C1', dir: '/w', cmd: 'claude', template: 'plain',
    finish: 'keep', schedule: { unit: 'day', days: [], minute: 60 }, firstSendWithoutReadiness: true };
  const changes = proposalChanges(clockDraft({ targetRuleId: 'a7' }), { project, rules: [saved] });
  assert.equal(changes[0].isNew, false);
  assert.deepEqual(changes[0].fields, [{ key: 'schedule', before: saved.schedule, after: clockRule.schedule }]);
  assert.equal(proposalChanges(clockDraft(), { project, rules: [saved] })[0].isNew, true);
  assert.equal(clockDraftProblem(proposal(), [], 0), null);
  assert.equal(clockDraftProblem(clockDraft(), [], 0), null);
  assert.equal(clockDraftProblem(clockDraft({ schedule: null }), [], 0), 'proposals.stale.schedule');
  assert.equal(clockDraftProblem(clockDraft({ targetRuleId: 'a7' }), [], 0), 'proposals.stale.rule');
  assert.equal(clockDraftProblem(clockDraft({ targetRuleId: 'a7' }), [saved], 0), null);
  assert.equal(clockDraftProblem(clockDraft({ schedule: { unit: 'once', days: [], minute: 0, anchor: '2026-01-01' } }), [], 4102444800), 'proposals.stale.past');
  assert.equal(clockDraftProblem(clockDraft({ schedule: { unit: 'day', days: [], minute: 0 } }), [], 4102444800), null);
  assert.deepEqual(proposalSummary(clockDraft()), [{ key: 'proposals.summary.clock', params: { name: 'Follow up' }, schedule: clockRule.schedule }]);
});
