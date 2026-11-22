import { reminderDue } from './reminder-model.js';
import { PI_COVERAGE } from './pi-model.js';
import { openAgentNotes } from './buffer-model.js';
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
// - Pi coverage: `piSignal` is the backend's word about a recognized Pi
//   (pi-model.js): `connected` is limited coverage, never an input request.
//   Like the Codex word it is kept beside the observation and feeds no
//   category, count or badge.
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
//   (`codex-signal`), or withdrawn channel permission (`channel-stopped`),
//   or because the target is a recognized Pi, which nothing but Send now
//   sends to (`pi-input-unverified`), and the model accepts these stage words
//   for a row still in the queue. An approval Deck could not re-read
//   (`authority-unverified` or `channel-unverified`) usually passes by itself and stays in the panel;
//   it is a held delivery only when the backend's plan marks it `lasting`
//   (review.rs `track_unverified`: the hold outlived its threshold, which
//   times the notice and proves nothing else). The model never measures that
//   itself. These words are one list with the backend's (limits.json
//   `delivery_waits`).
// - Turn age and marks: `since` is when the backend accepted the current
//   episode (presentation only: "Turn ended" lists the longest-ended first
//   and says how long ago). `mark` is an MCP client's suggestion for that
//   same episode (needs-you, can-close, working; mcp/cards.rs): the row
//   shows it and the user may tick the can-close ones; no category, list,
//   pending or Dock count, badge, notification or side effect reads either.
//   `marked(cards)` says, per filter, how many of the cards that filter
//   lists carry a mark of any kind: the filter buttons show it so a mark
//   can be found without opening each filter. It changes no list.
// - Bell: `rang(card)` passes on the backend's `bell` (bell.rs): a program
//   in a session without agent state rang the terminal bell and nobody has
//   looked since. The backend alone decides it and when it was viewed (the
//   Deck window in front and a pane showing the card); nothing here infers
//   or clears it. It joins `pending` and the list like a held delivery, is
//   never a card status, the card badge or a filter, and says only that the
//   bell rang — not that anything ended, succeeded or needs input.
// - Agent notes: `noted(card)` counts the open notes an Agent left in the
//   card's scratchpad for the user to judge (buffer-model.js, mcp/notes.rs;
//   Board data, not an observation). A card with one joins `pending` under
//   its own heading ("Waiting for your judgment") unless a stronger reason
//   lists it; the row says how many. It is never a status, the card's
//   attention badge, the Dock count or a notification: the Agent did not
//   stop, the notes wait for the user to come back.
import { CARD_QUIET_SECS, effectiveCardStatus, itemDead } from './pure.js';

/* the marks an MCP client may set on a card (mcp/cards.rs MARKS) */
export const CARD_MARKS = Object.freeze(['needs-you', 'can-close', 'working']);
export const ATTENTION_FILTERS = Object.freeze(['pending', 'input', 'waiting', 'done', 'ended', 'followed', 'unavailable', 'stopped', 'reminder', 'reminders']);
export const ATTENTION_SECTION_LABELS = Object.freeze({
  /* The pending list's sections, in row order, each with its heading. Every
     kind `attentionRows` returns under 'pending' has one; a bell is a row
     reason, not a filter, so it has its own heading key. */
  input: 'attention.filter.input', waiting: 'attention.filter.waiting', bell: 'attention.section.bell',
  done: 'attention.filter.done', notes: 'attention.section.notes', reminder: 'attention.filter.reminder', followed: 'attention.filter.followed',
});
/* most pressing first: one reason per session */
export const DELIVERY_WAITS = Object.freeze(['ambiguous', 'failed', 'review', 'channel-stopped', 'channel-unverified', 'external', 'first-send', 'codex-signal', 'pi-input-unverified', 'authority-unverified']);
/* the ones the backend's plan decides; never read from hook state here */
const LIVE_DELIVERY_WAITS = Object.freeze(['first-send', 'codex-signal', 'pi-input-unverified', 'channel-stopped']);
/* the third, only once the backend says the hold has lasted */
const liveWait = plan => LIVE_DELIVERY_WAITS.includes(plan.stage)
  || (['authority-unverified', 'channel-unverified'].includes(plan.stage) && plan.lasting === true);

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
    if (!session || !liveWait(plan)) continue;
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
  const noted = card => openAgentNotes(card).length;
  const get = card => {
    const value = card && snapshots.get(card.id);
    return value?.session === card?.session ? value : null;
  };
  const category = card => {
    const value = get(card);
    if (!value) return 'unknown';
    if (!value.alive) return 'stopped';
    if (value.agent === 'needs-input') return 'input';
    /* a card kept quiet (notify.rs `CardLabel::quiet`): its routine
       endings are not attention rows either */
    if (value.agent === 'turn-done' && !value.seen) return card.quietDone === true ? 'other' : 'done';
    if (!value.agent) return 'unavailable';
    return 'other';
  };
  /* "Turn ended", the list batch close works from: a current observation
     of a live card whose agent ended its turn, viewed or not, quiet or
     not. A stale snapshot is never offered — it cannot say the turn is
     still over. Presentation only: offering a card closes nothing. */
  const ended = card => {
    const value = get(card);
    return !!value && value.alive && !value.stale && value.agent === 'turn-done';
  };
  const matches = (card, filter) => {
    const kind = category(card);
    return filter === 'all' || (filter === 'reminder' ? reminderDue(card) : filter === 'reminders' ? !!card.reminder : false) || (filter === 'followed' ? card.pinned === true
      : filter === 'pending' ? kind === 'input' || kind === 'done' || card.pinned === true || reminderDue(card) || !!waiting(card) || rang(card) || noted(card) > 0
      : filter === 'waiting' ? !!waiting(card)
      : filter === 'ended' ? ended(card)
      : filter === 'unavailable' ? kind === 'unavailable' || kind === 'unknown' : kind === filter);
  };
  return {
    get, category, matches, waiting, rang, ended, noted,
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
        const mark = info.card_mark;
        snapshots.set(card.id, {
          session: card.session, alive: info.alive, agent, status,
          codexSignal: info.alive && CODEX_SIGNAL_TRUST.includes(info.codex_signal) ? info.codex_signal : null,
          piSignal: info.alive && PI_COVERAGE.includes(info.pi_signal) ? info.pi_signal : null,
          idle: info.idle_secs ?? null, observedAt: now, stale: false, episode, episodeViewed,
          /* one line about what Claude said when this episode's turn ended
             (turn_summary.rs, opt-in): the backend sends it only for the
             current episode, so a new turn drops it */
          summary: episode != null && typeof info.summary === 'string' && info.summary.trim()
            ? info.summary.trim().slice(0, 200) : null,
          /* and, for a card with follow-ups, which one the model would pick
             (1-based) and why: it marks a button, nothing more */
          followChoice: episode != null && Number.isInteger(info.follow_choice) ? info.follow_choice : null,
          followReason: episode != null && typeof info.follow_reason === 'string' && info.follow_reason.trim()
            ? info.follow_reason.trim().slice(0, 120) : null,
          /* when this episode began (backend clock, ms): how long a turn
             has been over. Presentation only. */
          since: episode != null && Number.isSafeInteger(info.episode_since) ? info.episode_since : null,
          /* an MCP client's suggestion for this episode (mcp/cards.rs):
             presentation only, gone with the episode */
          mark: episode != null && mark && CARD_MARKS.includes(mark.mark)
            ? { mark: mark.mark, note: typeof mark.note === 'string' ? mark.note.slice(0, 200) : '', by: typeof mark.by === 'string' ? mark.by.slice(0, 120) : '' }
            : null,
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
      const counts = { all: cards.length, pending: 0, input: 0, waiting: 0, done: 0, ended: 0, followed: 0, unavailable: 0, stopped: 0, unknown: 0 };
      for (const card of cards) {
        const kind = category(card);
        if (kind in counts) counts[kind]++;
        if (ended(card)) counts.ended++;
        if (card.pinned === true) counts.followed++;
        if (waiting(card)) counts.waiting++;
        if (kind === 'input' || kind === 'done' || card.pinned === true || reminderDue(card) || waiting(card) || rang(card) || noted(card)) counts.pending++;
        if (kind === 'unknown') counts.unavailable++;
      }
      if (cards.some(card => card.reminder)) { counts.reminder = cards.filter(card => reminderDue(card)).length; counts.reminders = cards.filter(card => card.reminder).length; }
      return counts;
    },
    /* per filter, the cards it lists that carry an MCP client's mark:
       shown on the filter buttons, read by nothing else */
    marked(cards) {
      const marked = {};
      const carrying = cards.filter(card => get(card)?.mark);
      for (const filter of ATTENTION_FILTERS) marked[filter] = carrying.filter(card => matches(card, filter)).length;
      return marked;
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
/* How long ago a turn ended, as an i18n key and its count: minutes under
   an hour, hours under two days, then days. Null without a start. */
export function endedAgo(since, now) {
  if (!Number.isSafeInteger(since) || since <= 0) return null;
  const minutes = Math.max(0, Math.floor((now - since) / 60000));
  if (minutes < 60) return { key: 'attention.endedMinutes', count: Math.max(1, minutes) };
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return { key: 'attention.endedHours', count: hours };
  return { key: 'attention.endedDays', count: Math.floor(hours / 24) };
}

/* The ids among `rows` an MCP client marked can-close: what "Tick the
   suggested" adds to the user's ticks (nothing is ticked without a click). */
export function suggestedToClose(rows, tracker) {
  return rows.filter(row => tracker.get(row.card)?.mark?.mark === 'can-close').map(row => row.card.id);
}

/* the listed cards an MCP orchestrator opened, one group per driver in the
   order its first card is listed (`driverOf(card)`: workers.js
   `workerDriver` for a worker card, else null): the "Tick those … opened"
   buttons of "Turn ended". A card no orchestrator opened belongs to no
   group. Presentation only: a group ticks rows, the close still asks. */
export function driverGroups(rows, driverOf) {
  const groups = new Map();
  for (const row of rows) {
    const driver = driverOf(row.card);
    if (!driver) continue;
    if (!groups.has(driver.key)) groups.set(driver.key, { key: driver.key, name: driver.name, orphaned: driver.orphaned, ids: [] });
    groups.get(driver.key).ids.push(row.card.id);
  }
  return [...groups.values()];
}

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
          : tracker.noted?.(card) && !['input', 'done'].includes(category) ? 'notes'
          : reminderDue(card) && !['input', 'done'].includes(category) ? 'reminder' : filter === 'pending' && !['input', 'done'].includes(category) ? 'followed' : category;
        ordered.push({ card, project, column, kind });
      }
    }
  }
  if (filter === 'reminder' || filter === 'reminders') return ordered.sort((a, b) => a.card.reminder.dueAt - b.card.reminder.dueAt);
  /* "Turn ended": the longest-ended first; a turn without a known start last */
  if (filter === 'ended') {
    const since = row => tracker.get(row.card)?.since ?? Number.MAX_SAFE_INTEGER;
    return ordered.sort((a, b) => since(a) - since(b));
  }
  if (filter !== 'pending') return ordered;
  return Object.keys(ATTENTION_SECTION_LABELS).flatMap(kind => ordered.filter(row => row.kind === kind));
}
