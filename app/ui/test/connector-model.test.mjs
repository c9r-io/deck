import test from 'node:test';
import assert from 'node:assert/strict';
import { connectorBufferOperationId, connectorId, newlyPairedDevice, normalizeTaskPreset, normalizeTaskPresets, unfinishedConnectorPlans } from '../js/connector-model.js';

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
