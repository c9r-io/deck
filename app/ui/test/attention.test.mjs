import test from 'node:test';
import assert from 'node:assert/strict';
import { ATTENTION_BADGE_LABELS, DELIVERY_WAITS, attentionBadge, createAttentionTracker, attentionRows, codexCoverageGap, deliveryWaits } from '../js/attention-model.js';
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

test('Codex coverage is diagnostic only and follows live session snapshots', () => {
  const tracker = createAttentionTracker();
  const card = { id: 'codex', session: 'pane', cmd: 'codex' };
  const poll = (codex_signal, rest = {}) => tracker.record([card], [{ name: 'pane', alive: true, idle_secs: 200, codex_signal, ...rest }]);
  assert.equal(codexCoverageGap(tracker.get(card)), null);
  for (const coverage of ['unknown', 'unavailable']) {
    poll(coverage);
    assert.equal(codexCoverageGap(tracker.get(card)), coverage);
    assert.equal(tracker.category(card), 'unavailable');
    assert.equal(tracker.counts([card]).pending, 0);
    assert.equal(attentionBadge(tracker, card), null);
    assert.equal(tracker.get(card).episode, null);
    tracker.fail();
    assert.equal(codexCoverageGap(tracker.get(card)), coverage);
    assert.equal(tracker.get(card).stale, true);
  }
  poll('trusted', { agent: 'working' });
  assert.equal(codexCoverageGap(tracker.get(card)), null);
  poll('unavailable', { agent: 'needs-input' });
  assert.equal(codexCoverageGap(tracker.get(card)), null, 'real observation outranks coverage even for an inconsistent row');
  poll('unavailable', { alive: false });
  assert.equal(codexCoverageGap(tracker.get(card)), null);
  for (const malformed of [undefined, null, 'daemon', {}, true]) {
    poll(malformed);
    assert.equal(codexCoverageGap(tracker.get(card)), null);
  }
  poll('unknown');
  assert.equal(codexCoverageGap(tracker.get({ ...card, session: 'replacement' })), null);
  poll(null);
  assert.equal(codexCoverageGap(tracker.get(card)), null, 'shell/other program removes the gap; saved codex command is not evidence');
});

test('12-card fixture: manual follow-up joins pending without changing live categories or placement', () => {
  const tracker = trackerOf();
  assert.deepEqual(tracker.counts(cards), { all: 12, pending: 5, input: 2, waiting: 0, done: 2, followed: 2, unavailable: 3, stopped: 1, unknown: 0 });
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
  assert.deepEqual(fresh.counts([]), { all: 0, pending: 0, input: 0, waiting: 0, done: 0, followed: 0, unavailable: 0, stopped: 0, unknown: 0 });
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

// The Dock badge is the union of these agent reasons and due card reminders
// (notify.rs `push_badge`); a reminder has its own chip, never this badge.
test('the card badge is the agent part of the Dock set: needs input or an unread ending, nothing else', () => {
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
    'as many badges as the agent reasons the Dock badge counts');
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

/* Signal Integrity FR-SI-02: observation, attention and freshness are three
   separate layers. Viewing (attention) and a failed poll (freshness) never
   rewrite what the agent reported; unknown words never become state. */
test('viewed and stale never change the observation; only closed words are state', () => {
  const tracker = trackerOf();
  const observed = card => { const { agent, status, alive, idle, observedAt } = tracker.get(card); return { agent, status, alive, idle, observedAt }; };
  const ending = cards.find(c => infos.find(i => i.name === c.session).agent === 'turn-done' && !c.read);
  const input = cards.find(c => infos.find(i => i.name === c.session).agent === 'needs-input');
  const before = [observed(ending), observed(input)];
  tracker.saw(ending);
  tracker.saw(input);
  assert.deepEqual([observed(ending), observed(input)], before, 'viewing is attention metadata only');
  assert.equal(tracker.get(ending).agent, 'turn-done', 'a viewed ending is still an ended turn');
  assert.equal(tracker.category(input), 'input', 'viewing an input request is not answering it');
  tracker.fail();
  assert.deepEqual([observed(ending), observed(input)], before, 'stale is freshness, not agent state');
  assert.equal(tracker.get(input).stale, true);
  assert.deepEqual(attentionBadge(tracker, input), { kind: 'input', stale: true }, 'the badge carries the staleness honestly');
  const probe = { id: 'probe', session: 'probe', projectId: 'Atlas', columnId: 'Working' };
  for (const word of ['stale', 'viewed', 'unread', 'done', 'idle', 'complete', 'settled']) {
    tracker.record([probe], [{ name: 'probe', alive: true, agent: word, idle_secs: 3 }], new Set(), 900);
    assert.equal(tracker.get(probe).agent, null, `${word} is not an agent state`);
  }
  tracker.record([probe], [{ name: 'probe', alive: false, agent: 'turn-done' }], new Set(), 901);
  assert.equal(tracker.get(probe).agent, null, 'a dead session reports no interaction state');
});

test('a held delivery is read from queue items alone: checkpoint, uncertain, dead, and the external row whose turn it is', () => {
  const row = (session, extra) => ({ session, state: 'pending', mode: 'chain', attempts: 0, added: 1, ...extra });
  const waits = deliveryWaits([
    row('review', { state: 'review' }),
    row('approved', { state: 'review-approved' }),
    row('uncertain', { state: 'ambiguous' }),
    row('retrying', { state: 'failed', attempts: 7 }),
    row('dead', { state: 'failed', attempts: 8 }),
    row('owner', {}),
    row('timed', { mode: 'at', external: true }),
    row('external', { external: true, group: 'g1', seq: 1 }),
    row('behind', { group: 'g2', seq: 1 }),
    row('behind', { external: true, group: 'g2', seq: 2 }),
    row('turn', { state: 'review-approved', group: 'g3', seq: 1 }),
    row('turn', { external: true, group: 'g3', seq: 2 }),
    row('granted', { external: true, authority: { step: 1 } }),
    row('paused', { external: true, paused: true }),
    row('both', { external: true }),
    row('both', { state: 'ambiguous' }),
    row('tie', { external: true, group: 'g4', added: 2 }),
    row('tie', { group: 'g4', added: 1 }),
  ]);
  assert.deepEqual(Object.fromEntries(waits), {
    review: 'review', uncertain: 'ambiguous', dead: 'failed', external: 'external', turn: 'external', both: 'ambiguous',
  });
  for (const reason of DELIVERY_WAITS) {
    for (const locale of ['en', 'zh-Hans']) assert.ok(dictionaries[locale][`attention.waiting.${reason}`], `${locale} ${reason}`);
  }
});

// F05: the two holds that need a live observation are not derived here at
// all. The backend's plan names the stage; the model only accepts those two
// words, for a row that is still queued, below every reason the queue holds.
test('a row the backend holds for a first interaction or for Codex Signal is a held delivery too', () => {
  const row = (id, session, extra) => ({ id, session, state: 'pending', mode: 'chain', attempts: 0, added: 1, ...extra });
  const items = [
    row('a', 'fresh'), row('b', 'codex'), row('c', 'both', { state: 'review' }), row('d', 'both'),
    row('e', 'machine'), row('f', 'agent'), row('g', 'unverified'), row('h', 'two'), row('i', 'two'),
    row('j', 'lasting'), row('k', 'lasting-two'), row('l', 'lasting-two'),
  ];
  const plans = [
    { item: 'a', stage: 'first-send' }, { item: 'b', stage: 'codex-signal' },
    { item: 'c', stage: 'review' }, { item: 'd', stage: 'first-send' },
    { item: 'e', stage: 'quiet' }, { item: 'f', stage: 'agent' }, { item: 'g', stage: 'authority-unverified' },
    { item: 'h', stage: 'codex-signal' }, { item: 'i', stage: 'first-send' },
    { item: 'gone', stage: 'first-send' }, { stage: 'first-send' }, null,
    /* an approval Deck cannot verify: a wait only when the backend says the
       hold has lasted, and the least pressing of the reasons */
    { item: 'j', stage: 'authority-unverified', lasting: true },
    { item: 'k', stage: 'authority-unverified', lasting: true }, { item: 'l', stage: 'codex-signal' },
    { item: 'e', stage: 'quiet', lasting: true },
  ];
  assert.deepEqual(Object.fromEntries(deliveryWaits(items, plans)), {
    fresh: 'first-send', codex: 'codex-signal', both: 'review', two: 'first-send',
    lasting: 'authority-unverified', 'lasting-two': 'codex-signal',
  });
  assert.deepEqual(Object.fromEntries(deliveryWaits(items)), { both: 'review' }, 'without a plan nothing live is claimed');
  assert.deepEqual(Object.fromEntries(deliveryWaits(items, undefined)), { both: 'review' });
  assert.deepEqual([...DELIVERY_WAITS], ['ambiguous', 'failed', 'review', 'channel-stopped', 'channel-unverified', 'external', 'first-send', 'codex-signal', 'authority-unverified']);
  const tracker = trackerOf();
  const card = cards.find(c => c.id === '03');
  tracker.deliveries([row('a', card.session)], [{ item: 'a', stage: 'first-send' }]);
  assert.equal(tracker.waiting(card), 'first-send');
  assert.equal(attentionBadge(tracker, card), null, 'never the agent badge');
  tracker.deliveries([row('a', card.session)], [{ item: 'a', stage: 'context' }]);
  assert.equal(tracker.waiting(card), null, 'the hold is over when the plan says so');
  tracker.deliveries([row('a', card.session)], [{ item: 'a', stage: 'authority-unverified' }]);
  assert.equal(tracker.waiting(card), null, 'a hold that has not lasted stays in the panel');
  tracker.deliveries([row('a', card.session)], [{ item: 'a', stage: 'authority-unverified', lasting: true }]);
  assert.equal(tracker.waiting(card), 'authority-unverified');
  tracker.deliveries([row('a', card.session)], [{ item: 'a', stage: 'context', lasting: false }]);
  assert.equal(tracker.waiting(card), null, 'and is withdrawn when the approval reads again');
});

test('a held delivery joins pending after input requests and stays off the badge', () => {
  const tracker = trackerOf();
  const before = tracker.counts(cards);
  const held = id => cards.find(c => c.id === id);
  // 01 requests input, 07 has an unread ending, 03 is neither
  tracker.deliveries([
    { session: held('01').session, state: 'review' },
    { session: held('07').session, state: 'ambiguous' },
    { session: held('03').session, state: 'failed', attempts: 8 },
    { session: 'no-such-card', state: 'review' },
  ]);
  assert.equal(tracker.waiting(held('03')), 'failed');
  assert.equal(tracker.waiting(held('02')), null);
  const counts = tracker.counts(cards);
  assert.equal(counts.waiting, 3);
  assert.equal(counts.pending, before.pending + 1, 'only the card that was not pending yet is added');
  assert.deepEqual({ ...counts, waiting: 0, pending: before.pending }, before, 'no agent category changes');
  const rows = attentionRows(projects, cards, tracker, 'pending');
  assert.deepEqual(rows.map(r => [r.card.id, r.kind]),
    [['01', 'input'], ['08', 'input'], ['03', 'waiting'], ['07', 'waiting'], ['10', 'done'], ['11', 'followed']]);
  assert.deepEqual(attentionRows(projects, cards, tracker, 'waiting').map(r => r.card.id), ['01', '03', '07']);
  assert.deepEqual(attentionBadge(tracker, held('03')), null, 'the badge stays the Dock set');
  assert.equal(attentionBadge(tracker, held('07')).kind, 'done');
  tracker.deliveries([]);
  assert.deepEqual(tracker.counts(cards), before);
});

test('a terminal bell is the backend\'s word passed on: a pending list row, never a status, a badge or a local guess', () => {
  const project = { id: 'P', name: 'P', columns: [{ id: 'C', name: 'C' }] };
  const card = id => ({ id, projectId: 'P', columnId: 'C', title: id, session: `s-${id}`, pinned: false });
  const cardsNow = [card('shell'), card('agent'), card('quiet'), card('stopped')];
  const info = (id, extra) => ({ name: `s-${id}`, alive: true, agent: null, idle_secs: 300, ...extra });
  const tracker = createAttentionTracker();
  const record = (bells, visible = new Set()) => tracker.record(cardsNow, [
    info('shell', { bell: bells.includes('shell') }),
    /* the backend never reports one for these; the model does not trust a slip either */
    info('agent', { agent: 'working', bell: bells.includes('agent') }),
    info('quiet', { bell: bells.includes('quiet') }),
    info('stopped', { alive: false, bell: bells.includes('stopped') }),
  ], visible, 100);
  record([]);
  assert.equal(tracker.rang(cardsNow[0]), false);
  assert.equal(tracker.counts(cardsNow).pending, 0);
  const before = cardsNow.map(c => [tracker.category(c), tracker.get(c).status]);
  record(['shell', 'agent', 'stopped']);
  assert.deepEqual(cardsNow.map(c => tracker.rang(c)), [true, false, false, false], 'only a live session without agent state');
  assert.deepEqual(cardsNow.map(c => [tracker.category(c), tracker.get(c).status]), before, 'category and status are untouched');
  assert.equal(tracker.counts(cardsNow).pending, 1);
  assert.equal(tracker.matches(cardsNow[0], 'pending'), true);
  assert.equal(attentionBadge(tracker, cardsNow[0]), null, 'not the card badge');
  const rows = attentionRows([project], cardsNow, tracker, 'pending');
  assert.deepEqual(rows.map(row => [row.card.id, row.kind]), [['shell', 'bell']]);
  /* viewing in the webview clears nothing: only the backend's next answer does */
  record(['shell'], new Set(['shell']));
  assert.equal(tracker.rang(cardsNow[0]), true);
  tracker.saw(cardsNow[0]);
  assert.equal(tracker.rang(cardsNow[0]), true);
  record([]);
  assert.equal(tracker.rang(cardsNow[0]), false);
  assert.equal(tracker.counts(cardsNow).pending, 0);
  /* values that are not exactly true are no bell */
  for (const bell of ['true', 1, {}, null]) {
    tracker.record(cardsNow, [info('shell', { bell }), info('agent'), info('quiet'), info('stopped')], new Set(), 100);
    assert.equal(tracker.rang(cardsNow[0]), false, JSON.stringify(bell));
  }
  /* order in the pending list: after a held delivery, before an unread ending */
  const order = createAttentionTracker();
  const many = [card('done'), card('bell'), card('input')];
  order.record(many, [info('done', { agent: 'turn-done', episode: 1 }), info('bell', { bell: true }), info('input', { agent: 'needs-input', episode: 2 })], new Set(), 100);
  assert.deepEqual(attentionRows([project], many, order, 'pending').map(row => row.kind), ['input', 'bell', 'done']);
  assert.equal(dictionaries.en['attention.bell'], 'Bell');
  assert.equal(dictionaries['zh-Hans']['attention.bell'], '响过铃');
});

test('channel permission waits use native stages and transient verification stays quiet', () => {
  const items = [{ id: 'channel-head', session: 'channel-session', mode: 'at', state: 'pending', external: true }];
  assert.equal(deliveryWaits(items, [{ item: 'channel-head', stage: 'channel-unverified' }]).size, 0);
  assert.equal(deliveryWaits(items, [{ item: 'channel-head', stage: 'channel-unverified', lasting: true }]).get('channel-session'), 'channel-unverified');
  assert.equal(deliveryWaits(items, [{ item: 'channel-head', stage: 'channel-stopped' }]).get('channel-session'), 'channel-stopped');
});
