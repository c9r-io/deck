// slack-mentions.js — the Slack mentions view: messages that @mention the user
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Backend-owned list. Rows come from `slack_mentions` (slack_mentions.rs):
//   the open mentions of the last week, newest first, with the state of
//   the last search, and for each mention, once read, its context (its
//   thread's first message and the replies just before it, or the
//   conversation's messages before it, with names and file names). The
//   content-free `slack-mentions-changed` event and opening the view (which
//   also asks for a fresh search) are the only pulls; nothing is cached
//   across launches here.
// - Range. Three buttons above the list choose how far back it reaches
//   (24 hours, 3 days, a week), each with its count (`mentionRanges`); the
//   choice starts at 24 hours each launch and is never saved.
// - Read here, decide here. Two panes: the list on the left (author, where,
//   when, the first line), the selected mention on the right in full with
//   its context and its three actions, so the user can judge a request
//   without opening Slack. ↑/↓ in the list move the selection; after a route
//   or a dismissal the next mention is selected.
// - Slack stays silent. Nothing in this view writes to Slack. "Open in
//   Slack" opens the message's permalink in the browser.
// - Routing is one choice: the menu lists the user's Slack badge rules
//   (slack-mentions-model.js `mentionRoutes`); choosing one calls
//   `slack_mention_route`, which hands the message to the dispatcher as that
//   rule's badge item. The card, its steps and the "created" notice then
//   come from inbound.js exactly as for a badge added in Slack; the context
//   shown here is for the reader and is not part of what is routed.
// - Dismissing sets a mention aside for good (`slack_mention_dismiss`):
//   it starts nothing and never stops a badge the user adds in Slack later.
//   "Dismiss all" dismisses the mentions shown (the chosen range) and asks once.
// - Handed over: mentions an MCP client handed to one of its Agent rules
//   are listed apart above the panes, with the client's and the rule's names
//   (`handedRows`; client names from `mcp_status`; the orchestrator card's
//   title instead while the card it handed over from is on the Board,
//   `workers.js`). They are not actionable
//   here: the card the rule started is on the Board.
// - Attention: the open count of the chosen range beside the sidebar
//   entry, nothing else. The
//   entry is hidden while Slack is not connected.
// - A leaf of the import graph: the view actions it calls (`render`,
//   `switchProject`, `leaveSessionView`, `closeBuffer`, `provider`) arrive
//   through `initMentions(deps)` from app.js, so board.js may import it.
import { $, ctx, inv, listen, state, store, uev } from './state.js';
import { formatDateTime, formatNumber, t } from './i18n.js';
import { confirmDialog, toast } from './dialogs.js';
import { channelBlockReason } from './channel-model.js';
import { workerOf } from './workers.js';
import { MENTION_RANGES, handedRows, mentionAfter, mentionErrorKey, mentionRanges, mentionRoutes, mentionSignature, mentionStatus, mentionsWithin } from './slack-mentions-model.js';

let render, switchProject, leaveSessionView, closeBuffer, provider;
let view = null;
let selected = null;
let listSignature = '';
let detailSignature = '';
let started = null;
let range = MENTION_RANGES[0];
let clients = [];
let handedSignature = '';

const DISMISS_MAX = 400;
const RANGE_LABEL = { 1: 'mentions.range1', 3: 'mentions.range3', 7: 'mentions.range7' };
const when = at => formatDateTime(new Date(at * 1000),
  { weekday: 'short', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });

const routes = () => mentionRoutes(ctx.settings?.inbound?.rules, store.projects, channelBlockReason);
const nowSecs = () => Math.floor(Date.now() / 1000);
const items = () => mentionsWithin(view, range, nowSecs());
const current = () => items().find(item => item.key === selected) || null;

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

/* one message of the reader pane: who, when, the text as written, its files */
function message(className, from, at, text, files) {
  const node = el('div', className);
  const meta = el('div', 'mention-meta');
  meta.append(el('span', 'mention-from', from), el('span', 'mention-time', when(at)));
  node.append(meta, el('p', 'mention-body', text));
  if (files?.length) node.appendChild(el('p', 'mention-files', t('mentions.files', { names: files.join(', ') })));
  return node;
}

function listRow(item) {
  const node = el('button', 'mention-row');
  node.type = 'button';
  node.dataset.key = item.key;
  node.setAttribute('role', 'option');
  const meta = el('div', 'mention-meta');
  meta.append(el('span', 'mention-from', item.from), el('span', 'mention-where', item.where), el('span', 'mention-time', when(item.at)));
  node.append(meta, el('div', 'mention-summary', item.text.split('\n')[0]));
  node.onclick = () => select(item.key, true);
  return node;
}

function paintDetail(choices) {
  const item = current();
  const next = JSON.stringify([item, choices.map(route => [route.id, route.label, route.disabled])]);
  if (next === detailSignature) return;
  detailSignature = next;
  const pane = $('mentions-detail');
  if (!item) { pane.replaceChildren(el('p', 'mention-empty', t(view?.enabled ? 'mentions.empty' : 'mentions.off'))); return; }
  const head = el('div', 'mention-meta mention-detail-head');
  head.append(el('span', 'mention-from', item.from), el('span', 'mention-where', item.where), el('span', 'mention-time', when(item.at)));
  const parts = [head];
  const context = item.context;
  if (!context) parts.push(el('p', 'mention-loading', t('mentions.contextLoading')));
  else if (context.before.length) {
    parts.push(el('h2', 'mention-section', t(context.thread ? 'mentions.contextThread' : 'mentions.contextBefore')));
    for (const m of context.before) parts.push(message('mention-context', m.from, m.at, m.text, m.files));
  }
  parts.push(el('h2', 'mention-section', t('mentions.theMention')));
  parts.push(message('mention-message', item.from, item.at, item.text, context?.files));
  const actions = el('div', 'mention-actions');
  const slack = el('button', 'btn mention-open', t('mentions.open'));
  slack.type = 'button';
  slack.disabled = !item.link;
  slack.onclick = () => inv('open_target', { kind: 'url', value: item.link, cwd: ctx.HOME }).catch(() => toast(t('mentions.openFailed')));
  const dismiss = el('button', 'btn mention-dismiss', t('mentions.dismiss'));
  dismiss.type = 'button';
  dismiss.onclick = () => dismissMentions([item.key]);
  actions.append(slack, routeMenu(item, choices), dismiss);
  parts.push(actions);
  pane.replaceChildren(...parts);
  pane.scrollTop = 0;
}

function paintSelection() {
  for (const node of $('mentions-list').children) {
    if (!node.dataset?.key) continue;
    const on = node.dataset.key === selected;
    node.setAttribute('aria-selected', String(on));
    node.dataset.selected = String(on);
  }
}

function select(key, focus = false) {
  selected = key;
  paintSelection();
  paintDetail(routes());
  if (focus) $('mentions-list').focus?.({ preventScroll: true });
}

/* the sidebar entry and, while the view is shown, both panes */
export function paintMentions() {
  const count = items().length;
  const btn = $('mentions-btn');
  btn.hidden = !view?.enabled;
  btn.classList.toggle('active', state.view === 'mentions');
  btn.setAttribute('aria-pressed', String(state.view === 'mentions'));
  $('mentions-count').textContent = count ? formatNumber(count) : '';
  $('mentions-view').hidden = state.view !== 'mentions';
  if (state.view !== 'mentions') return;
  $('mentions-back').textContent = t('attention.backProject', { name: provider.project(state.projectId)?.name || '' });
  for (const { days, count: n } of mentionRanges(view, nowSecs())) {
    const button = $(`mentions-range-${days}`);
    button.textContent = t(RANGE_LABEL[days], { count: formatNumber(n) });
    button.setAttribute('aria-pressed', String(days === range));
  }
  const status = mentionStatus(view);
  $('mentions-status').textContent = t(status.key, status.params.at
    ? { time: formatDateTime(new Date(status.params.at * 1000)) } : status.params);
  $('mentions-dismiss-all').disabled = !count;
  const choices = routes();
  $('mentions-no-routes').hidden = !view?.enabled || choices.length > 0;
  if (!current()) selected = items()[0]?.key || null;
  const next = mentionSignature(items(), choices);
  if (next !== listSignature) {
    listSignature = next;
    $('mentions-list').replaceChildren(...items().map(listRow));
  }
  paintSelection();
  paintDetail(choices);
  paintHanded();
}

/* where the mentions an MCP client took went */
function paintHanded() {
  const ownerOf = card => {
    const owner = card && workerOf(card)?.orchestratorCardId;
    return (owner && store.cards.find(c => c.id === owner)?.title) || '';
  };
  const rows = handedRows(view, range, nowSecs(), clients, ctx.settings?.inbound?.rules, ownerOf);
  const box = $('mentions-handed');
  box.hidden = !rows.length;
  $('mentions-handed-title').textContent = t('mentions.handedTitle', { count: formatNumber(rows.length) });
  const next = JSON.stringify(rows);
  if (next === handedSignature) return;
  handedSignature = next;
  $('mentions-handed-list').replaceChildren(...rows.map(row => {
    const node = el('div', 'mention-handed');
    node.append(el('span', 'mention-from', row.from), el('span', 'mention-where', row.where),
      el('span', 'mention-time', when(row.at)), el('span', 'mention-summary', row.line),
      el('span', 'mention-handed-to', t('mentions.handedTo', { client: row.client, rule: row.rule })));
    return node;
  }));
}

export async function refreshMentions() {
  try { view = await inv('slack_mentions'); } catch (_) { return; }
  if (view?.handed?.length) {
    clients = await inv('mcp_status').then(status => status?.clients || []).catch(() => clients);
  }
  paintMentions();
}

/* the mentions leave the list; the next one is selected */
function drop(keys) {
  if (!view?.items) return;
  if (keys.includes(selected)) selected = mentionAfter(items(), keys);
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
  const keys = items().map(item => item.key);
  if (!keys.length) return;
  if (!(await confirmDialog(t('mentions.dismissAllConfirm', { count: formatNumber(keys.length) })))) return;
  await dismissMentions(keys);
}

function onListKey(event) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  const list = items();
  if (!list.length) return;
  event.preventDefault();
  const at = list.findIndex(item => item.key === selected);
  const step = event.key === 'ArrowDown' ? 1 : -1;
  const next = list[Math.max(0, Math.min(list.length - 1, at + step))];
  select(next.key);
  [...$('mentions-list').children].find(node => node.dataset?.key === next.key)?.scrollIntoView?.({ block: 'nearest' });
}

export function showMentions() {
  closeBuffer();
  if (state.view === 'session') leaveSessionView();
  state.view = 'mentions';
  state.sessionId = null;
  listSignature = '';
  detailSignature = '';
  render();
  $('mentions-list').focus?.({ preventScroll: true });
  inv('slack_mentions_refresh').catch(() => {});
  refreshMentions();
}

export function initMentions(deps) {
  ({ render, switchProject, leaveSessionView, closeBuffer, provider } = deps);
  $('mentions-btn').onclick = () => showMentions();
  $('mentions-back').onclick = () => switchProject(state.projectId);
  $('mentions-dismiss-all').onclick = () => dismissAll();
  $('mentions-list').onkeydown = onListKey;
  for (const days of MENTION_RANGES) {
    $(`mentions-range-${days}`).onclick = () => { range = days; paintMentions(); };
  }
}

/* the listener and the first pull; called once the Board is the user's */
export function startMentions() {
  if (started) return started;
  started = listen('slack-mentions-changed', refreshMentions)
    .catch(() => uev('listen-fail', 'slack-mentions-changed'))
    .then(refreshMentions);
  return started;
}
