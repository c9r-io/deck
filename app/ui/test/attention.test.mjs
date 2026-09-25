import test from 'node:test';
import assert from 'node:assert/strict';
import { ATTENTION_BADGE_LABELS, attentionBadge, createAttentionTracker, attentionRows } from '../js/attention-model.js';
import { NOTIFY_COUNTED_FILTERS } from '../js/notify-model.js';
import { dictionaries } from '../js/i18n.js';
import fixture from './fixtures/attention-fixture.mjs';
const projects = ['Atlas', 'Beacon', 'Cedar'].map(id => ({ id, name: id, columns: ['Attention', 'Working', 'Queued', 'Parked'].map(name => ({ id: name, name })) }));
const cards = fixture.cards.map(c => ({ ...c, pinned: c.pin === true, projectId: c.project, columnId: c.group, session: `fixture-${c.id}` }));
const infos = fixture.cards.map(c => ({ name: `fixture-${c.id}`, alive: c.state !== 'stopped', agent: c.source === 'hook' ? c.state : null, idle_secs: c.state === 'quiet' ? 240 : 3 }));
const trackerOf = () => {
  const tracker = createAttentionTracker();
  tracker.record(cards, infos, new Set(cards.filter(c => c.read).map(c => c.id)), 100);
  return tracker;
};

test('12-card fixture: manual follow-up joins pending without changing live categories or placement', () => {
  const tracker = trackerOf();
  assert.deepEqual(tracker.counts(cards), { all: 12, pending: 5, input: 2, done: 2, followed: 2, unavailable: 3, stopped: 1, unknown: 0 });
  assert.equal(tracker.counts(cards.filter(c => c.projectId === 'Atlas')).pending, 1);
  assert.equal(tracker.category(cards[0]), 'input', 'viewed input remains pending');
  assert.equal(tracker.category(cards[2]), 'other', 'viewed turn ended is not unread');
  assert.equal(tracker.category(cards[10]), 'other', 'a starred working card is not an input request');
  const original = JSON.stringify({ projects, cards });
  const rows = attentionRows(projects, cards, tracker, 'pending');
  assert.deepEqual(rows.map(r => r.card.id), ['01', '08', '07', '10', '11']);
  assert.deepEqual(rows.map(r => r.column.id), ['Working', 'Parked', 'Working', 'Working', 'Working']);
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
  assert.deepEqual(fresh.counts([]), { all: 0, pending: 0, input: 0, done: 0, followed: 0, unavailable: 0, stopped: 0, unknown: 0 });
  assert.equal(fresh.freshness([]).kind, 'fresh');
});

test('manual column labels and order are authoritative, even with identical names and duplicate references', () => {
  const tracker = trackerOf();
  const renamed = projects.map(p => ({ ...p, columns: [...p.columns].reverse().map(c => ({ ...c, name: 'custom' })) })).reverse();
  assert.deepEqual(attentionRows(renamed, [...cards, cards[0]], tracker, 'input').map(r => r.card.id), ['08', '01']);
  assert.equal(attentionRows(projects, cards, tracker, 'all').length, 12);
  assert.equal(attentionRows(projects, cards, tracker, 'no-such-filter').length, 0);
});

test('manual follow-up survives viewing, every live state, session replacement and restart', () => {
  const card = { ...cards[10], pinned: true };
  const tracker = createAttentionTracker();
  const original = JSON.stringify(card);
  const check = () => {
    assert.equal(tracker.matches(card, 'pending'), true);
    assert.equal(tracker.matches(card, 'followed'), true);
    assert.equal(tracker.counts([card]).pending, 1, 'overlapping reasons count once');
    assert.equal(tracker.counts([card]).followed, 1);
    assert.equal(attentionRows(projects, [card], tracker, 'pending').length, 1);
  };
  check(); // No poll yet; persisted human intent is already known.
  for (const info of [
    { alive: true, agent: 'working' }, { alive: true, agent: 'needs-input' },
    { alive: true, agent: 'turn-done' }, { alive: true, agent: null }, { alive: false },
  ]) {
    tracker.record([card], [{ name: card.session, ...info }]);
    check();
    tracker.saw(card);
    check();
    tracker.fail();
    check();
  }
  assert.equal(JSON.stringify(card), original, 'observations never write the manual flag');
  const restored = JSON.parse(original);
  restored.session = 'replacement';
  assert.equal(createAttentionTracker().matches(restored, 'pending'), true);
  restored.pinned = false;
  assert.equal(createAttentionTracker().matches(restored, 'pending'), false);
});

test('unfollowing clears only the manual reason, while unread endings and input requests remain', () => {
  const tracker = createAttentionTracker();
  const card = { ...cards[10], pinned: true };
  for (const agent of ['needs-input', 'turn-done']) {
    tracker.record([card], [{ name: card.session, alive: true, agent }]);
    card.pinned = false;
    assert.equal(tracker.matches(card, 'pending'), true);
    assert.equal(tracker.matches(card, 'followed'), false);
    card.pinned = true;
  }
  tracker.saw(card);
  assert.equal(attentionRows(projects, [card], tracker, 'pending')[0].kind, 'followed');
  card.pinned = false;
  assert.equal(tracker.matches(card, 'pending'), false);
});

test('followed filter includes overlapping live reasons without changing project and group order', () => {
  const tracker = trackerOf();
  assert.deepEqual(attentionRows(projects, cards, tracker, 'followed').map(r => r.card.id), ['01', '11']);
  const stopped = { ...cards[4], pinned: true };
  assert.equal(tracker.matches(stopped, 'stopped'), true);
  assert.equal(attentionRows(projects, [stopped], tracker, 'pending')[0].kind, 'followed');
});

test('the card badge is the Dock set: needs input or an unread ending, nothing else', () => {
  const tracker = trackerOf();
  const kinds = Object.fromEntries(cards.map(card => [card.id, attentionBadge(tracker, card)?.kind ?? null]));
  // every card: a badge exactly when the attention category is input or done
  for (const card of cards) {
    const category = tracker.category(card);
    assert.equal(kinds[card.id], ['input', 'done'].includes(category) ? category : null, `card ${card.id} (${category})`);
  }
  assert.deepEqual(cards.filter(card => kinds[card.id]).map(card => `${card.id}:${kinds[card.id]}`),
    ['01:input', '07:done', '08:input', '10:done']);
  const counts = tracker.counts(cards);
  assert.equal(Object.values(kinds).filter(Boolean).length,
    NOTIFY_COUNTED_FILTERS.reduce((sum, filter) => sum + counts[filter], 0),
    'as many badges as the Dock badge counts');
  assert.deepEqual(Object.keys(ATTENTION_BADGE_LABELS), [...NOTIFY_COUNTED_FILTERS]);
  // manual follow-up alone, working, quiet, unavailable and stopped carry none
  for (const id of ['02', '03', '04', '05', '06', '09', '11', '12']) assert.equal(kinds[id], null, `card ${id}`);
  for (const [locale, dictionary] of Object.entries(dictionaries)) {
    for (const key of Object.values(ATTENTION_BADGE_LABELS)) assert.equal(typeof dictionary[key], 'string', `${locale} ${key}`);
  }
});

test('the card badge follows one attention episode through viewing and back', () => {
  const tracker = createAttentionTracker();
  const card = cards[1];
  const as = (agent, visible = new Set(), now = 0) => tracker.record(cards,
    infos.map(info => info.name === card.session ? { ...info, agent } : info), visible, now);
  const badge = () => attentionBadge(tracker, card);
  assert.equal(badge(), null, 'no observation yet: nothing is claimed');
  as('working', undefined, 1);
  assert.equal(badge(), null);
  as('needs-input', undefined, 2);
  assert.deepEqual(badge(), { kind: 'input', stale: false });
  assert.ok(tracker.saw(card));
  assert.deepEqual(badge(), { kind: 'input', stale: false }, 'viewing does not answer the question');
  as('needs-input', new Set([card.id]), 3);
  assert.deepEqual(badge(), { kind: 'input', stale: false });
  as('working', undefined, 4);
  assert.equal(badge(), null, 'working again clears it');
  as('turn-done', undefined, 5);
  assert.deepEqual(badge(), { kind: 'done', stale: false });
  const before = tracker.get(card).status;
  assert.ok(tracker.saw(card));
  assert.equal(badge(), null, 'a viewed ending clears at once…');
  assert.equal(tracker.get(card).status, before, '…while the card status stays the same');
  as('turn-done', undefined, 6);
  assert.equal(badge(), null, 'a repeated report of the same ending does not re-arm it');
  as('working', undefined, 7);
  as('turn-done', undefined, 8);
  assert.deepEqual(badge(), { kind: 'done', stale: false }, 'the next ending shows again');
  tracker.fail();
  assert.deepEqual(badge(), { kind: 'done', stale: true }, 'a failed poll keeps it, marked old');
  assert.equal(tracker.saw(card), false, 'and a stale snapshot cannot be acknowledged');
  as('turn-done', undefined, 9);
  assert.deepEqual(badge(), { kind: 'done', stale: false });
  tracker.record(cards, infos.map(info => info.name === card.session ? { ...info, alive: false, agent: null } : info), new Set(), 10);
  assert.equal(badge(), null, 'a stopped session carries none');
  assert.equal(attentionBadge(tracker, { ...card, session: 'other-session' }), null,
    'a snapshot of a previous session never labels the card');
});
