// The MCP client's permissions (mcp-permissions-model.js): the dependencies
// the dialog enforces, the shape the backend takes, the worker type checks
// that mirror the backend's, and the row's summary.
import test from 'node:test';
import assert from 'node:assert/strict';
import { MAX_WORKER_TYPES, defaultWorkerType, initialPermissions, permissionSummary, permissionsPayload, workerTypeErrorKey, workerTypesProblem } from '../js/mcp-permissions-model.js';

const claude = { name: 'claude', cmd: 'claude --permission-mode auto', columnId: 'C1', description: 'Write code' };

test('a new client starts with nothing allowed; an existing one with its own choices', () => {
  assert.deepEqual(initialPermissions(null), { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, workerTypes: [] });
  const client = { allowCreate: true, allowWorkers: true, allowReplies: true, workerTypes: [claude] };
  const copy = initialPermissions(client);
  assert.equal(copy.allowCreate, true);
  copy.workerTypes[0].name = 'changed';
  assert.equal(client.workerTypes[0].name, 'claude', 'the dialog edits a copy');
});

test('children of an unchecked parent are sent as off', () => {
  const payload = permissionsPayload({ allowWorkers: false, allowReplies: true, workerFirstSend: true, workerTypes: [claude] });
  assert.deepEqual(payload, { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, workerTypes: [] });
  const on = permissionsPayload({ allowWorkers: true, allowReplies: true, workerFirstSend: true,
    workerTypes: [{ name: ' claude ', cmd: 'claude   --permission-mode  auto', columnId: 'C1', description: ' Write code ' }] });
  assert.deepEqual(on.workerTypes, [claude], 'trimmed, single spaces');
  assert.equal(on.allowReplies && on.workerFirstSend, true);
});

test('worker types are checked as the backend checks them', () => {
  const problem = types => workerTypesProblem({ allowWorkers: true, workerTypes: types });
  assert.equal(workerTypesProblem({ allowWorkers: false, workerTypes: [] }), null);
  assert.equal(problem([]), 'mcp.workersNeedTypes');
  assert.equal(problem([claude]), null);
  assert.equal(problem([{ ...claude, name: 'Claude' }]), 'mcp.typeNameInvalid');
  assert.equal(problem([claude, claude]), 'mcp.typeNameInvalid');
  assert.equal(problem([{ ...claude, cmd: 'bash' }]), 'mcp.typeCommandInvalid');
  assert.equal(problem([{ ...claude, cmd: 'claude; rm' }]), 'mcp.typeCommandInvalid');
  assert.equal(problem([{ ...claude, name: 'codex', cmd: 'codex' }]), 'mcp.typeCodexDaemon');
  assert.equal(problem([{ ...claude, name: 'codex', cmd: 'codex --no-daemon' }]), null);
  assert.equal(problem([{ ...claude, columnId: '' }]), 'mcp.typeListInvalid');
  assert.equal(problem([{ ...claude, description: 'x'.repeat(201) }]), 'mcp.typeListInvalid');
  assert.equal(problem(Array.from({ length: MAX_WORKER_TYPES + 1 }, (_, i) => ({ ...claude, name: `t${i}` }))), 'mcp.typeListInvalid');
});

test('the default type is Claude in the first column; refusals map to one sentence each', () => {
  assert.deepEqual(defaultWorkerType([{ id: 'C9' }, { id: 'C2' }], 'd'), { name: 'claude', cmd: 'claude', columnId: 'C9', description: 'd' });
  assert.equal(defaultWorkerType([], 'd').columnId, '');
  assert.equal(workerTypeErrorKey('mcp-worker-type-codex-daemon'), 'mcp.typeCodexDaemon');
  assert.equal(workerTypeErrorKey('mcp-worker-type-name'), 'mcp.typeNameInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-command'), 'mcp.typeCommandInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-list'), 'mcp.typeListInvalid');
  assert.equal(workerTypeErrorKey('other'), null);
});

test('the row says what the client may do, nothing once revoked', () => {
  assert.deepEqual(permissionSummary({ allowCreate: true, allowAutomations: true, allowWorkers: true, allowReplies: false }),
    ['mcp.summaryCreate', 'mcp.automationsOn', 'mcp.workersNoReplies']);
  assert.deepEqual(permissionSummary({ allowWorkers: true, allowReplies: true }), ['mcp.workersOn']);
  assert.deepEqual(permissionSummary({ revoked: true, allowCreate: true }), []);
  assert.deepEqual(permissionSummary(null), []);
});
