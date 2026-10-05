import test from 'node:test';
import assert from 'node:assert/strict';
import { connectorBufferOperationId, connectorFirstSendClaim, connectorId, connectorRunPlan, newlyPairedDevice, normalizeTaskPreset, normalizeTaskPresets, unfinishedConnectorPlans } from '../js/connector-model.js';

test('a pairing is identified as the one new active device', () => {
  const before = [{ id: 'D1', revoked: false }, { id: 'D2', revoked: true }];
  assert.equal(newlyPairedDevice(before, before), null);
  assert.deepEqual(newlyPairedDevice(before, [...before, { id: 'D3', name: 'phone', revoked: false }]),
    { id: 'D3', name: 'phone', revoked: false });
  assert.equal(newlyPairedDevice(before, [{ id: 'D1', revoked: true }]), null);
  assert.equal(newlyPairedDevice(null, null), null);
});

const columns = [{ id: 'C1' }];
const preset = { id: 'R1', name: 'Fix issue', columnId: 'C1', title: 'Remote task', dir: '~/work', cmd: 'codex', steps: ['inspect', 'fix'] };

test('task presets retain only bounded desktop-owned Codex or Claude launch plans', () => {
  assert.deepEqual(normalizeTaskPreset(preset, columns), preset);
  assert.equal(normalizeTaskPreset({ ...preset, cmd: 'bash' }, columns), null);
  assert.equal(normalizeTaskPreset({ ...preset, cmd: 'codex --full-auto' }, columns)?.cmd, 'codex --full-auto');
  assert.equal(normalizeTaskPreset({ ...preset, cmd: 'codex;zsh' }, columns), null);
  assert.equal(normalizeTaskPreset({ ...preset, cmd: '/opt/bin/claude' }, columns), null);
  assert.equal(normalizeTaskPreset({ ...preset, columnId: 'missing' }, columns), null);
  assert.deepEqual(normalizeTaskPresets([preset, preset], columns), [preset]);
});

test('connector card and step identities are deterministic from the opaque journal handle', async () => {
  assert.equal(await connectorId('S', 'handle', 'card'), await connectorId('S', 'handle', 'card'));
  assert.notEqual(await connectorId('B', 'handle', 'step/0'), await connectorId('B', 'handle', 'step/1'));
  const expected = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('connector-buffer:handle:N1'));
  assert.equal(await connectorBufferOperationId('handle', 'N1'), 'B' + Buffer.from(expected).toString('hex').slice(0, 32));
  const card = { connectorRun: { initialQueued: false } };
  assert.deepEqual(unfinishedConnectorPlans([card, { connectorRun: { initialQueued: true } }]), [card]);
});

test('a preset keeps the first-send choice only as an explicit true', () => {
  assert.deepEqual(normalizeTaskPreset({ ...preset, firstSend: true }, columns), { ...preset, firstSend: true });
  for (const off of [false, undefined, null, 'true', 1]) {
    assert.equal('firstSend' in normalizeTaskPreset({ ...preset, firstSend: off }, columns), false, String(off));
  }
});

test('only a run that froze the choice claims it, with its preset, project and command handle', () => {
  const run = { handle: 'a'.repeat(64), presetId: 'R1', initialQueued: false };
  assert.deepEqual(connectorFirstSendClaim({ projectId: 'P1', connectorRun: { ...run, firstSend: true } }),
    { firstSend: { rule: 'R1', event: 'a'.repeat(64), presetProject: 'P1' } });
  for (const card of [{ projectId: 'P1', connectorRun: run }, { projectId: 'P1', connectorRun: { ...run, firstSend: 'true' } },
    { projectId: 'P1' }, null]) assert.deepEqual(connectorFirstSendClaim(card), {});
});

test('a phone task run freezes its steps and, with a first step, the preset\'s first-send choice', () => {
  const plain = connectorRunPlan('h', preset, ['B0', 'B1'], 10);
  assert.deepEqual(plain, { handle: 'h', presetId: 'R1', initialQueued: false, initialSteps: [
    { operationId: 'B0', text: 'inspect', mode: 'at', at: 10, tpl: 'R1', tplIdx: 1, tplTotal: 2 },
    { operationId: 'B1', text: 'fix', mode: 'chain', at: null, tpl: 'R1', tplIdx: 2, tplTotal: 2 }] });
  assert.deepEqual(connectorRunPlan('h', { ...preset, firstSend: true }, ['B0', 'B1'], 10), { ...plain, firstSend: true });
  // no steps: nothing to send first, nothing frozen
  assert.deepEqual(connectorRunPlan('h', { ...preset, steps: [], firstSend: true }, [], 10),
    { handle: 'h', presetId: 'R1', initialSteps: [], initialQueued: true });
});
