// Desktop bridge for Deck MCP Board operations. Native owns authorization,
// scopes and the durable operation ledger; this module is the only MCP path
// into the Board's serialized persist-before-commit transaction.
import { inv, listen } from './state.js';
import { provider } from './board.js';

let draining = false;
let again = false;

async function finish(operationId, state, code = null, tmuxSession = null) {
  return inv('mcp_complete', { operationId, state, code, tmuxSession });
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
      error?.stage === 'orphan' ? 'orphan-session' : 'create-failed').catch(() => {});
  }
}

async function closeSession(pending) {
  const card = provider.get(pending.result?.cardId);
  if (!card) {
    await finish(pending.operationId, 'committed');
    return;
  }
  // Revalidate the principal, generation, control epoch and current revoke
  // state immediately before entering the authoritative Board close path.
  await inv('mcp_validate', { operationId: pending.operationId });
  const result = await provider.close(card.id, { detail: true, quiet: true });
  await finish(pending.operationId, result.ok ? 'committed' : 'rejected',
    result.ok ? null : 'close-failed').catch(() => {});
}

async function handle(item) {
  let pending;
  try { pending = await inv('mcp_claim', { operationId: item.operationId }); } catch (_) { return; }
  try {
    if (pending.kind === 'session-create') return await createSession(pending);
    if (pending.kind === 'session-close') return await closeSession(pending);
    await finish(pending.operationId, 'rejected', 'unsupported-operation');
  } catch (_) {
    await finish(pending.operationId, 'ambiguous', 'board-transaction-unknown').catch(() => {});
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
