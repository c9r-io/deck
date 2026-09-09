// Queue evidence and C v01 human inspection. These views never authorize a
// send from agent/quiet UI state: decisions use a backend preview token and
// persist before release. Opened/seen is unrelated to inspected. No new Board
// entry, hook installation, terminal parsing or prompt history capture.
import { ctx, inv } from './state.js';
import { t } from './i18n.js';
import { confirmDialog, toast } from './dialogs.js';
import { formatInterval } from './i18n.js';

export const isReview = i => i && ['review', 'review-approved'].includes(i.state);
const stageKeys = {
  review: 'queue.stage.review', 'review-approved': 'queue.stage.approved', ambiguous: 'queue.meta.ambiguous',
  firing: 'queue.meta.sending', failed: 'queue.gaveUp', paused: 'queue.meta.paused', retry: 'queue.failedRetrying',
  previous: 'queue.stage.previous', iteration: 'queue.stage.iteration', gap: 'queue.stage.gap', time: 'queue.stage.time',
  quiet: 'queue.stage.quiet', unknown: 'queue.stage.unknown', context: 'queue.stage.context',
};
const node = (tag, cls, text) => {
  const e = document.createElement(tag); e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};
const button = (id, action, label, run) => {
  const b = node('button', 'btn', label); b.type = 'button'; b.dataset.queueFocus = `${id}:${action}`;
  b.onclick = async () => {
    b.disabled = true;
    try { await run(); } catch (e) { toast(t('queue.review.failed')); }
    finally { b.disabled = false; }
  };
  return b;
};

export function stageText(item) {
  const plan = (ctx.queueCache.plans || []).find(p => p.item === item.id);
  if (!plan || Date.now() / 1000 - plan.checked_at > 40) return t('queue.stage.unknown');
  return t(stageKeys[plan.stage] || 'queue.stage.unknown', {
    duration: formatInterval(plan.stage === 'gap' ? Math.max(0, plan.gap_until - plan.checked_at) : (plan.quiet_remaining || 0)),
  });
}

export async function cancelQueueList(item, refresh) {
  if (!await confirmDialog(t('queue.review.cancelConfirm'))) return;
  await inv('queue_cancel_list', { id: item.id });
  await refresh();
}

export function reviewRow(item, refresh, viewTerminal) {
  const row = node('div', 'qg-review'); row.dataset.qkey = item.id;
  const body = node('div', 'qg-review-body');
  body.append(node('div', 'qg-review-title', item.text),
    node('p', 'qg-review-status', t(item.state === 'review' ? 'queue.stage.review' : 'queue.stage.approved')),
    node('p', 'q-hint', t('queue.review.boundary')));
  const actions = node('div', 'qg-review-actions');
  actions.append(button(item.id, 'view', t('queue.review.view'), viewTerminal));
  if (item.state === 'review') {
    actions.append(button(item.id, 'confirm', t('queue.review.inspect'), async () => {
      const preview = await inv('queue_review_preview', { id: item.id });
      const message = preview.next_text == null ? t('queue.review.lastConfirm')
        : t('queue.review.confirm', { prompt: preview.next_text, current: preview.current_process || t('queue.context.noProcess'),
          expected: preview.expected_process || t('queue.review.compatibility') });
      if (!await confirmDialog(message)) return;
      await inv('queue_review_confirm', { decision: preview.decision });
      await refresh();
    }));
  }
  actions.append(button(item.id, 'cancel', t('queue.review.cancel'), () => cancelQueueList(item, refresh)));
  row.append(body, actions);
  return row;
}

export function executionPlan(g, card, otherLists, refresh) {
  const box = node('div', 'q-execution-plan');
  const schedule = node('div', 'q-plan-conditions');
  const review = g.rows.some(i => i.review_each);
  const mode = node('div', 'q-plan-mode');
  mode.append(node('strong', '', t('queue.plan')),
    button(g.head.id, 'mode', t(review ? 'queue.review.enabled' : 'queue.review.disabled'), async () => {
      if (!await confirmDialog(t(review ? 'queue.review.disableConfirm' : 'queue.review.enableConfirm'))) return;
      await inv('queue_review_mode', { id: g.head.id, enabled: !review }); await refresh();
    }));
  schedule.append(mode, node('p', '', t(g.head.mode === 'every' || g.head.rule ? 'queue.plan.repeat' : 'queue.plan.current')),
    node('p', 'q-hint', t('queue.plan.conditions')),
    node('p', 'q-plan-stage', stageText(g.rows.find(i => i.state !== 'review-approved') || g.head)),
    node('p', 'q-hint', t('queue.plan.others', { count: otherLists })));
  const evidence = node('div', 'q-plan-evidence');
  const observation = ctx.attention.get(card);
  const agentKeys = { working: 'queue.signal.working', 'needs-input': 'queue.signal.input', 'turn-done': 'queue.signal.done' };
  evidence.append(node('strong', '', t('queue.plan.evidence')),
    node('p', '', t(observation?.stale ? 'queue.stage.unknown' : (agentKeys[observation?.agent] || 'queue.signal.none'))),
    node('p', 'q-hint', t('queue.plan.unverified')));
  if (g.head.last_context) evidence.append(node('p', 'q-hint', t('queue.plan.targetTime', {
    time: new Date(g.head.last_context.checked_at * 1000).toLocaleTimeString(),
  })));
  evidence.append(node('p', 'q-hint', t(g.head.expected_process ? 'queue.plan.expected' : 'queue.plan.compatibility', { process: g.head.expected_process })));
  box.append(schedule, evidence); return box;
}

const historyOpen = new Set();
export function queueHistory(card) {
  const details = node('details', 'q-history'); details.open = historyOpen.has(card.id);
  details.ontoggle = () => { if (details.open) historyOpen.add(card.id); else historyOpen.delete(card.id); };
  const summary = node('summary', '', t('queue.history')); summary.dataset.queueFocus = `${card.id}:history`;
  details.append(summary, node('p', 'q-hint', t('queue.history.limit')));
  const deliveries = (ctx.queueCache.deliveries || []).filter(d => d.session === card.session);
  const reviews = (ctx.queueCache.reviews || []).filter(d => d.session === card.session);
  const entries = [...deliveries.map(d => ({ ...d, type: d.assumed ? 'queue.history.assumed' : 'queue.history.sent' })),
    ...reviews.map(r => ({ ...r, id: r.delivery, type: r.next ? 'queue.history.checked' : 'queue.history.last' }))]
    .sort((a, b) => b.at - a.at);
  for (const d of entries) details.append(node('p', 'q-history-record', `${new Date(d.at * 1000).toLocaleString()} · ${t(d.type)} · ${d.id}`));
  if (!entries.length) details.append(node('p', 'q-hint', t('queue.history.empty')));
  return details;
}
