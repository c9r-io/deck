import test from 'node:test';
import assert from 'node:assert/strict';
import { reminderShort, localCandidates, localParts, shortcutTime, noteValid, reminderDue, reminderClaim, sameReminder, reminderAction, retirementKey, rememberRetirement, retirementBlocked, reminderRequestId, reminderTick } from '../js/reminder-model.js';
import { createAttentionTracker, attentionRows } from '../js/attention-model.js';
const card = () => ({ id: 'card-a', session: 'same-name', reminder: { id: 'unique', revision: 1, dueAt: 2000, due: false, timeZone: 'Asia/Tokyo', note: '', inAppOnly: false } });
test("the card label is the time today, month/day and time otherwise, in the reminder's own zone", () => {
  const at = (iso, timeZone = 'Asia/Tokyo') => ({ dueAt: Date.parse(iso), timeZone });
  const now = Date.parse('2026-10-05T00:30:00Z');   // 09:30 on 5 October in Tokyo
  assert.equal(reminderShort(at('2026-10-05T03:00:00Z'), now), '12:00');
  assert.equal(reminderShort(at('2026-10-05T14:59:00Z'), now), '23:59');
  // tomorrow in Tokyo although still the 5th in UTC; no leading zero in the date
  assert.equal(reminderShort(at('2026-10-05T15:00:00Z'), now), '10/6 00:00');
  assert.equal(reminderShort(at('2026-11-09T00:05:00Z'), now), '11/9 09:05');
  assert.equal(reminderShort(at('2027-01-01T00:00:00Z'), now), '2027/1/1 09:00');
  // a reminder that is past keeps its date (the card says Due instead)
  assert.equal(reminderShort(at('2026-10-04T03:00:00Z'), now), '10/4 12:00');
  // the same instant in another zone: 14:00 today in Tokyo, 01:00 tomorrow in New York
  assert.equal(reminderShort(at('2026-10-05T05:00:00Z'), now), '14:00');
  assert.equal(reminderShort(at('2026-10-05T05:00:00Z', 'America/New_York'), now), '10/5 01:00');
});

test('time round trips reject gaps, invalid dates and ambiguous implicit choices', () => {
  assert.deepEqual(localCandidates('2026-03-08T02:30', 'America/New_York'), []);
  assert.equal(localCandidates('2026-11-01T01:30', 'America/New_York').length, 2);
  for (const input of ['bad', '2026-02-30T12:00', '2026-09-30T25:00']) assert.deepEqual(localCandidates(input, 'UTC'), []);
  assert.deepEqual(localCandidates('2026-09-30T12:00', 'Unknown/Zone'), []);
  const [utc] = localCandidates('2026-09-30T12:00', 'Asia/Tokyo');
  assert.equal(localParts(utc, 'Asia/Tokyo'), '2026-09-30T12:00');
  assert.equal(localParts(utc, 'UTC'), '2026-09-30T03:00');
  assert.equal(localParts(Date.parse('2026-09-30T18:00Z'), 'Asia/Kathmandu'), '2026-09-30T23:45');
});
test('shortcuts use the next natural week and tomorrow at nine', () => {
  const monday = Date.parse('2026-10-05T08:00Z');
  assert.equal(shortcutTime('monday', monday, 'UTC'), '2026-10-12T09:00');
  assert.equal(shortcutTime('monday', Date.parse('2026-10-11T11:00Z'), 'UTC'), '2026-10-12T09:00');
  assert.equal(shortcutTime('tomorrow', monday, 'UTC'), '2026-10-06T09:00');
  assert.equal(shortcutTime('hour', monday, 'UTC'), '2026-10-05T09:00');
});
test('notes have matching UTF-8 bounds and cannot become multiline content', () => {
  for (const good of ['', 'x'.repeat(280), '中'.repeat(93)]) assert.equal(noteValid(good), true);
  for (const bad of [null, '中'.repeat(94), 'x'.repeat(281), 'a\nb', 'a\rb', '\0']) assert.equal(noteValid(bad), false);
});
test('old and duplicate system actions cannot overwrite new reminders or resurrect cancelled cards', () => {
  const c = card(); const action = { ...reminderClaim(c), kind: 'snooze' };
  const next = reminderAction(c, action, 5000);
  assert.equal(next.reminder.dueAt, 3605000);
  c.reminder = next.reminder;
  assert.equal(reminderAction(c, action, 9000), null, 'detector rejects old-action overwrite mutation');
  assert.equal(reminderAction({ ...c, id: 'replacement' }, { ...reminderClaim(c), cardId: 'card-a', kind: 'open' }, 0), null);
  assert.equal(reminderAction({ id: 'card-a' }, action, 0), null);
  assert.equal(reminderAction(c, { ...reminderClaim(c), kind: 'dismiss' }, 0), null);
  assert.equal(reminderAction(c, { ...reminderClaim(c), kind: 'snooze' }, NaN), null);
  assert.deepEqual(reminderAction(c, { ...reminderClaim(c), kind: 'open' }, 0), { open: c.id });
  assert.equal(reminderClaim(null), null); assert.equal(sameReminder(null, action), false);
});
test('due is independent of viewing, agent episodes and host clock rollback', () => {
  const c = card(); assert.equal(reminderDue(c, 1999), false); assert.equal(reminderDue(c, 2000), true);
  c.reminder.due = true; assert.equal(reminderDue(c, 0), true); assert.equal(reminderDue(null), false);
  const t = createAttentionTracker();
  for (const agent of ['needs-input', 'turn-done', 'working', null]) {
    t.record([c], [{ name: c.session, alive: true, agent, episode: 1 }], new Set(), 3000);
    t.saw(c); assert.equal(t.counts([c]).pending, 1); assert.equal(t.matches(c, 'reminder'), true);
  }
  const future = card(); future.id = 'future'; future.reminder.dueAt = Date.now() + 100000;
  assert.equal(t.matches(future, 'reminder'), false); assert.equal(t.matches(future, 'reminders'), true);
  const p = [{ id: 'p', columns: [{ id: 'col' }] }]; c.projectId = future.projectId = 'p'; c.columnId = future.columnId = 'col';
  assert.deepEqual(attentionRows(p, [future, c], t, 'reminders').map(row => row.card.id), [c.id, future.id]);
});
test('the periodic reconcile runs only while the Board has a reminder', () => {
  let cards = [{ id: 'plain' }, { id: 'other', reminderRetirements: ['exit:1:2:$3'] }]; let runs = 0;
  const tick = reminderTick(() => cards, () => { runs += 1; });
  tick(); tick();
  assert.equal(runs, 0, 'nothing to latch or keep registered: no IPC');
  cards = [{ id: 'plain' }, card()]; tick();
  assert.equal(runs, 1, 'a future reminder is work: its due instant and its registration');
  cards[1].reminder.due = true; tick();
  assert.equal(runs, 2, 'a due reminder still is: its notification can be answered');
  cards[1].reminder.inAppOnly = true; tick();
  assert.equal(runs, 3, 'so is one that only shows inside deck');
  delete cards[1].reminder; tick();
  assert.equal(runs, 3, 'the Board is read at every tick, not once');
});
test('ending a reminder never releases the same blocked retirement; new generations remain ordinary', () => {
  const c = card(); const key = retirementKey('exit', c, '123:456:$1');
  rememberRetirement(c, key); rememberRetirement(c, key); delete c.reminder;
  assert.equal(retirementBlocked(c, key), true, 'detector rejects deferred-delete mutation');
  assert.equal(retirementBlocked(c, retirementKey('exit', c, '123:456:$2')), false);
  const restored = JSON.parse(JSON.stringify(c)); assert.equal(retirementBlocked(restored, key), true);
  c.origin = { source: 'clock', key: 'run-1', badge: 'rule-1' };
  const run = retirementKey('run', c); rememberRetirement(c, run);
  assert.equal(retirementBlocked(c, run), true); c.origin.key = 'run-2'; assert.equal(retirementBlocked(c, retirementKey('run', c)), false);
  rememberRetirement(c, retirementKey('exit', c, '123:456:$2')); assert.equal(c.reminderRetirements.length, 2);
  assert.equal(retirementKey('exit', c, null), null); assert.equal(retirementKey('run', {}), null); rememberRetirement(c, null);
});

test('request identity encodes card bytes separately from reminder revision', () => {
  const c=card(); assert.equal(reminderRequestId(c), 'deck.reminder.636172642d61.unique.1');
});
