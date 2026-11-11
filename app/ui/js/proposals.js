// proposals.js — drafted configuration: what an MCP client drafted for the
// user to approve, under Needs attention
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Backend-owned list (`mcp_proposals`, mcp/proposals.rs), pulled at start
//   and on the content-free `mcp-proposals-changed` event; nothing is stored
//   here. Needs attention shows each pending draft as one row (attention.js
//   `updateRows` asks `pendingNow` and `proposalRow`); the review dialog
//   shows the whole draft: every step in full, the rule's fields, what
//   changes against the Board and settings as they are now, and what the
//   draft asks for (automatic continuation, the first step at once).
// - Four answers. Later closes the dialog. Reject records `rejected` and
//   writes nothing. Approve writes the draft as it is: the template through
//   the Board transaction (`provider.saveTemplate` / `saveTemplateFollowUps`),
//   then the rule through the one settings writer (`persistInbound`), with
//   the approval (`approveRule`) and the first-send choice exactly as the
//   draft asked — the user's click IS the approval, as a tick in the editor
//   is. "Save and edit" writes the same without automatic sending or the
//   first-send choice and opens the automations editor (or the template
//   manager) on it; the user ticks those there. Only then is the decision
//   recorded (`mcp_proposal_decide`), so a client never learns of a save
//   that did not happen. A draft that no longer fits (its project, column
//   or template moved: proposals-model.js `proposalProblem`) is not saved.
// - A leaf: Board, project switching and the editors arrive through
//   `initProposals(deps)`.
import { $, ctx, genId, inv, listen, uev } from './state.js';
import { formatNumber, t } from './i18n.js';
import { confirmDialog, toast } from './dialogs.js';
import { persistInbound } from './settings.js';
import { approveRule, mergeRules, withFirstSend } from './automation-model.js';
import { channelBlockReason } from './channel-model.js';
import { agentRuleOf, draftedBy, pendingProposals, proposalChanges, proposalProblem, proposalRule, proposalSummary, proposalTemplate } from './proposals-model.js';

let deps = { provider: null, refresh: () => {}, switchProject: () => {}, openRuleEditor: async () => {}, openTemplates: () => {} };
let proposals = [];
let started = null;
let shown = null;     // the proposal the dialog shows
let busy = false;

export const pendingNow = () => pendingProposals(proposals);
export const drafter = ruleId => draftedBy(proposals, ruleId);

export async function refreshProposals() {
  let list;
  try { list = await inv('mcp_proposals'); } catch (_) { return; }
  const next = Array.isArray(list) ? list : [];
  const changed = JSON.stringify(next) !== JSON.stringify(proposals);
  proposals = next;
  if (!changed) return;
  if (shown && !pendingNow().some(p => p.id === shown.id)) closeProposal();
  deps.refresh();
}

const node = (tag, cls = '', text = '') => {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (text) el.textContent = text;
  return el;
};

const projectOf = p => deps.provider?.project(p.projectId) || null;

/* one Needs attention row for a pending draft */
export function proposalRow(existing, p) {
  const el = existing || (() => {
    const row = node('div', 'attention-row proposal-row');
    row.append(node('span', 'attention-card-name'), node('span', 'attention-origin'), node('span', 'attention-reason'), node('span', 'attention-viewed'));
    const open = node('button', 'btn attention-open'); open.type = 'button';
    row.append(open);
    return row;
  })();
  el.querySelector('.attention-card-name').textContent = t('proposals.rowTitle', { name: p.clientName || '—' });
  el.querySelector('.attention-origin').textContent = projectOf(p)?.name || '—';
  el.querySelector('.attention-reason').textContent = proposalSummary(p).map(part => t(part.key, part.params)).join(' · ');
  el.querySelector('.attention-viewed').textContent = p.note || '';
  const open = el.querySelector('.attention-open');
  open.textContent = t('proposals.review');
  open.setAttribute('aria-label', t('proposals.reviewNamed', { name: p.clientName || '—' }));
  open.onclick = () => openProposal(p.id, open);
  return el;
}

/* the sidebar count's suffix while drafts wait */
export const draftsCountText = () => {
  const count = pendingNow().length;
  return count ? t('proposals.countShort', { count: formatNumber(count) }) : '';
};

/* ---------- the review dialog ---------- */

const yesNo = value => t(value ? 'proposals.yes' : 'proposals.no');

function fieldText(key, value, project) {
  if (key === 'columnId') return (project?.columns || []).find(c => c.id === value)?.name || value;
  if (key === 'finish') return t(value === 'close' ? 'automation.finish.close' : 'automation.finish.keep');
  if (typeof value === 'boolean') return yesNo(value);
  return value || t('proposals.empty');
}

function kv(list, label, value) {
  list.append(node('dt', '', label), node('dd', '', value));
}

function paintDialog(p) {
  const project = projectOf(p);
  const rules = ctx.settings?.inbound?.rules || [];
  const changes = proposalChanges(p, { project, rules });
  $('prop-title').textContent = t('proposals.title', { name: p.clientName || '—', project: project?.name || '—' });
  $('prop-note').textContent = p.note || '';
  $('prop-note').hidden = !p.note;
  const body = $('prop-body');
  body.replaceChildren();
  for (const change of changes) {
    if (change.kind === 'template') {
      body.append(node('h3', '', t(change.isNew ? 'proposals.templateNew' : 'proposals.templateChanged', { name: change.name })));
      const steps = node('ol', 'prop-steps');
      for (const step of change.steps) {
        const li = node('li', `prop-step prop-step-${step.change}`);
        if (step.change !== 'same' && !change.isNew) li.append(node('span', 'prop-badge', t(step.change === 'new' ? 'proposals.step.new' : 'proposals.step.changed')));
        li.append(node('pre', 'prop-step-text', step.text));
        steps.append(li);
      }
      body.append(steps);
      if (change.removed) body.append(node('p', 'set-hint', t('proposals.stepsRemoved', { count: formatNumber(change.removed) })));
      if (change.followUps.length) {
        body.append(node('p', 'prop-label', t('proposals.followUps')));
        const list = node('ul', 'prop-followups');
        for (const text of change.followUps) list.append(node('li', '', text));
        body.append(list);
      }
    } else {
      const draft = p.rule;
      body.append(node('h3', '', t(change.isNew ? 'proposals.ruleNew' : 'proposals.ruleChanged', { id: draft.automationId })));
      const list = node('dl', 'prop-kv');
      kv(list, t('proposals.field.name'), draft.name || '—');
      kv(list, t('proposals.field.columnId'), fieldText('columnId', draft.columnId, project));
      kv(list, t('proposals.field.dir'), draft.dir);
      kv(list, t('proposals.field.cmd'), draft.cmd);
      kv(list, t('proposals.field.template'), draft.template);
      kv(list, t('proposals.field.finish'), fieldText('finish', draft.finish));
      kv(list, t('proposals.field.reviewEach'), yesNo(draft.reviewEach));
      kv(list, t('proposals.field.autoContinue'), t(draft.autoContinue ? (draft.external ? 'proposals.asks.autoExternal' : 'proposals.asks.auto') : 'proposals.asks.no'));
      kv(list, t('proposals.field.firstSend'), t(draft.firstSend ? 'proposals.asks.firstSend' : 'proposals.asks.no'));
      body.append(list);
      if (change.fields.length) {
        body.append(node('p', 'prop-label', t('proposals.changes')));
        const diff = node('ul', 'prop-diff');
        for (const field of change.fields) {
          diff.append(node('li', '', t('proposals.changeLine', {
            field: t(`proposals.field.${field.key}`),
            before: fieldText(field.key, field.before, project),
            after: fieldText(field.key, field.after, project),
          })));
        }
        body.append(diff);
      }
      if (draft.firstSend) body.append(node('p', 'set-hint prop-risk', t('proposals.risk.firstSend')));
      if (draft.autoContinue && draft.external) body.append(node('p', 'set-hint prop-risk', t('proposals.risk.external', { name: p.clientName || '—' })));
    }
  }
  const problem = proposalProblem(p, project);
  $('prop-problem').textContent = problem ? t(problem) : '';
  $('prop-problem').hidden = !problem;
  $('prop-approve').disabled = !!problem;
  $('prop-edit').disabled = !!problem;
}

let opener = null;

export function openProposal(id, from = null) {
  const p = pendingNow().find(x => x.id === id);
  if (!p) return;
  shown = p;
  opener = from;
  paintDialog(p);
  $('prop').style.display = 'flex';
  $('prop-approve').focus();
}

export function closeProposal() {
  shown = null;
  $('prop').style.display = 'none';
  if (opener?.isConnected) opener.focus();
  opener = null;
}

/* write the draft as the user chose: `edited` saves without automatic
   sending and without the first-send choice. Returns the saved rule's id. */
async function writeDraft(p, { edited }) {
  const { provider } = deps;
  const project = projectOf(p);
  const problem = proposalProblem(p, project);
  if (problem) throw new Error(problem);
  if (p.template) {
    await provider.saveTemplate(p.projectId, p.template.name, p.template.steps);
    const had = (provider.project(p.projectId)?.templates || []).find(tp => tp.name === p.template.name)?.followUps || [];
    if ((p.template.followUps || []).length || had.length) {
      await provider.saveTemplateFollowUps(p.projectId, p.template.name, p.template.followUps || []);
    }
  }
  if (!p.rule) return null;
  const now = provider.project(p.projectId);
  const inbound = ctx.settings.inbound;
  const previous = agentRuleOf(inbound.rules, p.rule.automationId);
  let rule = proposalRule(p, { previous, genId });
  if (channelBlockReason(rule, now)) throw new Error('proposals.stale.blocked');
  rule = withFirstSend(rule, !edited && p.rule.firstSend === true);
  if (!edited && p.rule.autoContinue) {
    rule = await approveRule(rule, proposalTemplate(p, now), { external: p.rule.external === true });
  }
  const current = ctx.settings.inbound;
  const ok = await persistInbound({ ...current, rules: mergeRules(current.rules, rule) });
  if (!ok) throw new Error('proposals.saveFailed');
  return rule.id;
}

async function decide(p, decision, ruleId = null) {
  try {
    await inv('mcp_proposal_decide', { proposalId: p.id, decision, ruleId });
  } catch (_) {
    /* saved (or not) as the user chose; only the client's answer is lost */
    toast(t('proposals.decideFailed'));
  }
  uev('proposals', decision);
  await refreshProposals();
}

async function act(kind) {
  const p = shown;
  if (!p || busy) return;
  busy = true;
  try {
    if (kind === 'reject') {
      closeProposal();
      await decide(p, 'rejected');
      toast(t('proposals.rejected', { name: p.clientName || '—' }));
      return;
    }
    if (p.template && (deps.provider.project(p.projectId)?.templates || []).some(tp => tp.name === p.template.name)) {
      /* the confirmation opens above this dialog */
      $('cfm').classList.add('cfm-over-dialog');
      let go;
      try { go = await confirmDialog(t('proposals.confirmReplace', { name: p.template.name })); }
      finally { $('cfm').classList.remove('cfm-over-dialog'); }
      if (!go) return;
    }
    const edited = kind === 'edit';
    let ruleId;
    try {
      ruleId = await writeDraft(p, { edited });
    } catch (error) {
      const key = String(error?.message || '');
      toast(t(key.startsWith('proposals.') ? key : 'proposals.saveFailed'));
      return;
    }
    closeProposal();
    await decide(p, edited ? 'edited' : 'approved', ruleId);
    toast(t(edited ? 'proposals.savedForEdit' : 'proposals.approved'));
    if (edited) {
      deps.switchProject(p.projectId);
      if (ruleId) await deps.openRuleEditor(ruleId);
      else deps.openTemplates();
    }
  } finally { busy = false; }
}

export function initProposals(next) {
  deps = { ...deps, ...next };
  $('prop-later').onclick = () => closeProposal();
  $('prop-reject').onclick = () => act('reject');
  $('prop-edit').onclick = () => act('edit');
  $('prop-approve').onclick = () => act('approve');
  $('prop').addEventListener('keydown', event => {
    if (event.key === 'Escape') { event.stopPropagation(); closeProposal(); }
  });
}

/* the listener and the first pull; called once the Board is the user's */
export function startProposals() {
  if (started) return started;
  started = listen('mcp-proposals-changed', refreshProposals)
    .catch(() => uev('listen-fail', 'mcp-proposals-changed'))
    .then(refreshProposals);
  return started;
}

export const proposalsForTests = { set: list => { proposals = list; }, writeDraft };
