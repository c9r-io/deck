// notify-model.js — the DOM-free half of away notifications (notify.rs owns
// the policy: what is posted, when, and the Dock badge). The webview only
// supplies what Rust cannot know: the card labels the system may show, the
// fact that an unread turn ending was viewed, and the closed status word
// for the Settings row. Nothing here reads the DOM or the Board store.
import { ATTENTION_FILTERS } from './attention-model.js';

export const NOTIFY_STATUS_WORDS = Object.freeze(['unsupported', 'not-determined', 'denied', 'authorized', 'provisional']);

/** Away notifications fire only on agent-status hook events, so Settings
 * names the dependency when BOTH integrations are known to be off. `hooks`
 * is null while their state is unknown (not read yet, or the read failed):
 * no claim is made then. Nothing is switched on or off here. */
export function notifyNeedsAgentStatus(hooks) {
  return !!hooks && hooks.claude !== true && hooks.codex !== true;
}

/** session → {title, project} for every card, sorted so the key is stable. */
export function cardLabels(cards, projects) {
  const names = new Map((projects || []).map(p => [p.id, String(p.name || '')]));
  return (cards || [])
    .filter(card => card && typeof card.session === 'string' && card.session)
    .map(card => ({
      session: card.session,
      title: String(card.title || '').slice(0, 512),
      project: names.get(card.projectId) || '',
    }))
    .sort((a, b) => (a.session < b.session ? -1 : a.session > b.session ? 1 : 0));
}

/** One string that changes exactly when the labels do. */
export function labelsKey(labels) {
  return labels.map(l => `${l.session}\u0000${l.title}\u0000${l.project}`).join('\u0001');
}

/** Sessions whose unread turn ending has now been viewed and not yet
 * reported: each is returned once per turn. `sent` is the caller's memory;
 * a session leaves it when its agent state is no longer `turn-done`, so
 * the next turn's viewing is reported again. */
export function seenDismissals(cards, tracker, sent) {
  const out = [];
  for (const card of cards || []) {
    const snapshot = tracker.get(card);
    if (snapshot?.agent === 'turn-done') {
      if (snapshot.seen && !sent.has(card.session)) {
        sent.add(card.session);
        out.push(card.session);
      }
    } else {
      sent.delete(card.session);
    }
  }
  return out;
}

/** The i18n key for a status word; anything unknown reads as unsupported. */
export function notifyStatusKey(status) {
  return `settings.notifyStatus.${NOTIFY_STATUS_WORDS.includes(status) ? status : 'unsupported'}`;
}

// The badge counts the same rows as the attention list minus manual
// follow-up; keep the filter names in sync with the model they mirror.
export const NOTIFY_COUNTED_FILTERS = Object.freeze(ATTENTION_FILTERS.filter(f => f === 'input' || f === 'done'));
