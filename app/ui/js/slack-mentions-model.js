// slack-mentions-model.js — the DOM-free half of the Slack mentions view
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - The list is the backend's (`slack_mentions`, slack_mentions.rs): open
//   mentions only, newest first. Nothing here filters, stores or reorders
//   it; the view paints what it was given.
// - A route is one of the user's Slack badge rules (`source: 'slack'`), in
//   badge order. A rule whose project is gone, or whose command or template
//   the channel admission refuses (`channelBlockReason`, the same check a
//   badge item passes in inbound.js), is listed but cannot be chosen, so the
//   menu says why instead of failing after the click.
// - The count is the number of open mentions, shown beside the sidebar
//   entry and nowhere else (no notification, no Dock badge).
// - Error words from the backend are closed: `settled` has its own sentence,
//   every other word is named inside the generic one.

/* the routes the menu offers, one per Slack badge rule */
export function mentionRoutes(rules, projects, blockReason) {
  return (Array.isArray(rules) ? rules : [])
    .filter(rule => rule?.source === 'slack' && rule.badge)
    .map(rule => {
      const project = (projects || []).find(value => value.id === rule.projectId);
      const label = `:${rule.badge}: → ${project?.name || '?'} / ${rule.template || '—'}`;
      return { id: rule.id, label, disabled: !project || !!blockReason(rule, project) };
    })
    .sort((a, b) => a.label.localeCompare(b.label));
}

/* the open count the sidebar shows; nothing while Slack is off */
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

/* the sentence for a refused route or dismissal */
export function mentionErrorKey(message) {
  return String(message) === 'settled' ? 'mentions.settled' : 'mentions.failed';
}

/* whether two paints show the same rows and routes (a repaint would
   otherwise close a menu the user has open) */
export function mentionSignature(view, routes) {
  return JSON.stringify([mentionCount(view) ? view.items.map(item => item.key) : [],
    routes.map(route => [route.id, route.label, route.disabled])]);
}
