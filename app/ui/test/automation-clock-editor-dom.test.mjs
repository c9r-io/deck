// The clock editor's schedule controls round-trip an interval: "every N",
// its counting-from date, the weekday chips and the days-of-month text go
// into the saved rule and come back when the rule is reopened; the preview
// line reads the controls as they stand; pause and resume keep the date.
// The real editor handlers, the real rule-list buttons and the real
// settings writer run here over the test document; only the native side is
// scripted. Layout and the real engine's input handling are the WKWebView
// smoke's business (SMOKE.md, the automation mode's editor round-trip).
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';
fakeDocument.querySelectorAll = () => [];
FakeElement.prototype.getAttribute = function getAttribute(name) { return this[name] ?? null; };
/* a painted rule row: one stable element per selector, a pause button only
   where the row's markup has one */
FakeElement.prototype.querySelector = function querySelector(selector) {
  if (selector === '.ar-pause' && !String(this.innerHTML || '').includes('ar-pause')) return null;
  this.parts ||= new Map();
  if (!this.parts.has(selector)) this.parts.set(selector, new FakeElement());
  return this.parts.get(selector);
};
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { initAutomation, openAutomations, openEditor } = await import('../js/automation.js');
const { provider } = await import('../js/board.js');
const { ctx, state, store } = await import('../js/state.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const { refreshChannelAuthority } = await import('../js/settings.js');
const { t } = await import('../js/i18n.js');
const { schedulePreview } = await import('../js/automation-model.js');
const get = id => fakeDocument.getElementById(id);
/* the list is repainted from nothing, as in a browser */
Object.defineProperty(get('auto-list'), 'innerHTML', { get: () => '', set(value) { if (!value) this.children = []; } });
for (const [id, values] of [['auto-trigger', ['channel', 'clock', 'slack']], ['auto-finish', ['keep', 'close']]]) {
  const buttons = values.map(value => { const button = new FakeElement('button'); button.dataset.v = value; return button; });
  get(id).querySelectorAll = () => buttons;
  get(id).querySelector = () => buttons.find(button => button['aria-pressed'] === 'true');
}
/* the weekday chips are real buttons the editor builds; the fake document
   answers the two selectors the editor uses for them */
const chips = get('auto-days');
Object.defineProperty(chips, 'innerHTML', { get: () => '', set(value) { if (!value) this.children = []; } });
Object.defineProperty(chips, 'childElementCount', { get() { return this.children.length; } });
chips.querySelectorAll = selector => chips.children.filter(b => !selector.includes('aria-pressed') || b['aria-pressed'] === 'true');
const chip = d => chips.children.find(b => b.dataset.d === String(d));
const pressed = () => chips.children.filter(b => b['aria-pressed'] === 'true').map(b => Number(b.dataset.d));

const project = { id: 'P1', columns: [{ id: 'C1', name: 'Working' }], templates: [{ name: 'tpl', steps: ['Review the sprint board.'] }] };
const turn = async (times = 4) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
const said = () => get('toasts').children.map(toast => toast.textContent);
const rules = () => ctx.settings.inbound.rules;
const fill = (id, value, event = 'input') => { get(id).value = value; get(id).fire(event); };

/* settings holding `initial`, the drawer open with its list painted; the
   native side answers loads and saves from an in-memory file */
async function scene(initial = []) {
  if (get('auto-cancel').onclick) get('auto-cancel').onclick();
  for (const element of ids.values()) element.disabled = false;
  get('auto-drawer').hidden = true;
  store.projects = [project]; store.cards = []; state.projectId = 'P1';
  ctx.settings = normalizeSettings({ inbound: { rules: initial } });
  const disk = { data: serializeSettings(ctx.settings), writes: [] };
  window.__TAURI__ = { event: { listen: async () => {} }, core: { invoke: async (command, args) => {
    if (command === 'inbound_runs') return [];
    if (command === 'load_settings') return { data: disk.data };
    if (command !== 'save_settings') return null;
    disk.data = args.data;
    disk.writes.push(disk.data);
    return disk.data;
  } } };
  initAutomation({ provider, activeProject: () => project, newSessionSummary: () => '', openProjectDefaults() {},
    projectDefaultsSummary: () => '', openSession() {}, newDefaultSession() {} });
  await refreshChannelAuthority();
  await openAutomations();
  await turn();
  return disk;
}

test('an interval schedule round-trips through the editor and the preview reads the controls', async () => {
  const disk = await scene();
  openEditor(null);
  await turn();
  assert.equal(get('auto-every').value, '1');
  assert.ok(get('auto-anchor-row').hidden && get('auto-anchor-hint').hidden, 'no date row while the rule repeats every unit');
  assert.deepEqual(pressed(), [1, 2, 3, 4, 5], 'a new rule starts on weekdays');
  get('auto-name').value = 'Sprint review';
  get('auto-column').value = 'C1';
  get('auto-template').value = 'tpl';
  fill('auto-unit', 'week', 'change');
  for (const d of [2, 3, 4, 5]) chip(d).onclick();
  assert.deepEqual(pressed(), [1]);
  fill('auto-every', '2');
  assert.ok(!get('auto-anchor-row').hidden && !get('auto-anchor-hint').hidden, 'the date row appears with the interval');
  assert.match(get('auto-anchor').value, /^\d{4}-\d{2}-\d{2}$/, 'today is filled in');
  fill('auto-anchor', '2026-10-19');
  fill('auto-time', '09:00');
  const expected = { unit: 'week', days: [1], minute: 540, every: 2, anchor: '2026-10-19' };
  assert.equal(get('auto-preview').textContent, t('automation.preview', { slots: schedulePreview(expected) }));
  await get('auto-save').onclick();
  await turn();
  assert.equal(disk.writes.length, 1, 'one settings write');
  assert.deepEqual(rules().map(rule => rule.schedule), [expected]);
  assert.deepEqual(JSON.parse(disk.writes[0]).inbound.rules[0].schedule, expected, 'the file holds the interval and its date');
  const saved = rules()[0];
  assert.ok(saved.since > 0);

  // reopened, every control shows what was saved
  openEditor(saved);
  await turn();
  assert.equal(get('auto-every').value, '2');
  assert.equal(get('auto-anchor').value, '2026-10-19');
  assert.ok(!get('auto-anchor-row').hidden);
  assert.deepEqual(pressed(), [1]);
  assert.equal(get('auto-preview').textContent, t('automation.preview', { slots: schedulePreview(expected) }));

  // every third month on the 1st and the 15th: more than one day of the month
  fill('auto-unit', 'month', 'change');
  fill('auto-dom', '15, 1');
  fill('auto-every', '3');
  fill('auto-anchor', '2026-10-01');
  await get('auto-save').onclick();
  await turn();
  const quarterly = { unit: 'month', days: [1, 15], minute: 540, every: 3, anchor: '2026-10-01' };
  assert.deepEqual(rules().map(rule => rule.schedule), [quarterly]);
  assert.equal(rules()[0].id, saved.id, 'the same rule');
  openEditor(rules()[0]);
  await turn();
  assert.equal(get('auto-dom').value, '1, 15', 'every saved day comes back, sorted');
  assert.equal(get('auto-every').value, '3');

  // a day list the backend would refuse is refused here, with nothing written
  fill('auto-dom', '0, 40');
  assert.equal(get('auto-preview').textContent, t('automation.preview', { slots: '—' }), 'no preview for a schedule that cannot be read');
  const writes = disk.writes.length;
  await get('auto-save').onclick();
  await turn();
  assert.equal(disk.writes.length, writes, 'nothing saved');
  assert.ok(said().includes(t('automation.badMonthDays')));
  assert.equal(fakeDocument.activeElement, get('auto-dom'));
  assert.deepEqual(rules().map(rule => rule.schedule), [quarterly], 'the stored rule is untouched');

  // back to a plain monthly rule: the date goes with the interval
  fill('auto-dom', '1, 15');
  fill('auto-every', '1');
  assert.ok(get('auto-anchor-row').hidden, 'the date row goes with the interval');
  const before = rules()[0].since;
  await get('auto-save').onclick();
  await turn();
  assert.deepEqual(rules().map(rule => rule.schedule), [{ unit: 'month', days: [1, 15], minute: 540 }]);
  assert.ok(!('every' in rules()[0].schedule) && !('anchor' in rules()[0].schedule), 'an interval of 1 leaves no trace');
  assert.ok(rules()[0].since >= before, 'a changed schedule starts fresh');
});

test('pausing and resuming an interval rule keeps its date; only resume moves since', async () => {
  const rule = { id: 'sprint', source: 'clock', badge: 'sprint', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'tpl', dir: '/w',
    name: 'Sprint', enabled: true, schedule: { unit: 'week', days: [1], minute: 540, every: 2, anchor: '2026-10-19' }, finish: 'keep', since: 1000, graceMin: 15 };
  await scene([rule]);
  const row = () => get('auto-list').children[0];
  assert.ok(row(), 'the rule is listed');
  row().querySelector('.ar-pause').onclick();
  await turn();
  assert.equal(rules()[0].enabled, false);
  assert.deepEqual(rules()[0].schedule, rule.schedule);
  assert.equal(rules()[0].since, 1000, 'pausing moves nothing');
  row().querySelector('.ar-pause').onclick();
  await turn();
  assert.equal(rules()[0].enabled, true);
  assert.deepEqual(rules()[0].schedule, rule.schedule, 'the date and interval are kept');
  assert.ok(rules()[0].since > 1000, 'resume starts fresh from now');
});
