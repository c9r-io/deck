// Production MCP Board bridge with a fake native ledger and the real Board
// persistence transaction.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null };
const { closeOutcome, drainMcp } = await import('../js/mcp.js');
const { listeners, store } = await import('../js/state.js');
const { panes } = await import('../js/board.js');
const { mcpErrorKey } = await import('../js/pure.js');
const { t } = await import('../js/i18n.js');
listeners.clear();

function project() {
  store.projects = [{ id: 'P1', name: 'Project', selected: 'C1', columns: [{ id: 'C1', name: 'Working' }] }];
  store.cards = [];
}

test('create starts the managed runner before persisting and commits the real card', async () => {
  project();
  const pending = { operationId: 'op_1', kind: 'session-create', result: {
    cardId: 'M1', sessionId: 'mcp_1', projectId: 'P1', title: 'MCP shell', cwd: '/tmp', generation: 'g_1',
  } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_start_session') return { created: true };
    if (cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 1);
  assert.equal(store.cards[0].origin.source, 'mcp');
  assert.equal(store.cards[0].cmd, '', 'project default commands never enter MCP sessions');
  assert.ok(calls.findIndex(([cmd]) => cmd === 'mcp_start_session') < calls.findIndex(([cmd]) => cmd === 'save_board'));
  const complete = calls.find(([cmd]) => cmd === 'mcp_complete')[1];
  assert.equal(complete.state, 'committed', JSON.stringify(calls));
  assert.equal(complete.tmuxSession, store.cards[0].session);
});

/* where an MCP-started card lands (mcp/workers.rs decides a worker's
   column or none): the column native named, else — none named, or gone
   meanwhile — the project's new-card column (pure.js newSessionColumn),
   never simply the first; a managed shell always the latter */
test('MCP-started cards open in the named column, else where the user’s new cards open', async () => {
  store.projects = [{ id: 'P1', name: 'Project', selected: null, columns: [
    { id: 'CA', name: 'Needs attention', semantic: 'attention' },
    { id: 'CW', name: 'In progress', semantic: 'working' },
    { id: 'CQ', name: 'Queued', semantic: 'queued' },
  ] }];
  store.cards = [];
  const worker = (id, columnId) => ({ operationId: `op_${id}`, kind: 'worker-start', result: {
    cardId: id, projectId: 'P1', columnId, cmd: 'claude', dir: '/tmp', title: id, type: 'claude',
  } });
  const shell = { operationId: 'op_M9', kind: 'session-create', result: {
    cardId: 'M9', sessionId: 'mcp_9', projectId: 'P1', title: 'MCP shell', cwd: '/tmp', generation: 'g_9',
  } };
  let items = [worker('W1', 'CQ'), worker('W2', null), worker('W3', 'gone'), shell];
  const all = [...items];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return all.find(item => item.operationId === args.operationId);
    if (cmd === 'start_session' || cmd === 'mcp_start_session') return { created: true };
    if (cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  const column = id => store.cards.find(card => card.id === id)?.columnId;
  assert.equal(column('W1'), 'CQ', 'the column native named');
  assert.equal(column('W2'), 'CW', 'none named: the working column, not the attention column first');
  assert.equal(column('W3'), 'CW', 'a column gone meanwhile: the same');
  assert.equal(column('M9'), 'CW', 'a managed shell: the same rule');
  store.projects[0].selected = 'CQ';
  store.cards = [];
  items = [worker('W4', null)]; all.push(...items);
  await drainMcp();
  assert.equal(column('W4'), 'CQ', 'the project’s selected new-card column first');
});

test('a Board save failure kills only the newly created session and records ambiguity', async () => {
  project();
  const pending = { operationId: 'op_2', kind: 'session-create', result: {
    cardId: 'M2', sessionId: 'mcp_2', projectId: 'P1', title: 'MCP shell', cwd: '/tmp', generation: 'g_2',
  } };
  let items = [pending]; let killed = 0; const states = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_start_session') return { created: true };
    if (cmd === 'save_board') throw new Error('disk full');
    if (cmd === 'kill_session') { killed++; return; }
    if (cmd === 'mcp_complete') { states.push(args.state); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(killed, 1);
  assert.equal(store.cards.length, 0);
  assert.deepEqual(states, ['ambiguous']);
});

test('close uses the ordinary Board close transaction and commits after persistence', async () => {
  project();
  store.cards = [{ id: 'M3', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M3', origin: { source: 'mcp' } }];
  const pending = { operationId: 'op_3', kind: 'session-close', result: { cardId: 'M3', sessionId: 'mcp_3' } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_close_admit') return { admission: 'close_admission' };
    if (cmd === 'queue_clear_sessions') return { removed: 0 };
    if (cmd === 'kill_session' || cmd === 'save_board' || cmd === 'mcp_complete') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 0, JSON.stringify(calls));
  const admitted = calls.findIndex(([cmd]) => cmd === 'mcp_close_admit');
  const cleared = calls.findIndex(([cmd]) => cmd === 'queue_clear_sessions');
  const killed = calls.findIndex(([cmd]) => cmd === 'kill_session');
  assert.ok(admitted < cleared && cleared < killed, JSON.stringify(calls));
  assert.equal(calls[cleared][1].mcpAdmission, 'close_admission');
  assert.equal(calls[killed][1].mcpAdmission, 'close_admission');
  assert.equal(calls.find(([cmd]) => cmd === 'mcp_complete')[1].state, 'committed');
});

test('a close that fails after admission is ambiguous, never rejected', async () => {
  project();
  store.cards = [{ id: 'M4', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M4', origin: { source: 'mcp' } }];
  const pending = { operationId: 'op_4', kind: 'session-close', result: { cardId: 'M4', sessionId: 'mcp_4' } };
  let items = [pending]; const completions = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_close_admit') return { admission: 'close_admission' };
    if (cmd === 'queue_clear_sessions') return { removed: 0 };
    if (cmd === 'kill_session') throw new Error('tmux failed');
    if (cmd === 'mcp_complete') { completions.push(args); return; }
    if (cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  await drainMcp();
  assert.equal(store.cards.length, 1, 'the card is kept');
  assert.deepEqual(completions.map(({ state, code }) => [state, code]), [['ambiguous', 'close-failed']]);
});

test('a remote close never retires a card that a pane is showing', async () => {
  project();
  store.cards = [{ id: 'M5', projectId: 'P1', columnId: 'C1', title: 'MCP', cmd: '', dir: '/tmp',
    session: 'deck-mcp-M5', origin: { source: 'mcp' } }];
  panes.set('deck-mcp-M5', { sid: 'M5', session: 'deck-mcp-M5' });
  const pending = { operationId: 'op_5', kind: 'session-close', result: { cardId: 'M5', sessionId: 'mcp_5' } };
  let items = [pending]; const calls = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push([cmd, args]);
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_complete' || cmd === 'save_board') return;
    throw new Error(`unexpected ${cmd}`);
  } } };
  try {
    await drainMcp();
  } finally {
    panes.delete('deck-mcp-M5');
  }
  assert.equal(store.cards.length, 1);
  assert.ok(!calls.some(([cmd]) => cmd === 'mcp_close_admit' || cmd === 'kill_session'), JSON.stringify(calls));
  const complete = calls.find(([cmd]) => cmd === 'mcp_complete')[1];
  assert.deepEqual([complete.state, complete.code], ['rejected', 'card-shown']);
});

/* a worker card its orchestrator card closes (`deck_worker_close`): the
   ordinary remote close — admitted at the first side effect, its token on
   the queue clear and the kill — refused before any side effect while a
   pane shows the card, with a Reminder or a kept scratchpad; a card already
   gone is committed; a refusal at admission leaves the card (native keeps
   the code it recorded) */
test('a worker close goes through the ordinary remote close and leaves what is the user’s', async () => {
  const worker = (id, extra = {}) => ({ id, projectId: 'P1', columnId: 'C1', title: id, cmd: 'claude', dir: '/tmp',
    session: `deck-${id}`, origin: { source: 'worker', badge: 'claude' }, ...extra });
  const run = async (cards, plan, { admit = true, shown = null } = {}) => {
    project();
    store.cards = cards;
    if (shown) panes.set(shown, { sid: 'x', session: shown });
    const pending = { operationId: `op_${plan.cardId}`, kind: 'worker-close', result: { orchestratorCardId: 'OA', ...plan } };
    let items = [pending]; const calls = [];
    window.__TAURI__ = { core: { invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
      if (cmd === 'mcp_claim') return pending;
      if (cmd === 'mcp_close_admit') { if (!admit) throw new Error('MCP worker close refused: episode-moved'); return { admission: 'close_w' }; }
      if (cmd === 'queue_clear_sessions') return { removed: 0 };
      if (cmd === 'kill_session' || cmd === 'save_board' || cmd === 'mcp_complete') return;
      throw new Error(`unexpected ${cmd}`);
    } } };
    try { await drainMcp(); } finally { if (shown) panes.delete(shown); }
    const complete = calls.find(([cmd]) => cmd === 'mcp_complete')[1];
    return { calls, outcome: [complete.state, complete.code] };
  };
  // closed: admitted first, its token on the queue clear and the kill
  let { calls, outcome } = await run([worker('W1')], { cardId: 'W1', session: 'deck-W1' });
  assert.deepEqual(outcome, ['committed', null]);
  assert.equal(store.cards.length, 0);
  const at = cmd => calls.findIndex(([name]) => name === cmd);
  assert.ok(at('mcp_close_admit') < at('queue_clear_sessions') && at('queue_clear_sessions') < at('kill_session'));
  assert.equal(calls[at('kill_session')][1].mcpAdmission, 'close_w');
  // already gone
  ({ outcome } = await run([], { cardId: 'W2', session: 'deck-W2' }));
  assert.deepEqual(outcome, ['committed', null]);
  // the user's: shown in a pane, a Reminder, a kept scratchpad — refused
  // before any side effect
  for (const [card, code, shown] of [
    [worker('W3'), 'card-shown', 'deck-W3'],
    [worker('W4', { reminder: { dueAt: Date.now() + 60_000 } }), 'reminder-protected', null],
    [worker('W5', { buffer: { entries: [{ id: 'e1', text: 'kept' }] } }), 'card-kept', null],
  ]) {
    ({ calls, outcome } = await run([card], { cardId: card.id, session: card.session }, { shown }));
    assert.deepEqual(outcome, ['rejected', code], card.id);
    assert.equal(store.cards.length, 1, `${card.id} stays`);
    assert.ok(!calls.some(([cmd]) => cmd === 'kill_session' || cmd === 'mcp_close_admit'), card.id);
  }
  // refused at admission (the card moved on): kept, nothing killed
  ({ calls, outcome } = await run([worker('W6')], { cardId: 'W6', session: 'deck-W6' }, { admit: false }));
  assert.equal(outcome[0], 'rejected');
  assert.equal(store.cards.length, 1);
  assert.ok(!calls.some(([cmd]) => cmd === 'kill_session'));
});

test('a failed native completion is surfaced as a fixed sentence, not swallowed or thrown', async () => {
  project();
  const pending = { operationId: 'op_6', kind: 'unknown-kind', result: {} };
  let items = [pending]; let completions = 0;
  window.__TAURI__ = { core: { invoke: async (cmd) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'mcp_complete') { completions++; throw new Error('MCP operation cannot take that result'); }
    throw new Error(`unexpected ${cmd}`);
  } } };
  const toasts = document.getElementById('toasts');
  const before = toasts.children.length;
  await drainMcp();
  assert.equal(completions, 1, 'one report, no retry storm');
  assert.equal(toasts.children.length, before + 1);
  assert.equal(toasts.children.at(-1).textContent, t('mcp.boardSyncFailed'));
  assert.doesNotMatch(toasts.children.at(-1).textContent, /cannot take that result/);
});

test('close outcomes and local error codes map to fixed states and sentences', () => {
  assert.deepEqual(closeOutcome({ ok: true }), { state: 'committed', code: null });
  assert.deepEqual(closeOutcome({ ok: false, stage: 'reminder-protected' }), { state: 'rejected', code: 'reminder-protected' });
  assert.deepEqual(closeOutcome({ ok: false, stage: 'shown' }), { state: 'rejected', code: 'card-shown' });
  assert.deepEqual(closeOutcome({ ok: false, admitted: true, stage: 'kill' }), { state: 'ambiguous', code: 'close-failed' });
  assert.deepEqual(closeOutcome({ ok: false, admitted: false }), { state: 'rejected', code: 'close-failed' });
  assert.equal(mcpErrorKey(new Error('mcp-session-busy')), 'mcp.errorBusy');
  assert.equal(mcpErrorKey('mcp-runner-stale'), 'mcp.errorStale');
  assert.equal(mcpErrorKey('mcp-client-revoked'), 'mcp.errorRevoked');
  assert.equal(mcpErrorKey('mcp-feature-disabled'), 'mcp.errorDisabled');
  assert.equal(mcpErrorKey('mcp-runner-unconfirmed'), 'mcp.errorUnconfirmed');
  assert.equal(mcpErrorKey('mcp-fence-unpersisted'), 'mcp.errorUnpersisted');
  assert.equal(mcpErrorKey('/Users/someone/private path'), 'mcp.actionFailed');
});

function bridge(pending, { save = async () => {} } = {}) {
  let items = [pending]; const done = []; let saves = 0;
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'mcp_pending') { const value = items; items = []; return value; }
    if (cmd === 'mcp_claim') return pending;
    if (cmd === 'save_board') { saves++; return save(); }
    if (cmd === 'mcp_complete') { done.push([args.state, args.code]); return; }
    throw new Error(`unexpected ${cmd}`);
  } } };
  return { done, saves: () => saves };
}

function twoProjects(card = {}) {
  store.projects = [
    { id: 'P1', name: 'One', columns: [{ id: 'A1', name: 'Todo' }, { id: 'A2', name: 'Done' }] },
    { id: 'P2', name: 'Two', columns: [{ id: 'B1', name: 'In' }] },
  ];
  store.cards = [{ id: 'S1', projectId: 'P1', columnId: 'A1', title: 'old', desc: '', cmd: 'claude', dir: '/w',
    session: 'deck-s1', launched: true, ...card }];
}

test('a card update renames and moves in one Board write, bound to what the client saw', async () => {
  twoProjects();
  const plan = { cardId: 'S1', client: 'Tidy', fromProjectId: 'P1', title: 'new', fromTitle: 'old',
    projectId: 'P2', columnId: 'B1', fromColumnId: 'A1' };
  let run = bridge({ operationId: 'op_u1', kind: 'card-update', result: plan });
  await drainMcp();
  assert.deepEqual(run.done, [['committed', null]]);
  assert.equal(run.saves(), 1);
  assert.deepEqual([store.cards[0].projectId, store.cards[0].columnId, store.cards[0].title, store.cards[0].session],
    ['P2', 'B1', 'new', 'deck-s1']);
  // the same plan again: the card is no longer where the client saw it
  run = bridge({ operationId: 'op_u2', kind: 'card-update', result: plan });
  await drainMcp();
  assert.deepEqual(run.done, [['rejected', 'card-changed']]);
  assert.equal(run.saves(), 0);
});

test('a card update is refused whole for a card that may not leave its project, or is gone', async () => {
  const move = { cardId: 'S1', fromProjectId: 'P1', projectId: 'P2', columnId: 'B1', fromColumnId: 'A1', title: 'new', fromTitle: 'old' };
  for (const [card, code] of [
    [{ inboundPlan: { initialQueued: false } }, 'card-starting'],
    [{ channelRun: { initialQueued: true, collecting: true } }, 'card-collecting'],
    [{ origin: { source: 'mcp', key: 'op', badge: 'managed' } }, 'card-managed'],
  ]) {
    twoProjects(card);
    const run = bridge({ operationId: 'op_u3', kind: 'card-update', result: move });
    await drainMcp();
    assert.deepEqual(run.done, [['rejected', code]]);
    assert.deepEqual([store.cards[0].projectId, store.cards[0].title, run.saves()], ['P1', 'old', 0]);
    // the same card still changes column inside its project
    const within = bridge({ operationId: 'op_u4', kind: 'card-update',
      result: { cardId: 'S1', fromProjectId: 'P1', projectId: 'P1', columnId: 'A2', fromColumnId: 'A1' } });
    await drainMcp();
    assert.deepEqual(within.done, [['committed', null]]);
    assert.equal(store.cards[0].columnId, 'A2');
  }
  twoProjects();
  store.cards = [];
  const gone = bridge({ operationId: 'op_u5', kind: 'card-update', result: move });
  await drainMcp();
  assert.deepEqual(gone.done, [['rejected', 'card-missing']]);
});

test('a card update whose Board save failed is ambiguous, never rejected', async () => {
  twoProjects();
  const run = bridge({ operationId: 'op_u6', kind: 'card-update',
    result: { cardId: 'S1', fromProjectId: 'P1', title: 'new', fromTitle: 'old' } },
  { save: async () => { throw new Error('disk full'); } });
  await drainMcp();
  assert.deepEqual(run.done, [['ambiguous', 'board-transaction-unknown']]);
  assert.equal(store.cards[0].title, 'old');
});
