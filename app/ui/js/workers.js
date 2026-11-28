// workers.js — worker cards: the cards an orchestrating MCP client drives
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Backend-owned list (`mcp_workers_ui`, mcp/workers.rs): which cards are
//   worker cards, the name of the client that drives each, and whether the
//   user took it over. Pulled at start and on the content-free
//   `mcp-workers-changed` event; nothing is stored here across launches.
// - Labels only: a worker card says "Driven by ‹orchestrator card›" (the
//   card its client's call came from, mcp/origin.rs; else the client's name)
//   on the Board and in the session header; once taken over it offers "Hand
//   back", the one way back (no timer hands a card back). A worker whose
//   orchestrator card left the Board is the user's (the backend holds its
//   lines the same way): it says so and offers the same "Hand back", which
//   gives it to all of the client's orchestrators.
// - Taking over: the user's own input into a worker card's pane (typing,
//   paste, a dropped file — never xterm's automatic replies, which
//   terminal-input.js tells apart) tells the backend once
//   (`mcp_worker_take_over`). The backend then holds the client's lines for
//   that card (`Hold::Human`); this module only reports the fact.
// - A worker card that reported to its orchestrator (`reportedAt`) says so
//   on its label, with the time; the report's text never reaches here.
//   While the newest report waits for that orchestrator's acknowledgment
//   (`reportPending`, `deck_worker_reports_ack`, mcp/reports.rs) the label
//   says "new report · not yet received" instead: the newest one only,
//   received, never read, reviewed or done.
// - Needs attention shows the same label on a worker card's row and groups
//   the cards one driver opened (`workerDriver`) for its "Tick those …
//   opened" button (attention.js).
// - A leaf: `render` arrives through `initWorkers(deps)`.
import { $, inv, listen, store, uev } from './state.js';
import { t } from './i18n.js';
import { toast } from './dialogs.js';
import { fmtClock } from './scheduler-model.js';

let render = () => {};
let workers = new Map();
let started = null;

/* the worker view of one card, if it is one */
export function workerOf(cardId) {
  return workers.get(cardId) || null;
}

export async function refreshWorkers() {
  let list;
  try { list = await inv('mcp_workers_ui'); } catch (_) { return; }
  const next = new Map((Array.isArray(list) ? list : []).map(w => [w.cardId, w]));
  const changed = next.size !== workers.size
    || [...next].some(([id, w]) => JSON.stringify(w) !== JSON.stringify(workers.get(id)));
  workers = next;
  if (changed) render();
}

/* the user's own input reached `session`'s pane: a worker card is taken over */
export function noteHumanInput(session) {
  const card = store.cards.find(c => c.session === session);
  const worker = card && workers.get(card.id);
  if (!worker || worker.takenOver) return;
  workers.set(card.id, { ...worker, takenOver: true });
  inv('mcp_worker_take_over', { cardId: card.id })
    .then(changed => {
      if (changed) { uev('workers', 'taken-over'); toast(t('workers.tookOver', { name: worker.clientName })); }
    })
    .catch(() => {})
    .finally(() => refreshWorkers());
  render();
}

export async function handBack(cardId) {
  const worker = workers.get(cardId);
  if (!worker) return;
  try {
    await inv('mcp_worker_hand_back', { cardId });
    uev('workers', 'handed-back');
    toast(t('workers.handedBack', { name: worker.clientName }));
  } catch (_) { toast(t('mcp.actionFailed')); }
  await refreshWorkers();
}

/* who drives a worker card: the orchestrator card while it is on the
   Board, else the client; `orphaned` once that card is gone. `key` groups
   the cards one driver opened (Needs attention's "Tick those … opened") */
export function workerDriver(worker, cards) {
  const owner = worker.orchestratorCardId
    ? cards.find(c => c.id === worker.orchestratorCardId) : null;
  const orphaned = Boolean(worker.orchestratorCardId) && !owner;
  return {
    key: owner ? `card:${owner.id}` : `${orphaned ? 'left' : 'client'}:${worker.clientName}`,
    name: (owner && owner.title) || worker.clientName,
    orphaned,
  };
}

/* who drives a worker card, as its label says it (`workerDriver`) */
export function workerLabel(worker, cards, now = new Date()) {
  const { name, orphaned } = workerDriver(worker, cards);
  const takenOver = worker.takenOver || orphaned;
  const key = orphaned ? 'workers.orphaned' : takenOver ? 'workers.takenOver' : 'workers.driven';
  const title = orphaned ? 'workers.orphanedTitle' : takenOver ? 'workers.takenOverTitle' : 'workers.drivenTitle';
  /* the card handed Deck a report for its orchestrator (mcp/reports.rs):
     that it did and when, never what it says */
  /* while its orchestrator has not acknowledged the newest one
     (`reportPending`, mcp/reports.rs `awaiting_ack`) it says so instead */
  const pending = worker.reportPending === true && Boolean(worker.reportedAt) && !takenOver;
  /* the receipt of the newest report (`receiptState`, mcp/reports.rs
     `receipt_state`): an untracked receipt (`unknown`, reports from before
     acknowledgments) is never shown as received, and `acknowledged` says
     received, never reviewed or handled */
  const receipt = takenOver ? '' : worker.receiptState;
  const [stateKey, stateTitle] = pending ? ['workers.reportPending', 'workers.reportPendingTitle']
    : receipt === 'unknown' ? ['workers.reportUnknown', 'workers.reportUnknownTitle']
    : receipt === 'acknowledged' ? ['workers.reportReceived', 'workers.reportReceivedTitle']
    : ['workers.reported', 'workers.reportedTitle'];
  const time = worker.reportedAt ? fmtClock(Math.floor(worker.reportedAt / 1000), now) : '';
  const reported = !worker.reportedAt ? '' : t(stateKey, { time });
  const titleText = t(title, { name, client: worker.clientName });
  return {
    takenOver, orphaned, pending,
    text: reported ? `${t(key, { name })} · ${reported}` : t(key, { name }),
    title: reported ? `${titleText} ${t(stateTitle)}` : titleText,
  };
}

/* a card's label: plain while driven, a button once taken over */
export function workerChip(s) {
  const worker = workerOf(s.id);
  if (!worker) return null;
  const label = workerLabel(worker, store.cards);
  const chip = document.createElement(label.takenOver ? 'button' : 'span');
  chip.className = 'card-worker' + (label.takenOver ? ' taken' : '') + (label.pending ? ' report-pending' : '');
  chip.textContent = label.text;
  chip.title = label.title;
  if (label.takenOver) {
    chip.type = 'button';
    chip.onclick = e => { e.stopPropagation(); handBack(s.id); };
  }
  return chip;
}

/* the session header's label for the open card */
export function paintWorkerButton(cardId) {
  const btn = $('worker-btn');
  if (!btn) return;
  const worker = workerOf(cardId);
  btn.hidden = !worker;
  if (!worker) return;
  const label = workerLabel(worker, store.cards);
  btn.textContent = label.text;
  btn.title = label.title;
  btn.disabled = !label.takenOver;
  btn.onclick = () => {
    const now = workerOf(cardId);
    if (now && workerLabel(now, store.cards).takenOver) handBack(cardId);
  };
}

export function initWorkers(deps) {
  ({ render } = deps);
}

/* the listener and the first pull; called once the Board is the user's */
export function startWorkers() {
  if (started) return started;
  started = listen('mcp-workers-changed', refreshWorkers)
    .catch(() => uev('listen-fail', 'mcp-workers-changed'))
    .then(refreshWorkers);
  return started;
}
