import test from 'node:test';
import assert from 'node:assert/strict';
import {
  heldSessions, planView, projectRecovery, recordedLabel, recoveryOffer, recoveryPlan, recoveryRequest,
  withAllSelected, withSelection,
} from '../js/recovery-model.js';
import { setLocale, t } from '../js/i18n.js';

const say = line => t(line.key, line.params);
const entry = (id, blocked = null, recordedAt = 1_800_000_000) => ({ cardId: id, session: `deck-${id}`, source: 'claude-code', blocked, recordedAt });
const raw = (entries, token = 'T1') => ({ token, boardKnown: true, entries });

test('an offer is read only through the fields this contract names', () => {
  assert.equal(recoveryOffer(null), null);
  assert.equal(recoveryOffer({ token: null, entries: [entry('A')] }), null, 'no token: nothing to ask');
  assert.equal(recoveryOffer(raw([])), null, 'no entries: nothing to ask');
  const offer = recoveryOffer(raw([
    { ...entry('A'), nativeId: 'x', executable: '/x' },
    entry('B', 'shell-open'),
    entry('C', 'a-word-this-build-does-not-know'),
    { session: 'deck-Z' },
  ]));
  assert.deepEqual(offer, {
    token: 'T1',
    entries: [
      { cardId: 'A', session: 'deck-A', blocked: null, recordedAt: 1_800_000_000 },
      { cardId: 'B', session: 'deck-B', blocked: 'shell-open', recordedAt: 1_800_000_000 },
      { cardId: 'C', session: 'deck-C', blocked: 'unknown', recordedAt: 1_800_000_000 },
    ],
  });
});

test('every tickable card starts ticked; a blocked card can never be ticked', () => {
  const offer = recoveryOffer(raw([entry('A'), entry('B', 'directory-missing'), entry('C')]));
  let plan = recoveryPlan(offer);
  assert.deepEqual(plan.selected, ['A', 'C']);
  assert.deepEqual(recoveryRequest(plan), { token: 'T1', cards: ['A', 'C'] });
  assert.deepEqual([...heldSessions(offer)], ['deck-A', 'deck-C'], 'only cards that can be recovered are held back');
  plan = withSelection(plan, 'B', true);
  assert.deepEqual(plan.selected, ['A', 'C'], 'blocked');
  plan = withSelection(plan, 'A', false);
  assert.deepEqual(plan.selected, ['C']);
  assert.equal(planView(plan).allSelected, false);
  plan = withAllSelected(plan, false);
  assert.equal(recoveryRequest(plan), null, 'nothing ticked: nothing to send');
  assert.equal(planView(plan).confirmEnabled, false);
  plan = withAllSelected(plan, true);
  assert.deepEqual(plan.selected, ['A', 'C']);
  assert.equal(planView(plan).allSelected, true);
  assert.equal(recoveryRequest(recoveryPlan(null)), null);
});

test('rows say when the session was recorded, or why a card cannot be recovered', () => {
  setLocale('en');
  const now = new Date(2026, 9, 24, 15, 0).getTime();
  const today = new Date(2026, 9, 24, 14, 32).getTime() / 1000;
  const yesterday = new Date(2026, 9, 23, 9, 5).getTime() / 1000;
  assert.equal(recordedLabel(today, now), '14:32');
  assert.equal(recordedLabel(yesterday, now), '10-23 09:05');
  assert.equal(recordedLabel(null, now), null);
  const plan = recoveryPlan(recoveryOffer(raw([
    entry('A', null, today), entry('B', 'observation-off'), entry('C', 'program-missing'), entry('D', 'shell-open'),
    entry('E', 'directory-missing'), entry('F', 'nonsense'),
  ])));
  const view = planView(plan, now);
  assert.deepEqual(view.rows.map(row => [row.cardId, row.selectable, row.checked, say(row.note)]), [
    ['A', true, true, 'Session as of 14:32'],
    ['B', false, false, 'Claude session re-entry observation is off'],
    ['C', false, false, 'The Claude program is no longer there'],
    ['D', false, false, 'The card’s shell is already open'],
    ['E', false, false, 'Its working directory no longer exists'],
    ['F', false, false, 'Cannot be recovered'],
  ]);
  assert.equal(say(view.confirm), 'Recover 1');
  setLocale('zh-Hans');
  assert.equal(say(view.confirm), '恢复 1 个');
  setLocale('en');
});

test('the result is the recovery’s own, with the restart’s per-card rows', () => {
  setLocale('en');
  const result = {
    requestId: 'V1', state: 'complete', serverReplaced: false,
    entries: [
      { cardId: 'A', session: 'deck-A', source: 'claude-code', outcome: 'confirmed', reason: null, warnings: [], shell: true, clientObserved: true, settled: true },
      { cardId: 'B', session: 'deck-B', source: 'claude-code', outcome: 'not-attempted', reason: 'user-action-required', warnings: [], shell: false, clientObserved: false, settled: true },
    ],
  };
  const view = projectRecovery(result, ['A', 'B', 'C']);
  assert.equal(say(view.title), 'Interrupted sessions');
  assert.ok(!/restart/i.test(say(view.title)), 'a recovery is not a restart');
  assert.equal(say(view.summary), '1 recovered · 2 to check');
  assert.deepEqual(view.rows.map(row => [row.cardId, row.tone, say(row.line)]), [
    ['A', 'ok', 'Claude session re-entry confirmed. No bypass flag was added.'],
    ['B', 'info', 'Re-entry was not attempted'],
    ['C', 'warn', 'The re-entry result is not known'],
  ]);
  assert.match(say(view.footer[0]), /does not offer these sessions again/);
  const lost = projectRecovery(null, ['A']);
  assert.deepEqual(lost.rows.map(row => row.tone), ['warn']);
  assert.equal(projectRecovery(null, []).summary, null);
});
