// notices.js — the Notifications view: local notifications and Slack mentions
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Backend-owned lists (notices-model.js): `local_notifications` (the
//   durable inbox, local_notify.rs) and `slack_mentions` (memory). The
//   content-free `local-notifications-changed` and `slack-mentions-changed`
//   events, a 30 s tick while the view is shown, and opening the view are
//   the only pulls; nothing is cached across launches here.
// - Intake: "Accept local notifications" is the switch
//   `inbound.sources.local.enabled`, saved through the ONE settings writer
//   (`persistInbound`); off is stored as absent. Below it, the bundled
//   `deck-notify`'s path (copy it, nothing is installed on PATH) and, per
//   producer, when Deck last received from it and how many in 24 hours.
//   No receipt is not "working" or "broken": producers report no health.
// - Read here, decide here. The list on the left (needs-you rows first, then
//   the settled ones under a fold), the selected row on the right in full
//   with its actions. Text is shown as text (`textContent`), never as markup;
//   a link opens through `open_target`.
// - A local row: route it to one local notification rule
//   (`local_notification_route`, with the occurrence and revision shown, so
//   a notification that changed meanwhile is refused as stale instead of
//   routed unseen), or dismiss it (`local_notification_dismiss`). A rule
//   that takes it automatically does so in the backend; this view only
//   shows that it did. Resolving, dismissing or routing never moves or
//   closes a card, and the producer's resolve never cancels a run.
// - A Slack row is the Slack mentions view's mention: route it to a Slack
//   badge rule (`slack_mention_route`) or dismiss it
//   (`slack_mention_dismiss`), exactly as there; its context is read in the
//   Slack mentions view, which "Read in Slack mentions" opens.
// - Attention: the count of local rows that need the user beside the
//   sidebar entry, nothing else (Slack keeps its count on its own entry).
// - A leaf of the import graph: `render`, `switchProject`,
//   `leaveSessionView`, `closeBuffer`, `provider` and `showMentions` arrive
//   through `initNotices(deps)` from app.js.
import { $, ctx, inv, listen, state, store, uev } from './state.js';
import { formatDateTime, formatNumber, t } from './i18n.js';
import { toast } from './dialogs.js';
import { channelBlockReason } from './channel-model.js';
import { persistInbound } from './settings.js';
import { allRows, filtered, labelLine, localCount, localRows, noticeFilters, routeErrorKey, routesFor, rowsSignature, slackRows, stateWords } from './notices-model.js';
import { mentionErrorKey } from './slack-mentions-model.js';

let render, switchProject, leaveSessionView, closeBuffer, provider, showMentions;
let local = null;
let slack = null;
let filter = 'all';
let selected = null;
let showSettled = false;
let listSignature = '';
let detailSignature = '';
let started = null;
let busy = false;

const nowSecs = () => Math.floor(Date.now() / 1000);
const when = at => formatDateTime(new Date(at * 1000),
  { weekday: 'short', month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
const rules = () => ctx.settings?.inbound?.rules || [];
const ruleName = id => rules().find(rule => rule.id === id)?.name || id;
const rows = () => allRows(localRows(local), slackRows(slack, nowSecs()));
const shown = () => {
  const { open, settled } = filtered(rows(), filter);
  return showSettled ? [...open, ...settled] : open;
};
const current = () => shown().find(row => row.id === selected) || null;

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function words(row) {
  return stateWords(row, ruleName).map(w => t(w.key, {
    ...(w.params || {}),
    ...(w.params?.reason ? { reason: t(`automation.run.${w.params.reason}`) } : {}),
  }));
}

function listRow(row) {
  const node = el('button', 'mention-row');
  node.type = 'button';
  node.dataset.id = row.id;
  node.setAttribute('role', 'option');
  const meta = el('div', 'mention-meta');
  meta.append(el('span', 'mention-from', row.source === 'slack' ? t('notices.slack') : row.producer),
    el('span', 'mention-where', row.source === 'slack' ? row.from : row.type),
    el('span', 'mention-time', when(row.at)));
  node.append(meta, el('div', 'mention-summary', row.title));
  const tags = words(row);
  if (tags.length) node.appendChild(el('div', 'notice-tag', tags.join(' · ')));
  node.onclick = () => select(row.id, true);
  return node;
}

function routeMenu(row) {
  const choices = routesFor(row, rules(), store.projects, channelBlockReason);
  const select = el('select', 'mention-route');
  select.setAttribute('aria-label', t('notices.routeNamed', { title: row.title }));
  const head = el('option', '', t('mentions.route'));
  head.value = '';
  select.appendChild(head);
  for (const route of choices) {
    const label = route.fits ? t('notices.routeFits', { label: route.label }) : route.label;
    const option = el('option', '', route.disabled ? t('mentions.unavailable', { label }) : label);
    option.value = route.id;
    option.disabled = route.disabled;
    select.appendChild(option);
  }
  const takenAlready = row.source === 'local' && row.attention === 'routed';
  select.disabled = takenAlready || !choices.some(route => !route.disabled);
  select.value = '';
  select.onchange = () => { const rule = select.value; select.value = ''; if (rule) route(row, rule); };
  return select;
}

function paintDetail() {
  const row = current();
  const next = JSON.stringify([row, rules().map(rule => [rule.id, rule.name, rule.enabled]), local?.error]);
  if (next === detailSignature) return;
  detailSignature = next;
  const pane = $('notices-detail');
  if (!row) {
    pane.replaceChildren(el('p', 'mention-empty', t(local?.error ? 'notices.unreadable' : 'notices.empty')));
    return;
  }
  const head = el('div', 'mention-meta mention-detail-head');
  head.append(el('span', 'mention-from', row.source === 'slack' ? row.from : row.producer),
    el('span', 'mention-where', row.source === 'slack' ? row.where : row.type),
    el('span', 'mention-time', t('notices.received', { time: when(row.at) })));
  if (row.occurredAt) head.appendChild(el('span', 'mention-time', t('notices.occurred', { time: when(row.occurredAt) })));
  const parts = [head];
  const tags = words(row);
  if (tags.length) {
    const line = el('div', 'notice-state');
    for (const word of tags) line.appendChild(el('span', '', word));
    parts.push(line);
  }
  parts.push(el('h2', 'mention-section', row.title));
  if (row.body && row.body !== row.title) {
    const body = el('div', 'mention-message');
    body.appendChild(el('p', 'mention-body', row.body));
    parts.push(body);
  }
  const labels = labelLine(row.labels);
  if (labels) parts.push(el('p', 'notice-labels', t('notices.labels', { labels })));
  if (row.source === 'local') parts.push(el('p', 'notice-labels', t('notices.producerNote')));
  const actions = el('div', 'mention-actions');
  const open = el('button', 'btn mention-open', t('notices.openLink'));
  open.type = 'button';
  open.disabled = !row.url;
  open.onclick = () => inv('open_target', { kind: 'url', value: row.url, cwd: ctx.HOME }).catch(() => toast(t('mentions.openFailed')));
  actions.append(open, routeMenu(row));
  const dismiss = el('button', 'btn mention-dismiss', t('mentions.dismiss'));
  dismiss.type = 'button';
  dismiss.disabled = row.attention !== 'open';
  dismiss.onclick = () => dismissRow(row);
  actions.appendChild(dismiss);
  if (row.source === 'slack') {
    const read = el('button', 'btn', t('notices.readInMentions'));
    read.type = 'button';
    read.onclick = () => showMentions();
    actions.appendChild(read);
  }
  parts.push(actions);
  pane.replaceChildren(...parts);
  pane.scrollTop = 0;
}

function paintSelection() {
  for (const node of $('notices-list').children) {
    if (!node.dataset?.id) continue;
    const on = node.dataset.id === selected;
    node.setAttribute('aria-selected', String(on));
    node.dataset.selected = String(on);
  }
}

function select(id, focus = false) {
  selected = id;
  paintSelection();
  paintDetail();
  if (focus) $('notices-list').focus?.({ preventScroll: true });
}

function paintIntake() {
  const enabled = local?.enabled === true;
  const accept = $('notices-accept');
  if (!busy) accept.checked = enabled;
  accept.disabled = busy;
  $('notices-intake-status').textContent = t(!enabled ? 'notices.intake.off'
    : local?.listening ? 'notices.intake.on' : 'notices.intake.starting');
  $('notices-cli').hidden = !local?.cliPath;
  $('notices-cli-path').textContent = local?.cliPath || '';
  const producers = local?.producers || [];
  $('notices-recent').textContent = producers.length
    ? producers.slice(0, 6).map(p => t('notices.recent', { producer: p.producer, time: when(p.lastAt),
      count: formatNumber(p.dayCount) })).join(' · ')
    : t('notices.recentNone');
}

function paintFilters(all) {
  const box = $('notices-filter');
  const list = noticeFilters(all, { slack: slack?.enabled === true });
  if (!list.some(f => f.id === filter)) filter = 'all';
  const next = JSON.stringify([list, filter]);
  if (box.dataset.signature === next) return;
  box.dataset.signature = next;
  box.replaceChildren(...list.map(f => {
    const button = el('button', '', t(f.id === 'all' ? 'notices.filter.all' : f.id === 'slack' ? 'notices.filter.slack'
      : 'notices.filter.producer', { producer: f.producer, count: formatNumber(f.count) }));
    button.type = 'button';
    button.dataset.v = f.id;
    button.setAttribute('aria-pressed', String(f.id === filter));
    button.onclick = () => { filter = f.id; listSignature = ''; paintNotices(); };
    return button;
  }));
}

/* the sidebar entry and, while the view is shown, everything in it */
export function paintNotices() {
  const btn = $('notices-btn');
  if (!btn) return;
  const count = localCount(local);
  btn.classList.toggle('active', state.view === 'notices');
  btn.setAttribute('aria-pressed', String(state.view === 'notices'));
  $('notices-count').textContent = count ? formatNumber(count) : '';
  $('notices-view').hidden = state.view !== 'notices';
  if (state.view !== 'notices') return;
  $('notices-back').textContent = t('attention.backProject', { name: provider.project(state.projectId)?.name || '' });
  paintIntake();
  const all = rows();
  paintFilters(all);
  const { open, settled } = filtered(all, filter);
  $('notices-no-routes').hidden = !open.some(row => row.source === 'local') || rules().some(rule => rule.source === 'local');
  if (!current()) selected = (open[0] || (showSettled ? settled[0] : null))?.id || null;
  const next = JSON.stringify([rowsSignature(open), rowsSignature(settled), showSettled, rules().map(r => [r.id, r.name])]);
  if (next !== listSignature) {
    listSignature = next;
    const fold = el('button', 'mention-fold', t(showSettled ? 'notices.settledHide' : 'notices.settledShow',
      { count: formatNumber(settled.length) }));
    fold.type = 'button';
    fold.setAttribute('aria-expanded', String(showSettled));
    fold.onclick = () => { showSettled = !showSettled; listSignature = ''; paintNotices(); };
    $('notices-list').replaceChildren(...open.map(listRow),
      ...(settled.length ? [fold, ...(showSettled ? settled.map(listRow) : [])] : []));
  }
  paintSelection();
  paintDetail();
}

export async function refreshNotices() {
  const [l, s] = await Promise.all([
    inv('local_notifications').catch(() => null),
    inv('slack_mentions').catch(() => slack),
  ]);
  if (l) local = l;
  slack = s;
  paintNotices();
}

async function route(row, rule) {
  try {
    if (row.source === 'slack') await inv('slack_mention_route', { key: row.key, rule });
    else await inv('local_notification_route', { id: row.id, occurrence: row.occurrence, revision: row.revision, rule });
  } catch (error) {
    uev('notices', 'route-refused');
    toast(row.source === 'slack' ? t(mentionErrorKey(error), { code: String(error) }) : t(routeErrorKey(error)));
    refreshNotices();
    return;
  }
  uev('notices', row.source === 'slack' ? 'slack-routed' : 'routed');
  toast(t('notices.routedToast', { rule: ruleName(rule) }));
  refreshNotices();
}

async function dismissRow(row) {
  try {
    if (row.source === 'slack') await inv('slack_mention_dismiss', { keys: [row.key] });
    else await inv('local_notification_dismiss', { items: [{ id: row.id, occurrence: row.occurrence }] });
  } catch (error) {
    toast(row.source === 'slack' ? t(mentionErrorKey(error), { code: String(error) }) : t('notices.error.unavailable'));
    return;
  }
  uev('notices', 'dismissed');
  const list = shown();
  const at = list.findIndex(r => r.id === row.id);
  selected = list[at + 1]?.id || list[at - 1]?.id || null;
  refreshNotices();
}

async function setAccept(on) {
  const inbound = ctx.settings?.inbound;
  if (!inbound) return;
  busy = true;
  paintIntake();
  const sources = { ...inbound.sources };
  if (on) sources.local = { enabled: true };
  else delete sources.local;
  const ok = await persistInbound({ ...inbound, sources }).catch(() => false);
  busy = false;
  if (!ok) toast(t('notices.acceptFailed'));
  uev('notices', on ? 'accept-on' : 'accept-off');
  /* the backend binds or unlinks the socket on the save's wake */
  setTimeout(refreshNotices, 400);
  refreshNotices();
}

function onListKey(event) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
  const list = shown();
  if (!list.length) return;
  event.preventDefault();
  const at = list.findIndex(row => row.id === selected);
  const step = event.key === 'ArrowDown' ? 1 : -1;
  const next = list[Math.max(0, Math.min(list.length - 1, at + step))];
  select(next.id);
  [...$('notices-list').children].find(node => node.dataset?.id === next.id)?.scrollIntoView?.({ block: 'nearest' });
}

let tick = null;
export function showNotices() {
  closeBuffer();
  if (state.view === 'session') leaveSessionView();
  state.view = 'notices';
  state.sessionId = null;
  listSignature = '';
  detailSignature = '';
  render();
  $('notices-list').focus?.({ preventScroll: true });
  refreshNotices();
  if (!tick) {
    tick = setInterval(() => { if (state.view === 'notices') refreshNotices(); }, 30_000);
    tick.unref?.();
  }
}

export function initNotices(deps) {
  ({ render, switchProject, leaveSessionView, closeBuffer, provider, showMentions } = deps);
  $('notices-btn').onclick = () => showNotices();
  $('notices-back').onclick = () => switchProject(state.projectId);
  $('notices-list').onkeydown = onListKey;
  $('notices-accept').onchange = event => setAccept(event.target.checked);
  $('notices-cli-copy').onclick = () => {
    if (!local?.cliPath) return;
    inv('write_clipboard', { text: local.cliPath }).then(() => toast(t('notices.copied')), () => toast(t('notices.copyFailed')));
  };
}

/* the listeners and the first pull; called once the Board is the user's */
export function startNotices() {
  if (started) return started;
  started = Promise.all([
    listen('local-notifications-changed', refreshNotices).catch(() => uev('listen-fail', 'local-notifications-changed')),
    listen('slack-mentions-changed', refreshNotices).catch(() => uev('listen-fail', 'slack-mentions-changed')),
  ]).then(refreshNotices);
  return started;
}
