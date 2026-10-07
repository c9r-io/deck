// queue-review.js — the ⏱ panel's evidence and inspection views (C v01)
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Read-only projections of `ctx.queueCache`: `stageText` (the backend's
//   selection stage for an item; a plan older than 40 s reads as unknown),
//   `sendPlan` (a list's inspection mode and its send conditions),
//   `sessionFacts` (what holds for the whole session, said once: lists that
//   may interleave, and the hook observation, shown APART from any list and
//   labelled unverified) and `queueHistory` (the delivery and inspection
//   ledgers: ids and times, no prompt text).
// - LAYERS. Always visible: a row's own stage (`rowStage`), a compatibility
//   target (input may reach a shell), and every hold that needs the user.
//   On demand, under a named disclosure: how sends are timed, the last target
//   check, the hook observation. A row that merely waits its turn behind the
//   row above it says nothing (`rowStage` is empty for a fresh `previous`
//   stage): the order already says it. Stale, unknown and every other stage
//   stay on the row. Disclosures remember being open across the panel's
//   re-renders and carry `data-queue-focus`; opening one fetches, saves and
//   releases nothing.
// - `reviewRow` is the one place a checkpoint is released. Inspect asks the
//   backend for a preview (`queue_review_preview`), shows the next prompt and
//   the observed vs expected process, and only a confirmed dialog returns
//   that exact decision (`queue_review_confirm`). Nothing here reads agent
//   state, viewed markers or quiet time to authorize a send; an opened or
//   seen card is unrelated to inspected.
// - `cancelQueueList` is the confirmed alternative that omits work without
//   claiming inspection. Buttons disable while their action is in flight and
//   carry `data-queue-focus` so a re-render can restore focus.
// - No new Board entry, hook installation, terminal parsing or prompt
//   history capture lives here.
// - Channel permission withdrawal and temporarily unverifiable permission
//   have separate plain-language stages; neither means Agent readiness.
import { ctx, inv } from './state.js';
import { t } from './i18n.js';
import { confirmDialog, toast } from './dialogs.js';
import { formatInterval } from './i18n.js';
import { approvedWait, firstSendOverrideWait } from './scheduler-model.js';

export const isReview = i => i && ['review', 'review-approved'].includes(i.state);
const stageKeys = {
  review: 'queue.stage.review', 'review-approved': 'queue.stage.approved', ambiguous: 'queue.meta.ambiguous',
  firing: 'queue.meta.sending', failed: 'queue.gaveUp', paused: 'queue.meta.paused', retry: 'queue.failedRetrying',
  previous: 'queue.stage.previous', iteration: 'queue.stage.iteration', gap: 'queue.stage.gap', time: 'queue.stage.time',
  quiet: 'queue.stage.quiet', unknown: 'queue.stage.unknown', context: 'queue.stage.context', agent: 'queue.stage.agent',
  'first-send': 'queue.stage.firstSend', external: 'queue.stage.external', 'codex-signal': 'queue.stage.codexSignal',
  'authority-unverified': 'queue.stage.authorityUnverified',
  'channel-stopped': 'queue.stage.channelStopped',
  'channel-unverified': 'queue.stage.channelUnverified',
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

const freshPlan = item => {
  const plan = (ctx.queueCache.plans || []).find(p => p.item === item.id);
  return !plan || Date.now() / 1000 - plan.checked_at > 40 ? null : plan;
};

/* a row's own stage, or '' for a row that only waits its turn in its list */
export const rowStage = item => (freshPlan(item)?.stage === 'previous' ? '' : stageText(item));

export function stageText(item) {
  const plan = freshPlan(item);
  if (!plan) return t('queue.stage.unknown');
  const text = t(stageKeys[plan.stage] || 'queue.stage.unknown', {
    duration: formatInterval(plan.stage === 'gap' ? Math.max(0, plan.gap_until - plan.checked_at) : (plan.quiet_remaining || 0)),
  });
  const shown = firstSendOverrideWait(plan) ? t('queue.stage.firstSendOverride', { stage: text }) : text;
  return approvedWait(plan) ? t('queue.stage.authorized', { stage: shown }) : shown;
}

export async function cancelQueueList(item, refresh) {
  if (!await confirmDialog(t('queue.review.cancelConfirm'))) return;
  await inv('queue_cancel_list', { id: item.id });
  await refresh();
}

/* `last` describes the list the checkpoint closes: whether it repeats, and
   whether the card's automation closes it after the program exits. Each
   consequence is said only where it applies. */
export function reviewRow(item, refresh, viewTerminal, last = {}) {
  const row = node('div', 'qg-review'); row.dataset.qkey = item.id;
  const body = node('div', 'qg-review-body');
  body.append(node('div', 'qg-review-title', item.text),
    node('p', 'qg-review-status', t(item.state === 'review' ? 'queue.stage.review' : 'queue.stage.approved')));
  if (item.state === 'review') body.append(node('p', 'q-hint', t('queue.review.boundary')));
  const actions = node('div', 'qg-review-actions');
  actions.append(button(item.id, 'view', t('queue.review.view'), viewTerminal));
  if (item.state === 'review') {
    actions.append(button(item.id, 'confirm', t('queue.review.inspect'), async () => {
      const preview = await inv('queue_review_preview', { id: item.id });
      const message = preview.next_text == null
        ? [t('queue.review.lastConfirm'), last.repeats ? t('queue.review.lastRepeat') : '', last.closes ? t('queue.review.lastClose') : '']
          .filter(Boolean).join('\n\n')
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

/* a disclosure that stays as the user left it across re-renders */
const opened = new Set();
function disclosure(key, cls, label) {
  const details = node('details', cls); details.open = opened.has(key);
  details.ontoggle = () => { if (details.open) opened.add(key); else opened.delete(key); };
  const summary = node('summary', '', label); summary.dataset.queueFocus = key;
  details.append(summary);
  return details;
}

export const listRepeatsOrRule = g => g.head.mode === 'every' || !!g.head.rule;

/* A list's send plan: how its rows are released (the inspection mode, a
   button because it is a choice) and, on demand, the conditions a send
   waits for. A compatibility target is a risk, so it is said outside. */
export function sendPlan(g, refresh) {
  const box = node('div', 'q-send-plan');
  const review = g.rows.some(i => i.review_each);
  const mode = node('div', 'q-plan-mode');
  mode.append(node('strong', '', t('queue.plan')),
    button(g.head.id, 'mode', t(review ? 'queue.review.enabled' : 'queue.review.disabled'), async () => {
      if (!await confirmDialog(t(review ? 'queue.review.disableConfirm' : 'queue.review.enableConfirm'))) return;
      await inv('queue_review_mode', { id: g.head.id, enabled: !review }); await refresh();
    }));
  box.append(mode);
  if (!g.head.expected_process) box.append(node('p', 'q-hint q-risk', t('queue.plan.compatibility')));
  const details = disclosure(`${g.head.id}:conditions`, 'q-plan-details', t('queue.plan.details'));
  details.append(node('p', '', t(listRepeatsOrRule(g) ? 'queue.plan.repeat' : 'queue.plan.current')),
    node('p', '', t('queue.plan.conditions')), node('p', '', t('queue.plan.signals')));
  if (g.rows.some(i => freshPlan(i)?.stage === 'first-send')) details.append(node('p', '', t('queue.plan.firstStep')));
  if (g.head.expected_process) details.append(node('p', '', t('queue.plan.expected', { process: g.head.expected_process })));
  if (g.head.last_context) details.append(node('p', '', t('queue.plan.targetTime', {
    time: new Date(g.head.last_context.checked_at * 1000).toLocaleTimeString(),
  })));
  box.append(details);
  return box;
}

/* What holds for the session whatever the list: said once, above the lists.
   Interleaving exists only with more than one list; the hook observation is
   one per session and is evidence, never a send condition. */
export function sessionFacts(card, lists) {
  const box = node('div', 'q-session-facts');
  if (lists > 1) box.append(node('p', 'q-hint', t('queue.plan.others', { count: lists })));
  const observation = ctx.attention.get(card);
  const agentKeys = { working: 'queue.signal.working', 'needs-input': 'queue.signal.input', 'turn-done': 'queue.signal.done' };
  const state = t(observation?.stale ? 'queue.stage.unknown' : (agentKeys[observation?.agent] || 'queue.signal.none'));
  const details = disclosure(`${card.id}:observation`, 'q-plan-details', t('queue.observation', { state }));
  details.append(node('p', '', t('queue.plan.unverified')));
  box.append(details);
  return box;
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
  /* why it was sent: an approval (step and class only) and who sent it */
  const why = d => [
    d.authority ? t(d.authority.class === 'bounded' ? 'queue.history.approvedMessage' : 'queue.history.approved', { step: d.authority.step + 1 }) : null,
    d.manual ? t('queue.history.manual') : null,
  ].filter(Boolean).map(text => ` · ${text}`).join('');
  for (const d of entries) details.append(node('p', 'q-history-record', `${new Date(d.at * 1000).toLocaleString()} · ${t(d.type)}${why(d)} · ${d.id}`));
  if (!entries.length) details.append(node('p', 'q-hint', t('queue.history.empty')));
  return details;
}
