// Desktop half of Connector command dispatch. Native owns the authenticated
// journal; this module is the only bridge into the serialized Board writer.
import { ctx, emit, inv, listen, store, uev } from './state.js';
import { provider } from './board.js';
import { mutateBoard } from './persistence.js';
import { addManual, addQueueCopy, bufferLimitError, deleteEntry, editEntry, emptyBuffer } from './buffer-model.js';
import { connectorBufferOperationId, connectorId, unfinishedConnectorPlans } from './connector-model.js';
import { expandHome } from './pure.js';
import { toast } from './dialogs.js';
import { t } from './i18n.js';

let draining = false; let again = false;
const complete = (handle, state, code = null, result = null) =>
  inv('connector_complete', { handle, state, code, result });
const revision = buffer => String(buffer?.revision || 0);
const notifyCard = cardId => { try { emit('list', provider.get(cardId)); } catch (_) {} };

function commandError(error, fallback = 'invalid-command') {
  const text = String(error?.message ?? error ?? '');
  if (/revision/i.test(text)) return 'revision-changed';
  if (/missing|not found|no longer exists/i.test(text)) return 'target-missing';
  if (/capacity|too many|full/i.test(text)) return 'capacity';
  if (/immutable/i.test(text)) return 'immutable';
  if (/unsupported.{0,24}target|supported.{0,12}agent|agent.only/i.test(text)) return 'unsupported-target';
  return fallback;
}

async function mutateBuffer(handle, request) {
  const cardId = request.cardId; let entryId = request.payload?.entryId || null; let savedRevision = null;
  let mutationPrepared = false;
  try { await mutateBoard(async draft => {
    await inv('connector_validate', { handle });
    const card = draft.cards.find(value => value.id === cardId);
    if (!card) throw new Error('card not found');
    const base = card.buffer || emptyBuffer();
    if (revision(base) !== request.expectedRevision) throw new Error('revision changed');
    let result;
    if (request.kind === 'buffer-add') {
      entryId = await connectorId('N', handle, 'entry');
      result = addManual(base, { id: entryId, text: request.payload.text, now: Date.now() });
    } else if (request.kind === 'buffer-edit') {
      result = editEntry(base, entryId, request.payload.text, Date.now());
    } else {
      if (!base.entries.some(entry => entry.id === entryId)) throw new Error('entry not found');
      result = { buffer: deleteEntry(base, entryId) };
    }
    if (result.error || bufferLimitError(result.buffer)) throw new Error(result.error || 'buffer capacity');
    mutationPrepared = true; card.buffer = result.buffer; savedRevision = revision(result.buffer);
  }); } catch (error) { if (error && typeof error === 'object') error.effectPossible = mutationPrepared; throw error; }
  notifyCard(cardId);
  try { await complete(handle, 'applied', null, { cardId, entryId, revision: savedRevision }); }
  catch (_) { await complete(handle, 'ambiguous', 'result-save-failed', null).catch(() => {}); }
}

async function queueBuffer(handle, request) {
  const cardId = request.cardId; const ids = request.payload?.entryIds || []; const prepared = [];
  let copiesPrepared = false;
  try { await mutateBoard(async draft => {
    await inv('connector_validate', { handle });
    const card = draft.cards.find(value => value.id === cardId);
    if (!card) throw new Error('card not found');
    let next = card.buffer || emptyBuffer();
    if (revision(next) !== request.expectedRevision) throw new Error('revision changed');
    for (const entryId of ids) {
      const operationId = await connectorBufferOperationId(handle, entryId);
      const result = addQueueCopy(next, entryId, operationId, Date.now());
      if (result.error) throw new Error(result.error);
      next = result.buffer; prepared.push({ entryId, operationId, text: result.copy.text, at: Math.floor(result.copy.createdAt / 1000) });
    }
    if (bufferLimitError(next)) throw new Error('buffer capacity');
    copiesPrepared = true; card.buffer = next;
  }); } catch (error) { if (error && typeof error === 'object') error.effectPossible = copiesPrepared; throw error; }
  let attempted = false; let finalRevision = null;
  try {
    await mutateBoard(async draft => {
      await inv('connector_validate_admission', { handle });
      const card = draft.cards.find(value => value.id === cardId);
      if (!card) throw new Error('card not found');
      const copies = (card.buffer?.entries || []).flatMap(entry => entry.copies || []);
      for (const item of prepared) {
        const copy = copies.find(value => value.operationId === item.operationId);
        if (!copy || copy.text !== item.text || copy.state !== 'uncertain') throw new Error('immutable copy missing');
        attempted = true;
        await inv('queue_add', { args: { session: card.session, cardId: card.id,
          operationId: copy.operationId, dir: card.dir, cmd: card.cmd, text: copy.text,
          mode: 'at', at: item.at } });
        copy.state = 'queued';
      }
      card.buffer.revision = (card.buffer.revision || 0) + 1;
      finalRevision = revision(card.buffer);
    });
  } catch (error) {
    await complete(handle, attempted ? 'ambiguous' : 'rejected', commandError(error), null).catch(() => {});
    return;
  }
  notifyCard(cardId);
  try { await complete(handle, 'applied', null, { cardId, revision: finalRevision, queued: prepared.length }); }
  catch (_) { await complete(handle, 'ambiguous', 'result-save-failed', null).catch(() => {}); }
}

async function reconcileConnectorCard(card) {
  if (!card?.connectorRun || card.connectorRun.initialQueued) return true;
  try { return await provider.queueConnectorPlan(card.id, card.connectorRun.handle); }
  catch (_) { toast(t('connector.queueFailed')); return false; }
}

async function createTask(handle, request) {
  const project = store.projects.find(value => value.id === request.payload?.projectId);
  const preset = project?.presets?.find(value => value.id === request.payload?.presetId);
  if (!project || !preset) throw new Error('preset not found');
  const id = await connectorId('S', handle, 'card');
  const operationIds = await Promise.all(preset.steps.map((_, index) => connectorId('B', handle, `step/${index}`)));
  const at = Math.floor(Date.now() / 1000);
  const initialSteps = preset.steps.map((text, index) => ({ operationId: operationIds[index], text,
    mode: index ? 'chain' : 'at', at: index ? null : at, tpl: preset.id,
    tplIdx: index + 1, tplTotal: preset.steps.length }));
  const connectorRun = { handle, presetId: preset.id, initialSteps, initialQueued: initialSteps.length === 0 };
  let card;
  try {
    ({ card } = await provider.createStarted({ id, projectId: project.id, columnId: preset.columnId,
      title: preset.title, dir: expandHome(preset.dir, ctx.HOME), cmd: preset.cmd, desc: preset.name,
      origin: { source: 'connector', key: handle, badge: preset.id }, connectorRun },
    { requireCreated: true, validateHandle: handle }));
  } catch (error) {
    const state = error?.stage === 'orphan' || error?.effectAttempted ? 'ambiguous' : 'rejected';
    await complete(handle, state, error?.stage === 'orphan' ? 'orphan-session' : commandError(error), null).catch(() => {});
    return;
  }
  try { await complete(handle, 'applied', null, { cardId: card.id }); }
  catch (_) { await complete(handle, 'ambiguous', 'result-save-failed', null).catch(() => {}); return; }
  await reconcileConnectorCard(card);
}

async function handlePending(pending) {
  let claimed;
  try { claimed = await inv('connector_claim', { handle: pending.handle }); } catch (_) { return; }
  const { handle, request } = claimed;
  try {
    if (['buffer-add', 'buffer-edit', 'buffer-delete'].includes(request.kind)) return await mutateBuffer(handle, request);
    if (request.kind === 'buffer-queue') return await queueBuffer(handle, request);
    if (request.kind === 'task-create') return await createTask(handle, request);
    if (['send-message', 'queue-pause', 'queue-cancel'].includes(request.kind)) {
      let attempted = false;
      try {
        await mutateBoard(async () => {
          await inv('connector_validate', { handle }); attempted = true;
          await inv('connector_execute_native', { handle });
          return { noop: true };
        });
      } catch (error) {
        if (attempted) await complete(handle, 'ambiguous', 'delivery-unknown', null).catch(() => {});
        else await complete(handle, 'rejected', commandError(error), null).catch(() => {});
      }
      return;
    }
    await complete(handle, 'rejected', 'unsupported-command', null);
  } catch (error) {
    await complete(handle, error?.effectPossible ? 'ambiguous' : 'rejected', commandError(error), null).catch(() => {});
  }
}

export async function drainConnector() {
  if (draining) { again = true; return; }
  draining = true;
  try {
    do {
      again = false;
      for (const card of unfinishedConnectorPlans(store.cards)) await reconcileConnectorCard(card);
      let pending; try { pending = await inv('connector_pending'); } catch (_) { return; }
      for (const item of pending || []) await handlePending(item);
    } while (again);
  } finally { draining = false; }
}

export function initConnector() {
  listen('connector-changed', drainConnector).catch(() => uev('listen-fail', 'connector-changed'));
  const timer = setInterval(drainConnector, 60_000); timer.unref?.();
}
