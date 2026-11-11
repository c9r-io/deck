// slack-mentions-model.js — the DOM-free half of the Slack mentions view
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - The list is the backend's (`slack_mentions`, slack_mentions.rs): open
//   mentions of the last week only, newest first. Nothing here stores or
//   reorders it.
// - Range: the view shows the mentions sent within one of three ranges
//   (`MENTION_RANGES`: 24 hours, 3 days, a week; 24 hours at every launch,
//   kept while Deck runs, never saved), and each range's button carries its
//   own count, so a mention outside the chosen range is never hidden
//   without a number saying so.
// - A route is one of the user's Slack badge rules (`source: 'slack'`), in
//   badge-name order, its badge shown as the emoji Slack shows (slack-emoji.js)
//   or, for a custom one, as `:name:`. A rule whose project is gone, or whose command or template
//   the channel admission refuses (`channelBlockReason`, the same check a
//   badge item passes in inbound.js), is listed but cannot be chosen, so the
//   menu says why instead of failing after the click.
// - Selection: after mentions leave the list, the next one is selected
//   (`mentionAfter`), so deciding one by one needs no extra click.
// - The count is the number of open mentions in the chosen range, shown
//   beside the sidebar entry and nowhere else (no notification, no Dock
//   badge).
// - Handed over: a mention an MCP client handed to one of its Agent rules
//   (`deck_mention_handoff`) leaves the list and is shown apart, within the
//   chosen range, with the client's and the rule's names (`handedRows`), so
//   nothing an agent took leaves without the user seeing where it went.
// - Error words from the backend are closed: `settled` has its own sentence,
//   every other word is named inside the generic one.

import { slackEmoji } from './slack-emoji.js';

/* the routes the menu offers, one per Slack badge rule; a standard badge is
   shown as its emoji (read faster than its name), a custom one as `:name:` */
export function mentionRoutes(rules, projects, blockReason) {
  return (Array.isArray(rules) ? rules : [])
    .filter(rule => rule?.source === 'slack' && rule.badge)
    .map(rule => {
      const project = (projects || []).find(value => value.id === rule.projectId);
      const label = `${slackEmoji(rule.badge) || `:${rule.badge}:`} → ${project?.name || '?'} / ${rule.template || '—'}`;
      return { id: rule.id, badge: rule.badge, label, disabled: !project || !!blockReason(rule, project) };
    })
    .sort((a, b) => a.badge.localeCompare(b.badge));
}

/* the ranges the view offers, in days; the first is the default */
export const MENTION_RANGES = [1, 3, 7];

/* the open mentions sent within `days` of `now` (seconds) */
export function mentionsWithin(view, days, now) {
  if (!mentionCount(view)) return [];
  const since = now - days * 86400;
  return view.items.filter(item => item.at >= since);
}

/* each range with its own count, for the range buttons */
export function mentionRanges(view, now) {
  return MENTION_RANGES.map(days => ({ days, count: mentionsWithin(view, days, now).length }));
}

/* the number of open mentions the backend listed; nothing while Slack is off */
export function mentionCount(view) {
  return view?.enabled && Array.isArray(view.items) ? view.items.length : 0;
}

/* the status line under the title: key + params for t() */
export function mentionStatus(view) {
  if (!view?.enabled) return { key: 'mentions.off', params: {} };
  if (view.lastError) return { key: 'mentions.error', params: { code: view.lastError } };
  if (!view.lastPoll) return { key: 'mentions.checking', params: {} };
  return { key: 'mentions.checked', params: { at: view.lastPoll } };
}

/* the handed-over mentions within `days` of `now`, newest first, each with
   the names of the client that took it (or of the orchestrator card,
   `ownerOf(workerCard)`) and the rule it went to (the id when the name is
   unknown) */
export function handedRows(view, days, now, clients, rules, ownerOf = () => '') {
  if (!view?.enabled || !Array.isArray(view.handed)) return [];
  const since = now - days * 86400;
  return view.handed
    .filter(item => item.at >= since)
    .map(item => ({
      key: item.key,
      from: item.from,
      where: item.where,
      at: item.at,
      line: String(item.text || '').split('\n')[0],
      /* the orchestrator card that handed it over, else the client */
      client: ownerOf(item.card) || (clients || []).find(c => c.id === item.client)?.name || item.client,
      rule: (rules || []).find(r => r?.source === 'agent' && r.badge === item.rule)?.name || item.rule,
    }));
}

/* the sentence for a refused route or dismissal */
export function mentionErrorKey(message) {
  return String(message) === 'settled' ? 'mentions.settled' : 'mentions.failed';
}

/* whether two paints show the same list rows (a repaint would otherwise
   drop the focus the user has in the list); a row changes when its names
   arrive with its context */
export function mentionSignature(items, routes) {
  return JSON.stringify([items.map(item => [item.key, item.from, item.text]),
    routes.map(route => [route.id, route.label, route.disabled])]);
}

/* the mention to select once `keys` left the list: the first one after the
   first that left, else the last before it, else none */
export function mentionAfter(items, keys) {
  const list = Array.isArray(items) ? items : [];
  const at = list.findIndex(item => keys.includes(item.key));
  if (at < 0) return list[0]?.key ?? null;
  const rest = list.filter(item => !keys.includes(item.key));
  const after = list.slice(at).find(item => !keys.includes(item.key));
  return (after || rest[rest.length - 1])?.key ?? null;
}
