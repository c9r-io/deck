// inbound.js — 自动响应: turn backend-detected badges into cards + queued prompts
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// The backend (inbound.rs) only says "something is pending" (content-free
// event). This module pulls the items, decides with the pure planner, creates
// the card through the ordinary Board transaction, enqueues the rule's
// template through the ordinary queue, and acks. Acks are what retire an
// item; an item whose card could not be created is left pending and the
// backend re-announces it, while a duplicate (card already exists) is acked
// without a second card — so a retry can never double-create.
import { inv, listen, store, uev } from './state.js';
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

async function ack(id, outcome, code) {
  uev('inbound', code);
  try { await inv('inbound_ack', { id, outcome }); }
  catch (e) { uev('inbound', 'ack-fail'); }
}

async function handleInbound(item) {
  const plan = planInbound(item, { cards: store.cards, projects: store.projects, home: HOME });
  const badge = item.event.badge;
  if (plan.outcome === 'duplicate') return ack(item.id, 'done', 'duplicate');
  if (plan.outcome === 'no-rule-target') {
    toast(t('inbound.noTarget', { badge }));
    return ack(item.id, 'skipped', 'no-rule-target');
  }
  if (plan.outcome === 'no-template') {
    toast(t('inbound.noTemplate', { badge, template: plan.template }));
    return ack(item.id, 'skipped', 'no-template');
  }
  let card;
  try {
    card = await provider.create(plan.card);
  } catch (e) {
    toast(t('inbound.createFailed', { badge }));
    uev('inbound', 'create-fail');
    return;   // stays pending; the backend announces it again
  }
  const base = { session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd };
  const now = Math.floor(Date.now() / 1000);
  let queued = 0;
  try {
    for (let k = 0; k < plan.steps.length; k++) {
      await inv('queue_add', { args: { ...base, text: plan.steps[k],
        mode: k === 0 ? 'at' : 'chain', at: k === 0 ? now : null,
        tpl: plan.template, tplIdx: k + 1, tplTotal: plan.steps.length } });
      queued++;
    }
  } catch (e) {
    toast(t('inbound.queueFailed', { badge, queued, total: plan.steps.length }));
    uev('inbound', 'queue-fail');
  }
  if (queued === plan.steps.length) toast(t('inbound.created', { badge, where: item.event.where }));
  return ack(item.id, 'done', 'created');
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initInbound() {
  listen('inbound-changed', drainInbound).catch(() => uev('listen-fail', 'inbound-changed'));
}
