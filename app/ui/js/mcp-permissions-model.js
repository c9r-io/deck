// mcp-permissions-model.js — the DOM-free half of the MCP client's permissions
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - One dialog decides every choice of a client at once (extension-mcp.js):
//   create sessions, trigger Agent automations, drive worker cards (with
//   their types, the first-send choice and shared replies), draft templates
//   and Agent rules for the user to approve (proposals.js). This module
//   owns the dependencies and the shape the backend takes
//   (`mcp_client_add`, `mcp_client_permissions`, mcp/workers.rs): a child
//   choice its parent does not allow is sent as off, so the dialog can never
//   save a state the backend would refuse for that reason.
// - Worker types mirror the backend's rules (`validate_worker_types`): at
//   most 8, lowercase names unique, a `claude` or `codex` command with
//   simple arguments, `--no-daemon` for Codex, a column, a one-line
//   description. The backend decides; these checks only say what is wrong
//   before a round trip.
// - Painting never moves a type's column (`typeColumnChoice`): a column of
//   another project stays chosen, named with its project; only a column no
//   project has any more falls back to the shown project's first.
// - The client row copies a ready setup: `claude mcp add --scope user` for
//   Claude Code (every directory, since an orchestrator may start anywhere)
//   or Codex's TOML (`claudeAddCommand`, `codexConfig`).
// - The scope rows are checked here before any directory is (`scopeProblem`),
//   and a directory preview's refusal names its message (`previewErrorKey`).
// - The default list offers a Claude type (`--permission-mode auto`) and a
//   Codex type (`--no-daemon --yolo`) in the project's first column, so
//   driving worker cards needs no further configuration.

export const MAX_WORKER_TYPES = 8;
const TYPE_NAME = /^[a-z0-9_-]{1,32}$/;
const COMMAND_WORD = /^[A-Za-z0-9\-_./:=+,@]+$/;

/* a client's choices as the dialog starts: the client's own (editing) or
   none (a new client) */
export function initialPermissions(client) {
  return {
    allowCreate: client?.allowCreate === true,
    allowAutomations: client?.allowAutomations === true,
    allowWorkers: client?.allowWorkers === true,
    allowReplies: client?.allowReplies === true,
    workerFirstSend: client?.workerFirstSend === true,
    allowDrafts: client?.allowDrafts === true,
    allowMentions: client?.allowMentions === true,
    ruleColumn: client?.ruleColumn === true,
    workerTypes: Array.isArray(client?.workerTypes) ? client.workerTypes.map(t => ({ ...t })) : [],
  };
}

/* the types a newly enabled worker choice starts with: Claude in auto mode
   and Codex (`--no-daemon`, which Deck's Signal needs) without approvals,
   both in the project's first column; `descriptions` by type name */
export function defaultWorkerTypes(columns, descriptions) {
  const columnId = columns?.[0]?.id || '';
  return [
    { name: 'claude', cmd: 'claude --permission-mode auto', columnId, description: descriptions?.claude || '' },
    { name: 'codex', cmd: 'codex --no-daemon --yolo', columnId, description: descriptions?.codex || '' },
  ];
}

/* the column choices of a type's row: the shown project's columns, plus
   the type's own column when it belongs to another project (kept, named
   "project › column"); a column no project has any more, or none, becomes
   the first shown one. Painting never moves a type's column on its own. */
export function typeColumnChoice(columns, projects, columnId) {
  const options = (columns || []).map(c => ({ id: c.id, name: c.name }));
  if (columnId && !options.some(o => o.id === columnId)) {
    for (const project of projects || []) {
      const column = (project.columns || []).find(c => c.id === columnId);
      if (column) {
        options.push({ id: column.id, name: `${project.name} › ${column.name}` });
        return { options, value: columnId };
      }
    }
  }
  if (options.some(o => o.id === columnId)) return { options, value: columnId };
  return { options, value: options[0]?.id || '' };
}

/* the choices as the backend takes them: children of an unchecked parent off */
export function permissionsPayload(p) {
  const workers = p.allowWorkers === true;
  return {
    allowCreate: p.allowCreate === true,
    allowAutomations: p.allowAutomations === true,
    allowWorkers: workers,
    allowReplies: workers && p.allowReplies === true,
    workerFirstSend: workers && p.workerFirstSend === true,
    allowDrafts: p.allowDrafts === true,
    allowMentions: p.allowMentions === true,
    ruleColumn: p.ruleColumn === true,
    workerTypes: workers ? p.workerTypes.map(t => ({
      name: String(t.name || '').trim(),
      cmd: String(t.cmd || '').trim().replace(/\s+/g, ' '),
      columnId: String(t.columnId || ''),
      description: String(t.description || '').trim(),
    })) : [],
  };
}

/* the first thing wrong with the worker types, as an i18n key, or null */
export function workerTypesProblem(p) {
  const payload = permissionsPayload(p);
  if (!payload.allowWorkers) return null;
  const types = payload.workerTypes;
  if (!types.length) return 'mcp.workersNeedTypes';
  if (types.length > MAX_WORKER_TYPES) return 'mcp.typeListInvalid';
  const names = new Set();
  for (const type of types) {
    if (!TYPE_NAME.test(type.name) || names.has(type.name)) return 'mcp.typeNameInvalid';
    names.add(type.name);
    const words = type.cmd.split(' ');
    if (!['claude', 'codex'].includes(words[0]) || !words.slice(1).every(word => COMMAND_WORD.test(word))) {
      return 'mcp.typeCommandInvalid';
    }
    if (words[0] === 'codex' && !words.includes('--no-daemon')) return 'mcp.typeCodexDaemon';
    if (!type.columnId || [...type.description].length > 200 || /\p{Cc}/u.test(type.description)) {
      return 'mcp.typeListInvalid';
    }
  }
  return null;
}

/* the backend's closed refusal word for a type list, as an i18n key */
export function workerTypeErrorKey(error) {
  const text = String(error);
  if (text.includes('mcp-worker-type-name')) return 'mcp.typeNameInvalid';
  if (text.includes('mcp-worker-type-command')) return 'mcp.typeCommandInvalid';
  if (text.includes('mcp-worker-type-codex-daemon')) return 'mcp.typeCodexDaemon';
  if (text.includes('mcp-worker-type-list')) return 'mcp.typeListInvalid';
  return null;
}

/* the client row's one-line summary of what it may do, as i18n keys */
export function permissionSummary(client) {
  if (!client || client.revoked) return [];
  const keys = [];
  if (client.allowCreate) keys.push('mcp.summaryCreate');
  if (client.allowAutomations) keys.push('mcp.automationsOn');
  if (client.allowWorkers) keys.push(client.allowReplies ? 'mcp.workersOn' : 'mcp.workersNoReplies');
  if (client.allowDrafts) keys.push('mcp.summaryDrafts');
  if (client.allowMentions) keys.push('mcp.summaryMentions');
  return keys;
}

/* a client's project scope as the dialog starts: all projects, or its rows
   (project id and its first directory); a new client starts with one empty
   row */
export function initialScope(client) {
  const rows = (client?.projects || []).map(p => ({ projectId: String(p.projectId || ''), root: String(p.roots?.[0] || '') }));
  return { all: client?.allProjects === true, rows: rows.length ? rows : [{ projectId: '', root: '' }] };
}

/* the scope as the backend takes it beside the permissions: all projects,
   a list of projects, or (an edit that kept it) nothing */
export function scopePayload({ allProjects, scopes }) {
  if (allProjects === true) return { allProjects: true };
  if (!Array.isArray(scopes)) return { allProjects: false };
  return { allProjects: false, projects: scopes.map(s => ({ projectId: s.project.id, roots: [s.root] })) };
}

/* one shell word: as is when plain, else single-quoted */
function shellWord(value) {
  const text = String(value);
  return /^[A-Za-z0-9_./:=+,@-]+$/.test(text) ? text : `'${text.replace(/'/g, `'\\''`)}'`;
}

/* the Claude Code command that registers this client for every directory
   (`--scope user`): the default scope is the current directory only, and an
   orchestrator may be started anywhere */
export function claudeAddCommand(adapterPath, clientId) {
  return `claude mcp add --scope user deck -- ${shellWord(adapterPath)} --client-id ${shellWord(clientId)}`;
}

/* the Codex configuration (Codex reads one global file) */
export function codexConfig(adapterPath, clientId) {
  return `[mcp_servers.deck]\ncommand = ${JSON.stringify(adapterPath)}\nargs = ["--client-id", ${JSON.stringify(clientId)}]\ndefault_tools_approval_mode = "writes"\n`;
}

/* the first thing wrong with the scope rows before any directory is
   checked, or null: a row without a known project, a row without a
   directory, then a project named twice. `index` is the row (null for a
   duplicate), `field` the input to focus. */
export function scopeProblem(rows) {
  for (const [index, row] of rows.entries()) {
    if (!row.known) return { index, field: 'project', key: 'mcp.projectRequired' };
    if (!row.root) return { index, field: 'root', key: 'mcp.rootRequired' };
  }
  const ids = rows.map(row => row.projectId);
  if (new Set(ids).size !== ids.length) return { index: null, field: null, key: 'mcp.projectDuplicate' };
  return null;
}

/* a directory preview's refusal word as an i18n key */
export function previewErrorKey(error) {
  return ({ not_found: 'mcp.rootNotFound', not_directory: 'mcp.rootNotDirectory', not_accessible: 'mcp.rootNotAccessible',
    too_broad: 'mcp.rootTooBroad' })[error] || 'mcp.rootUnavailable';
}
