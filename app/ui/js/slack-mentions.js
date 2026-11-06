// slack-mentions.js — the Slack mentions view: messages that @mention the user
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Backend-owned list. Rows come from `slack_mentions` (slack_mentions.rs):
//   the open mentions of the last 30 days, newest first, with the state of
//   the last search. The content-free `slack-mentions-changed` event and
//   opening the view (which also asks for a fresh search) are the only
//   pulls; nothing is cached across launches here.
// - Slack stays silent. Nothing in this view writes to Slack. "Open in
//   Slack" opens the message's permalink in the browser.
// - Routing is one choice: the row's menu lists the user's Slack badge rules
//   (slack-mentions-model.js `mentionRoutes`); choosing one calls
//   `slack_mention_route`, which hands the message to the dispatcher as that
//   rule's badge item. The card, its steps and the "created" notice then
//   come from inbound.js exactly as for a badge added in Slack. The row
//   leaves the list at once; a refusal says why and leaves it.
// - Dismissing sets a mention aside for good (`slack_mention_dismiss`):
//   it starts nothing and never stops a badge the user adds in Slack later.
//   "Dismiss all" asks once.
// - Attention: the open count beside the sidebar entry, nothing else. The
//   entry is hidden while Slack is not connected.
// - A leaf of the import graph: the view actions it calls (`render`,
//   `switchProject`, `leaveSessionView`, `closeBuffer`, `provider`) arrive
//   through `initMentions(deps)` from app.js, so board.js may import it.
import { $, ctx, inv, listen, state, store, uev } from './state.js';
import { formatDateTime, formatNumber, t } from './i18n.js';
import { confirmDialog, toast } from './dialogs.js';
import { channelBlockReason } from './channel-model.js';
import { mentionCount, mentionErrorKey, mentionRoutes, mentionSignature, mentionStatus } from './slack-mentions-model.js';

let render, switchProject, leaveSessionView, closeBuffer, provider;
let view = null;
let signature = '';
let started = null;
const expanded = new Set();

const DISMISS_MAX = 400;
const when = at => formatDateTime(new Date(at * 1000),
  { weekday: 'short', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });

const routes = () => mentionRoutes(ctx.settings?.inbound?.rules, store.projects, channelBlockReason);

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function routeMenu(item, choices) {
  const select = el('select', 'mention-route');
  select.setAttribute('aria-label', t('mentions.routeNamed', { from: item.from }));
  const head = el('option', '', t('mentions.route'));
  head.value = '';
  select.appendChild(head);
  for (const route of choices) {
    const option = el('option', '', route.disabled ? t('mentions.unavailable', { label: route.label }) : route.label);
    option.value = route.id;
    option.disabled = route.disabled;
    select.appendChild(option);
  }
  select.disabled = !choices.some(route => !route.disabled);
  select.value = '';
  select.onchange = () => { const rule = select.value; select.value = ''; if (rule) routeMention(item, rule); };
  return select;
}

function row(item, choices) {
  const node = el('div', 'mention-row');
  node.dataset.key = item.key;
  const meta = el('div', 'mention-meta');
  meta.append(el('span', 'mention-from', item.from), el('span', 'mention-where', item.where), el('span', 'mention-time', when(item.at)));
  const text = el('button', 'mention-text', item.text);
  text.type = 'button';
  const open = expanded.has(item.key);
  text.dataset.expanded = String(open);
  text.setAttribute('aria-expanded', String(open));
  text.onclick = () => {
    const next = !expanded.has(item.key);
    if (next) expanded.add(item.key); else expanded.delete(item.key);
    text.dataset.expanded = String(next);
    text.setAttribute('aria-expanded', String(next));
  };
  const actions = el('div', 'mention-actions');
  const slack = el('button', 'btn mention-open', t('mentions.open'));
  slack.type = 'button';
  slack.disabled = !item.link;
  slack.onclick = () => inv('open_target', { kind: 'url', value: item.link, cwd: ctx.HOME }).catch(() => toast(t('mentions.openFailed')));
  const dismiss = el('button', 'btn mention-dismiss', t('mentions.dismiss'));
  dismiss.type = 'button';
  dismiss.onclick = () => dismissMentions([item.key]);
  actions.append(slack, routeMenu(item, choices), dismiss);
  node.append(meta, text, actions);
  return node;
}

/* the sidebar entry and, while the view is shown, the list */
export function paintMentions() {
  const count = mentionCount(view);
  const btn = $('mentions-btn');
  btn.hidden = !view?.enabled;
  btn.classList.toggle('active', state.view === 'mentions');
  btn.setAttribute('aria-pressed', String(state.view === 'mentions'));
  $('mentions-count').textContent = count ? formatNumber(count) : '';
  $('mentions-view').hidden = state.view !== 'mentions';
  if (state.view !== 'mentions') return;
  $('mentions-back').textContent = t('attention.backProject', { name: provider.project(state.projectId)?.name || '' });
  const status = mentionStatus(view);
  $('mentions-status').textContent = t(status.key, status.params.at
    ? { time: formatDateTime(new Date(status.params.at * 1000)) } : status.params);
  $('mentions-dismiss-all').disabled = !count;
  const choices = routes();
  $('mentions-no-routes').hidden = !view?.enabled || choices.length > 0;
  const next = mentionSignature(view, choices);
  if (next === signature) return;
  signature = next;
  const list = $('mentions-list');
  const items = count ? view.items : [];
  for (const key of [...expanded]) if (!items.some(item => item.key === key)) expanded.delete(key);
  list.replaceChildren(...(items.length ? items.map(item => row(item, choices))
    : [el('p', 'mention-empty', t(view?.enabled ? 'mentions.empty' : 'mentions.off'))]));
}

export async function refreshMentions() {
  try { view = await inv('slack_mentions'); } catch (_) { return; }
  paintMentions();
}

function drop(keys) {
  if (!view?.items) return;
  view = { ...view, items: view.items.filter(item => !keys.includes(item.key)) };
  paintMentions();
}

async function routeMention(item, rule) {
  try { await inv('slack_mention_route', { key: item.key, rule }); }
  catch (error) {
    uev('mentions', 'route-refused');
    toast(t(mentionErrorKey(error), { code: String(error) }));
    refreshMentions();
    return;
  }
  uev('mentions', 'routed');
  drop([item.key]);
}

async function dismissMentions(keys) {
  try { await inv('slack_mention_dismiss', { keys: keys.slice(0, DISMISS_MAX) }); }
  catch (error) { toast(t(mentionErrorKey(error), { code: String(error) })); return; }
  uev('mentions', 'dismissed');
  drop(keys);
}

async function dismissAll() {
  const keys = (view?.items || []).map(item => item.key);
  if (!keys.length) return;
  if (!(await confirmDialog(t('mentions.dismissAllConfirm', { count: formatNumber(keys.length) })))) return;
  await dismissMentions(keys);
}

export function showMentions() {
  closeBuffer();
  if (state.view === 'session') leaveSessionView();
  state.view = 'mentions';
  state.sessionId = null;
  signature = '';
  render();
  $('mentions-title').focus?.({ preventScroll: true });
  inv('slack_mentions_refresh').catch(() => {});
  refreshMentions();
}

export function initMentions(deps) {
  ({ render, switchProject, leaveSessionView, closeBuffer, provider } = deps);
  $('mentions-btn').onclick = () => showMentions();
  $('mentions-back').onclick = () => switchProject(state.projectId);
  $('mentions-dismiss-all').onclick = () => dismissAll();
}

/* the listener and the first pull; called once the Board is the user's */
export function startMentions() {
  if (started) return started;
  started = listen('slack-mentions-changed', refreshMentions)
    .catch(() => uev('listen-fail', 'slack-mentions-changed'))
    .then(refreshMentions);
  return started;
}
