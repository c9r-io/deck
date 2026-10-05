// notify-model.js — the DOM-free half of away notifications (notify.rs owns
// the policy: what is posted, when, and the Dock badge). The webview only
// supplies what Rust cannot know: the card labels the system may show, the
// fact that an unread turn ending was viewed, and the closed status word
// for the Settings row. Nothing here reads the DOM or the Board store.
import { ATTENTION_FILTERS } from './attention-model.js';

export const NOTIFY_STATUS_WORDS = Object.freeze(['unsupported', 'not-determined', 'denied', 'authorized', 'provisional']);

/** The agent half of away notifications (an input request, a turn ending)
 * fires only on agent-status hook events, so Settings names the dependency
 * when BOTH integrations are known to be off. A held delivery is Deck's own
 * source and is announced without them; the sentence says both. `hooks`
 * is null while their state is unknown (not read yet, or the read failed):
 * no claim is made then. Nothing is switched on or off here. */
export function notifyNeedsAgentStatus(hooks) {
  return !!hooks && hooks.claude !== true && hooks.codex !== true;
}

/** What Settings says after an agent status integration was switched. Turning
 * one on while away notifications are off adds, to the same toast, that they
 * are a second switch just below: the user asked for agent status and is one
 * step from hearing about it when away. No dialog, no permission request, and
 * the notification switch is not touched. */
export function agentHooksToastKey(enabled, notifyAway) {
  if (!enabled) return 'settings.agentHooksDisabled';
  return notifyAway === true ? 'settings.agentHooksEnabled' : 'settings.agentHooksEnabledNotifyOff';
}

/* A label's title: at most this many bytes of UTF-8, cut at a character
   boundary. notify.rs holds the same bound and cuts the same way
   (test/fixtures/limits.json `notify`). Counting UTF-16 units instead let
   a long CJK title through that the backend then refused, together with
   every other card's label, and could leave half a character, which fails
   the whole call in the backend's deserializer. */
export const LABEL_TITLE_MAX_BYTES = 512;
export function labelTitle(title) {
  const bytes = new TextEncoder().encode(String(title || ''));
  let end = Math.min(bytes.length, LABEL_TITLE_MAX_BYTES);
  // a cut inside a character steps back to where that character starts
  while (end < bytes.length && end > 0 && (bytes[end] & 0xC0) === 0x80) end--;
  return new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes.subarray(0, end));
}

/** session → {title, project} for every card, sorted so the key is stable. */
export function cardLabels(cards, projects) {
  const names = new Map((projects || []).map(p => [p.id, String(p.name || '')]));
  return (cards || [])
    .filter(card => card && typeof card.session === 'string' && card.session)
    .map(card => ({
      session: card.session,
      title: labelTitle(card.title),
      project: names.get(card.projectId) || '',
    }))
    .sort((a, b) => (a.session < b.session ? -1 : a.session > b.session ? 1 : 0));
}

/** One string that changes exactly when the labels do. */
export function labelsKey(labels) {
  return labels.map(l => `${l.session}\u0000${l.title}\u0000${l.project}`).join('\u0001');
}

/** The in-flight key of one dismissal. */
export const dismissKey = (session, episode) => `${session}\u0000${episode}`;

/** Viewed turn endings the backend does not know about yet (FR-SI-05):
 * each `{card, session, episode}` names the EXACT episode that was
 * displayed, so a backend that has meanwhile advanced keeps its newer
 * ending unread. Attention bookkeeping only — viewing is not handling. A
 * dismissal is done when the backend says `episode_viewed` (or acked it);
 * until then it is returned again on every sync, except while `inflight`
 * holds it, so a failed call is retried by the normal sync path. */
export function seenDismissals(cards, tracker, inflight) {
  const out = [];
  for (const card of cards || []) {
    const snapshot = tracker.get(card);
    if (snapshot?.agent !== 'turn-done' || !snapshot.seen || snapshot.episode == null || snapshot.episodeViewed) continue;
    if (inflight.has(dismissKey(card.session, snapshot.episode))) continue;
    out.push({ card, session: card.session, episode: snapshot.episode });
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
