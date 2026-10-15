import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement } from './fixtures/dom-fixture.mjs';
import {
  extensionContributions, initializeExtensionContributions, refreshAgentContributions, validateExtensionContributions,
  extensionController, runExtensionLifecycle, extensionControlsBusy,
} from '../js/settings-extensions.js';

const elements = new Map();
const mounted = [];
const $ = id => {
  if (!elements.has(id)) {
    const element = new FakeElement();
    element.insertAdjacentHTML = (_where, html) => mounted.push({ id, html });
    elements.set(id, element);
  }
  return elements.get(id);
};
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const writes = [], writeGate = deferred();
let confirmations = 0;
let confirmBehavior = async () => true;
let pi = { signal: false, lifecycle: false, installation: 'absent' };
let statusAnswer = () => ({ agents: {
  'claude-code': { signal: true, lifecycle: false, installation: null },
  codex: { signal: false, lifecycle: null, installation: null }, pi,
} });
const calls = [];
const inv = async (command, args) => {
  calls.push(command);
  if (command === 'agent_hooks_status') return statusAnswer();
  if (command === 'agent_hooks_set') {
    writes.push([command, args]);
    if (args.agent === 'pi') { await writeGate.promise; pi = { ...pi, signal: args.enable, installation: 'current' }; }
  }
};

initializeExtensionContributions({
  $, inv, ctx: { settings: { notifyAway: false } }, t: key => key, toast() {}, uev() {},
  confirmDialog: async () => { confirmations++; return confirmBehavior(); },
  beginFeedback: () => ({}), finishFeedback() {}, renderNotifyDependency() {},
  listen: async () => () => {}, store: { projects: [] }, formatNumber: String, formatDateTime: String,
  commitSettings: async () => true, persistInbound: async () => true, drainWrites: async () => {},
});

const initialCalls = calls.slice();

test('registry validation rejects duplicate extension and control identities', () => {
  assert.deepEqual(extensionContributions().map(entry => entry.id), ['claude-code', 'codex', 'pi', 'slack', 'bergamot', 'connector', 'mcp', 'voice']);
  assert.throws(() => validateExtensionContributions([{ id: 'x' }, { id: 'x' }]), /duplicate/);
  assert.throws(() => validateExtensionContributions([
    { id: 'x', agentControls: [{ boxId: 'same' }] }, { id: 'y', agentControls: [{ boxId: 'same' }] },
  ]), /duplicate agent control/);
});

test('the registry mounts declared panels and projects the agents map through one read', async () => {
  assert.ok(mounted.some(entry => entry.id === 'set-extension-agents' && entry.html.includes('set-item-agent-status')));
  assert.ok(mounted.some(entry => entry.id === 'set-extension-terminal' && entry.html.includes('set-item-local-translation')));
  await refreshAgentContributions();
  assert.deepEqual([$('set-agent-hooks').checked, $('set-codex-hooks').checked, $('set-claude-reentry').checked],
    [true, false, false]);
  assert.equal($('set-pi-hooks').disabled, false);
});

test('an agents map is authoritative when one source is missing or malformed', async () => {
  statusAnswer = () => ({ agents: {
    'claude-code': { signal: false, lifecycle: false, installation: null },
    codex: { signal: 'yes', lifecycle: null, installation: null }, pi,
  }, codex: true });
  await refreshAgentContributions();
  assert.equal($('set-agent-hooks').checked, false);
  assert.equal($('set-codex-hooks').checked, false);
  assert.equal($('set-codex-hooks').indeterminate, true);
  assert.equal($('set-codex-hooks').disabled, true);
  statusAnswer = () => ({ agents: {
    'claude-code': { signal: true, lifecycle: false, installation: null },
    codex: { signal: false, lifecycle: null, installation: null }, pi,
  } });
});

test('partial Pi payloads keep each absent capability unknown', async () => {
  pi = { lifecycle: true, installation: 'current' };
  await refreshAgentContributions();
  assert.deepEqual(
    [$('set-pi-hooks').checked, $('set-pi-hooks').indeterminate, $('set-pi-hooks').disabled],
    [false, true, true],
  );
  assert.deepEqual(
    [$('set-pi-lifecycle').checked, $('set-pi-lifecycle').indeterminate, $('set-pi-lifecycle').disabled],
    [true, false, false],
  );

  pi = { signal: true, lifecycle: false, installation: 'configured' };
  await refreshAgentContributions();
  assert.equal($('set-pi-hooks').checked, false);
  assert.equal($('set-pi-hooks').indeterminate, true);
  assert.equal($('set-pi-hooks').disabled, true);
});

test('an unusable Pi installation refuses handlers before confirmation or write', async () => {
  pi = { signal: true, lifecycle: false, installation: 'unsupported-location' };
  await refreshAgentContributions();
  const beforeWrites = writes.length, beforeConfirmations = confirmations;
  $('set-pi-lifecycle').checked = true;
  await $('set-pi-lifecycle').onchange();
  assert.equal(writes.length, beforeWrites);
  assert.equal(confirmations, beforeConfirmations);
  assert.equal($('set-pi-lifecycle').disabled, true);
  pi = { signal: false, lifecycle: false, installation: 'absent' };
  await refreshAgentContributions();
});

test('a control becoming unusable during confirmation cannot write', async () => {
  pi = { signal: false, lifecycle: false, installation: 'absent' };
  await refreshAgentContributions();
  const answer = deferred(), beforeWrites = writes.length;
  confirmBehavior = () => answer.promise;
  $('set-pi-hooks').checked = true;
  const changing = $('set-pi-hooks').onchange();
  await new Promise(resolve => setImmediate(resolve));
  pi = { signal: false, lifecycle: false, installation: 'unsupported-location' };
  await refreshAgentContributions();
  answer.resolve(true);
  await changing;
  assert.equal(writes.length, beforeWrites);
  assert.equal($('set-pi-hooks').disabled, true);
  confirmBehavior = async () => true;
  pi = { signal: false, lifecycle: false, installation: 'absent' };
  await refreshAgentContributions();
});

test('Pi signal and lifecycle controls share the external-settings write lock', async () => {
  $('set-pi-hooks').checked = true;
  const signal = $('set-pi-hooks').onchange();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal($('set-pi-lifecycle').disabled, true);
  $('set-pi-lifecycle').checked = true;
  await $('set-pi-lifecycle').onchange();
  assert.deepEqual(writes, [['agent_hooks_set', { agent: 'pi', enable: true }]]);
  writeGate.resolve();
  await signal;
  assert.equal($('set-pi-hooks').checked, true);
  assert.equal($('set-pi-lifecycle').checked, false);
});


test('mount is once-only and feature controllers stay inert until an explicit action', () => {
  assert.deepEqual(initialCalls, [], 'construction and mounting perform no native reads or activation');
  const count = mounted.length;
  const beforeWrites = writes.length;
  initializeExtensionContributions();
  assert.equal(mounted.length, count);
  assert.equal(writes.length, beforeWrites);
  assert.equal(extensionControlsBusy(), false);
  assert.throws(() => extensionController('unknown'), /unavailable/);
  assert.throws(() => runExtensionLifecycle('enable'), /Unknown Settings lifecycle/);
});

test('lifecycle reaches subsequent controllers after synchronous and asynchronous failures', async () => {
  const ids = ['slack', 'bergamot', 'voice'];
  const originals = ids.map(id => extensionController(id).close);
  const reached = [];
  try {
    for (const id of ids) extensionController(id).close = () => {
      reached.push(id);
      if (id === 'slack') throw new Error('close failed');
      if (id === 'bergamot') return Promise.reject(new Error('close failed'));
    };
    const results = await runExtensionLifecycle('close');
    assert.deepEqual(reached, ids);
    assert.deepEqual(results.map(result => result.status), ['rejected', 'rejected', 'fulfilled', 'fulfilled', 'fulfilled']);
  } finally {
    ids.forEach((id, index) => { extensionController(id).close = originals[index]; });
  }
});
