import { reminderDue } from './reminder-model.js';
// attention-model.js — runtime-only observations and derived attention.
// No Board writes, hook installation, inferred readiness, or durable ordering.
// Three layers, kept apart:
// - Observation: `alive`, `agent` (a closed interaction word: working =
//   interaction active, needs-input = input requested, turn-done =
//   interaction ended — never task/program completion), `status`, `idle`.
//   Only a successful poll writes it.
// - Codex coverage: `codexSignal` is the backend's generation-bound diagnostic
//   evidence, independent of observation/attention. A gap never creates a
//   pending episode, readiness or permission; an unknown cause stays unknown.
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
// - Delivery waiting: `deliveryWaits` reads Deck's own queue items (the copy
//   scheduler.js already holds), never an agent word or a plan snapshot. A
//   session is waiting when a row is at a human checkpoint (`review`), its
//   delivery is uncertain (`ambiguous`), it stopped retrying (`failed`, dead),
//   or the row whose turn it is carries external content no approval covers
//   (`external`: the group's head only, never a paused row). It says a
//   delivery is held, not that the work is stuck. It joins `pending` and the
//   list; it is not an agent observation, so the card badge and the Dock
//   badge (attentionBadge) leaves it out on purpose; the Dock count and the
//   away notification take it from the backend as Deck's own source
//   (notify.rs, scheduler `delivery_waits`). Two holds need a live
//   observation and are NOT derived here: the backend's plan (`plans`,
//   review.rs `plan_item`) names a row held for a first agent interaction
//   (`first-send`) or because Codex Signal cannot be attributed
//   (`codex-signal`), and the model accepts exactly those two stage words
//   for a row still in the queue. An approval Deck could not re-read
//   (`authority-unverified`) usually passes by itself and stays in the panel.
//   The six words are one list with the backend's (limits.json
//   `delivery_waits`).
// - Bell: `rang(card)` passes on the backend's `bell` (bell.rs): a program
//   in a session without agent state rang the terminal bell and nobody has
//   looked since. The backend alone decides it and when it was viewed (the
//   Deck window in front and a pane showing the card); nothing here infers
//   or clears it. It joins `pending` and the list like a held delivery, is
//   never a card status, the card badge or a filter, and says only that the
//   bell rang — not that anything ended, succeeded or needs input.
import { CARD_QUIET_SECS, effectiveCardStatus, itemDead } from './pure.js';

export const ATTENTION_FILTERS = Object.freeze(['pending', 'input', 'waiting', 'done', 'followed', 'unavailable', 'stopped', 'reminder', 'reminders']);
/* most pressing first: one reason per session */
export const DELIVERY_WAITS = Object.freeze(['ambiguous', 'failed', 'review', 'external', 'first-send', 'codex-signal']);
/* the two the backend's plan decides; never read from hook state here */
const LIVE_DELIVERY_WAITS = Object.freeze(['first-send', 'codex-signal']);

export function deliveryWaits(items, plans = []) {
  const heads = new Map();
  const order = i => i.seq ?? 1;
  for (const i of items) {
    if (!i.group || i.state === 'review-approved') continue;
    const head = heads.get(i.group);
    if (!head || order(i) < order(head) || (order(i) === order(head) && i.added < head.added)) heads.set(i.group, i);
  }
  const waits = new Map();
  for (const i of items) {
    const reason = i.state === 'ambiguous' ? 'ambiguous' : itemDead(i) ? 'failed' : i.state === 'review' ? 'review'
      : i.external === true && i.mode === 'chain' && !i.authority && !i.paused && (!i.group || heads.get(i.group) === i) ? 'external' : null;
    const known = waits.get(i.session);
    if (reason && (!known || DELIVERY_WAITS.indexOf(reason) < DELIVERY_WAITS.indexOf(known))) waits.set(i.session, reason);
  }
  const sessions = new Map(items.map(i => [i.id, i.session]));
  for (const plan of plans || []) {
    const session = plan && sessions.get(plan.item);
    if (!session || !LIVE_DELIVERY_WAITS.includes(plan.stage)) continue;
    const known = waits.get(session);
    if (!known || DELIVERY_WAITS.indexOf(plan.stage) < DELIVERY_WAITS.indexOf(known)) waits.set(session, plan.stage);
  }
  return waits;
}
export const CODEX_SIGNAL_TRUST = Object.freeze(['unknown', 'trusted', 'unavailable']);

export function createAttentionTracker() {
  const snapshots = new Map();
  let lastSuccess = null;
  let failed = false;
  let waits = new Map();
  const waiting = card => (card && waits.get(card.session)) || null;
  const rang = card => get(card)?.bell === true;
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
    return filter === 'all' || (filter === 'reminder' ? reminderDue(card) : filter === 'reminders' ? !!card.reminder : false) || (filter === 'followed' ? card.pinned === true
      : filter === 'pending' ? kind === 'input' || kind === 'done' || card.pinned === true || reminderDue(card) || !!waiting(card) || rang(card)
      : filter === 'waiting' ? !!waiting(card)
      : filter === 'unavailable' ? kind === 'unavailable' || kind === 'unknown' : kind === filter);
  };
  return {
    get, category, matches, waiting, rang,
    deliveries(items, plans) { waits = deliveryWaits(items || [], plans || []); },
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
          codexSignal: info.alive && CODEX_SIGNAL_TRUST.includes(info.codex_signal) ? info.codex_signal : null,
          idle: info.idle_secs ?? null, observedAt: now, stale: false, episode, episodeViewed,
          bell: info.alive && !agent && info.bell === true,
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
      const counts = { all: cards.length, pending: 0, input: 0, waiting: 0, done: 0, followed: 0, unavailable: 0, stopped: 0, unknown: 0 };
      for (const card of cards) {
        const kind = category(card);
        if (kind in counts) counts[kind]++;
        if (card.pinned === true) counts.followed++;
        if (waiting(card)) counts.waiting++;
        if (kind === 'input' || kind === 'done' || card.pinned === true || reminderDue(card) || waiting(card) || rang(card)) counts.pending++;
        if (kind === 'unknown') counts.unavailable++;
      }
      if (cards.some(card => card.reminder)) { counts.reminder = cards.filter(card => reminderDue(card)).length; counts.reminders = cards.filter(card => card.reminder).length; }
      return counts;
    },
  };
}

// The diagnostic applies only to a live target without a real observation.
// A stale snapshot retains its explanation with the existing "old" mark;
// stopped/replaced sessions and a newly established signal remove the gap.
export function codexCoverageGap(snapshot) {
  if (!snapshot?.alive || snapshot.agent) return null;
  return ['unknown', 'unavailable'].includes(snapshot.codexSignal) ? snapshot.codexSignal : null;
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
        /* an input request outranks a held delivery; the row names both */
        const kind = category !== 'input' && tracker.waiting(card) ? 'waiting'
          : tracker.rang(card) ? 'bell'
          : reminderDue(card) && !['input', 'done'].includes(category) ? 'reminder' : filter === 'pending' && !['input', 'done'].includes(category) ? 'followed' : category;
        ordered.push({ card, project, column, kind });
      }
    }
  }
  if (filter === 'reminder' || filter === 'reminders') return ordered.sort((a, b) => a.card.reminder.dueAt - b.card.reminder.dueAt);
  if (filter !== 'pending') return ordered;
  return ['input', 'waiting', 'bell', 'done', 'reminder', 'followed'].flatMap(kind => ordered.filter(row => row.kind === kind));
}
