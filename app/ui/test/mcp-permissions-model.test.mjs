// The MCP client's permissions (mcp-permissions-model.js): the dependencies
// the dialog enforces, the shape the backend takes, the worker type checks
// that mirror the backend's, and the row's summary.
import test from 'node:test';
import assert from 'node:assert/strict';
import { MAX_WORKER_TYPES, defaultWorkerTypes, initialPermissions, initialScope, permissionSummary, permissionsPayload, scopePayload, previewErrorKey, scopeProblem, typeColumnChoice, workerTypeErrorKey, workerTypesProblem } from '../js/mcp-permissions-model.js';

const claude = { name: 'claude', cmd: 'claude --permission-mode auto', columnId: 'C1', description: 'Write code' };

test('a new client starts with nothing allowed; an existing one with its own choices', () => {
  assert.deepEqual(initialPermissions(null), { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, allowDrafts: false, allowMentions: false, ruleColumn: false, workerTypes: [] });
  const client = { allowCreate: true, allowWorkers: true, allowReplies: true, workerTypes: [claude] };
  const copy = initialPermissions(client);
  assert.equal(copy.allowCreate, true);
  copy.workerTypes[0].name = 'changed';
  assert.equal(client.workerTypes[0].name, 'claude', 'the dialog edits a copy');
});

test('children of an unchecked parent are sent as off', () => {
  const payload = permissionsPayload({ allowWorkers: false, allowReplies: true, workerFirstSend: true, workerTypes: [claude] });
  assert.deepEqual(payload, { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, allowDrafts: false, allowMentions: false, ruleColumn: false, workerTypes: [] });
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
  assert.deepEqual(defaultWorkerTypes([{ id: 'C9' }, { id: 'C2' }], { claude: 'a', codex: 'b' }), [
    { name: 'claude', cmd: 'claude --permission-mode auto', columnId: 'C9', description: 'a' },
    { name: 'codex', cmd: 'codex --no-daemon --yolo', columnId: 'C9', description: 'b' },
  ]);
  assert.equal(defaultWorkerTypes([], {})[0].columnId, '');
  // the defaults pass the dialog's own checks
  assert.equal(workerTypesProblem({ allowWorkers: true, workerTypes: defaultWorkerTypes([{ id: 'C1' }], { claude: 'a', codex: 'b' }) }), null);
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
  // drafting stands alone: it needs no other choice
  assert.deepEqual(permissionSummary({ allowDrafts: true }), ['mcp.summaryDrafts']);
  assert.equal(permissionsPayload({ allowDrafts: true, workerTypes: [] }).allowDrafts, true);
  assert.equal(initialPermissions({ allowDrafts: true }).allowDrafts, true);
  assert.deepEqual(permissionSummary({ allowMentions: true }), ['mcp.summaryMentions']);
  assert.equal(permissionsPayload({ allowMentions: true, workerTypes: [] }).allowMentions, true);
  assert.equal(initialPermissions({ allowMentions: true }).allowMentions, true);
  assert.deepEqual(permissionSummary({ revoked: true, allowCreate: true }), []);
  assert.deepEqual(permissionSummary(null), []);
});

test('the type’s-column choice round-trips; absent means beside the orchestrator card', () => {
  assert.equal(initialPermissions({ ruleColumn: true }).ruleColumn, true);
  assert.equal(initialPermissions({}).ruleColumn, false);
  assert.equal(permissionsPayload({ ruleColumn: true, workerTypes: [] }).ruleColumn, true);
});

test('a scope round-trips: all projects, a list, or nothing when kept', () => {
  assert.deepEqual(initialScope(null), { all: false, rows: [{ projectId: '', root: '' }] });
  assert.deepEqual(initialScope({ allProjects: true, projects: [] }).all, true);
  assert.deepEqual(initialScope({ projects: [{ projectId: 'P1', roots: ['/a', '/b'] }] }).rows, [{ projectId: 'P1', root: '/a' }]);
  assert.deepEqual(scopePayload({ allProjects: true, scopes: [] }), { allProjects: true });
  assert.deepEqual(scopePayload({ allProjects: false, scopes: null }), { allProjects: false });
  assert.deepEqual(scopePayload({ allProjects: false, scopes: [{ project: { id: 'P1' }, root: '/a' }] }),
    { allProjects: false, projects: [{ projectId: 'P1', roots: ['/a'] }] });
});

test('a type keeps its column when the shown project lacks it', () => {
  const projects = [
    { id: 'p1', name: 'One', columns: [{ id: 'a', name: 'Todo' }] },
    { id: 'p2', name: 'Two', columns: [{ id: 'b', name: 'Doing' }] },
  ];
  const kept = typeColumnChoice(projects[0].columns, projects, 'b');
  assert.equal(kept.value, 'b');
  assert.deepEqual(kept.options.map(o => o.name), ['Todo', 'Two › Doing']);
  assert.equal(typeColumnChoice(projects[0].columns, projects, 'a').value, 'a');
  // a column no project has any more, or none, becomes the first shown one
  assert.equal(typeColumnChoice(projects[0].columns, projects, 'gone').value, 'a');
  assert.equal(typeColumnChoice(projects[0].columns, projects, '').value, 'a');
  assert.equal(typeColumnChoice([], projects, '').value, '');
});

test('scope rows are checked in order before any directory', () => {
  assert.equal(scopeProblem([{ known: true, projectId: 'p1', root: '/a' }]), null);
  assert.deepEqual(scopeProblem([{ known: false, root: '/a' }]), { index: 0, field: 'project', key: 'mcp.projectRequired' });
  assert.deepEqual(scopeProblem([{ known: true, projectId: 'p1', root: '' }]), { index: 0, field: 'root', key: 'mcp.rootRequired' });
  assert.deepEqual(scopeProblem([{ known: true, projectId: 'p1', root: '/a' }, { known: true, projectId: 'p2', root: '' }]),
    { index: 1, field: 'root', key: 'mcp.rootRequired' });
  assert.deepEqual(scopeProblem([{ known: true, projectId: 'p1', root: '/a' }, { known: true, projectId: 'p1', root: '/b' }]),
    { index: null, field: null, key: 'mcp.projectDuplicate' });
});

test('a preview refusal names its own message', () => {
  assert.equal(previewErrorKey('too_broad'), 'mcp.rootTooBroad');
  assert.equal(previewErrorKey('not_found'), 'mcp.rootNotFound');
  assert.equal(previewErrorKey('unavailable'), 'mcp.rootUnavailable');
  assert.equal(previewErrorKey(undefined), 'mcp.rootUnavailable');
});
