// Local, user-confirmed managed-session closure before shell replacement.
// Backend status is the authority; a changed review or uncertain close stops
// before the ordinary restart command. The backend rechecks under its gate.
export function managedBlockers(status) {
  return (status?.restartBlockers || []).filter(item => item.kind === 'managed-session');
}

const identities = items => items.map(item => `${item.cardId}\0${item.session}`).sort();
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
function closeFailure(code, cardId, closedCount) {
  const error = new Error(code);
  error.cardId = cardId;
  error.closedCount = closedCount;
  return error;
}

export async function closeManagedForRestart(review, { readStatus, getCard, closeCard, closePane }) {
  const expected = managedBlockers(review);
  if (!expected.length || expected.length !== (review.restartBlockers || []).length) {
    throw new Error('managed-review-changed');
  }
  let current;
  try { current = await readStatus(); }
  catch (_) { throw new Error('managed-status-unavailable'); }
  if (current.serverPid !== review.serverPid || current.serverStartedAt !== review.serverStartedAt ||
      current.impactToken !== review.impactToken || !same(identities(managedBlockers(current)), identities(expected)) ||
      managedBlockers(current).length !== (current.restartBlockers || []).length) {
    throw new Error('managed-review-changed');
  }
  let closedCount = 0;
  for (const blocker of [...expected].sort((a, b) => a.cardId.localeCompare(b.cardId))) {
    const card = getCard(blocker.cardId);
    if (!card || card.session !== blocker.session) throw closeFailure('managed-close-rejected', blocker.cardId, closedCount);
    let result;
    try { result = await closeCard(card.id); }
    catch (_) { throw closeFailure('managed-close-ambiguous', blocker.cardId, closedCount); }
    if (!result?.ok || !result.applied) throw closeFailure(
      result?.admitted || ['kill', 'cancel'].includes(result?.stage)
        ? 'managed-close-ambiguous' : 'managed-close-rejected', blocker.cardId, closedCount);
    closePane(card.id);
    let verified;
    try { verified = await readStatus(); }
    catch (_) { throw closeFailure('managed-close-ambiguous', blocker.cardId, closedCount); }
    if (verified.serverPid !== review.serverPid || verified.serverStartedAt !== review.serverStartedAt ||
        managedBlockers(verified).some(item => item.cardId === blocker.cardId || item.session === blocker.session) ||
        (verified.sessions || []).some(item => item.name === blocker.session) || getCard(card.id)) {
      throw closeFailure('managed-close-ambiguous', blocker.cardId, closedCount);
    }
    closedCount++;
  }
  let fresh;
  try { fresh = await readStatus(); }
  catch (_) { throw new Error('managed-status-unavailable'); }
  const managedNames = new Set(expected.map(item => item.session));
  const ordinary = (review.sessions || []).filter(item => !managedNames.has(item.name)).map(item => item.name).sort();
  if (fresh.serverPid !== review.serverPid || fresh.serverStartedAt !== review.serverStartedAt ||
      (fresh.restartBlockers || []).length || !same((fresh.sessions || []).map(item => item.name).sort(), ordinary)) {
    throw new Error('managed-review-changed');
  }
  return fresh;
}
