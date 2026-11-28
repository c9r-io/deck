// The Quota area (quota.js), its model (quota-model.js) and the Settings
// switch's lines (extension-quota.js) against the fake document: one row
// per account, a chip only on a card linked to one, stale and past-reset
// windows said as such, and nothing of it decides anything.
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, innerWidth: 1280, innerHeight: 800, addEventListener() {} };
fakeDocument.body = new FakeElement('body');

const model = await import('../js/quota-model.js');
const quota = await import('../js/quota.js');
const ext = await import('../js/extension-quota.js');
const { t } = await import('../js/i18n.js');

const NOW = Date.now();
const account = (id, kind, folders, windows, extra = {}) => ({
  id, kind, byAccount: true, plan: null, folders: folders.map(name => ({ name, path: `/Users/u/${name}` })),
  windows, observedAt: NOW - 1000, source: kind === 'codex' ? 'rollout' : 'statusline', stale: false, sessions: [], ...extra,
});
const win = (name, minutes, usedPercent, extra = {}) => ({ name, minutes, usedPercent, resetsAt: NOW + 2 * 3600e3 + 60e3, expired: false, ...extra });
const SNAPSHOT = {
  codex: null,
  accounts: [
    account('acct-1', 'claude', ['.claude-work'], [win('five_hour', 300, 62), win('seven_day', 10080, 31)], { sessions: ['deck-card-a'] }),
    account('acct-2', 'codex', ['.codex'], [win('seven_day', 10080, 96)], { plan: 'prolite', sessions: ['deck-card-b'] }),
    account('acct-3', 'agy', ['.gemini'], [win('gemini-weekly', 10080, 10, { expired: true })], { stale: true }),
  ],
};

test('the model grades, names and links without deciding anything', () => {
  assert.equal(model.level(79.9), 'ok');
  assert.equal(model.level(80), 'warn');
  assert.equal(model.level(95), 'danger');
  assert.equal(model.windowShort({ minutes: 300 }), '5h');
  assert.equal(model.windowShort({ minutes: 10080 }), '7d');
  assert.equal(model.windowShort({ minutes: 90 }), '90m');
  assert.equal(model.windowShort({ name: 'primary' }), 'primary');
  assert.equal(model.accountLabel(SNAPSHOT.accounts[0]), 'Claude · .claude-work');
  assert.equal(model.accountLabel({ kind: 'agy', folders: [] }), 'agy');
  assert.equal(model.tightest(SNAPSHOT.accounts[2]), null, 'a window past its reset is not the tightest');
  assert.equal(model.tightestOverall(SNAPSHOT).usedPercent, 96);
  assert.equal(model.accountForSession(SNAPSHOT, 'deck-card-b').id, 'acct-2');
  assert.equal(model.accountForSession(SNAPSHOT, 'nobody'), null);
  assert.equal(model.accountForSession(SNAPSHOT, ''), null);
  assert.deepEqual(model.chipFor(SNAPSHOT, 'deck-card-a'),
    { account: 'acct-1', text: 'Claude .claude-work 62%', level: 'ok', stale: false });
  assert.equal(model.chipFor(SNAPSHOT, 'nobody'), null);
  const tr = (key, vars) => `${key}:${JSON.stringify(vars || {})}`;
  assert.equal(model.resetIn(NOW - 1, NOW, tr), 'quota.resetDone:{}');
  assert.equal(model.resetIn(NOW + 5 * 60e3, NOW, tr), 'quota.resetMinutes:{"m":5}');
  assert.equal(model.resetIn(NOW + 134 * 60e3, NOW, tr), 'quota.resetHours:{"h":2,"m":14}');
  assert.equal(model.resetIn(NOW + 3 * 86400e3, NOW, tr), 'quota.resetDays:{"d":3,"h":0}');
  assert.match(model.clock(NOW), /^\d\d:\d\d$/);
});

test('the area shows one row per account and hides while there is none', async () => {
  const calls = [];
  let answer = { accounts: [], codex: null };
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'quota_snapshot') return structuredClone(answer);
    return null;
  } } };
  const opened = [];
  quota.initQuota({ provider: { list: () => [{ id: 'C1', session: 'deck-card-b', title: 'Fix parser' }] }, openSession: id => opened.push(id) });
  await quota.refreshQuota();
  assert.equal(ids.get('quota-area').hidden, true, 'nothing observed: no area');
  answer = SNAPSHOT;
  let heard = null;
  quota.onQuotaChange(next => { heard = next; });
  await quota.refreshQuota();
  assert.equal(heard.accounts.length, 3, 'open panes repaint their chips');
  assert.equal(ids.get('quota-area').hidden, false);
  const rows = ids.get('quota-list').children;
  assert.equal(rows.length, 3);
  assert.equal(rows[0].dataset.account, 'acct-1');
  assert.equal(ids.get('quota-ring').textContent, '96');
  assert.match(ids.get('quota-ring').className, /quota-danger/);
  // the stale agy row says when its numbers are from, its past-reset window shows no percent
  const agyHead = rows[2].children[0];
  assert.equal(agyHead.children[1].className, 'quota-asof');
  const agyWin = rows[2].children[1].children[0];
  assert.equal(agyWin.children[2].textContent, '—');
  // the detail lists the cards on the account; a click opens one
  const anchor = rows[1];
  anchor.getBoundingClientRect = () => ({ right: 200, top: 300 });
  quota.showDetail('acct-2', anchor);
  const pop = fakeDocument.body.children.at(-1);
  assert.equal(pop.id, 'quota-pop');
  const card = pop.children.find(node => node.className === 'quota-pop-card');
  assert.equal(card.textContent, 'Fix parser');
  card.onclick();
  assert.deepEqual(opened, ['C1']);
  assert.equal(pop.isConnected, false, 'opening a card closes the detail');
  quota.showDetail('missing', anchor);
  // an unreadable Codex home is said in the area
  answer = { accounts: [], codex: 'unreadable' };
  await quota.refreshQuota();
  assert.equal(ids.get('quota-area').hidden, false);
  assert.equal(ids.get('quota-list').children[0].textContent, t('quota.codexUnreadable'));
  // a refused read keeps what is shown
  window.__TAURI__.core.invoke = async () => { throw 'gone'; };
  await quota.refreshQuota();
  assert.equal(ids.get('quota-list').children.length, 1);
});

test('a card chip appears only for a linked card and goes with the link', async () => {
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async () => structuredClone(SNAPSHOT) } };
  await quota.refreshQuota();
  const head = new FakeElement('div');
  const px = new FakeElement('button');
  let chip = null;
  px.before = node => { chip = node; head.children.push(node); };
  head.querySelector = sel => (sel === '.px' ? px : sel === '.quotachip' ? chip : null);
  quota.paintQuotaChip(head, 'deck-card-b');
  assert.match(chip.className, /quota-danger/);
  assert.equal(chip.textContent, t('quota.chip', { text: 'Codex .codex 96%' }));
  chip.remove = () => { chip = null; };
  quota.paintQuotaChip(head, 'deck-card-x');
  assert.equal(chip, null);
  quota.paintQuotaChip(null, 'deck-card-b');
});

test('the switch line names each agent and the release-only limit', () => {
  const tr = (key) => key;
  assert.equal(ext.quotaLine(null, false, { t: tr }), 'settings.readFailed');
  assert.equal(ext.quotaLine({}, true, { t: tr }), 'settings.quotaChanging');
  assert.equal(ext.quotaLine({ claude: 'user', agy: 'absent', writable: false }, false, { t: tr }),
    'Claude Code: settings.quotaUser · agy: settings.quotaAbsent · settings.quotaReleaseOnly');
  assert.equal(ext.quotaOn({ claude: 'on', agy: 'absent' }), true);
  assert.equal(ext.quotaOn({ claude: 'on', agy: 'off' }), false);
  assert.equal(ext.quotaOn({ claude: 'absent', agy: 'absent' }), false);
  assert.equal(ext.quotaOn(null), false);
  assert.equal(ext.CONTRIBUTION.settings[0].section, 'agents');
});

test('the switch controller writes once and asks the area to look again', async () => {
  const calls = [];
  let events = 0;
  window.dispatchEvent = () => { events++; };
  const toasts = [];
  let refuse = false;
  const $ = id => fakeDocument.getElementById(id);
  let confirm = true;
  const host = { $, t: key => key, toast: m => toasts.push(m), confirmDialog: async () => confirm, inv: async (cmd, args) => {
    calls.push([cmd, args]);
    if (refuse && cmd === 'quota_set') throw 'release-only';
    if (cmd === 'quota_status') return { claude: 'off', agy: 'absent', writable: true };
    if (cmd === 'quota_set') return { claude: 'on', agy: 'absent', writable: true };
    return null;
  } };
  fakeDocument.body.insertAdjacentHTML('beforebegin', ext.CONTRIBUTION.slots[0].html);
  const controller = ext.CONTRIBUTION.create(host);
  controller.mount();
  await controller.refresh();
  assert.equal($('set-quota').checked, false);
  assert.equal($('set-quota').disabled, false);
  confirm = false;
  assert.equal(await controller.set(true), false, 'a declined question writes nothing');
  assert.equal(calls.filter(([c]) => c === 'quota_set').length, 0);
  confirm = true;
  assert.equal(await controller.set(true), true);
  assert.deepEqual(calls.filter(([c]) => c === 'quota_set'), [['quota_set', { enable: true }]]);
  assert.equal(events, 1);
  assert.deepEqual(toasts, ['settings.quotaEnabled']);
  refuse = true;
  assert.equal(await controller.set(false), false);
  assert.equal(controller.isPending(), false);
  controller.localeChanged();
});
