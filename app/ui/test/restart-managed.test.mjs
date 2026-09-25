import test from 'node:test';
import assert from 'node:assert/strict';
import { closeManagedForRestart, managedBlockers } from '../js/restart-managed.js';

const blocker = (cardId = 'M1') => ({ kind: 'managed-session', cardId, session: `deck-${cardId}` });
const impact = (name, count = 1) => ({ name, paneCount: count });
const status = (blockers = [blocker()], token = 'old') => ({
  serverPid: 42, serverStartedAt: 123, impactToken: token, restartBlockers: blockers,
  sessions: [impact('deck-M1'), impact('deck-ordinary')].filter(item =>
    item.name !== 'deck-M1' || blockers.some(b => b.session === item.name)),
  sessionCount: blockers.length + 1, paneCount: blockers.length + 1,
});

test('managed closure uses Board close, verifies status, and returns fresh impact', async () => {
  const review = status();
  const fresh = status([], 'fresh');
  const calls = [];
  let card = { id: 'M1', session: 'deck-M1' };
  const result = await closeManagedForRestart(review, {
    readStatus: async () => { calls.push('status'); return calls.includes('close') ? fresh : review; },
    getCard: () => card,
    closeCard: async () => { calls.push('close'); card = null; return { ok: true, applied: true }; },
    closePane: () => calls.push('pane'),
  });
  assert.equal(result, fresh);
  assert.deepEqual(calls, ['status', 'close', 'pane', 'status', 'status']);
  assert.equal(result.impactToken, 'fresh');
});

test('a rejected or ambiguous Board close stops before restart', async () => {
  for (const result of [{ ok: false, applied: false }, { ok: false, applied: false, admitted: true }]) {
    let reads = 0;
    await assert.rejects(closeManagedForRestart(status(), {
      readStatus: async () => { reads++; return status(); },
      getCard: () => ({ id: 'M1', session: 'deck-M1' }),
      closeCard: async () => result,
      closePane: () => assert.fail('no pane close'),
    }), error => {
      assert.equal(error.message, result.admitted ? 'managed-close-ambiguous' : 'managed-close-rejected');
      assert.equal(error.cardId, 'M1');
      assert.equal(error.closedCount, 0);
      return true;
    });
    assert.equal(reads, 1);
  }
});

test('new blocker at confirmation or during closure requires a new review', async () => {
  assert.equal(managedBlockers(status()).length, 1);
  await assert.rejects(closeManagedForRestart(status(), {
    readStatus: async () => status([blocker(), blocker('M2')]),
  }), { message: 'managed-review-changed' });
  let closed = false;
  await assert.rejects(closeManagedForRestart(status(), {
    readStatus: async () => closed ? status([blocker('M2')], 'fresh') : status(),
    getCard: () => closed ? null : { id: 'M1', session: 'deck-M1' },
    closeCard: async () => { closed = true; return { ok: true, applied: true }; },
    closePane: () => {},
  }), { message: 'managed-review-changed' });
});

test('unexpected server identity after close fails closed', async () => {
  let closed = false;
  const changed = { ...status([], 'fresh'), serverPid: 43 };
  await assert.rejects(closeManagedForRestart(status(), {
    readStatus: async () => closed ? changed : status(),
    getCard: () => closed ? null : { id: 'M1', session: 'deck-M1' },
    closeCard: async () => { closed = true; return { ok: true, applied: true }; },
    closePane: () => {},
  }), { message: 'managed-close-ambiguous' });
});
