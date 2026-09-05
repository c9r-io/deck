// persistence.js — one global Board transaction queue
// Part of deck's no-build frontend: native ES modules, no bundler.
import { ctx, inv, store } from './state.js';
import { createSerialTransactionQueue } from './pure.js';

const PERSISTED_CARD_KEYS = new Set([
  'id', 'projectId', 'columnId', 'title', 'desc', 'cmd', 'dir', 'session', 'pinned', 'origin',
]);

/* A card created by 自动响应 remembers which external item it came from, so
   the same badge on the same message can never create a second card. Only
   identifiers are kept — never the message. */
export function cardOrigin(c) {
  const o = c && c.origin;
  if (!o || typeof o !== 'object') return undefined;
  const { source, key, badge } = o;
  if (typeof source !== 'string' || typeof key !== 'string' || typeof badge !== 'string') return undefined;
  return { source, key, badge };
}

export function boardData(projects = store.projects, cards = store.cards) {
  return {
    projects,
    cards: cards.map(c => ({
      id: c.id, projectId: c.projectId, columnId: c.columnId,
      title: c.title, desc: c.desc || '', cmd: c.cmd, dir: c.dir, session: c.session,
      pinned: c.pinned === true,
      ...(cardOrigin(c) ? { origin: cardOrigin(c) } : {}),
    })),
  };
}

function snapshotBoard() {
  return { projects: store.projects, cards: store.cards };
}

/* Polling may update runtime-only fields while the disk write is in flight.
   Preserve those newest observations when the durable candidate commits. */
function commitBoard(candidate) {
  const live = new Map(store.cards.map(c => [c.id, c]));
  candidate.cards = candidate.cards.map(card => {
    const current = live.get(card.id);
    if (!current) return card;
    const runtime = {};
    for (const [key, value] of Object.entries(current)) {
      if (!PERSISTED_CARD_KEYS.has(key)) runtime[key] = value;
    }
    return { ...card, ...runtime };
  });
  store.projects = candidate.projects;
  store.cards = candidate.cards;
}

const transactions = createSerialTransactionQueue({
  snapshot: snapshotBoard,
  serialize: candidate => JSON.stringify(boardData(candidate.projects, candidate.cards), null, 2),
  persist: (_candidate, json) => inv('save_board', { data: json }),
  commit: commitBoard,
});

let debounced = [];

function enqueuePendingDebounced() {
  clearTimeout(ctx.saveTimer);
  ctx.saveTimer = null;
  if (!debounced.length) return null;
  const batch = debounced;
  debounced = [];
  const pending = transactions.enqueue(async candidate => {
    let last;
    for (const entry of batch) last = await entry.mutate(candidate);
    return last;
  });
  pending.then(
    () => batch.forEach(entry => entry.onCommit && entry.onCommit()),
    error => batch.forEach(entry => entry.onError && entry.onError(error)),
  );
  return pending;
}

/** Pending debounced edits are ordered immediately before this barrier. */
export function mutateBoard(mutate) {
  enqueuePendingDebounced();
  return transactions.enqueue(mutate);
}

export function mutateBoardDebounced(mutate, { delay = 300, onCommit, onError } = {}) {
  debounced.push({ mutate, onCommit, onError });
  clearTimeout(ctx.saveTimer);
  ctx.saveTimer = setTimeout(() => {
    const pending = enqueuePendingDebounced();
    if (pending) pending.catch(() => {});
  }, delay);
}

export async function flushBoardMutations() {
  const pending = enqueuePendingDebounced();
  if (pending) await pending;
  await transactions.idle();
}
