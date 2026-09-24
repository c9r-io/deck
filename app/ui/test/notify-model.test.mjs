import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttentionTracker } from '../js/attention-model.js';
import { NOTIFY_COUNTED_FILTERS, NOTIFY_STATUS_WORDS, cardLabels, labelsKey, notifyStatusKey, seenDismissals } from '../js/notify-model.js';

const projects = [{ id: 'P1', name: 'deck' }, { id: 'P2', name: 'site' }];
const card = (id, session, title, projectId) => ({ id, session, title, projectId });

test('card labels carry session, bounded title and project name, sorted by session', () => {
  const labels = cardLabels([
    card('C2', 'deck-card-zz', 'x'.repeat(600), 'P2'),
    card('C1', 'deck-card-aa', 'Fix parser', 'P1'),
    card('C3', 'deck-card-mm', 'Orphan', 'P9'),
    { id: 'C4', title: 'no session' },
  ], projects);
  assert.deepEqual(labels.map(l => l.session), ['deck-card-aa', 'deck-card-mm', 'deck-card-zz']);
  assert.equal(labels[0].project, 'deck');
  assert.equal(labels[1].project, '', 'a project that no longer exists has no name');
  assert.equal(labels[2].title.length, 512);
  assert.equal(cardLabels(null, null).length, 0);
});

test('the labels key changes exactly when a label changes', () => {
  const a = cardLabels([card('C1', 'deck-card-aa', 'Fix parser', 'P1')], projects);
  const b = cardLabels([card('C1', 'deck-card-aa', 'Fix parser', 'P1')], projects);
  assert.equal(labelsKey(a), labelsKey(b));
  const renamed = cardLabels([card('C1', 'deck-card-aa', 'Fix parser v2', 'P1')], projects);
  assert.notEqual(labelsKey(a), labelsKey(renamed));
  const moved = cardLabels([card('C1', 'deck-card-aa', 'Fix parser', 'P2')], projects);
  assert.notEqual(labelsKey(a), labelsKey(moved));
  assert.equal(labelsKey([]), '');
});

test('a viewed turn ending is reported once per turn', () => {
  const tracker = createAttentionTracker();
  const cards = [card('C1', 'deck-card-aa', 'A', 'P1'), card('C2', 'deck-card-bb', 'B', 'P1')];
  const info = (name, agent) => ({ name, alive: true, agent, idle_secs: 0 });
  const sent = new Set();
  tracker.record(cards, [info('deck-card-aa', 'turn-done'), info('deck-card-bb', 'needs-input')], new Set(), 1000);
  assert.deepEqual(seenDismissals(cards, tracker, sent), [], 'unread: nothing to dismiss');
  // the pane showed card A
  tracker.record(cards, [info('deck-card-aa', 'turn-done'), info('deck-card-bb', 'needs-input')], new Set(['C1']), 2000);
  assert.deepEqual(seenDismissals(cards, tracker, sent), ['deck-card-aa']);
  assert.deepEqual(seenDismissals(cards, tracker, sent), [], 'reported once');
  // the next turn: working clears the memory, its ending is reported again once viewed
  tracker.record(cards, [info('deck-card-aa', 'working'), info('deck-card-bb', 'needs-input')], new Set(), 3000);
  assert.deepEqual(seenDismissals(cards, tracker, sent), []);
  assert.ok(!sent.has('deck-card-aa'));
  tracker.record(cards, [info('deck-card-aa', 'turn-done'), info('deck-card-bb', 'needs-input')], new Set(['C1']), 4000);
  assert.deepEqual(seenDismissals(cards, tracker, sent), ['deck-card-aa']);
  // viewing a needs-input never dismisses: the question is still open
  tracker.record(cards, [info('deck-card-aa', 'turn-done'), info('deck-card-bb', 'needs-input')], new Set(['C1', 'C2']), 5000);
  assert.deepEqual(seenDismissals(cards, tracker, sent), []);
  // a card without a snapshot is ignored
  assert.deepEqual(seenDismissals([card('C9', 'deck-card-zz', 'Z', 'P1')], tracker, sent), []);
});

test('status words are closed and unknown words read as unsupported', () => {
  assert.deepEqual([...NOTIFY_STATUS_WORDS], ['unsupported', 'not-determined', 'denied', 'authorized', 'provisional']);
  assert.equal(notifyStatusKey('authorized'), 'settings.notifyStatus.authorized');
  assert.equal(notifyStatusKey('denied'), 'settings.notifyStatus.denied');
  assert.equal(notifyStatusKey('anything'), 'settings.notifyStatus.unsupported');
  assert.equal(notifyStatusKey(undefined), 'settings.notifyStatus.unsupported');
  assert.deepEqual([...NOTIFY_COUNTED_FILTERS], ['input', 'done']);
});
