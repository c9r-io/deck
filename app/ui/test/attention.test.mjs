import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttentionTracker, attentionRows } from '../js/attention-model.js';
import fixture from './fixtures/attention-fixture.mjs';
const projects = ['Atlas', 'Beacon', 'Cedar'].map(id => ({ id, name: id, columns: ['Attention', 'Working', 'Queued', 'Parked'].map(name => ({ id: name, name })) }));
const cards = fixture.cards.map(c => ({ ...c, projectId: c.project, columnId: c.group, session: `fixture-${c.id}` }));
const infos = fixture.cards.map(c => ({ name: `fixture-${c.id}`, alive: c.state !== 'stopped', agent: c.source === 'hook' ? c.state : null, idle_secs: c.state === 'quiet' ? 240 : 3 }));
const trackerOf = () => {
  const tracker = createAttentionTracker();
  tracker.record(cards, infos, new Set(cards.filter(c => c.read).map(c => c.id)), 100);
  return tracker;
};

test('approved 12-card fixture: inputs and unread endings are independent of manual placement and stars', () => {
  const tracker = trackerOf();
  assert.deepEqual(tracker.counts(cards), { all: 12, pending: 4, input: 2, done: 2, unavailable: 3, stopped: 1, unknown: 0 });
  assert.equal(tracker.counts(cards.filter(c => c.projectId === 'Atlas')).pending, 1);
  assert.equal(tracker.category(cards[0]), 'input', 'viewed input remains pending');
  assert.equal(tracker.category(cards[2]), 'other', 'viewed turn ended is not unread');
  assert.equal(tracker.category(cards[10]), 'other', 'a starred working card is not an input request');
  const original = JSON.stringify({ projects, cards });
  const rows = attentionRows(projects, cards, tracker, 'pending');
  assert.deepEqual(rows.map(r => r.card.id), ['01', '08', '07', '10']);
  assert.deepEqual(rows.map(r => r.column.id), ['Working', 'Parked', 'Working', 'Working']);
  assert.equal(JSON.stringify({ projects, cards }), original, 'derived ordering never changes durable arrays');
});

test('successful viewing clears only the current ending; repeated reports do not re-arm it', () => {
  const tracker = trackerOf();
  const card = cards[6];
  tracker.saw(card);
  assert.equal(tracker.get(card).status, 'done');
  tracker.record(cards, infos, new Set(), 200);
  assert.equal(tracker.category(card), 'other');
  const working = infos.map(i => i.name === card.session ? { ...i, agent: 'working' } : i);
  tracker.record(cards, working, new Set(), 300);
  tracker.record(cards, infos, new Set(), 400);
  assert.equal(tracker.category(card), 'done');
  tracker.record(cards, infos, new Set([card.id]), 500);
  assert.equal(tracker.category(card), 'other', 'already visible successful pane has no unread ending');
});

test('quiet is not readiness; a hooked silent worker remains working', () => {
  const tracker = trackerOf();
  assert.equal(tracker.get(cards[5]).status, 'waiting');
  assert.equal(tracker.get(cards[8]).status, 'waiting');
  tracker.record(cards, infos.map(i => i.name === cards[1].session ? { ...i, idle_secs: 1200 } : i));
  assert.equal(tracker.get(cards[1]).status, 'running');
  assert.equal(tracker.category(cards[1]), 'other');
  assert.deepEqual(attentionRows(projects, cards, tracker, 'unavailable').map(r => r.card.id), ['04', '06', '09']);
  assert.deepEqual(attentionRows(projects, cards, tracker, 'stopped').map(r => r.card.id), ['05']);
});

test('first poll unknown, failed snapshots and partial responses never become an all-clear', () => {
  const tracker = createAttentionTracker();
  assert.equal(tracker.freshness(cards).kind, 'unknown');
  assert.equal(tracker.counts(cards).unknown, 12);
  assert.equal(tracker.saw(cards[6]), false);
  tracker.record(cards, infos, new Set(), 100);
  tracker.fail();
  assert.deepEqual(tracker.freshness(cards), { kind: 'stale', lastSuccess: 100 });
  assert.equal(tracker.get(cards[6]).stale, true);
  assert.equal(tracker.saw(cards[6]), false, 'stale state cannot acknowledge an unseen turn');
  tracker.record(cards, infos.filter(i => i.name !== cards[6].session), new Set(), 200);
  assert.equal(tracker.get(cards[6]).stale, true);
  assert.equal(tracker.get(cards[0]).stale, false);
  assert.equal(tracker.freshness(cards).kind, 'stale');
  assert.equal(tracker.category(cards[6]), 'done');
  tracker.record(cards, infos, new Set(), 300);
  assert.deepEqual(tracker.freshness(cards), { kind: 'fresh', lastSuccess: 300 });
  tracker.record(cards, [{ name: cards[6].session, alive: 'bad' }], new Set(), 400);
  assert.equal(tracker.get(cards[6]).stale, true, 'malformed row is treated as missing');
});

test('session replacement, removed cards, new tracker and changed input episode do not inherit viewing', () => {
  const tracker = trackerOf();
  const replacement = { ...cards[0], session: 'replacement' };
  assert.equal(tracker.get(replacement), null);
  assert.equal(tracker.category(replacement), 'unknown');
  tracker.record([replacement], [{ name: 'replacement', alive: true, agent: 'needs-input' }]);
  assert.equal(tracker.get(cards[0]), null);
  assert.equal(tracker.get(cards[6]), null);
  assert.equal(tracker.get(replacement).seen, false);
  tracker.saw(replacement);
  tracker.record([replacement], [{ name: 'replacement', alive: true, agent: 'working' }]);
  tracker.record([replacement], [{ name: 'replacement', alive: true, agent: 'needs-input' }]);
  assert.equal(tracker.get(replacement).seen, false);
  const fresh = createAttentionTracker();
  fresh.record(cards, infos);
  assert.equal(fresh.category(cards[2]), 'done', 'no promise of persistent read state after restart');
  fresh.record([], []);
  assert.deepEqual(fresh.counts([]), { all: 0, pending: 0, input: 0, done: 0, unavailable: 0, stopped: 0, unknown: 0 });
  assert.equal(fresh.freshness([]).kind, 'fresh');
});

test('manual column labels and order are authoritative, even with identical names and duplicate references', () => {
  const tracker = trackerOf();
  const renamed = projects.map(p => ({ ...p, columns: [...p.columns].reverse().map(c => ({ ...c, name: 'custom' })) })).reverse();
  assert.deepEqual(attentionRows(renamed, [...cards, cards[0]], tracker, 'input').map(r => r.card.id), ['08', '01']);
  assert.equal(attentionRows(projects, cards, tracker, 'all').length, 12);
  assert.equal(attentionRows(projects, cards, tracker, 'no-such-filter').length, 0);
});
