// Reviewed templates enter in one queue transaction; an opted-in run needs
// an explicit final inspection even if enqueue fails. No hook releases it.
// inbound.js — 自动化 dispatch: turn a fired trigger into a card + queued prompts
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// The backend (inbound.rs) only says "something is pending" (content-free
// event). This module pulls the items, decides with the pure planner, creates
// the card through the ordinary Board transaction, enqueues the rule's
// template through the ordinary queue, and acks. Acks are what retire an
// item; an item whose card could not be created is left pending and the
// backend re-announces it, while a duplicate (card already exists) is acked
// without a second card — so a retry can never double-create. A clock item
// (automation.js owns the rules) is the same path with two differences: a
// slot whose rule still has a card on the Board is acked `busy`, and a
// created run's ack carries the card id so the backend's run ledger can be
// closed later by `inbound_run_ended`. A badge item's ack carries the card
// id too, so a badge-started run is finished (and listed) like a clock one.
import { ctx, inv, listen, store, uev } from './state.js';
import { provider } from './board.js';
import { toast } from './dialogs.js';
import { planInbound } from './pure.js';
import { t } from './i18n.js';

let draining = false;
let again = false;

export async function drainInbound() {
  if (draining) { again = true; return; }
  draining = true;
  try {
    do {
      again = false;
      let items = [];
      try { items = await inv('inbound_pending'); } catch (e) { return; }
      for (const item of items || []) await handleInbound(item);
    } while (again);
  } finally {
    draining = false;
  }
}

async function ack(id, outcome, code, extra = {}) {
  uev('inbound', code);
  try { await inv('inbound_ack', { id, outcome, ...extra }); }
  catch (e) { uev('inbound', 'ack-fail'); }
}

const ruleLabel = item => (item.event.source === 'clock' ? (item.rule.name || item.rule.id) : `:${item.event.badge}:`);

async function handleInbound(item) {
  const plan = planInbound(item, { cards: store.cards, projects: store.projects, home: ctx.HOME });
  const badge = item.event.badge;
  const clock = item.event.source === 'clock';
  const skip = reason => ack(item.id, 'skipped', reason, clock ? { reason } : {});
  if (plan.outcome === 'duplicate') return ack(item.id, 'done', 'duplicate');
  if (plan.outcome === 'busy') {
    toast(t('automation.busy', { name: ruleLabel(item) }));
    return skip('busy');
  }
  if (plan.outcome === 'no-rule-target') {
    toast(t('inbound.noTarget', { badge: ruleLabel(item) }));
    return skip('no-rule-target');
  }
  if (plan.outcome === 'no-template') {
    toast(t('inbound.noTemplate', { badge: ruleLabel(item), template: plan.template }));
    return skip('no-template');
  }
  let card;
  try {
    card = await provider.create(plan.card);
  } catch (e) {
    toast(t('inbound.createFailed', { badge }));
    uev('inbound', 'create-fail');
    return;   // stays pending; the backend announces it again
  }
  const base = { session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd, reviewEach: item.rule.reviewEach === true };
  const now = Math.floor(Date.now() / 1000);
  let queued = 0;
  try {
    if (base.reviewEach) {
      await inv('queue_add_reviewed_list', { args: { ...base, text: plan.steps[0], mode: 'at', at: now,
        tpl: plan.template, tplIdx: 1, tplTotal: plan.steps.length }, texts: plan.steps });
      queued = plan.steps.length;
    } else for (let k = 0; k < plan.steps.length; k++) {
      await inv('queue_add', { args: { ...base, text: plan.steps[k],
        mode: k === 0 ? 'at' : 'chain', at: k === 0 ? now : null,
        tpl: plan.template, tplIdx: k + 1, tplTotal: plan.steps.length } });
      queued++;
    }
  } catch (e) {
    toast(t('inbound.queueFailed', { badge, queued, total: plan.steps.length }));
    uev('inbound', 'queue-fail');
  }
  if (queued === plan.steps.length) {
    toast(clock ? t('automation.created', { name: card.title }) : t('inbound.created', { badge, where: item.event.where }));
  }
  return ack(item.id, 'done', 'created', { card: card.id });
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initInbound() {
  listen('inbound-changed', drainInbound).catch(() => uev('listen-fail', 'inbound-changed'));
}
