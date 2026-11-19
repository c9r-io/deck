// proposals-model.js — the DOM-free half of drafted configuration
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - A proposal (backend mcp/proposals.rs) is a template and/or one rule an
//   MCP client drafted for the user — an Agent handover rule, or a clock
//   rule (`rule.trigger === 'clock'`, its `schedule` already in the stored
//   shape), which once saved is an ordinary clock rule; the backend checked it
//   when it was drafted. Nothing here grants anything: the rule this module
//   builds is saved, and its approval signed, only by the user's click in
//   proposals.js, through the same writers the template manager and the
//   automations editor use.
// - `proposalRule` turns a draft into the rule the editor would save
//   (automation-model.js `composeRule` shape): an existing Agent rule of the
//   same automation id keeps its id and pause state; a new one takes a fresh
//   id. Approval and the first-send choice are added by the caller, only
//   for "Approve" and only as the draft asked.
// - `proposalChanges` lists what an approval changes against the Board and
//   settings as they are NOW (the draft may be older): fields of an
//   existing rule, and which steps of an existing template change.
// - `proposalProblem` re-checks what may have moved since the draft was
//   made (project, column, template the rule names): a stale draft is not
//   saved, the user rejects it or the client drafts again.
// - `draftedBy` names the client whose approved draft saved a rule, for the
//   drawer's label (presentation only).

import { onceInstant } from './pure.js';

export const PENDING = 'pending';

export const pendingProposals = list => (Array.isArray(list) ? list : []).filter(p => p?.status === PENDING);

export const isClockDraft = proposal => proposal?.rule?.trigger === 'clock';

/* the saved rule a draft changes: the Agent rule of its automation id, or
   the clock rule a clock draft names (the backend admitted it: one of the
   client's own) */
export const draftPrevious = (rules, proposal) => (isClockDraft(proposal)
  ? (Array.isArray(rules) ? rules : []).find(rule => rule?.source === 'clock' && rule.id === proposal.rule.targetRuleId) || null
  : agentRuleOf(rules, proposal.rule.automationId));

/* the rule the draft saves; `previous` is the rule it changes, if any. A
   clock draft saves an ordinary clock rule (its badge is its id), whose
   `since` moves to now like any new or changed schedule's. */
export function proposalRule(proposal, { previous = null, genId, nowSecs = Math.floor(Date.now() / 1000) }) {
  const draft = proposal.rule;
  if (isClockDraft(proposal)) {
    const id = previous ? previous.id : genId('a');
    const changed = !previous || JSON.stringify(previous.schedule) !== JSON.stringify(draft.schedule);
    return {
      id, source: 'clock', badge: id,
      projectId: proposal.projectId, columnId: draft.columnId,
      cmd: draft.cmd, template: draft.template, dir: draft.dir, name: draft.name || '',
      enabled: previous && !changed ? previous.enabled !== false : true,
      finish: draft.finish === 'close' ? 'close' : 'keep',
      ...(draft.reviewEach ? { reviewEach: true } : {}),
      schedule: draft.schedule,
      graceMin: Number.isInteger(draft.graceMin) ? draft.graceMin : previous?.graceMin ?? 15,
      since: changed ? nowSecs : previous.since,
      ...(!changed && previous.ranAt ? { ranAt: previous.ranAt } : {}),
    };
  }
  return {
    id: previous ? previous.id : genId('R'),
    source: 'agent',
    badge: draft.automationId,
    projectId: proposal.projectId,
    columnId: draft.columnId,
    cmd: draft.cmd,
    template: draft.template,
    dir: draft.dir,
    name: draft.name || '',
    enabled: previous ? previous.enabled !== false : true,
    finish: draft.finish === 'close' ? 'close' : 'keep',
    ...(draft.reviewEach ? { reviewEach: true } : {}),
  };
}

export const agentRuleOf = (rules, automationId) => (Array.isArray(rules) ? rules : [])
  .find(rule => rule?.source === 'agent' && rule.badge === automationId) || null;

const templateOf = (project, name) => (project?.templates || []).find(tp => tp.name === name) || null;

/* the template the rule will use once the draft is saved */
export function proposalTemplate(proposal, project) {
  if (proposal.template && (!proposal.rule || proposal.rule.template === proposal.template.name)) {
    return { name: proposal.template.name, steps: proposal.template.steps, followUps: proposal.template.followUps || [] };
  }
  return proposal.rule ? templateOf(project, proposal.rule.template) : null;
}

/* null when the draft can still be saved as it is; else an i18n key */
export function proposalProblem(proposal, project) {
  if (!project) return 'proposals.stale.project';
  if (proposal.rule) {
    if (!(project.columns || []).some(column => column.id === proposal.rule.columnId)) return 'proposals.stale.column';
    if (!proposalTemplate(proposal, project)) return 'proposals.stale.template';
  }
  return null;
}

/* a clock draft's own staleness against the saved rules and the clock: the
   rule it changes is gone, or its one-time slot passed while it waited */
export function clockDraftProblem(proposal, rules, nowSecs = Math.floor(Date.now() / 1000)) {
  if (!isClockDraft(proposal)) return null;
  if (!proposal.rule.schedule) return 'proposals.stale.schedule';
  if (proposal.rule.targetRuleId && !draftPrevious(rules, proposal)) return 'proposals.stale.rule';
  const slot = onceInstant(proposal.rule.schedule);
  return slot != null && slot <= nowSecs ? 'proposals.stale.past' : null;
}

/* {kind:'template', name, isNew, steps:[{text, change:'same'|'changed'|'new'}], removed}
   and {kind:'rule', isNew, fields:[{key, before, after}]} against now */
export function proposalChanges(proposal, { project, rules }) {
  const out = [];
  if (proposal.template) {
    const before = templateOf(project, proposal.template.name);
    const old = before?.steps || [];
    out.push({
      kind: 'template',
      name: proposal.template.name,
      isNew: !before,
      steps: proposal.template.steps.map((text, i) => ({
        text, change: !before ? 'new' : i >= old.length ? 'new' : old[i] === text ? 'same' : 'changed',
      })),
      removed: Math.max(0, old.length - proposal.template.steps.length),
      followUps: proposal.template.followUps || [],
    });
  }
  if (proposal.rule) {
    const previous = draftPrevious(rules, proposal);
    const draft = proposal.rule;
    const clock = isClockDraft(proposal);
    const fields = [];
    if (previous) {
      const pairs = [
        ...(clock ? [['schedule', JSON.stringify(previous.schedule), JSON.stringify(draft.schedule)]] : []),
        ['name', previous.name || '', draft.name || ''],
        ['columnId', previous.columnId, draft.columnId],
        ['dir', previous.dir || '', draft.dir],
        ['cmd', previous.cmd, draft.cmd],
        ['template', previous.template, draft.template],
        ['finish', previous.finish === 'close' ? 'close' : 'keep', draft.finish === 'close' ? 'close' : 'keep'],
        ['reviewEach', previous.reviewEach === true, draft.reviewEach === true],
        ...(clock ? [] : [['autoContinue', !!previous.autoSend, draft.autoContinue === true]]),
        ['firstSend', previous.firstSendWithoutReadiness === true, draft.firstSend === true],
      ];
      for (const [key, before, after] of pairs) {
        if (before === after) continue;
        /* a schedule is compared as text and shown as the schedule itself */
        fields.push(key === 'schedule' ? { key, before: previous.schedule, after: draft.schedule } : { key, before, after });
      }
    }
    out.push({ kind: 'rule', isNew: !previous, fields });
  }
  return out;
}

/* the client name whose approved (or edited) draft saved `ruleId` */
export function draftedBy(list, ruleId) {
  const hit = (Array.isArray(list) ? list : [])
    .filter(p => p && ['approved', 'edited'].includes(p.status) && p.ruleId === ruleId)
    .sort((a, b) => (b.decidedAt || 0) - (a.decidedAt || 0))[0];
  return hit ? hit.clientName : null;
}

/* the one-line summary of a pending draft on its Needs attention row */
export function proposalSummary(proposal) {
  const parts = [];
  if (proposal.template) parts.push({ key: 'proposals.summary.template', params: { name: proposal.template.name } });
  /* a clock draft's part carries its schedule; the caller words it */
  if (isClockDraft(proposal)) parts.push({ key: 'proposals.summary.clock', params: { name: proposal.rule.name }, schedule: proposal.rule.schedule });
  else if (proposal.rule) parts.push({ key: 'proposals.summary.rule', params: { name: proposal.rule.name || proposal.rule.automationId } });
  return parts;
}
