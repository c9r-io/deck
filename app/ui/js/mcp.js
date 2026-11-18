// Desktop bridge for Deck MCP Board operations. Native owns authorization,
// scopes and the durable operation ledger; this module is the only MCP path
// into the Board's serialized persist-before-commit transaction.
//
// Close outcomes: refused before admission (card shown in a pane, plan no
// longer current) → rejected; failed after admission (a queue/tmux side
// effect may have run) → ambiguous, never rejected; committed only after the
// Board write. A failed report to native is surfaced as a toast with no free
// text — native keeps the operation pending/admitted and restart turns it
// ambiguous, so nothing is guessed. Pending work that predates a Deck restart
// is never replayed here: native marks it ambiguous on load.
import { inv, listen, state } from './state.js';
import { provider } from './board.js';
import { toast } from './dialogs.js';
import { t } from './i18n.js';

let draining = false;
let again = false;

async function finish(operationId, state, code = null, tmuxSession = null) {
  try {
    await inv('mcp_complete', { operationId, state, code, tmuxSession });
  } catch (_) {
    toast(t('mcp.boardSyncFailed'));
  }
}

async function createSession(pending) {
  const plan = pending.result || {};
  const existing = provider.get(plan.cardId);
  if (existing) {
    await finish(pending.operationId, 'committed', null, existing.session);
    return;
  }
  const project = provider.project(plan.projectId);
  const column = project?.columns?.find(value => value.id === project.selected) || project?.columns?.[0];
  if (!project || !column) {
    await finish(pending.operationId, 'rejected', 'project-missing');
    return;
  }
  try {
    const { card } = await provider.createStarted({
      id: plan.cardId,
      projectId: project.id,
      columnId: column.id,
      title: plan.title,
      cmd: '',
      dir: plan.cwd,
      desc: 'MCP managed shell',
      origin: { source: 'mcp', key: pending.operationId, badge: 'managed' },
    }, {
      requireCreated: true,
      start: card => inv('mcp_start_session', {
        operationId: pending.operationId,
        name: card.session,
        dir: card.dir,
      }),
      validateHandle: null,
    });
    await finish(pending.operationId, 'committed', null, card.session);
  } catch (error) {
    await finish(pending.operationId, error?.effectAttempted ? 'ambiguous' : 'rejected',
      error?.stage === 'orphan' ? 'orphan-session' : 'create-failed');
  }
}

/* a worker card a driving client asked for (`deck_worker_start`,
   mcp/workers.rs): an ordinary card in the type's column, its command the
   user's own for that type, started like any card; native queues its first
   line once this is committed. Quiet routine turn endings, as every worker
   card's. */
async function startWorker(pending) {
  const plan = pending.result || {};
  const existing = provider.get(plan.cardId);
  if (existing) {
    await finish(pending.operationId, 'committed', null, existing.session);
    return;
  }
  const project = provider.project(plan.projectId);
  const column = project?.columns?.find(value => value.id === plan.columnId) || project?.columns?.[0];
  if (!project || !column) {
    await finish(pending.operationId, 'rejected', 'project-missing');
    return;
  }
  try {
    const { card } = await provider.createStarted({
      id: plan.cardId,
      projectId: project.id,
      columnId: column.id,
      title: plan.title,
      cmd: plan.cmd,
      dir: plan.dir,
      origin: { source: 'worker', key: pending.operationId, badge: plan.type },
      quietDone: true,
    }, { requireCreated: true });
    await finish(pending.operationId, 'committed', null, card.session);
  } catch (error) {
    await finish(pending.operationId, error?.effectAttempted ? 'ambiguous' : 'rejected',
      error?.stage === 'orphan' ? 'orphan-session' : 'create-failed');
  }
}

export function closeOutcome(result) {
  if (result.ok) return { state: 'committed', code: null };
  if (result.stage === 'reminder-protected') return { state: 'rejected', code: 'reminder-protected' };
  if (result.stage === 'shown') return { state: 'rejected', code: 'card-shown' };
  if (result.admitted) return { state: 'ambiguous', code: 'close-failed' };
  return { state: 'rejected', code: 'close-failed' };
}

async function closeSession(pending) {
  const card = provider.get(pending.result?.cardId);
  if (!card) {
    await finish(pending.operationId, 'committed');
    return;
  }
  const result = await provider.close(card.id, {
    detail: true, quiet: true, mcpOperationId: pending.operationId,
  });
  const outcome = closeOutcome(result);
  await finish(pending.operationId, outcome.state, outcome.code);
}

/* one card update a client asked for (`deck_card_update`, mcp/cards.rs):
   the rename and the move in ONE Board transaction, refused whole when
   the card is no longer where or named as the client saw it, or may not
   leave its project now (pure.js `projectMoveBlock`). A refusal wrote
   nothing, so it is `rejected`; a save that threw is unknown (`handle`).
   A move tells the user only while they look at the Board it left or
   reached. */
export const cardUpdateRefusal = reason => ({
  changed: 'card-changed', starting: 'card-starting', collecting: 'card-collecting', managed: 'card-managed',
}[reason] || 'card-missing');

async function updateCard(pending) {
  const plan = pending.result || {};
  const moving = typeof plan.columnId === 'string';
  const outcome = await provider.updateCard(plan.cardId, {
    ...(moving ? { projectId: plan.projectId, columnId: plan.columnId } : {}),
    ...(typeof plan.title === 'string' ? { title: plan.title } : {}),
    expect: { projectId: plan.fromProjectId, ...(moving ? { columnId: plan.fromColumnId } : {}),
      ...(typeof plan.fromTitle === 'string' ? { title: plan.fromTitle } : {}) },
  });
  if (!outcome.ok) {
    await finish(pending.operationId, 'rejected', cardUpdateRefusal(outcome.reason));
    return;
  }
  await finish(pending.operationId, 'committed');
  const card = provider.get(plan.cardId);
  if (outcome.moved && card && state.view === 'board'
    && [plan.fromProjectId, plan.projectId].includes(state.projectId)) {
    toast(t('mcp.cardMoved', { client: plan.client || 'MCP', title: card.title, project: outcome.project, column: outcome.column }));
  }
}

async function handle(item) {
  let pending;
  try { pending = await inv('mcp_claim', { operationId: item.operationId }); } catch (_) { return; }
  try {
    if (pending.kind === 'session-create') return await createSession(pending);
    if (pending.kind === 'session-close') return await closeSession(pending);
    if (pending.kind === 'worker-start') return await startWorker(pending);
    if (pending.kind === 'card-update') return await updateCard(pending);
    await finish(pending.operationId, 'rejected', 'unsupported-operation');
  } catch (_) {
    await finish(pending.operationId, 'ambiguous', 'board-transaction-unknown');
  }
}

export async function drainMcp() {
  if (draining) { again = true; return; }
  draining = true;
  try {
    do {
      again = false;
      let pending;
      try { pending = await inv('mcp_pending'); } catch (_) { return; }
      for (const item of pending || []) await handle(item);
    } while (again);
  } finally { draining = false; }
}

export function initMcp() {
  listen('mcp-changed', drainMcp).catch(() => {});
  const timer = setInterval(drainMcp, 15_000);
  timer.unref?.();
}
