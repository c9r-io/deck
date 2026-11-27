// notices-model.js — the DOM-free half of the Notifications view
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - One page, two backend lists, nothing copied. Local notifications come
//   from `local_notifications` (local_notify.rs, the durable inbox); Slack
//   mentions from `slack_mentions` (slack_mentions.rs, memory only), read
//   exactly as the Slack mentions view reads them (`openWithin` over the
//   longest range). Both become one row shape (`localRows`, `slackRows`) so
//   the page can filter, sort and show them alike; each row keeps its
//   `source`, and every action goes back to that source's own command.
// - Two state axes on a local row: `external` (active | resolved |
//   withdrawn) is what its producer said; `attention` (open | dismissed |
//   routed) is Deck's. A row needs the user (`needsUser`) only while it is
//   open AND active: a resolved one, a dismissed one and a routed one are
//   listed apart under "Settled" and never counted. A dismissed row its
//   producer changed since is marked updated, still not counted.
// - Filters: All, Slack (while Slack is connected), and one per producer
//   seen, each with its count of rows that need the user. A producer is a
//   name a program chose, not a verified identity.
// - Routes: a local row goes to one of the user's local notification rules
//   (`localRoutes`, settings order, the order a rule takes them in), a Slack
//   row to a Slack badge rule (`mentionRoutes`); a rule the channel
//   admission refuses is listed but cannot be chosen.
// - Error words from the backend are closed and each has its sentence.
import { mentionRoutes, openWithin } from './slack-mentions-model.js';

const WEEK = 7;

/* the local inbox's items as rows */
export function localRows(view) {
  return (Array.isArray(view?.items) ? view.items : []).map(item => ({
    source: 'local', id: item.id, producer: item.producer, key: item.key, type: item.type,
    title: item.title, body: item.body || '', url: item.url || '', labels: item.labels || {},
    at: item.changedAt, firstAt: item.firstAt, occurredAt: item.occurredAt ?? null,
    revision: item.revision, occurrence: item.occurrence,
    external: item.external, attention: item.attention, updated: item.updated === true,
    routing: item.routing || null,
  }));
}

/* the open Slack mentions of the last week as rows */
export function slackRows(view, now) {
  if (!view?.enabled) return [];
  return openWithin(view, WEEK, now).map(item => ({
    source: 'slack', id: `slack:${item.key}`, key: item.key, producer: 'slack', type: 'mention',
    title: String(item.text || '').split('\n')[0], body: item.text || '', url: item.link || '',
    labels: {}, at: item.at, from: item.from, where: item.where,
    external: 'active', attention: 'open', updated: false, routing: null,
  }));
}

export const needsUser = row => row.attention === 'open' && row.external === 'active';

/* newest first; equal times keep a stable order */
export const allRows = (local, slack) => [...local, ...slack]
  .sort((a, b) => (b.at - a.at) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));

const inFilter = (row, filter) => filter === 'all'
  || (filter === 'slack' ? row.source === 'slack' : row.source === 'local' && filter === `local:${row.producer}`);

/* the filter buttons: All, Slack while it is on, then each producer seen */
export function noticeFilters(rows, { slack = false } = {}) {
  const count = filter => rows.filter(row => needsUser(row) && inFilter(row, filter)).length;
  const producers = [...new Set(rows.filter(row => row.source === 'local').map(row => row.producer))].sort();
  return [
    { id: 'all', count: count('all') },
    ...(slack ? [{ id: 'slack', count: count('slack') }] : []),
    ...producers.map(producer => ({ id: `local:${producer}`, producer, count: count(`local:${producer}`) })),
  ];
}

/* the rows a filter shows: those that need the user, then the settled ones */
export function filtered(rows, filter) {
  const mine = rows.filter(row => inFilter(row, filter));
  return { open: mine.filter(needsUser), settled: mine.filter(row => !needsUser(row)) };
}

/* the sidebar count: local rows that need the user (Slack keeps its own) */
export const localCount = view => localRows(view).filter(needsUser).length;

/* the local notification rules a row may go to, in settings order */
export function localRoutes(rules, projects, blockReason, row = null) {
  return (Array.isArray(rules) ? rules : [])
    .filter(rule => rule?.source === 'local')
    .map(rule => {
      const project = (projects || []).find(p => p.id === rule.projectId);
      const disabled = !project || rule.enabled === false || !!blockReason(rule, project);
      return { id: rule.id, label: rule.name || rule.localMatch?.producer || rule.id, disabled,
        /* a click may route to any local rule, as a Slack mention may go to
           any badge rule; the ones whose match fits are marked */
        fits: !!row && ruleFits(rule, row) };
    });
}

export const routesFor = (row, rules, projects, blockReason) => (row?.source === 'slack'
  ? mentionRoutes(rules, projects, blockReason) : localRoutes(rules, projects, blockReason, row));

/* which rules fit a local row, in settings order (presentation: the
   backend decides an automatic route) */
export function patternMatches(pattern, value) {
  const p = String(pattern ?? '');
  return p.endsWith('*') ? String(value).startsWith(p.slice(0, -1)) : p === value;
}

export function ruleFits(rule, row) {
  const m = rule?.localMatch;
  if (!m || row?.source !== 'local') return false;
  return patternMatches(m.producer, row.producer)
    && (!m.types?.length || m.types.some(t => patternMatches(t, row.type)))
    && Object.entries(m.labels || {}).every(([k, v]) => Object.hasOwn(row.labels, k) && patternMatches(v, row.labels[k]));
}

/* the state words of a row, as i18n keys with their params */
export function stateWords(row, ruleName = id => id) {
  const words = [];
  if (row.external === 'resolved') words.push({ key: 'notices.state.resolved' });
  if (row.external === 'withdrawn') words.push({ key: 'notices.state.withdrawn' });
  if (row.attention === 'dismissed') words.push({ key: row.updated ? 'notices.state.dismissedUpdated' : 'notices.state.dismissed' });
  const r = row.routing;
  if (r) {
    const rule = ruleName(r.rule);
    if (r.state === 'pending') words.push({ key: 'notices.state.routing', params: { rule } });
    else if (r.state === 'done') words.push({ key: r.by === 'auto' ? 'notices.state.routedAuto' : 'notices.state.routed', params: { rule } });
    else words.push({ key: 'notices.state.skipped', params: { rule, reason: r.reason || '' } });
  }
  return words;
}

/* the backend's closed refusal words */
const ROUTE_ERRORS = {
  settings: 'notices.error.settings', 'no-rule': 'notices.error.noRule', board: 'notices.error.board',
  'no-template': 'notices.error.noTemplate', missing: 'notices.error.missing', stale: 'notices.error.stale',
  taken: 'notices.error.taken', unavailable: 'notices.error.unavailable',
};
export const routeErrorKey = code => ROUTE_ERRORS[String(code)] || 'notices.error.unavailable';

/* labels as one line: `key=value` pairs in name order */
export const labelLine = labels => Object.keys(labels || {}).sort().map(k => `${k}=${labels[k]}`).join(' · ');

/* what changes the list or the reader: compared before repainting */
export const rowsSignature = rows => JSON.stringify(rows.map(row => [row.id, row.at, row.attention, row.external,
  row.updated, row.routing?.state, row.routing?.rule, row.revision]));
