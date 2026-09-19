import test from 'node:test';
import assert from 'node:assert/strict';
import { createVoiceTarget } from '../js/voice-target.js';

const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { resolve, promise }; };
function fixture() {
  const pane = { attached: true }, calls = [];
  const panes = new Map([['a', pane], ['b', { attached: true }]]);
  const cards = new Set(['a', 'b']);
  const waits = [];
  const adapter = createVoiceTarget({
    getPane: name => panes.get(name), hasCard: id => cards.has(id),
    cancelSelection: async p => { calls.push(['selection', p]); await waits.shift()?.promise; },
    scrollBottom: async session => { calls.push(['scroll', session]); await waits.shift()?.promise; },
    setDelivering: session => calls.push(['gate', session]), resetInput: () => calls.push(['reset']),
  });
  return { adapter, pane, panes, cards, calls, waits };
}
const target = { session: 'a', cardId: 'a', id: 1 };

test('preparation orders selection cleanup before scroll; only the owner can release the input gate', async () => {
  const f = fixture();
  await f.adapter.prepareTarget(target);
  f.adapter.afterDelivery({ ...target });
  assert.deepEqual(f.calls, [['gate', 'a'], ['selection', f.pane], ['scroll', 'a']]);
  f.adapter.afterDelivery(target); f.adapter.afterDelivery(target);
  assert.deepEqual(f.calls.slice(-2), [['gate', null], ['reset']]);
  assert.equal(f.calls.filter(([kind]) => kind === 'reset').length, 1);
});

test('old preparation and cleanup cannot release a new recording, including the same pane', async () => {
  for (const session of ['a', 'b']) {
    const f = fixture(), old = deferred(); f.waits.push(old);
    const pending = f.adapter.prepareTarget(target);
    const next = { session, cardId: session, id: 2 };
    await f.adapter.prepareTarget(next);
    old.resolve();
    await assert.rejects(pending, error => error === 'target-not-visible');
    f.adapter.afterDelivery(target);
    assert.equal(f.calls.filter(([kind]) => kind === 'reset').length, 0);
    assert.deepEqual(f.calls.filter(([kind]) => kind === 'scroll'), [['scroll', session]]);
    f.adapter.afterDelivery(next);
    assert.deepEqual(f.calls.slice(-2), [['gate', null], ['reset']]);
  }
});

test('missing, detached, replaced or removed targets are rejected before typing', async () => {
  const missing = fixture(); missing.panes.delete('a');
  await assert.rejects(missing.adapter.prepareTarget(target), error => error === 'target-not-visible');
  assert.deepEqual(missing.calls, []);
  for (const stage of ['selection', 'scroll']) {
    for (const change of ['detach', 'replace', 'remove']) {
      const f = fixture(), wait = deferred();
      if (stage === 'scroll') f.waits.push(null);
      f.waits.push(wait);
      const pending = f.adapter.prepareTarget(target);
      await Promise.resolve(); await Promise.resolve();
      if (change === 'detach') f.pane.attached = false;
      if (change === 'replace') f.panes.set('a', { attached: true });
      if (change === 'remove') f.cards.delete('a');
      wait.resolve();
      await assert.rejects(pending, error => error === 'target-not-visible');
      f.adapter.afterDelivery(target);
      assert.deepEqual(f.calls.slice(-2), [['gate', null], ['reset']]);
    }
  }
});
