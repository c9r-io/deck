// attention-model.js — runtime-only observations and derived attention.
// No Board writes, hook installation, inferred readiness, or durable ordering.
// A snapshot belongs to a card AND its session identity. Missing/failed polls
// retain explicitly stale observations. Read means successfully displayed,
// never handled; a new observed status re-arms it. No turn IDs exist, so an
// unobserved working→done between polls cannot be distinguished from a repeat.
import { CARD_QUIET_SECS, effectiveCardStatus } from './pure.js';

export const ATTENTION_FILTERS = Object.freeze(['pending', 'input', 'done', 'unavailable', 'stopped']);

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
    return filter === 'all' || (filter === 'pending' ? kind === 'input' || kind === 'done'
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
        snapshots.set(card.id, {
          session: card.session, alive: info.alive, agent, status,
          idle: info.idle_secs ?? null, observedAt: now, stale: false,
          seen: !!((old?.status === status && old?.agent === agent && old.seen) || visible.has(card.id)),
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
    freshness(cards) {
      const known = cards.filter(card => get(card));
      return {
        kind: !known.length && cards.length ? 'unknown'
          : failed || known.length !== cards.length || known.some(card => get(card).stale) ? 'stale' : 'fresh',
        lastSuccess,
      };
    },
    counts(cards) {
      const counts = { all: cards.length, pending: 0, input: 0, done: 0, unavailable: 0, stopped: 0, unknown: 0 };
      for (const card of cards) {
        const kind = category(card);
        if (kind in counts) counts[kind]++;
        if (kind === 'input' || kind === 'done') counts.pending++;
        if (kind === 'unknown') counts.unavailable++;
      }
      return counts;
    },
  };
}

// Presentation order only. Never sort the durable card/project arrays.
export function attentionRows(projects, cards, tracker, filter) {
  const ordered = [];
  const added = new Set();
  for (const project of projects) for (const column of project.columns) {
    for (const card of cards) {
      if (card.projectId !== project.id || card.columnId !== column.id || added.has(card.id)) continue;
      added.add(card.id);
      if (tracker.matches(card, filter)) ordered.push({ card, project, column, kind: tracker.category(card) });
    }
  }
  if (filter !== 'pending') return ordered;
  return [...ordered.filter(row => row.kind === 'input'), ...ordered.filter(row => row.kind === 'done')];
}
