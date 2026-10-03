import test from 'node:test';
import assert from 'node:assert/strict';
import { createAttentionTracker } from '../js/attention-model.js';
import { NOTIFY_COUNTED_FILTERS, NOTIFY_STATUS_WORDS, cardLabels, dismissKey, labelsKey, notifyNeedsAgentStatus, notifyStatusKey, seenDismissals } from '../js/notify-model.js';

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

test('a label title is cut by UTF-8 bytes at a character boundary, the unit the backend counts', () => {
  const title = text => cardLabels([card('C1', 'deck-card-aa', text, 'P1')], projects)[0].title;
  const bytes = text => new TextEncoder().encode(text).length;
  assert.equal(title('题'.repeat(170)), '题'.repeat(170), '510 bytes: as it is');
  assert.equal(title('题'.repeat(171)), '题'.repeat(170), '513 bytes: its first whole characters');
  assert.equal(title('题'.repeat(600)), '题'.repeat(170));
  assert.equal(title('é'.repeat(256)), 'é'.repeat(256), 'exactly at the bound');
  assert.equal(title('a' + 'é'.repeat(256)), 'a' + 'é'.repeat(255));
  assert.equal(title('a' + '😀'.repeat(300)), 'a' + '😀'.repeat(127), 'never half a character');
  for (const text of ['题'.repeat(171), 'a' + '😀'.repeat(300), 'x'.repeat(600)]) {
    assert.ok(bytes(title(text)) <= 512, `${bytes(title(text))} bytes`);
    assert.ok(text.startsWith(title(text)), 'a prefix of the card\'s own title');
  }
  // a string that is not valid Unicode would fail the whole call in the
  // backend's deserializer, with every other card's label
  assert.equal(title('ok\ud83d'), 'ok�');
  assert.ok(!JSON.stringify(cardLabels([card('C1', 'deck-card-aa', 'a' + '😀'.repeat(300), 'P1')], projects)).includes('\\ud83d'));
  // nothing else about a title is touched
  for (const text of ['Fix parser', '', '﻿with a mark', ' spaced  ']) assert.equal(title(text), text);
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

test('a viewed ending is dismissed by its exact episode until the backend knows, retrying after a failure', () => {
  const tracker = createAttentionTracker();
  const cards = [card('C1', 'deck-card-aa', 'A', 'P1'), card('C2', 'deck-card-bb', 'B', 'P1')];
  const info = (name, agent, episode, viewed = false) => ({ name, alive: true, agent, idle_secs: 0, episode, episode_viewed: viewed });
  const inflight = new Set();
  const out = () => seenDismissals(cards, tracker, inflight).map(({ session, episode }) => `${session}:${episode}`);
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 3), info('deck-card-bb', 'needs-input', 4)], new Set(), 1000);
  assert.deepEqual(out(), [], 'unread: nothing to dismiss');
  // the pane showed card A: its exact episode is due
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 3), info('deck-card-bb', 'needs-input', 4)], new Set(['C1']), 2000);
  assert.deepEqual(out(), ['deck-card-aa:3']);
  // in flight: not sent twice
  inflight.add(dismissKey('deck-card-aa', 3));
  assert.deepEqual(out(), []);
  // the call FAILED: in-flight cleared, the next sync retries the same episode
  inflight.delete(dismissKey('deck-card-aa', 3));
  assert.deepEqual(out(), ['deck-card-aa:3'], 'a failed dismissal is retried');
  // acknowledged: known viewed, nothing more to send
  tracker.confirmViewed(cards[0], 3);
  assert.deepEqual(out(), []);
  // the backend's own truth after the next poll keeps it read
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 3, true), info('deck-card-bb', 'needs-input', 4)], new Set(), 3000);
  assert.equal(tracker.category(cards[0]), 'other');
  assert.deepEqual(out(), []);
  // a NEW episode re-arms and, once viewed, is due by its own id
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 7), info('deck-card-bb', 'needs-input', 4)], new Set(), 4000);
  assert.equal(tracker.category(cards[0]), 'done');
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 7), info('deck-card-bb', 'needs-input', 4)], new Set(['C1']), 5000);
  assert.deepEqual(out(), ['deck-card-aa:7']);
  // confirming a stale episode changes nothing
  tracker.confirmViewed(cards[0], 3);
  assert.deepEqual(out(), ['deck-card-aa:7']);
  // viewing a needs-input never dismisses: the question is still open
  tracker.record(cards, [info('deck-card-aa', 'turn-done', 7, true), info('deck-card-bb', 'needs-input', 4)], new Set(['C1', 'C2']), 6000);
  assert.deepEqual(out(), []);
  // a card without a snapshot, or an episode-less snapshot, is ignored
  assert.deepEqual(seenDismissals([card('C9', 'deck-card-zz', 'Z', 'P1')], tracker, inflight), []);
  tracker.record(cards, [{ name: 'deck-card-aa', alive: true, agent: 'turn-done', idle_secs: 0 }, info('deck-card-bb', 'needs-input', 4)], new Set(['C1']), 7000);
  assert.deepEqual(out(), [], 'no episode, nothing exact to dismiss');
});

test('status words are closed and unknown words read as unsupported', () => {
  assert.deepEqual([...NOTIFY_STATUS_WORDS], ['unsupported', 'not-determined', 'denied', 'authorized', 'provisional']);
  assert.equal(notifyStatusKey('authorized'), 'settings.notifyStatus.authorized');
  assert.equal(notifyStatusKey('denied'), 'settings.notifyStatus.denied');
  assert.equal(notifyStatusKey('anything'), 'settings.notifyStatus.unsupported');
  assert.equal(notifyStatusKey(undefined), 'settings.notifyStatus.unsupported');
  assert.deepEqual([...NOTIFY_COUNTED_FILTERS], ['input', 'done']);
});

test('the agent-status dependency is named only when both hooks are known to be off', () => {
  assert.equal(notifyNeedsAgentStatus({ claude: false, codex: false }), true, 'both off');
  assert.equal(notifyNeedsAgentStatus({ claude: true, codex: false }), false, 'Claude Code on');
  assert.equal(notifyNeedsAgentStatus({ claude: false, codex: true }), false, 'Codex on');
  assert.equal(notifyNeedsAgentStatus({ claude: true, codex: true }), false, 'both on');
  assert.equal(notifyNeedsAgentStatus(null), false, 'unknown state makes no claim');
  assert.equal(notifyNeedsAgentStatus(undefined), false);
});
