// The Slack mentions view (slack-mentions.js) and its model against the fake
// document: what the sidebar shows, what a row offers, and that routing and
// dismissing are one backend call each, never a write anywhere else.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { MENTION_RANGES, handedRows, mentionAfter, mentionCount, mentionErrorKey, mentionRanges, mentionRoutes, mentionSignature, mentionStatus, mentionsWithin } = await import('../js/slack-mentions-model.js');
const { initMentions, paintMentions, refreshMentions, showMentions, startMentions } = await import('../js/slack-mentions.js');
const { channelBlockReason } = await import('../js/channel-model.js');
const { ctx, state, store } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');

const RULES = [
  { id: 'R-eyes', source: 'slack', badge: 'eyes', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
  { id: 'R-memo', source: 'slack', badge: 'memo', projectId: 'P1', columnId: 'C1', cmd: 'bash', template: 'triage' },
  { id: 'R-gone', source: 'slack', badge: 'zap', projectId: 'P9', columnId: 'C1', cmd: 'claude', template: 'triage' },
  { id: 'R-custom', source: 'slack', badge: 'decklive', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
  { id: 'R-clock', source: 'clock', badge: 'R-clock', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
];
const PROJECTS = [{ id: 'P1', name: 'Proj', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['look at {{msg.text}}'] }] }];
const CONTEXT = { from: 'Bob', thread: true, text: '@Me second, please', files: ['migration.log'],
  before: [{ from: 'carol', text: 'staging fails', at: 1793942500, files: [] },
    { from: 'dave', text: 'rolled back', at: 1793942510, files: ['a.log'] }] };
// the view filters by the clock, so the fixtures are minutes old
const NOW = Math.floor(Date.now() / 1000);
const ITEMS = [
  { key: 'C1/1793942527.287979', text: '@me second\nmore', from: 'bob', where: '#dev', link: 'https://x/p2', at: NOW - 60, context: CONTEXT },
  { key: 'C1/1793942517.831339', text: '@me first', from: 'alice', where: 'DM', link: '', at: NOW - 120, context: null },
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
      if (cmd === 'mcp_status') return { clients: [{ id: 'client_a', name: 'orchestrator' }] };
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
const detail = () => ids.get('mentions-detail').children;
const actions = () => detail().find(child => child.className === 'mention-actions').children;
const texts = () => detail().flatMap(node => [node.textContent, ...(node.children || []).map(child => child.textContent)]);
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('the model lists Slack badge rules only, sorted, and says which cannot be chosen', () => {
  const routes = mentionRoutes(RULES, PROJECTS, channelBlockReason);
  assert.deepEqual(routes.map(route => [route.id, route.disabled]), [['R-custom', false], ['R-eyes', false], ['R-memo', true], ['R-gone', true]], 'in badge-name order');
  assert.equal(routes[1].label, '👀 → Proj / triage', 'a standard badge shows as its emoji');
  assert.equal(routes[3].label, '⚡ → ? / triage', 'a rule whose project is gone is shown, not chosen');
  assert.equal(routes[0].label, ':decklive: → Proj / triage', 'a custom badge keeps its name');
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
  assert.notEqual(mentionSignature(ITEMS, routes), mentionSignature(ITEMS.slice(1), routes));
  assert.notEqual(mentionSignature(ITEMS, routes),
    mentionSignature([{ ...ITEMS[0], from: 'Bob' }, ITEMS[1]], routes), 'names arriving repaint the row');
  const three = [{ key: 'a' }, { key: 'b' }, { key: 'c' }];
  assert.equal(mentionAfter(three, ['a']), 'b');
  assert.equal(mentionAfter(three, ['b']), 'c');
  assert.equal(mentionAfter(three, ['c']), 'b', 'the last one hands over to the one before');
  assert.equal(mentionAfter(three, ['a', 'b', 'c']), null);
  assert.equal(mentionAfter(three, ['x']), 'a');
  assert.equal(mentionAfter(null, ['a']), null);
});

test('the ranges narrow the list, and each says its own count', () => {
  const now = 1_000_000;
  const at = hours => ({ key: `C/${hours}`, at: now - hours * 3600 });
  const view = { enabled: true, items: [at(1), at(24), at(25), at(72), at(73), at(168)] };
  assert.deepEqual(MENTION_RANGES, [1, 3, 7], '24 hours first: the default');
  assert.deepEqual(mentionsWithin(view, 1, now).map(item => item.key), ['C/1', 'C/24'], 'the bounds are inclusive');
  assert.deepEqual(mentionRanges(view, now), [{ days: 1, count: 2 }, { days: 3, count: 4 }, { days: 7, count: 6 }]);
  assert.deepEqual(mentionRanges({ enabled: false, items: view.items }, now).map(range => range.count), [0, 0, 0], 'nothing while Slack is off');
  assert.deepEqual(mentionsWithin(null, 7, now), []);
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
  assert.equal(rows()[0].dataset.selected, 'true', 'the newest is selected');
  assert.equal(rows()[0].children[1].textContent, '@me second', 'the list shows the first line');
  // the reader pane: the thread, then the mention in full, with its files
  assert.ok(texts().includes(t('mentions.contextThread')));
  assert.ok(texts().includes('staging fails') && texts().includes('rolled back'));
  assert.ok(texts().includes(t('mentions.files', { names: 'a.log' })));
  assert.ok(texts().includes(t('mentions.theMention')));
  assert.ok(texts().includes('@me second\nmore'));
  assert.ok(texts().includes(t('mentions.files', { names: 'migration.log' })));
  const [open, menu, dismiss] = actions();
  assert.equal(open.textContent, t('mentions.open'));
  assert.deepEqual(menu.children.map(option => [option.value, option.disabled]),
    [['', false], ['R-custom', false], ['R-eyes', false], ['R-memo', true], ['R-gone', true]]);
  assert.equal(menu.children[3].textContent, t('mentions.unavailable', { label: '📝 → Proj / triage' }));
  assert.equal(menu.disabled, false);
  assert.equal(dismiss.textContent, t('mentions.dismiss'));

  // ↓ selects the next one; its context is still being read
  const keyEvent = key => ({ key, prevented: 0, preventDefault() { this.prevented++; } });
  const down = keyEvent('ArrowDown');
  ids.get('mentions-list').onkeydown(down);
  assert.equal(down.prevented, 1);
  assert.equal(rows()[1].dataset.selected, 'true');
  assert.ok(texts().includes(t('mentions.contextLoading')));
  assert.equal(actions()[0].disabled, true, 'no permalink, nothing to open');
  ids.get('mentions-list').onkeydown(keyEvent('ArrowDown'));
  assert.equal(rows()[1].dataset.selected, 'true', 'the last one stays');
  ids.get('mentions-list').onkeydown(keyEvent('ArrowUp'));
  assert.equal(rows()[0].dataset.selected, 'true');
  ids.get('mentions-list').onkeydown(keyEvent('x'));
  rows()[1].onclick();
  assert.equal(rows()[1].dataset.selected, 'true', 'a click selects');
  rows()[0].onclick();

  actions()[0].onclick();
  await tick();
  assert.deepEqual(b.named('open_target').at(-1)[1], { kind: 'url', value: 'https://x/p2', cwd: ctx.HOME });

  // routing: one backend call naming the message and the rule; the row goes
  // and the next one is selected
  const route = actions()[1];
  route.value = 'R-eyes';
  route.onchange();
  await tick();
  assert.deepEqual(b.named('slack_mention_route').at(-1)[1], { key: ITEMS[0].key, rule: 'R-eyes' });
  assert.equal(rows().length, 1);
  assert.equal(rows()[0].dataset.selected, 'true');
  assert.equal(ids.get('mentions-count').textContent, '1');
  assert.equal(b.calls.some(([cmd]) => /queue|provider|board_save/.test(cmd)), false, 'the dispatcher, not this view, makes the card');

  // a refusal says why, keeps the row and pulls again
  b.set({ enabled: true, lastPoll: 1793942600, lastError: null, items: [ITEMS[1]] });
  b.refuse('slack_mention_route', 'settled');
  const pulls = b.named('slack_mentions').length;
  const second = actions()[1];
  second.value = 'R-eyes';
  second.onchange();
  await tick();
  assert.equal(rows().length, 1);
  assert.ok(b.named('slack_mentions').length > pulls);
  assert.equal(ids.get('toasts').children.at(-1).textContent, t('mentions.settled'));
  b.refuse(null);

  // dismissing: one call, the row goes, the reader says the list is empty
  actions()[2].onclick();
  await tick();
  assert.deepEqual(b.named('slack_mention_dismiss').at(-1)[1], { keys: [ITEMS[1].key] });
  assert.equal(rows().length, 0);
  assert.equal(detail()[0].textContent, t('mentions.empty'));
  assert.equal(ids.get('mentions-dismiss-all').disabled, true);
  ids.get('mentions-list').onkeydown(keyEvent('ArrowDown'));

  ids.get('mentions-back').onclick();
  assert.deepEqual(moves.at(-1), ['switch', 'P1']);
});

test('the range buttons choose how far back the list and its count reach', async () => {
  setLocale('en');
  ctx.settings = { inbound: { sources: { slack: { enabled: true } }, rules: RULES } };
  store.projects = PROJECTS;
  state.view = 'mentions';
  const older = { ...ITEMS[1], key: 'C1/older', from: 'erin', at: NOW - 2 * 86400 };
  const b = backend({ enabled: true, lastPoll: 9, lastError: null, items: [...ITEMS, older] });
  await refreshMentions();
  const label = days => ids.get(`mentions-range-${days}`);
  assert.deepEqual([1, 3, 7].map(days => label(days).textContent), ['24 hours · 2', '3 days · 3', '1 week · 3'],
    'each range says its count, so the older one is not hidden without a number');
  assert.equal(label(1)['aria-pressed'], 'true', '24 hours by default');
  assert.equal(rows().length, 2);
  assert.equal(ids.get('mentions-count').textContent, '2', 'the sidebar counts the chosen range');
  label(3).onclick();
  assert.equal(label(3)['aria-pressed'], 'true');
  assert.equal(label(1)['aria-pressed'], 'false');
  assert.equal(rows().length, 3);
  assert.equal(ids.get('mentions-count').textContent, '3');
  ids.get('mentions-dismiss-all').onclick();
  await tick();
  assert.equal(ids.get('cfm-msg').textContent, t('mentions.dismissAllConfirm', { count: '3' }), 'dismiss all = the mentions shown');
  ctx.cfmResolve(false);
  await tick();
  label(1).onclick();
  assert.equal(rows().length, 2);
  assert.equal(b.named('slack_mention_dismiss').length, 0);
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
  assert.equal(actions()[1].disabled, true, 'nothing to choose');

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
  actions()[2].onclick();
  await tick();
  assert.equal(rows().length, 2, 'a refused dismissal keeps the row');
  assert.equal(ids.get('toasts').children.at(-1).textContent, t('mentions.failed', { code: 'shape' }));

  b.set({ enabled: false, lastPoll: null, lastError: null, items: [] });
  await b.heard['slack-mentions-changed']?.();
  await refreshMentions();
  assert.equal(ids.get('mentions-btn').hidden, true);
  assert.equal(ids.get('mentions-count').textContent, '');
  assert.equal(ids.get('mentions-status').textContent, t('mentions.off'));
  assert.equal(detail()[0].textContent, t('mentions.off'));

  window.__TAURI__.core.invoke = async () => { throw new Error('gone'); };
  await refreshMentions();
  assert.equal(ids.get('mentions-btn').hidden, true, 'a failed pull keeps what was shown');
  state.view = 'board';
});

test('mentions an agent took are shown apart, with where each went', () => {
  const now = 1_800_000_000;
  const view = { enabled: true, items: [], handed: [
    { key: 'C1/1.1', text: 'fix the build\nplease', from: 'bob', where: '#ci', at: now - 60, client: 'client_a', rule: 'triage', card: null },
    { key: 'C1/2.2', text: 'old one', from: 'eve', where: '#ci', at: now - 5 * 86400, client: 'client_x', rule: 'gone', card: 'c1' },
  ] };
  const clients = [{ id: 'client_a', name: 'orchestrator' }];
  const rules = [{ source: 'agent', badge: 'triage', name: 'Triage' }, { source: 'slack', badge: 'gone', name: 'not an agent rule' }];
  assert.deepEqual(handedRows(view, 1, now, clients, rules), [
    { key: 'C1/1.1', from: 'bob', where: '#ci', at: now - 60, line: 'fix the build', client: 'orchestrator', rule: 'Triage' },
  ], 'within the range, with names');
  const week = handedRows(view, 7, now, clients, rules);
  assert.equal(week.length, 2);
  assert.deepEqual([week[1].client, week[1].rule], ['client_x', 'gone'], 'ids when the names are unknown');
  assert.deepEqual(handedRows({ ...view, enabled: false }, 7, now, clients, rules), []);
  assert.deepEqual(handedRows({ enabled: true, items: [] }, 7, now, clients, rules), []);
});

test('the view lists what an agent took above the panes, with names', async () => {
  setLocale('en');
  ctx.settings = { inbound: { sources: { slack: { enabled: true } }, rules: [...RULES,
    { id: 'G1', source: 'agent', badge: 'triage', name: 'Triage', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' }] } };
  store.projects = PROJECTS;
  state.view = 'mentions';
  const handed = { ...ITEMS[0], key: 'C1/handed', text: 'fix CI\nnow', client: 'client_a', rule: 'triage', card: null };
  const b = backend({ enabled: true, lastPoll: 9, lastError: null, items: [ITEMS[1]], handed: [handed] });
  await refreshMentions();
  assert.equal(ids.get('mentions-handed').hidden, false);
  assert.equal(ids.get('mentions-handed-title').textContent, t('mentions.handedTitle', { count: '1' }));
  const row = ids.get('mentions-handed-list').children[0];
  assert.deepEqual(row.children.map(child => child.textContent).slice(-2), ['fix CI', t('mentions.handedTo', { client: 'orchestrator', rule: 'Triage' })]);
  assert.equal(rows().length, 1, 'not in the open list');
  assert.equal(b.named('mcp_status').length, 1, 'client names pulled only when something was handed');
  b.set({ enabled: true, lastPoll: 9, lastError: null, items: [ITEMS[1]], handed: [] });
  await refreshMentions();
  assert.equal(ids.get('mentions-handed').hidden, true);
  assert.equal(b.named('mcp_status').length, 1);
  state.view = 'board';
});
