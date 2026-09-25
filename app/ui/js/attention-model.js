// attention-model.js — runtime-only observations and derived attention.
// No Board writes, hook installation, inferred readiness, or durable ordering.
// Three layers, kept apart:
// - Observation: `alive`, `agent` (a closed interaction word: working =
//   interaction active, needs-input = input requested, turn-done =
//   interaction ended — never task/program completion), `status`, `idle`.
//   Only a successful poll writes it.
// - Attention: `seen` (read/unread) and the derived categories. Read means
//   successfully displayed, never handled. With a backend episode (FR-SI-05:
//   `episode` is Deck's opaque token for one accepted observation) the
//   backend's `episode_viewed` is the authoritative truth: the same episode
//   stays read across pane switches, webview reloads and missed polls, and a
//   new episode re-arms. Local `seen` only bridges the moment between a view
//   and the acknowledged `notify_dismiss(session, episode)`. Snapshots
//   without an episode keep the status rule: a new observed status re-arms.
//   Viewing never changes an observation.
// - Freshness: `stale` and `freshness()`. A failed or partial poll keeps the
//   last observation and marks it stale; stale is not an agent state and
//   never changes `agent`/`status`.
// A snapshot belongs to a card AND its session identity. Source interaction
// identity (an agent's own turn id) stays backend-private; what crosses the
// projection boundary is Deck's local, opaque `episode` for each accepted
// observation. It distinguishes observation episodes even across missed
// polls (an ending, a new turn and a second ending between two polls arrive
// as a NEW episode, not a repeat), pane switches and a recreated webview.
// Only snapshots without an episode fall back to status comparison, where a
// missed working→done cannot be told from a repeat. Presentation (Board badge, list, tab dot, Dock) reads these
// categories; none of them is side-effect authority (tests/signal_census.rs).
import { CARD_QUIET_SECS, effectiveCardStatus } from './pure.js';

export const ATTENTION_FILTERS = Object.freeze(['pending', 'input', 'done', 'followed', 'unavailable', 'stopped']);

export function createAttentionTracker() {
  const snapshots = new Map();
  let lastSuccess = null;
  let failed = false;
  const get = card => {
    const value = card && snapshots.get(card.id);
    return value?.session === card?.session ? value : null;
  };
  const category = card => {
    const value = get(card);
    if (!value) return 'unknown';
    if (!value.alive) return 'stopped';
    if (value.agent === 'needs-input') return 'input';
    if (value.agent === 'turn-done' && !value.seen) return 'done';
    if (!value.agent) return 'unavailable';
    return 'other';
  };
  const matches = (card, filter) => {
    const kind = category(card);
    return filter === 'all' || (filter === 'followed' ? card.pinned === true
      : filter === 'pending' ? kind === 'input' || kind === 'done' || card.pinned === true
      : filter === 'unavailable' ? kind === 'unavailable' || kind === 'unknown' : kind === filter);
  };
  return {
    get, category, matches,
    record(cards, infos, visible, now) {
      visible = visible || new Set();
      now = now ?? Date.now();
      const byName = new Map(infos.map(info => [info.name, info]));
      const ids = new Set(cards.map(card => card.id));
      for (const id of snapshots.keys()) if (!ids.has(id)) snapshots.delete(id);
      let complete = true;
      for (const card of cards) {
        const old = get(card);
        const info = byName.get(card.session);
        if (!info || typeof info.alive !== 'boolean') {
          if (old) old.stale = true;
          complete = false;
          continue;
        }
        const agent = info.alive && ['working', 'needs-input', 'turn-done'].includes(info.agent) ? info.agent : null;
        const status = effectiveCardStatus(info.alive, agent, info.idle_secs != null && info.idle_secs >= CARD_QUIET_SECS);
        const episode = agent && Number.isSafeInteger(info.episode) ? info.episode : null;
        const episodeViewed = episode != null && info.episode_viewed === true;
        snapshots.set(card.id, {
          session: card.session, alive: info.alive, agent, status,
          idle: info.idle_secs ?? null, observedAt: now, stale: false, episode, episodeViewed,
          seen: episode != null
            ? !!(episodeViewed || visible.has(card.id) || (old?.episode === episode && old.seen))
            : !!((old?.status === status && old?.agent === agent && old.seen) || visible.has(card.id)),
        });
      }
      failed = !complete;
      if (complete) lastSuccess = now;
    },
    fail() {
      failed = true;
      for (const value of snapshots.values()) value.stale = true;
    },
    saw(card) {
      const value = get(card);
      if (!value || value.stale) return false;
      value.seen = true;
      return true;
    },
    /* the backend acknowledged `notify_dismiss(session, episode)`: until
       the next poll says so itself, this episode is known viewed */
    confirmViewed(card, episode) {
      const value = get(card);
      if (value && value.episode === episode) value.episodeViewed = true;
    },
    freshness(cards) {
      const known = cards.filter(card => get(card));
      return {
        kind: !known.length && cards.length ? 'unknown'
          : failed || known.length !== cards.length || known.some(card => get(card).stale) ? 'stale' : 'fresh',
        lastSuccess,
      };
    },
    counts(cards) {
      const counts = { all: cards.length, pending: 0, input: 0, done: 0, followed: 0, unavailable: 0, stopped: 0, unknown: 0 };
      for (const card of cards) {
        const kind = category(card);
        if (kind in counts) counts[kind]++;
        if (card.pinned === true) counts.followed++;
        if (kind === 'input' || kind === 'done' || card.pinned === true) counts.pending++;
        if (kind === 'unknown') counts.unavailable++;
      }
      return counts;
    },
  };
}

/* The Board card's attention badge. The same live set the Dock badge counts
   (needs-input plus unread turn-done, notify.rs) and the Needs-attention list
   shows minus manual follow-up, read from `category`, so the three surfaces
   cannot disagree and viewing an ending (`saw`) clears it without any change
   to the card. `stale` passes the snapshot's own honesty through. Nothing
   here depends on whether a native notification was posted. */
export const ATTENTION_BADGE_LABELS = Object.freeze({ input: 'attention.filter.input', done: 'attention.filter.done' });

export function attentionBadge(tracker, card) {
  const kind = tracker.category(card);
  if (kind !== 'input' && kind !== 'done') return null;
  return { kind, stale: tracker.get(card).stale === true };
}

// Presentation order only. Never sort the durable card/project arrays.
export function attentionRows(projects, cards, tracker, filter) {
  const ordered = [];
  const added = new Set();
  for (const project of projects) for (const column of project.columns) {
    for (const card of cards) {
      if (card.projectId !== project.id || card.columnId !== column.id || added.has(card.id)) continue;
      added.add(card.id);
      if (tracker.matches(card, filter)) {
        const category = tracker.category(card);
        const kind = filter === 'pending' && !['input', 'done'].includes(category) ? 'followed' : category;
        ordered.push({ card, project, column, kind });
      }
    }
  }
  if (filter !== 'pending') return ordered;
  return ['input', 'done', 'followed'].flatMap(kind => ordered.filter(row => row.kind === kind));
}
