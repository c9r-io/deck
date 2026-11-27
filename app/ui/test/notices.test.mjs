// The Notifications view (notices.js) and its model against the fake
// document: one page over the local inbox and the Slack mention list, each
// row's actions going back to its own source's command, the intake switch
// saved through the one settings writer, and text shown as text.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const model = await import('../js/notices-model.js');
const { initNotices, paintNotices, refreshNotices, showNotices, startNotices } = await import('../js/notices.js');
const { channelBlockReason } = await import('../js/channel-model.js');
const { ctx, state, store } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');

const NOW = Math.floor(Date.now() / 1000);
const RULES = [
  { id: 'lci', source: 'local', badge: 'lci', name: 'CI failures', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage',
    enabled: true, localMatch: { producer: 'ci-*', types: ['build.*'], labels: { env: 'prod' }, auto: true } },
  { id: 'lall', source: 'local', badge: 'lall', name: '', projectId: 'P1', columnId: 'C1', cmd: 'bash', template: 'triage',
    enabled: true, localMatch: { producer: '*', auto: false } },
  { id: 'R-eyes', source: 'slack', badge: 'eyes', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage' },
];
const PROJECTS = [{ id: 'P1', name: 'Proj', columns: [{ id: 'C1' }], templates: [{ name: 'triage', steps: ['look at {{msg.text}}'] }] }];
const item = (id, extra = {}) => ({ id, producer: 'ci-main', key: id, type: 'build.failed', title: `Build ${id} failed`,
  body: '<b>step 3</b>\nfailed', url: 'https://ci.example/1', labels: { env: 'prod' }, revision: 0, occurrence: 1,
  external: 'active', attention: 'open', firstAt: NOW - 100, changedAt: NOW - 100, occurredAt: null, updated: false,
  routing: null, ...extra });
const LOCAL = {
  enabled: true, listening: true, cliPath: '/Applications/deck.app/Contents/MacOS/deck-notify', error: null,
  items: [
    item('n-1', { changedAt: NOW - 10 }),
    item('n-2', { producer: 'backup', type: 'notification', labels: {}, attention: 'dismissed', updated: true }),
    item('n-3', { external: 'resolved', routing: { by: 'auto', rule: 'lci', state: 'done', card: 'S1' }, attention: 'routed' }),
  ],
  producers: [{ producer: 'ci-main', lastAt: NOW - 10, dayCount: 2 }],
};
const SLACK = { enabled: true, items: [{ key: 'C1/1.2', text: 'hi @me\nmore', from: 'bob', where: '#dev', link: 'https://x/p', at: NOW - 50 }] };

function backend() {
  const calls = [];
  let local = structuredClone(LOCAL);
  let refuse = null;
  const heard = {};
  window.__TAURI__ = {
    event: { listen: async (name, fn) => { heard[name] = fn; } },
    core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (refuse && cmd === refuse.cmd) throw refuse.word;
      if (cmd === 'local_notifications') return structuredClone(local);
      if (cmd === 'slack_mentions') return structuredClone(SLACK);
      if (cmd === 'local_notification_dismiss') return 1;
      if (cmd === 'settings_save' || cmd === 'save_settings') return null;
      return null;
    } },
  };
  return { calls, heard, set(next) { local = next; }, refuse(cmd, word) { refuse = cmd ? { cmd, word } : null; },
    named: name => calls.filter(([cmd]) => cmd === name) };
}

const rows = () => ids.get('notices-list').children.filter(node => node.className === 'mention-row');
const detail = () => ids.get('notices-detail').children;
const actions = () => detail().find(child => child.className === 'mention-actions').children;
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

test('the model: two sources, one row shape, two state axes, filters and counts', () => {
  const local = model.localRows(LOCAL);
  const slack = model.slackRows(SLACK, NOW);
  assert.equal(slack[0].source, 'slack');
  assert.equal(slack[0].title, 'hi @me', 'the first line');
  assert.deepEqual(model.slackRows({ enabled: false, items: SLACK.items }, NOW), [], 'nothing while Slack is off');
  const all = model.allRows(local, slack);
  assert.deepEqual(all.map(row => row.id), ['n-1', 'slack:C1/1.2', 'n-2', 'n-3'], 'newest first');
  assert.deepEqual(all.filter(model.needsUser).map(row => row.id), ['n-1', 'slack:C1/1.2'],
    'dismissed, resolved and routed ones never need the user');
  assert.deepEqual(model.noticeFilters(all, { slack: true }).map(f => [f.id, f.count]),
    [['all', 2], ['slack', 1], ['local:backup', 0], ['local:ci-main', 1]]);
  assert.deepEqual(model.noticeFilters(all).map(f => f.id), ['all', 'local:backup', 'local:ci-main'], 'Slack only while it is on');
  const { open, settled } = model.filtered(all, 'local:ci-main');
  assert.deepEqual([open.map(r => r.id), settled.map(r => r.id)], [['n-1'], ['n-3']]);
  assert.equal(model.localCount(LOCAL), 1);
  assert.equal(model.localCount(null), 0);
  assert.deepEqual(model.localRoutes(RULES, PROJECTS, channelBlockReason).map(r => [r.id, r.label, r.disabled]),
    [['lci', 'CI failures', false], ['lall', '*', true]], 'settings order; a refused command cannot be chosen');
  assert.equal(model.routesFor(slack[0], RULES, PROJECTS, channelBlockReason)[0].id, 'R-eyes', 'a Slack row routes to badge rules');
  assert.equal(model.ruleFits(RULES[0], local[0]), true);
  assert.equal(model.ruleFits(RULES[0], { ...local[0], labels: { env: 'dev' } }), false);
  assert.equal(model.ruleFits(RULES[0], { ...local[0], producer: 'backup' }), false);
  assert.equal(model.ruleFits(RULES[1], local[1]), true, '* fits anything');
  assert.equal(model.ruleFits(RULES[2], local[0]), false);
  assert.deepEqual(model.stateWords(local[2], id => `rule ${id}`).map(w => w.key),
    ['notices.state.resolved', 'notices.state.routedAuto']);
  assert.deepEqual(model.stateWords(local[1]).map(w => w.key), ['notices.state.dismissedUpdated']);
  assert.deepEqual(model.stateWords({ ...local[0], routing: { state: 'skipped', rule: 'lci', reason: 'no-template' } }).map(w => w.key),
    ['notices.state.skipped']);
  assert.deepEqual(model.stateWords({ ...local[0], routing: { state: 'pending', rule: 'lci' }, external: 'withdrawn' }).map(w => w.key),
    ['notices.state.withdrawn', 'notices.state.routing']);
  for (const code of ['settings', 'no-rule', 'board', 'no-template', 'missing', 'stale', 'taken', 'unavailable']) {
    assert.notEqual(t(model.routeErrorKey(code)), model.routeErrorKey(code), `${code} has its sentence`);
  }
  assert.equal(model.routeErrorKey('weird'), 'notices.error.unavailable');
  assert.equal(model.labelLine({ z: '1', a: '2' }), 'a=2 · z=1');
  assert.notEqual(model.rowsSignature(local), model.rowsSignature(local.slice(1)));
  assert.equal(model.patternMatches('ci-*', 'ci-main'), true);
  assert.equal(model.patternMatches('ci', 'ci-main'), false);
});

test('the view: one page, actions go to each source, the switch through the settings writer', async () => {
  setLocale('en');
  ids.clear();
  ctx.settings = { inbound: { sources: { slack: { enabled: true } }, rules: RULES } };
  store.projects = PROJECTS;
  state.projectId = 'P1';
  state.view = 'board';
  const b = backend();
  const moves = [];
  initNotices({
    provider: { project: id => PROJECTS.find(project => project.id === id) },
    render: () => paintNotices(),
    switchProject: id => moves.push(['switch', id]),
    leaveSessionView: () => moves.push(['leave']),
    closeBuffer: () => moves.push(['close-buffer']),
    showMentions: () => moves.push(['mentions']),
  });
  await startNotices();
  assert.equal(typeof b.heard['local-notifications-changed'], 'function', 'a content-free listener');
  assert.equal(ids.get('notices-count').textContent, '1', 'the local rows that need the user');
  assert.equal(ids.get('notices-view').hidden, true);
  showNotices();
  await tick();
  assert.equal(state.view, 'notices');
  assert.equal(ids.get('notices-view').hidden, false);
  assert.equal(rows().length, 2, 'the two that need the user; settled ones are folded');
  assert.equal(ids.get('notices-accept').checked, true);
  assert.equal(ids.get('notices-cli').hidden, false);
  assert.equal(ids.get('notices-cli-path').textContent, LOCAL.cliPath);
  assert.match(ids.get('notices-recent').textContent, /ci-main/);
  // the body is text: markup inside it stays characters
  const body = detail().find(node => node.className === 'mention-message').children[0];
  assert.equal(body.textContent, '<b>step 3</b>\nfailed');
  // route the selected local row: one call with what the user saw
  const menu = actions().find(node => node.className === 'mention-route');
  menu.value = 'lci';
  await menu.onchange();
  const [[, routed]] = b.named('local_notification_route');
  assert.deepEqual(routed, { id: 'n-1', occurrence: 1, revision: 0, rule: 'lci' });
  // a refusal names its reason
  b.refuse('local_notification_route', 'stale');
  menu.value = 'lci';
  await menu.onchange();
  b.refuse(null);
  // the link opens through the one opener
  await actions().find(node => node.className === 'btn mention-open').onclick();
  assert.equal(b.named('open_target')[0][1].value, 'https://ci.example/1');
  // dismiss: the occurrence it was
  const dismiss = actions().find(node => node.className === 'btn mention-dismiss');
  await dismiss.onclick();
  assert.deepEqual(b.named('local_notification_dismiss')[0][1], { items: [{ id: 'n-1', occurrence: 1 }] });
  // the Slack row's actions are the Slack mention commands
  const slackRow = rows().find(node => node.dataset.id === 'slack:C1/1.2');
  slackRow.onclick();
  await tick();
  const slackMenu = actions().find(node => node.className === 'mention-route');
  slackMenu.value = 'R-eyes';
  await slackMenu.onchange();
  assert.deepEqual(b.named('slack_mention_route')[0][1], { key: 'C1/1.2', rule: 'R-eyes' });
  await actions().find(node => node.className === 'btn mention-dismiss').onclick();
  assert.deepEqual(b.named('slack_mention_dismiss')[0][1], { keys: ['C1/1.2'] });
  actions().at(-1).onclick();
  assert.deepEqual(moves.at(-1), ['mentions'], 'its context is read in the Slack mentions view');
  // the filter narrows to one producer; the fold shows the settled ones
  ids.get('notices-filter').children.find(button => button.dataset.v === 'local:ci-main').onclick();
  assert.equal(rows().length, 1);
  ids.get('notices-list').children.find(node => node.className === 'mention-fold').onclick();
  assert.equal(rows().length, 2, 'the routed one under the fold');
  // copying the path is one clipboard call
  ids.get('notices-cli-copy').onclick();
  assert.equal(b.named('write_clipboard')[0][1].text, LOCAL.cliPath);
  // an unreadable inbox says so instead of an empty list
  b.set({ ...LOCAL, items: [], producers: [], error: 'unreadable' });
  ids.get('notices-filter').children.find(button => button.dataset.v === 'all')?.onclick();
  await refreshNotices();
  b.set({ enabled: false, listening: false, cliPath: null, error: null, items: [], producers: [] });
  await refreshNotices();
  assert.equal(ids.get('notices-cli').hidden, true);
  assert.equal(ids.get('notices-intake-status').textContent, t('notices.intake.off'));
  ids.get('notices-back').onclick();
  assert.deepEqual(moves.at(-1), ['switch', 'P1']);
});
