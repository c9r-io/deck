import test from 'node:test';
import assert from 'node:assert/strict';
import { createMcpSessionUiGate, resetMcpSessionControls } from '../js/mcp-session-ui.js';

const button = () => ({ hidden: false, disabled: false, dataset: { human: 'true', active: 'true' }, textContent: 'old', title: 'old' });

test('MCP controls reset before every request and fail closed for unmanaged, stale, and error', () => {
  const gate = createMcpSessionUiGate();
  const control = button(), grant = button();
  for (const status of [{ managed: false }, { managed: true, stale: true }, null]) {
    const epoch = gate.begin();
    resetMcpSessionControls(control, grant);
    assert.equal(gate.accept(epoch, 'A', 'A', status), false);
    assert.equal(gate.mayAct('A'), false);
    for (const target of [control, grant]) {
      assert.equal(target.hidden, true);
      assert.equal(target.disabled, true);
      assert.deepEqual(target.dataset, {});
      assert.equal(target.textContent, '');
      assert.equal(target.title, '');
    }
  }
});

test('MCP gate accepts current managed states and rejects old card and same-card replies', () => {
  const gate = createMcpSessionUiGate();
  const first = gate.begin();
  const current = gate.begin();
  assert.equal(gate.accept(first, 'A', 'A', { managed: true }), false);
  assert.equal(gate.accept(current, 'A', 'B', { managed: true }), false);
  assert.equal(gate.mayAct('A'), false);
  assert.equal(gate.accept(current, 'A', 'A', { managed: true, activeJob: false }), true);
  assert.equal(gate.mayAct('A'), true);
  assert.equal(gate.mayAct('B'), false);
  const newer = gate.begin();
  assert.equal(gate.mayAct('A'), false);
  assert.equal(gate.accept(current, 'A', 'A', { managed: true }), false);
  assert.equal(gate.accept(newer, 'A', 'A', { managed: true, humanControl: true, activeJob: true }), true);
});
