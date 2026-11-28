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
//   simple arguments, `--no-daemon` for Codex, a one-line description. The
//   backend decides; these checks only say what is wrong before a round
//   trip. A type has no column: where its cards open is the start's
//   (mcp/workers.rs `worker_start`), and a `columnId` a type still carries
//   from an older build is dropped here (`permissionsPayload`).
// - A type may list the models a start may choose (comma-separated, each a
//   single simple argument not starting with `-`, at most 20, no repeats),
//   only on a command that names no model itself (`--model`, `-m…`,
//   `--models`, `--provider`, any `…model=…`): Deck then appends
//   `--model <the listed entry>` (mcp/workers.rs `start_command`).
// - The client row copies a ready setup: `claude mcp add --scope user` for
//   Claude Code (every directory, since an orchestrator may start anywhere)
//   or Codex's TOML (`claudeAddCommand`, `codexConfig`).
// - The scope rows are checked here before any directory is (`scopeProblem`),
//   and a directory preview's refusal names its message (`previewErrorKey`).
//   A project may have several directories: one row each, all shown and
//   all saved (`initialScope`, `scopePayload`); a directory the client
//   already holds that does not resolve now (an unmounted volume) is kept
//   as stored (`heldRoot`; the backend `validate_client_scopes`).
// - One switch, `autoWorktrees` (mcp/grants.rs `auto_worktree_scope`):
//   worker cards and managed sessions may also start in a registered linked
//   worktree of a reached project's repository, wherever it is. It adds
//   start directories only; reads stay on the directories above.
// - The default list offers a Claude type (`--permission-mode auto`) and a
//   Codex type (`--no-daemon --yolo`), so driving worker cards needs no
//   further configuration.

export const MAX_WORKER_TYPES = 8;
const TYPE_NAME = /^[a-z0-9_-]{1,32}$/;
const COMMAND_WORD = /^[A-Za-z0-9\-_./:=+,@]+$/;
/* a Claude worker under its own config folder (a second account): one literal
   absolute `CLAUDE_CONFIG_DIR=<folder>` before `claude` (twin of
   admission.rs `config_root`); whether the folder exists only the backend
   can say (`mcp-worker-type-config-root`) */
const CONFIG_ROOT = /^CLAUDE_CONFIG_DIR=(\/[A-Za-z0-9\-_.:+,@]+(?:\/[A-Za-z0-9\-_.:+,@]+)*)$/;

/* the command without its config folder, and the folder, or the command alone */
function configRoot(words) {
  const match = CONFIG_ROOT.exec(words[0] || '');
  if (!match) return { root: null, words };
  const root = match[1];
  if (root.length > 1024 || root.split('/').slice(1).some(s => s === '.' || s === '..')) return { root: null, words: [] };
  return { root, words: words.slice(1) };
}
export const MAX_TYPE_MODELS = 20;

/* a model list as typed (separated by commas, ASCII or full-width, or by
   line breaks) or as stored (an array): entries trimmed of spaces and tabs,
   empty ones dropped */
export function modelLines(value) {
  const lines = Array.isArray(value) ? value : String(value || '').split(/[,，\n]/);
  return lines.map(line => String(line).trim()).filter(Boolean);
}

/* whether a command word already chooses a model (mirrors `names_a_model`) */
function namesAModel(word) {
  return ['--model', '--models', '--provider'].includes(word)
    || word.startsWith('--provider=')
    || word.includes('model=')
    || (word.startsWith('-m') && !word.startsWith('--'));
}

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
    allowCards: client?.allowCards === true,
    allowCardEdits: client?.allowCards === true && client?.allowCardEdits === true,
    ruleColumn: client?.ruleColumn === true,
    autoWorktrees: client?.autoWorktrees === true,
    workerTypes: Array.isArray(client?.workerTypes) ? client.workerTypes.map(t => ({ ...t })) : [],
  };
}

/* the types a newly enabled worker choice starts with: Claude in auto mode
   and Codex (`--no-daemon`, which Deck's Signal needs) without approvals;
   `descriptions` by type name */
export function defaultWorkerTypes(descriptions) {
  return [
    { name: 'claude', cmd: 'claude --permission-mode auto', description: descriptions?.claude || '' },
    { name: 'codex', cmd: 'codex --no-daemon --yolo', description: descriptions?.codex || '' },
  ];
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
    allowCards: p.allowCards === true,
    allowCardEdits: p.allowCards === true && p.allowCardEdits === true,
    ruleColumn: p.ruleColumn === true,
    autoWorktrees: p.autoWorktrees === true,
    workerTypes: workers ? p.workerTypes.map(t => ({
      name: String(t.name || '').trim(),
      cmd: String(t.cmd || '').trim().replace(/\s+/g, ' '),
      description: String(t.description || '').trim(),
      ...(modelLines(t.models).length ? { models: modelLines(t.models) } : {}),
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
    const { root, words } = configRoot(type.cmd.split(' '));
    // a Pi worker's lines go through Deck's extension in Pi (agent_pi/handover.rs);
    // an agy worker's are typed under agy's first-interaction gate (agent_agy.rs)
    if (!['claude', 'codex', 'pi', 'agy'].includes(words[0]) || !words.slice(1).every(word => COMMAND_WORD.test(word))
      || (root !== null && words[0] !== 'claude')) {
      return 'mcp.typeCommandInvalid';
    }
    // Deck's hooks are in ~/.claude: another folder needs settings of its own
    if (root !== null && !root.endsWith('/.claude')
      && !words.some(word => word === '--settings' || word.startsWith('--settings='))) {
      return 'mcp.typeConfigHooks';
    }
    if (words[0] === 'codex' && !words.includes('--no-daemon')) return 'mcp.typeCodexDaemon';
    if ([...type.description].length > 200 || /\p{Cc}/u.test(type.description)) {
      return 'mcp.typeListInvalid';
    }
    const models = type.models || [];
    if (!models.length) continue;
    if (words.slice(1).some(namesAModel)) return 'mcp.typeModelInCommand';
    if (models.length > MAX_TYPE_MODELS || new Set(models).size !== models.length
      || !models.every(model => model.length <= 100 && !model.startsWith('-') && COMMAND_WORD.test(model))) {
      return 'mcp.typeModelsInvalid';
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
  if (text.includes('mcp-worker-type-pi-observer')) return 'mcp.typePiObserver';
  if (text.includes('mcp-worker-type-agy-signal')) return 'mcp.typeAgySignal';
  if (text.includes('mcp-worker-type-models')) return 'mcp.typeModelsInvalid';
  if (text.includes('mcp-worker-type-model-in-command')) return 'mcp.typeModelInCommand';
  if (text.includes('mcp-worker-type-config-root')) return 'mcp.typeConfigRoot';
  if (text.includes('mcp-worker-type-config-hooks')) return 'mcp.typeConfigHooks';
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
  if (client.allowCards) keys.push('mcp.summaryCards');
  if (client.allowCards && client.allowCardEdits) keys.push('mcp.summaryCardEdits');
  return keys;
}

/* a client's project scope as the dialog starts: all projects, or one row
   per directory of each project (a project may have several, approved
   2026-11-28; every one is shown, so saving keeps them all); a new client
   starts with one empty row */
export function initialScope(client) {
  const rows = (client?.projects || []).flatMap(p => {
    const roots = Array.isArray(p.roots) && p.roots.length ? p.roots : [''];
    return roots.map(root => ({ projectId: String(p.projectId || ''), root: String(root || '') }));
  });
  return { all: client?.allProjects === true, rows: rows.length ? rows : [{ projectId: '', root: '' }] };
}

/* whether a row's directory is one the client already holds for that
   project, exactly as stored: such a row is kept when its directory does
   not resolve now (an unmounted volume) instead of blocking the save */
export function heldRoot(client, projectId, root) {
  return (client?.projects || []).some(p => p.projectId === projectId && (p.roots || []).includes(root));
}

/* the scope as the backend takes it beside the permissions: all projects,
   a list of projects (each once, with all of its rows' directories), or
   (an edit that kept it) nothing */
export function scopePayload({ allProjects, scopes }) {
  if (allProjects === true) return { allProjects: true };
  if (!Array.isArray(scopes)) return { allProjects: false };
  const projects = [];
  for (const s of scopes) {
    const entry = projects.find(p => p.projectId === s.project.id);
    if (entry) { if (!entry.roots.includes(s.root)) entry.roots.push(s.root); }
    else projects.push({ projectId: s.project.id, roots: [s.root] });
  }
  return { allProjects: false, projects };
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
   directory, then one directory named twice for one project (a project may
   have several rows, one per directory). `index` is the row (null for a
   duplicate), `field` the input to focus. */
export function scopeProblem(rows) {
  for (const [index, row] of rows.entries()) {
    if (!row.known) return { index, field: 'project', key: 'mcp.projectRequired' };
    if (!row.root) return { index, field: 'root', key: 'mcp.rootRequired' };
  }
  const pairs = rows.map(row => `${row.projectId}\u0000${row.root}`);
  if (new Set(pairs).size !== pairs.length) return { index: null, field: null, key: 'mcp.rootDuplicate' };
  return null;
}

/* a directory preview's refusal word as an i18n key */
export function previewErrorKey(error) {
  return ({ not_found: 'mcp.rootNotFound', not_directory: 'mcp.rootNotDirectory', not_accessible: 'mcp.rootNotAccessible',
    too_broad: 'mcp.rootTooBroad' })[error] || 'mcp.rootUnavailable';
}

/* ---------- worker capacity (Settings › MCP) ---------- */

/* twin of mcp/workers.rs: DEFAULT_WORKER_CAPACITY, MAX_WORKER_CAPACITY and
   CLIENT_FACTOR */
export const DEFAULT_WORKER_CAPACITY = 16;
export const MAX_WORKER_CAPACITY = 32;
const CLIENT_FACTOR = 4;
/* above this many, the row warns about memory */
const WARN_ABOVE = 16;
/* a rough memory share of one agent's worker card, for the estimate only */
const GB_PER_WORKER = 0.5;

/* what the row says about a typed capacity: whether Deck takes it, the
   client's number that follows from it, and above WARN_ABOVE the memory
   estimate against this Mac's (`memoryBytes`, 0 when unknown) */
export function workerCapacityView(value, memoryBytes) {
  const text = String(value ?? '').trim();
  const count = /^\d+$/.test(text) ? Number(text) : NaN;
  const valid = Number.isInteger(count) && count >= 1 && count <= MAX_WORKER_CAPACITY;
  const memoryGb = memoryBytes > 0 ? Math.round(memoryBytes / 2 ** 30) : 0;
  return {
    valid,
    count: valid ? count : null,
    clientCount: valid ? count * CLIENT_FACTOR : null,
    warn: valid && count > WARN_ABOVE,
    estimateGb: valid ? Math.ceil(count * GB_PER_WORKER) : null,
    memoryGb,
  };
}
