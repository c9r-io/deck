// missed-once.js against the fake document: a one-time automation whose day
// passed without a run is listed, "Reschedule" opens its editor, and
// "Remove" deletes the rule and the template only it used — nothing before
// the user's click.
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument } from './fixtures/dom-fixture.mjs';
/* a row's parts by class, searched through its children */
FakeElement.prototype.querySelector = function querySelector(selector) {
  const name = selector.replace(/^\./, '');
  for (const child of this.children || []) {
    if (String(child.className || '').split(' ').includes(name)) return child;
    const inner = child.querySelector(selector);
    if (inner) return inner;
  }
  return null;
};

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { initMissedOnce, missedCountText, missedNow, missedOnceForTests, missedRow, startMissedOnce } = await import('../js/missed-once.js');
const { ctx } = await import('../js/state.js');
const { cfmDone } = await import('../js/dialogs.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const { refreshChannelAuthority } = await import('../js/settings.js');
const { setLocale } = await import('../js/i18n.js');

const tick = async (times = 6) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
const once = (id, extra = {}) => ({ id, source: 'clock', badge: id, projectId: 'P1', columnId: 'C1', cmd: 'claude', template: `${id}-tpl`,
  name: id, enabled: true, finish: 'keep', since: 1, graceMin: 15, schedule: { unit: 'once', days: [], minute: 570, anchor: '2020-01-01' }, ...extra });

test('a missed one-time automation waits for the user: reschedule opens it, remove deletes it and its own template', async () => {
  setLocale('en');
  ctx.settings = normalizeSettings({ inbound: { rules: [
    once('gone'), once('ran', { ranAt: 1577870000 }), once('later', { schedule: { unit: 'once', days: [], minute: 570, anchor: '2999-01-02' } }),
    once('started'),
  ] } });
  const disk = { data: serializeSettings(ctx.settings), writes: [] };
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (cmd, args) => {
    if (cmd === 'inbound_runs') return [{ rule: 'started', card: 'S1', started: 1577870000, outcome: 'running' }];
    if (cmd === 'load_settings') return { data: disk.data };
    if (cmd === 'save_settings') { disk.data = args.data; disk.writes.push(args.data); return disk.data; }
    return null;
  } } };
  const opened = [], switched = [], deleted = [];
  let refreshed = 0;
  initMissedOnce({
    provider: { project: id => (id === 'P1' ? { id, name: 'Deck' } : null), deleteTemplate: async (...args) => { deleted.push(args); } },
    refresh: () => { refreshed += 1; }, switchProject: id => switched.push(id), openRuleEditor: async id => { opened.push(id); },
  });
  await refreshChannelAuthority();
  /* before the run list arrives the rule whose record was not written reads as missed too */
  assert.deepEqual(missedNow().map(rule => rule.id), ['gone', 'started']);
  await startMissedOnce();
  assert.equal(await startMissedOnce(), undefined, 'started once');
  assert.ok(refreshed >= 1);
  assert.deepEqual(missedNow().map(rule => rule.id), ['gone'], 'the run list says the other one ran');
  assert.equal(missedCountText(), '1 missed');

  const row = missedRow(null, missedNow()[0]);
  assert.equal(row.querySelector('.attention-card-name').textContent, 'gone');
  assert.equal(row.querySelector('.attention-origin').textContent, 'Deck');
  assert.match(row.querySelector('.attention-reason').textContent, /2020-01-01/);
  assert.equal(missedRow(row, missedNow()[0]), row, 'an existing row is repainted');
  await row.querySelector('.missed-reschedule').onclick();
  assert.deepEqual([switched, opened], [['P1'], ['gone']]);
  assert.equal(disk.writes.length, 0, 'rescheduling writes nothing by itself');

  /* remove: declined, then confirmed */
  let pending = row.querySelector('.missed-remove').onclick();
  await tick();
  cfmDone(false);
  await pending;
  assert.equal(disk.writes.length, 0);
  pending = row.querySelector('.missed-remove').onclick();
  await tick();
  cfmDone(true);
  await pending;
  await tick();
  assert.deepEqual(ctx.settings.inbound.rules.map(rule => rule.id), ['ran', 'later', 'started']);
  assert.deepEqual(deleted, [['P1', 'gone-tpl']]);
  assert.equal(missedCountText(), '');
  missedOnceForTests.setRuns([]);
  assert.deepEqual(missedNow().map(rule => rule.id), ['started']);
});
