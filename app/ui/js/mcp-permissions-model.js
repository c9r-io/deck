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
// - The default list offers one Claude type in the project's first column,
//   so driving worker cards needs no further configuration.

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
    workerTypes: Array.isArray(client?.workerTypes) ? client.workerTypes.map(t => ({ ...t })) : [],
  };
}

/* the type a newly enabled worker choice starts with */
export function defaultWorkerType(columns, description) {
  return { name: 'claude', cmd: 'claude', columnId: columns?.[0]?.id || '', description };
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
  return keys;
}
