// The Slack mentions view (slack-mentions.js) and its model against the fake
// document: what the sidebar shows, what a row offers, and that routing and
// dismissing are one backend call each, never a write anywhere else.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { mentionCount, mentionErrorKey, mentionRoutes, mentionSignature, mentionStatus } = await import('../js/slack-mentions-model.js');
const { initMentions, paintMentions, refreshMentions, showMentions, startMentions } = await import('../js/slack-mentions.js');
const { channelBlockReason } = await import('../js/channel-model.js');
const { ctx, state, store } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');

const RULES = [
  { id: 'R-eyes', source: 'slack', badge: 'eyes', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
  { id: 'R-memo', source: 'slack', badge: 'memo', projectId: 'P1', columnId: 'C1', cmd: 'bash', template: 'triage' },
  { id: 'R-gone', source: 'slack', badge: 'zap', projectId: 'P9', columnId: 'C1', cmd: 'claude', template: 'triage' },
  { id: 'R-clock', source: 'clock', badge: 'R-clock', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
];
const PROJECTS = [{ id: 'P1', name: 'Proj', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['look at {{msg.text}}'] }] }];
const ITEMS = [
  { key: 'C1/1793942527.287979', text: '@me second', from: 'bob', where: '#dev', link: 'https://x/p2', at: 1793942527 },
  { key: 'C1/1793942517.831339', text: '@me first', from: 'alice', where: 'DM', link: '', at: 1793942517 },
];

function backend(initial) {
  const calls = [];
  let current = initial;
  let refuse = null;
  const heard = {};
  window.__TAURI__ = {
    event: { listen: async (name, fn) => { heard[name] = fn; } },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'slack_mentions') return structuredClone(current);
      if (refuse && cmd === refuse.cmd) throw refuse.word;
      if (cmd === 'slack_mention_dismiss') return args.keys.length;
      return null;
    } },
  };
  return {
    calls, heard,
    set(next) { current = next; },
    refuse(cmd, word) { refuse = cmd ? { cmd, word } : null; },
    named: name => calls.filter(([cmd]) => cmd === name),
  };
}

const rows = () => ids.get('mentions-list').children.filter(node => node.className === 'mention-row');
const actions = node => node.children.find(child => child.className === 'mention-actions').children;
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('the model lists Slack badge rules only, sorted, and says which cannot be chosen', () => {
  const routes = mentionRoutes(RULES, PROJECTS, channelBlockReason);
  assert.deepEqual(routes.map(route => [route.id, route.disabled]), [['R-eyes', false], ['R-memo', true], ['R-gone', true]]);
  assert.equal(routes[0].label, ':eyes: → Proj / triage');
  assert.equal(routes[2].label, ':zap: → ? / triage', 'a rule whose project is gone is shown, not chosen');
  assert.deepEqual(mentionRoutes(null, PROJECTS, channelBlockReason), []);
  assert.equal(mentionCount(null), 0);
  assert.equal(mentionCount({ enabled: false, items: ITEMS }), 0, 'nothing while Slack is off');
  assert.equal(mentionCount({ enabled: true, items: ITEMS }), 2);
  assert.deepEqual(mentionStatus(null), { key: 'mentions.off', params: {} });
  assert.deepEqual(mentionStatus({ enabled: true, lastError: 'ratelimited' }), { key: 'mentions.error', params: { code: 'ratelimited' } });
  assert.deepEqual(mentionStatus({ enabled: true }), { key: 'mentions.checking', params: {} });
  assert.deepEqual(mentionStatus({ enabled: true, lastPoll: 5 }), { key: 'mentions.checked', params: { at: 5 } });
  assert.equal(mentionErrorKey('settled'), 'mentions.settled');
  assert.equal(mentionErrorKey('rule'), 'mentions.failed');
  assert.notEqual(mentionSignature({ enabled: true, items: ITEMS }, routes), mentionSignature({ enabled: true, items: ITEMS.slice(1) }, routes));
});

test('the view shows the open mentions, routes one with one call and dismisses with another', async () => {
  setLocale('en');
  ids.clear();
  ctx.settings = { inbound: { sources: { slack: { enabled: true } }, rules: RULES } };
  store.projects = PROJECTS;
  state.projectId = 'P1';
  state.view = 'board';
  const b = backend({ enabled: true, lastPoll: 1793942600, lastError: null, items: ITEMS });
  const moves = [];
  initMentions({
    provider: { project: id => PROJECTS.find(project => project.id === id) },
    render: () => paintMentions(),
    switchProject: id => moves.push(['switch', id]),
    leaveSessionView: () => moves.push(['leave']),
    closeBuffer: () => moves.push(['close-buffer']),
  });
  await startMentions();
  assert.equal(typeof b.heard['slack-mentions-changed'], 'function', 'one content-free listener');
  assert.equal(ids.get('mentions-btn').hidden, false);
  assert.equal(ids.get('mentions-count').textContent, '2');
  assert.equal(ids.get('mentions-view').hidden, true, 'the Board stays in front');

  state.view = 'session';
  showMentions();
  await tick();
  assert.deepEqual(moves.slice(0, 2), [['close-buffer'], ['leave']], 'the session view is left first');
  assert.equal(state.view, 'mentions');
  assert.equal(b.named('slack_mentions_refresh').length, 1, 'opening asks for a fresh search');
  assert.equal(ids.get('mentions-view').hidden, false);
  assert.equal(ids.get('mentions-status').textContent.startsWith('Last checked'), true);
  assert.equal(ids.get('mentions-no-routes').hidden, true);
  assert.equal(rows().length, 2);
  const [open, menu, dismiss] = actions(rows()[0]);
  assert.equal(open.textContent, t('mentions.open'));
  assert.deepEqual(menu.children.map(option => [option.value, option.disabled]),
    [['', false], ['R-eyes', false], ['R-memo', true], ['R-gone', true]]);
  assert.equal(menu.children[2].textContent, t('mentions.unavailable', { label: ':memo: → Proj / triage' }));
  assert.equal(menu.disabled, false);
  assert.equal(dismiss.textContent, t('mentions.dismiss'));
  assert.equal(actions(rows()[1])[0].disabled, true, 'no permalink, nothing to open');

  // the text opens and closes in place
  const text = rows()[0].children.find(child => child.className === 'mention-text');
  text.onclick();
  assert.equal(text.dataset.expanded, 'true');
  text.onclick();
  assert.equal(text.dataset.expanded, 'false');

  open.onclick();
  await tick();
  assert.deepEqual(b.named('open_target').at(-1)[1], { kind: 'url', value: 'https://x/p2', cwd: ctx.HOME });

  // routing: one backend call naming the message and the rule; the row goes
  menu.value = 'R-eyes';
  menu.onchange();
  await tick();
  assert.deepEqual(b.named('slack_mention_route').at(-1)[1], { key: ITEMS[0].key, rule: 'R-eyes' });
  assert.equal(rows().length, 1);
  assert.equal(ids.get('mentions-count').textContent, '1');
  assert.equal(b.calls.some(([cmd]) => /queue|provider|board_save/.test(cmd)), false, 'the dispatcher, not this view, makes the card');

  // a refusal says why, keeps the row and pulls again
  b.set({ enabled: true, lastPoll: 1793942600, lastError: null, items: [ITEMS[1]] });
  b.refuse('slack_mention_route', 'settled');
  const pulls = b.named('slack_mentions').length;
  const second = actions(rows()[0])[1];
  second.value = 'R-eyes';
  second.onchange();
  await tick();
  assert.equal(rows().length, 1);
  assert.ok(b.named('slack_mentions').length > pulls);
  assert.equal(ids.get('toasts').children.at(-1).textContent, t('mentions.settled'));
  b.refuse(null);

  // dismissing: one call, the row goes, the list says it is empty
  actions(rows()[0])[2].onclick();
  await tick();
  assert.deepEqual(b.named('slack_mention_dismiss').at(-1)[1], { keys: [ITEMS[1].key] });
  assert.equal(rows().length, 0);
  assert.equal(ids.get('mentions-list').children[0].textContent, t('mentions.empty'));
  assert.equal(ids.get('mentions-dismiss-all').disabled, true);

  ids.get('mentions-back').onclick();
  assert.deepEqual(moves.at(-1), ['switch', 'P1']);
});

test('dismiss all asks once; no route, an error or Slack off are said, not hidden', async () => {
  setLocale('en');
  ctx.settings = { inbound: { sources: { slack: { enabled: true } }, rules: [] } };
  store.projects = PROJECTS;
  state.view = 'mentions';
  const b = backend({ enabled: true, lastPoll: null, lastError: 'ratelimited', items: ITEMS });
  await refreshMentions();
  assert.equal(ids.get('mentions-no-routes').hidden, false);
  assert.equal(ids.get('mentions-status').textContent, t('mentions.error', { code: 'ratelimited' }));
  assert.equal(actions(rows()[0])[1].disabled, true, 'nothing to choose');

  ids.get('mentions-dismiss-all').onclick();
  await tick();
  assert.equal(ids.get('cfm-msg').textContent, t('mentions.dismissAllConfirm', { count: '2' }));
  ctx.cfmResolve(false);
  await tick();
  assert.equal(b.named('slack_mention_dismiss').length, 0, 'declined: nothing is dismissed');
  ids.get('mentions-dismiss-all').onclick();
  await tick();
  ctx.cfmResolve(true);
  await tick();
  assert.deepEqual(b.named('slack_mention_dismiss').at(-1)[1], { keys: ITEMS.map(item => item.key) });

  b.refuse('slack_mention_dismiss', 'shape');
  b.set({ enabled: true, lastPoll: 9, lastError: null, items: ITEMS });
  await refreshMentions();
  actions(rows()[0])[2].onclick();
  await tick();
  assert.equal(rows().length, 2, 'a refused dismissal keeps the row');
  assert.equal(ids.get('toasts').children.at(-1).textContent, t('mentions.failed', { code: 'shape' }));

  b.set({ enabled: false, lastPoll: null, lastError: null, items: [] });
  await b.heard['slack-mentions-changed']?.();
  await refreshMentions();
  assert.equal(ids.get('mentions-btn').hidden, true);
  assert.equal(ids.get('mentions-count').textContent, '');
  assert.equal(ids.get('mentions-status').textContent, t('mentions.off'));

  window.__TAURI__.core.invoke = async () => { throw new Error('gone'); };
  await refreshMentions();
  assert.equal(ids.get('mentions-btn').hidden, true, 'a failed pull keeps what was shown');
  state.view = 'board';
});
