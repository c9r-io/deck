// Reviewed templates enter in one queue transaction; an opted-in run needs
// an explicit final inspection even if enqueue fails. No hook releases it.
// inbound.js — 自动化 dispatch: turn a fired trigger into a card + queued prompts
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// The backend (inbound.rs) only says "something is pending" (content-free
// event). This module pulls the items, decides with the pure planner, creates
// the card through the ordinary Board transaction, enqueues the rule's
// template through the ordinary queue, and acks only after every row is
// queued. The frozen plan lives on the card until then: a queue failure or
// restart replays stable operation IDs on the same card, never a second card.
// A clock item
// (automation.js owns the rules) is the same path with two differences: a
// slot whose rule still has a card on the Board is acked `busy`, and a
// created run's ack carries the card id so the backend's run ledger can be
// closed later by `inbound_run_ended`. A badge item's ack carries the card
// id too, so a badge-started run is finished (and listed) like a clock one.
// A badge item's text is someone else's Slack message, so it passes the
// channel admission (`channelBlockReason`, acked `blocked` before any card
// exists) and is queued through the native agent-only gate
// (`channel_queue_add*`); a clock item's text is the user's own template.
// A badge rule the user approved for automatic sending (automation-model.js
// `grantState` is `valid` for the CURRENT rule and template) freezes that
// approval into the plan — `{rule, grant, trigger, classes}`, ids and closed
// words only — and each queued row carries its step's claim; the backend
// re-checks every claim against settings (scheduler/authority.rs) and the
// rows stay `external` either way. A later rule edit changes future runs
// only; revoking the approval stops this run's unsent rows too.
import { ctx, genId, inv, listen, store, uev } from './state.js';
import { provider } from './board.js';
import { toast } from './dialogs.js';
import { planInbound } from './pure.js';
import { t } from './i18n.js';
import { bufferLimitError, emptyBuffer, upsertExternal } from './buffer-model.js';
import { channelBlockReason, channelDigestId, channelRunExpired, channelSource, channelTemplatePlan, collectingCard, unfinishedChannelPlans } from './channel-model.js';
import { expandHome, normalizeTemplateStep } from './pure.js';
import { grantState } from './automation-model.js';

let draining = false;
let again = false;
let channelDraining = false;
let channelAgain = false;

async function reconcileInboundCard(card) {
  const plan = card.inboundPlan;
  if (!plan || plan.initialQueued) return true;
  try { return await provider.queueInboundPlan(card.id, card.origin.key); }
  catch (_) {
    toast(t('inbound.planPending')); uev('inbound', 'queue-fail'); return false;
  }
}

async function reconcileChannelCard(card) {
  const run = card.channelRun;
  if (!run || run.initialQueued || !Array.isArray(run.initialSteps)) return true;
  try {
    return await provider.queueChannelPlan(card.id, run.groupKey);
  } catch (_) {
    toast(t('channel.queueFailed')); uev('inbound', 'channel-queue-fail'); return false;
  }
}

async function ackChannel(id) {
  try { await inv('channel_ack', { id }); return true; }
  catch (_) { uev('inbound', 'ack-fail'); return false; }
}

async function handleChannel(item) {
  const exact = store.cards.find(card => card.origin?.source === 'channel' && card.origin.key === item.operationKey);
  if (exact) {
    await ackChannel(item.id);
    await reconcileChannelCard(exact);
    return;
  }
  for (const card of [...store.cards]) {
    if (channelRunExpired(card.channelRun, Math.floor(Date.now() / 1000))) {
      try { await provider.setChannelRun(card.id, card.channelRun.groupKey, { collecting: false }); }
      catch (_) { toast(t('channel.expirySaveFailed')); return; }
    }
  }
  let card = collectingCard(store.cards, item);
  const entryId = await channelDigestId('E', item.operationKey);
  if (card) {
    const base = card.buffer || emptyBuffer();
    const added = upsertExternal(base, { id: entryId, text: item.body, source: channelSource(item), now: item.occurredAt * 1000 });
    if (added.error === 'immutable') { toast(t('channel.eventConflict')); return; }
    if (added.error || bufferLimitError(added.buffer)) { toast(t('channel.bufferFull')); return; }
    if (!added.noop) {
      try { await provider.appendChannelEvent(card.id, base.revision || 0, item.groupKey, added.buffer, Math.floor(Date.now() / 1000)); }
      catch (_) { channelAgain = true; return; }
    }
    await ackChannel(item.id);
    return;
  }
  const project = store.projects.find(value => value.id === item.target.projectId);
  const column = project?.columns.find(value => value.id === item.target.columnId);
  const plan = channelTemplatePlan(item, project, Math.floor(Date.now() / 1000));
  if (!project || !column || plan.error) {
    toast(t({ template: 'channel.noTemplate', command: 'channel.blockedCommand',
      'template-leading-message': 'channel.blockedTemplate' }[plan.error] || 'channel.noTarget'));
    return;
  }
  const operationIds = await Promise.all(plan.texts.map((_, index) => channelDigestId('B', `${item.operationKey}/step/${index}`)));
  const initialSteps = plan.texts.map((text, index) => ({ operationId: operationIds[index], text,
    mode: index ? 'chain' : 'at', at: index ? null : plan.at,
    tpl: plan.template, tplIdx: index + 1, tplTotal: plan.texts.length }));
  const added = upsertExternal(emptyBuffer(), { id: entryId, text: item.body, source: channelSource(item), now: item.occurredAt * 1000 });
  if (added.error || bufferLimitError(added.buffer)) { toast(t('channel.bufferFull')); return; }
  added.buffer.collecting = true;
  const id = await channelDigestId('S', item.operationKey);
  const channelRun = { groupKey: item.groupKey, firstEventId: item.eventId, connectionId: item.connectionId,
    workspaceId: item.workspaceId, channelId: item.channelId, ruleId: item.ruleId,
    lastCollectedAt: Math.floor(Date.now() / 1000), idleMinutes: item.target.idleMinutes, collecting: true,
    initialSteps, initialQueued: false };
  try {
    ({ card } = await provider.createStarted({ id, projectId: project.id, columnId: column.id,
      title: plan.title, dir: expandHome(item.target.dir, ctx.HOME), cmd: item.target.cmd,
      desc: plan.template, origin: { source: 'channel', key: item.operationKey, badge: item.ruleId },
      buffer: added.buffer, channelRun }, { requireCreated: true }));
  } catch (error) {
    toast(t(error?.stage === 'orphan' ? 'channel.orphan' : 'channel.createFailed'));
    return;
  }
  await ackChannel(item.id);
  await reconcileChannelCard(card);
}

export async function drainChannel() {
  if (channelDraining) { channelAgain = true; return; }
  channelDraining = true;
  try {
    do {
      channelAgain = false;
      for (const card of unfinishedChannelPlans(store.cards)) await reconcileChannelCard(card);
      let items;
      try { items = await inv('channel_pending'); } catch (_) { return; }
      for (const item of items || []) await handleChannel(item);
    } while (channelAgain);
  } finally { channelDraining = false; }
}

export async function drainInbound() {
  if (draining) { again = true; return; }
  draining = true;
  try {
    do {
      again = false;
      for (const card of store.cards.filter(value => value.inboundPlan && !value.inboundPlan.initialQueued)) {
        await reconcileInboundCard(card);
      }
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

/* the approval this run may carry: only a Slack badge rule's, only while it
   is valid for the rule and template as they are NOW, and only when every
   template step became one queued step (so step k is template step k) */
async function frozenApproval(item, plan) {
  const rule = item.rule;
  if (rule?.source !== 'slack' || !rule.autoSend) return null;
  const project = store.projects.find(value => value.id === rule.projectId);
  const template = (project?.templates || []).find(value => value.name === plan.template);
  if (!template || template.steps.length !== plan.steps.length) return null;
  if ((await grantState(rule, template)) !== 'valid') return null;
  /* proof material for bounded steps: the event the run was made from and
     each such step's approved skeleton; the backend re-expands the skeleton
     over its own copy of the event and compares bytes */
  const skeletons = rule.autoSend.classes.map((cls, k) => (cls === 'bounded' ? normalizeTemplateStep(template.steps[k]) : null));
  return { rule: rule.id, grant: rule.autoSend.digest, trigger: 'slack-badge', classes: [...rule.autoSend.classes],
    event: item.event.key, skeletons };
}

const ruleLabel = item => (item.event.source === 'clock' ? (item.rule.name || item.rule.id) : `:${item.event.badge}:`);

async function handleInbound(item) {
  const plan = planInbound(item, { cards: store.cards, projects: store.projects, home: ctx.HOME });
  const badge = item.event.badge;
  const clock = item.event.source === 'clock';
  const skip = reason => ack(item.id, 'skipped', reason, clock ? { reason } : {});
  if (plan.outcome === 'duplicate') {
    if (!(await reconcileInboundCard(plan.card))) return;
    return ack(item.id, 'done', 'duplicate', { card: plan.card.id });
  }
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
  /* a badge carries someone else's Slack message: the channel admission
     applies (an agent command, no line led by a placeholder) */
  const blocked = !clock && channelBlockReason(item.rule, store.projects.find(p => p.id === item.rule.projectId));
  if (blocked) {
    toast(t(blocked === 'command' ? 'inbound.blockedCommand' : 'inbound.blockedTemplate', { badge }));
    return skip('blocked');
  }
  const now = Math.floor(Date.now() / 1000);
  const authority = clock ? null : await frozenApproval(item, plan);
  const cardId = genId('S');
  const operationId = await channelDigestId('B', `${cardId}/list`);
  const initialSteps = await Promise.all(plan.steps.map(async (text, index) => ({
    operationId: await channelDigestId('B', `${cardId}/step/${index}`),
    text, mode: index ? 'chain' : 'at', at: index ? null : now,
    tpl: plan.template, tplIdx: index + 1, tplTotal: plan.steps.length,
  })));
  let card;
  try {
    card = await provider.create({ ...plan.card, id: cardId, inboundPlan: {
      operationId, reviewEach: item.rule.reviewEach === true, initialSteps, initialQueued: false,
      ...(authority ? { authority } : {}),
    } });
  } catch (e) {
    toast(t('inbound.createFailed', { badge }));
    uev('inbound', 'create-fail');
    return;   // stays pending; the backend announces it again
  }
  if (!(await reconcileInboundCard(card))) return;
  toast(clock ? t('automation.created', { name: card.title }) : t('inbound.created', { badge, where: item.event.where }));
  return ack(item.id, 'done', 'created', { card: card.id });
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initInbound() {
  listen('channel-changed', drainChannel).catch(() => uev('listen-fail', 'channel-changed'));
  listen('inbound-changed', drainInbound).catch(() => uev('listen-fail', 'inbound-changed'));
  const timer = setInterval(() => { drainChannel(); drainInbound(); }, 60_000);
  timer.unref?.();
}
