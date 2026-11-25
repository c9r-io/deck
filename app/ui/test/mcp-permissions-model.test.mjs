// The MCP client's permissions (mcp-permissions-model.js): the dependencies
// the dialog enforces, the shape the backend takes, the worker type checks
// that mirror the backend's, and the row's summary.
import test from 'node:test';
import assert from 'node:assert/strict';
import { MAX_TYPE_MODELS, MAX_WORKER_TYPES, claudeAddCommand, modelLines, codexConfig, defaultWorkerTypes, initialPermissions, initialScope, permissionSummary, permissionsPayload, scopePayload, previewErrorKey, scopeProblem, workerTypeErrorKey, workerTypesProblem } from '../js/mcp-permissions-model.js';

const claude = { name: 'claude', cmd: 'claude --permission-mode auto', description: 'Write code' };

test('a new client starts with nothing allowed; an existing one with its own choices', () => {
  assert.deepEqual(initialPermissions(null), { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, allowDrafts: false, allowMentions: false, allowCards: false, allowCardEdits: false, ruleColumn: false, workerTypes: [] });
  const client = { allowCreate: true, allowWorkers: true, allowReplies: true, workerTypes: [claude] };
  const copy = initialPermissions(client);
  assert.equal(copy.allowCreate, true);
  copy.workerTypes[0].name = 'changed';
  assert.equal(client.workerTypes[0].name, 'claude', 'the dialog edits a copy');
});

test('children of an unchecked parent are sent as off', () => {
  const payload = permissionsPayload({ allowWorkers: false, allowReplies: true, workerFirstSend: true, workerTypes: [claude] });
  assert.deepEqual(payload, { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false, allowDrafts: false, allowMentions: false, allowCards: false, allowCardEdits: false, ruleColumn: false, workerTypes: [] });
  const on = permissionsPayload({ allowWorkers: true, allowReplies: true, workerFirstSend: true,
    workerTypes: [{ name: ' claude ', cmd: 'claude   --permission-mode  auto', columnId: 'C1', description: ' Write code ' }] });
  assert.deepEqual(on.workerTypes, [claude], 'trimmed, single spaces, a column an older build gave the type dropped');
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
  // a Pi worker: the command passes here; the backend says when Pi's work status is off
  assert.equal(problem([{ ...claude, name: 'terra', cmd: 'pi --model gpt-5.6-terra' }]), null);
  assert.equal(problem([{ ...claude, name: 'terra', cmd: 'pie' }]), 'mcp.typeCommandInvalid');
  assert.equal(problem([{ ...claude, name: 'codex', cmd: 'codex --no-daemon' }]), null);
  assert.equal(problem([{ ...claude, columnId: '' }]), null, 'a type has no column');
  assert.equal(problem([{ ...claude, description: 'x'.repeat(201) }]), 'mcp.typeListInvalid');
  assert.equal(problem(Array.from({ length: MAX_WORKER_TYPES + 1 }, (_, i) => ({ ...claude, name: `t${i}` }))), 'mcp.typeListInvalid');
});

test('a type may list models: one per line, only on a command that names none', () => {
  const on = types => ({ allowWorkers: true, workerTypes: types });
  const problem = types => workerTypesProblem(on(types));
  const type = (cmd, models) => ({ name: 'a', cmd, description: '', models });
  assert.deepEqual(modelLines(' sonnet,\topus ,, haiku\t'), ['sonnet', 'opus', 'haiku'], 'commas; spaces and tabs trimmed');
  assert.deepEqual(modelLines('sonnet，opus'), ['sonnet', 'opus'], 'a full-width comma separates too');
  assert.deepEqual(modelLines(' sonnet \n\nopus\n'), ['sonnet', 'opus'], 'line breaks still separate (a pasted list)');
  assert.deepEqual(modelLines(['x', ' ']), ['x']);
  // the payload carries the trimmed lines, and no key when there are none
  assert.deepEqual(permissionsPayload(on([type('claude', 'sonnet\n opus ')])).workerTypes[0].models, ['sonnet', 'opus']);
  assert.equal('models' in permissionsPayload(on([type('claude', '\n')])).workerTypes[0], false);
  assert.equal(problem([type('claude --permission-mode auto', 'sonnet\nopus')]), null);
  assert.equal(problem([type('pi', 'openai/gpt-5.6-terra\nminimax/MiniMax-M3\nanthropic/claude-opus-5-5:high')]), null);
  assert.equal(problem([type('codex --no-daemon --yolo', ['gpt-5.6'])]), null);
  // a command that names its model keeps working without a list...
  assert.equal(problem([type('claude --model sonnet --permission-mode auto', '')]), null);
  assert.equal(problem([type('codex --no-daemon --yolo -m gpt', '')]), null);
  // ...and not with one
  for (const cmd of ['claude --model sonnet', 'claude --model=x', 'codex --no-daemon -m x', 'codex --no-daemon -mx',
    'codex --no-daemon -c model=x', 'pi --models a,b', 'pi --provider openai', 'pi --provider=openai']) {
    assert.equal(problem([type(cmd, 'x')]), 'mcp.typeModelInCommand', cmd);
  }
  assert.equal(problem([type('claude --fallback-model haiku', 'opus')]), null);
  for (const bad of ['--yolo', 'a b', '$(x)', 'x\nx', 'm'.repeat(101)]) {
    assert.equal(problem([type('claude', bad)]), 'mcp.typeModelsInvalid', bad);
  }
  const many = Array.from({ length: MAX_TYPE_MODELS + 1 }, (_, i) => `m${i}`);
  assert.equal(problem([type('claude', many)]), 'mcp.typeModelsInvalid');
  assert.equal(problem([type('claude', many.slice(1))]), null);
  assert.equal(workerTypeErrorKey('mcp-worker-type-models'), 'mcp.typeModelsInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-model-in-command'), 'mcp.typeModelInCommand');
});

test('the default types are Claude and Codex, without a column; refusals map to one sentence each', () => {
  assert.deepEqual(defaultWorkerTypes({ claude: 'a', codex: 'b' }), [
    { name: 'claude', cmd: 'claude --permission-mode auto', description: 'a' },
    { name: 'codex', cmd: 'codex --no-daemon --yolo', description: 'b' },
  ]);
  assert.equal(defaultWorkerTypes()[0].description, '');
  // the defaults pass the dialog's own checks
  assert.equal(workerTypesProblem({ allowWorkers: true, workerTypes: defaultWorkerTypes({ claude: 'a', codex: 'b' }) }), null);
  assert.equal(workerTypeErrorKey('mcp-worker-type-codex-daemon'), 'mcp.typeCodexDaemon');
  assert.equal(workerTypeErrorKey('mcp-worker-type-name'), 'mcp.typeNameInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-command'), 'mcp.typeCommandInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-pi-observer'), 'mcp.typePiObserver');
  assert.equal(workerTypeErrorKey('mcp-worker-type-list'), 'mcp.typeListInvalid');
  assert.equal(workerTypeErrorKey('mcp-worker-type-config-root'), 'mcp.typeConfigRoot');
  assert.equal(workerTypeErrorKey('mcp-worker-type-config-hooks'), 'mcp.typeConfigHooks');
  assert.equal(workerTypeErrorKey('other'), null);
});

test('a Claude worker type may name its own config folder (a second account), never an alias', () => {
  const problem = cmd => workerTypesProblem({ allowWorkers: true, workerTypes: [{ name: 'work', cmd, description: 'x' }] });
  const settings = '--settings /Users/u/.claude/settings.json';
  assert.equal(problem(`CLAUDE_CONFIG_DIR=/Users/u/.claude-work claude ${settings}`), null);
  assert.equal(problem('CLAUDE_CONFIG_DIR=/Users/u/.claude claude'), null, 'the default folder has Deck’s hooks');
  assert.equal(problem('CLAUDE_CONFIG_DIR=/Users/u/.claude-work claude'), 'mcp.typeConfigHooks');
  for (const cmd of [
    'claude-work',
    `CLAUDE_CONFIG_DIR=~/.claude-work claude ${settings}`,
    `CLAUDE_CONFIG_DIR=$HOME/.claude-work claude ${settings}`,
    `CLAUDE_CONFIG_DIR=/a/../b claude ${settings}`,
    `CLAUDE_CONFIG_DIR=/a//b claude ${settings}`,
    'CLAUDE_CONFIG_DIR=/a codex --no-daemon',
    'CLAUDE_CONFIG_DIR=/a',
    'ANTHROPIC_API_KEY=x claude',
  ]) {
    assert.equal(problem(cmd), 'mcp.typeCommandInvalid', cmd);
  }
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
  assert.deepEqual(permissionSummary({ allowCards: true }), ['mcp.summaryCards']);
  assert.equal(permissionsPayload({ allowCards: true, workerTypes: [] }).allowCards, true);
  assert.equal(initialPermissions({ allowCards: true }).allowCards, true);
  // organizing cards needs the snapshot: without it the choice is off everywhere
  assert.deepEqual(permissionSummary({ allowCards: true, allowCardEdits: true }), ['mcp.summaryCards', 'mcp.summaryCardEdits']);
  assert.deepEqual(permissionSummary({ allowCardEdits: true }), []);
  assert.equal(permissionsPayload({ allowCards: true, allowCardEdits: true, workerTypes: [] }).allowCardEdits, true);
  assert.equal(permissionsPayload({ allowCardEdits: true, workerTypes: [] }).allowCardEdits, false);
  assert.equal(initialPermissions({ allowCardEdits: true }).allowCardEdits, false);
  assert.deepEqual(permissionSummary({ revoked: true, allowCreate: true }), []);
  assert.deepEqual(permissionSummary(null), []);
});

test('the not-beside choice round-trips; absent means beside the orchestrator card', () => {
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

test('the Claude Code command registers Deck for every directory', () => {
  assert.equal(claudeAddCommand('/Applications/deck.app/Contents/MacOS/deck-mcp', 'client_ab12'),
    'claude mcp add --scope user deck -- /Applications/deck.app/Contents/MacOS/deck-mcp --client-id client_ab12');
  assert.equal(claudeAddCommand("/Apps/My Deck's.app/deck-mcp", 'c1'),
    "claude mcp add --scope user deck -- '/Apps/My Deck'\\''s.app/deck-mcp' --client-id c1");
  assert.equal(codexConfig('/A/deck-mcp', 'c1'),
    '[mcp_servers.deck]\ncommand = "/A/deck-mcp"\nargs = ["--client-id", "c1"]\ndefault_tools_approval_mode = "writes"\n');
});
