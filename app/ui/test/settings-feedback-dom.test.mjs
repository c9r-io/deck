// Settings feedback is rendered by the production settings handlers. Native
// reads, durable writes, and post-save effects are the only mocked boundary;
// controlled promises make every intermediate phase observable.
import test, { afterEach, mock } from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument, ids } from './fixtures/dom-fixture.mjs';

fakeDocument.querySelectorAll = () => [];
fakeDocument.documentElement = new FakeElement('html');
fakeDocument.documentElement.style.setProperty = function setProperty(name, value) { this[name] = value; };
globalThis.document = fakeDocument;
globalThis.window = {
  __TAURI__: null,
  dispatchEvent() {},
  matchMedia: () => ({ matches: true, addEventListener() {}, removeEventListener() {} }),
};

const { ctx } = await import('../js/state.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const {
  initSettings, loadSettings, openSettings, setFontScale,
} = await import('../js/settings.js');

const get = id => fakeDocument.getElementById(id);
const deferred = () => {
  let resolve; let reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
};
const turn = async (times = 4) => {
  for (let index = 0; index < times; index += 1) await new Promise(resolve => setImmediate(resolve));
};
const gates = new Set();
const regression = (name, fn) => test(name, { timeout: 5000 }, fn);

afterEach(async () => {
  for (const gate of gates) gate.resolve(null);
  await turn(8);
  for (const root of ids.values()) {
    for (const row of descendants(root)) {
      if (row.dataset?.settingKey) row.children[2]?.onclick?.();
    }
  }
  gates.clear();
});

initSettings();

function gate() {
  const value = deferred();
  gates.add(value);
  return value;
}

function descendants(node) {
  return (node.children || []).flatMap(child => [child, ...descendants(child)]);
}

function feedback(setting, key) {
  return descendants(get(`set-item-${setting}`))
    .find(node => node.dataset?.settingKey === key && node.isConnected !== false && !node.hidden);
}

function feedbackParts(setting, key) {
  const row = feedback(setting, key);
  assert.ok(row, `${setting} has feedback for ${key}`);
  assert.equal(row.children.length, 3, 'feedback has label, live status, and dismiss control');
  assert.equal(row.role, 'status');
  return { row, label: row.children[0], status: row.children[1], dismiss: row.children[2] };
}

function checkboxState(id) {
  const box = get(id);
  return { checked: box.checked, indeterminate: box.indeterminate, disabled: box.disabled };
}

async function settingsScene({ saves = [], nativeLocales = [], hookReads = [], hookWrites = [] } = {}) {
  const initial = normalizeSettings({
    locale: 'en', theme: 'deck-dark', accent: 'teal', editor: 'Visual Studio Code', fontScale: 1,
  });
  const disk = { data: serializeSettings(initial), writes: [] };
  const savePlans = saves.map(ok => ({ ok, started: deferred(), release: gate() }));
  const localePlans = nativeLocales.map(ok => ({ ok, started: deferred(), release: gate() }));
  const readPlans = hookReads.map(() => gate());
  const hookWriteQueue = [...hookWrites];
  const localeQueue = [...localePlans];
  const readQueue = [...readPlans];
  const calls = [];
  let localeArmed = false;
  window.__TAURI__ = { event: { listen: async () => () => {} }, core: { invoke: async (command, args) => {
    calls.push({ command, args: args == null ? args : structuredClone(args) });
    if (command === 'load_settings') return { data: disk.data };
    if (command === 'save_settings') {
      const plan = savePlans[disk.writes.length];
      assert.ok(plan, `unexpected settings write ${disk.writes.length + 1}`);
      disk.writes.push({ args: structuredClone(args) });
      plan.started.resolve();
      await plan.release.promise;
      if (!plan.ok) throw new Error('mock settings write failed');
      disk.data = args.data;
      return disk.data;
    }
    if (command === 'set_native_locale' && localeArmed && localeQueue.length) {
      const plan = localeQueue.shift();
      plan.started.resolve();
      await plan.release.promise;
      if (!plan.ok) throw new Error('mock native locale update failed');
      return null;
    }
    if (command === 'agent_hooks_status' && readQueue.length) return readQueue.shift().promise;
    if (command === 'agent_hooks_set' && hookWriteQueue.length) {
      if (!hookWriteQueue.shift()) throw new Error('mock hook write failed');
      return null;
    }
    if (command === 'detect_editors') return [];
    if (command === 'log_size') return 0;
    return null;
  } } };
  await loadSettings();
  await turn();
  calls.length = 0;
  localeArmed = true;
  get('toasts').replaceChildren();
  get('set-theme').value = ctx.settings.theme;
  get('set-accent').value = ctx.settings.accent;
  get('set-editor').value = ctx.settings.editor;
  get('set-locale').value = ctx.settings.locale;
  return { calls, disk, savePlans, localePlans, readPlans };
}

regression('field feedback exposes saving, saved, applying, and apply-failed phases', async () => {
  const { savePlans, localePlans } = await settingsScene({ saves: [true, true], nativeLocales: [false] });

  get('set-theme').value = 'light';
  const theme = get('set-theme').onchange();
  await savePlans[0].started.promise;
  let parts = feedbackParts('appearance', 'theme');
  assert.equal(parts.row.dataset.phase, 'saving');
  assert.ok(parts.label.textContent);
  assert.ok(parts.status.textContent);
  assert.equal(parts.dismiss.hidden, true, 'pending feedback cannot be dismissed');
  savePlans[0].release.resolve();
  await theme;
  parts = feedbackParts('appearance', 'theme');
  assert.equal(parts.row.dataset.phase, 'saved');

  get('set-locale').value = 'zh-Hans';
  const locale = get('set-locale').onchange();
  await savePlans[1].started.promise;
  assert.equal(feedbackParts('language', 'locale').row.dataset.phase, 'saving');
  savePlans[1].release.resolve();
  await localePlans[0].started.promise;
  assert.equal(feedbackParts('language', 'locale').row.dataset.phase, 'applying');
  localePlans[0].release.resolve();
  await locale;
  parts = feedbackParts('language', 'locale');
  assert.equal(parts.row.dataset.phase, 'apply-failed');
  assert.equal(parts.dismiss.hidden, false);
});

regression('failures persist through reopen and another field success until explicit dismissal', async () => {
  const { calls, savePlans } = await settingsScene({ saves: [false, true] });
  get('set-theme').value = 'light';
  const failed = get('set-theme').onchange();
  await savePlans[0].started.promise;
  savePlans[0].release.resolve();
  await failed;
  let parts = feedbackParts('appearance', 'theme');
  assert.equal(parts.row.dataset.phase, 'failed');
  assert.equal(get('settings-issue-count').hidden, false);
  assert.equal(get('settings-issue-count').textContent, '1');
  assert.equal(get('set-save-issues').hidden, false);
  assert.ok(get('set-save-summary').textContent);

  get('set-close').onclick();
  await openSettings({ section: 'terminal' });
  assert.equal(feedbackParts('appearance', 'theme').row.dataset.phase, 'failed', 'closing does not acknowledge failure');

  get('set-editor').value = 'Cursor';
  const editor = get('set-editor').onchange();
  await savePlans[1].started.promise;
  savePlans[1].release.resolve();
  await editor;
  assert.equal(feedbackParts('editor', 'editor').row.dataset.phase, 'saved');
  parts = feedbackParts('appearance', 'theme');
  assert.equal(parts.row.dataset.phase, 'failed', 'another choice succeeding does not clear the issue');

  const beforeNavigate = calls.length;
  get('set-save-issues').onclick();
  assert.equal(fakeDocument.activeElement, get('set-item-appearance'));
  assert.equal(calls.length, beforeNavigate, 'the footer navigates without retrying, writing, or dismissing');
  assert.equal(parts.row.isConnected, true);

  const beforeDismiss = calls.length;
  parts.dismiss.onclick();
  assert.equal(calls.length, beforeDismiss, 'dismissal has no native side effect');
  assert.equal(parts.row.hidden, true);
  assert.equal(feedback('appearance', 'theme'), undefined);
  assert.equal(get('settings-issue-count').hidden, true);
  assert.equal(get('set-save-issues').hidden, true);
});

regression('a newer same-key operation owns feedback when an older result settles', async () => {
  const { savePlans } = await settingsScene({ saves: [false, true] });
  const first = setFontScale(1.2);
  await savePlans[0].started.promise;
  const second = setFontScale(1.4);
  assert.equal(feedbackParts('appearance', 'font').row.dataset.phase, 'saving');

  savePlans[0].release.resolve();
  await first;
  await savePlans[1].started.promise;
  assert.equal(feedbackParts('appearance', 'font').row.dataset.phase, 'saving',
    'the older failure cannot replace the newer pending state');
  assert.equal(get('settings-issue-count').hidden, true);

  savePlans[1].release.resolve();
  await second;
  assert.equal(feedbackParts('appearance', 'font').row.dataset.phase, 'saved');
  assert.equal(get('settings-issue-count').hidden, true);
});

regression('hook reads show bounded unknown and failure states; retry recovers without writing', async () => {
  const { calls, readPlans } = await settingsScene({ hookReads: [null, null, null] });
  const opening = openSettings({ section: 'agents' });
  await opening;
  assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: true, disabled: true });
  assert.deepEqual(checkboxState('set-codex-hooks'), { checked: false, indeterminate: true, disabled: true });
  assert.deepEqual(checkboxState('set-claude-reentry'), { checked: false, indeterminate: true, disabled: true });
  assert.deepEqual(checkboxState('set-pi-hooks'), { checked: false, indeterminate: true, disabled: true });
  assert.deepEqual(checkboxState('set-pi-lifecycle'), { checked: false, indeterminate: true, disabled: true });
  readPlans[0].reject(new Error('mock unreadable hooks'));
  await turn();
  assert.ok(get('set-agent-status-read').textContent);
  assert.ok(get('set-claude-reentry-read').textContent);
  assert.equal(get('set-agent-status-retry').hidden, false);
  assert.equal(get('set-claude-reentry-retry').hidden, false);

  const retry = get('set-agent-status-retry').onclick();
  assert.equal(get('set-agent-status-retry').disabled, true);
  readPlans[1].resolve({
    claude: true, codex: false, claude_reentry: 'yes',
    pi: false, pi_lifecycle: false, pi_installation: 'absent',
  });
  await retry;
  assert.deepEqual(checkboxState('set-agent-hooks'), { checked: true, indeterminate: false, disabled: false });
  assert.deepEqual(checkboxState('set-codex-hooks'), { checked: false, indeterminate: false, disabled: false });
  assert.deepEqual(checkboxState('set-claude-reentry'), { checked: false, indeterminate: true, disabled: true },
    'only a strict boolean makes an individual switch usable');
  assert.deepEqual(checkboxState('set-pi-hooks'), { checked: false, indeterminate: false, disabled: false });
  assert.equal(get('set-agent-status-read').textContent, '');
  assert.equal(get('set-agent-status-retry').hidden, true);

  const reentryRetry = get('set-claude-reentry-retry').onclick();
  readPlans[2].resolve({ claude: true, codex: false, claude_reentry: true });
  await reentryRetry;
  assert.deepEqual(checkboxState('set-claude-reentry'), { checked: true, indeterminate: false, disabled: false });
  assert.equal(get('set-claude-reentry-read').textContent, '');
  assert.equal(get('set-claude-reentry-retry').hidden, true);
  assert.equal(calls.filter(call => /_set$/.test(call.command) || call.command === 'save_settings').length, 0);
});

regression('a hook read times out, stays unknown after its late reply, and a fresh retry wins', async () => {
  const { calls, readPlans } = await settingsScene({ hookReads: [null, null] });
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    await openSettings({ section: 'agents' });
    assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: true, disabled: true });

    mock.timers.tick(10_000);
    await turn();
    assert.ok(get('set-agent-status-read').textContent, 'the expired read has visible failure feedback');
    assert.equal(get('set-agent-status-retry').hidden, false);
    assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: true, disabled: true });

    const retry = get('set-agent-status-retry').onclick();
    assert.equal(get('set-agent-status-retry').disabled, true);
    readPlans[0].resolve({
      claude: true, codex: true, claude_reentry: true,
      pi: true, pi_lifecycle: true, pi_installation: 'current',
    });
    await turn();
    assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: true, disabled: true },
      'the late result from the expired native read cannot paint over the retry');

    readPlans[1].resolve({
      claude: false, codex: true, claude_reentry: false,
      pi: false, pi_lifecycle: false, pi_installation: 'absent',
    });
    await retry;
    assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: false, disabled: false });
    assert.deepEqual(checkboxState('set-codex-hooks'), { checked: true, indeterminate: false, disabled: false });
    assert.equal(get('set-agent-status-read').textContent, '');
    assert.equal(get('set-agent-status-retry').hidden, true);
    assert.equal(calls.filter(call => /_set$/.test(call.command) || call.command === 'save_settings').length, 0);
  } finally { mock.timers.reset(); }
});

regression('a failed hook write keeps field feedback through closing and reopening Settings', async () => {
  const { readPlans } = await settingsScene({ hookReads: [null, null], hookWrites: [false] });
  await openSettings({ section: 'agents' });
  readPlans[0].resolve({
    claude: true, codex: false, claude_reentry: false,
    pi: false, pi_lifecycle: false, pi_installation: 'absent',
  });
  await turn();

  get('set-agent-hooks').checked = false;
  await get('set-agent-hooks').onchange();
  let parts = feedbackParts('agent-status', 'hook:claude');
  assert.equal(parts.row.dataset.phase, 'failed');
  assert.equal(parts.dismiss.hidden, false);
  assert.equal(get('settings-issue-count').textContent, '1');

  get('set-close').onclick();
  await openSettings({ section: 'agents' });
  parts = feedbackParts('agent-status', 'hook:claude');
  assert.equal(parts.row.dataset.phase, 'failed', 'reopening does not acknowledge a failed hook write');
  assert.equal(get('set-save-issues').hidden, false);
  readPlans[1].resolve({
    claude: true, codex: false, claude_reentry: false,
    pi: false, pi_lifecycle: false, pi_installation: 'absent',
  });
  await turn();
});

regression('newer hook reads win and an unknown disabled control refuses writes', async () => {
  const { calls, readPlans } = await settingsScene({ hookReads: [null, null, null] });
  await openSettings({ section: 'agents' });
  get('set-close').onclick();
  await openSettings({ section: 'agents' });
  readPlans[1].resolve({ claude: true, codex: false, claude_reentry: true });
  await turn();
  assert.deepEqual(checkboxState('set-agent-hooks'), { checked: true, indeterminate: false, disabled: false });
  assert.deepEqual(checkboxState('set-claude-reentry'), { checked: true, indeterminate: false, disabled: false });
  readPlans[0].resolve({ claude: false, codex: true, claude_reentry: false });
  await turn();
  assert.equal(get('set-agent-hooks').checked, true, 'an older open cannot overwrite the newer read');
  assert.equal(get('set-claude-reentry').checked, true);

  const staleRetry = get('set-agent-status-retry').onclick();
  assert.deepEqual(checkboxState('set-agent-hooks'), { checked: false, indeterminate: true, disabled: true });
  get('set-agent-hooks').checked = false;
  await get('set-agent-hooks').onchange();
  assert.equal(calls.filter(call => call.command === 'agent_hooks_set').length, 0,
    'a programmatic change event cannot turn an unknown status into write authority');
  readPlans[2].resolve({ claude: true, codex: false, claude_reentry: true });
  await staleRetry;
  await turn();
  assert.equal(get('set-agent-hooks').checked, true);
});
