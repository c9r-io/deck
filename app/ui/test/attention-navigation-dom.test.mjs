// Attention-to-Board/session continuity on a bounded fake DOM. The injected
// navigation dependencies prove that locating has no session side effect and
// that a re-poll cannot lose the exact Attention return point.
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement } from './fixtures/dom-fixture.mjs';

class NavElement extends FakeElement {
  constructor(tag = 'div') {
    super(tag);
    this.queries = new Map();
    this.scrollTop = 0;
  }
  querySelector(selector) { return this.queries.get(selector) || null; }
  focus() { document.activeElement = this; }
  scrollIntoView(options) { this.scrolled = options; }
}

const ids = new Map([
  ['attention-list', new NavElement()],
  ['attention-btn', new NavElement('button')],
  ['attention-back', new NavElement('button')],
]);
let boardCards = [];
const documentListeners = new Map();
globalThis.document = {
  activeElement: null,
  getElementById: id => ids.get(id) || null,
  querySelectorAll: selector => selector === '#columns .card' ? boardCards : [],
  createElement: tag => new NavElement(tag),
  addEventListener(type, fn) {
    const listeners = documentListeners.get(type) || [];
    listeners.push(fn);
    documentListeners.set(type, listeners);
  },
};
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {}, dispatchEvent() {} };

const { ctx, state, store } = await import('../js/state.js');
const { initAttention, locateAttentionCard, openAttentionCard } = await import('../js/attention.js');
const { attentionReturnOnOpen } = await import('../js/layout.js');

const cards = new Map();
let snapshot;
let pollImpl = async () => {};
let switches = [];
let opens = [];

initAttention({
  pollNow: (...args) => pollImpl(...args),
  provider: {
    get: id => cards.get(id),
    project: id => store.projects.find(project => project.id === id),
  },
  render() {},
  switchProject(pid, opts) {
    switches.push({ pid, opts });
    ctx.attentionReturn = opts.attentionReturn;
    state.projectId = pid;
    state.view = 'board';
    state.sessionId = null;
  },
  leaveSessionView() {},
  closeBuffer() {},
  openSession: async (id, opts) => { opens.push({ id, opts }); return true; },
  openSettings() {},
});

function reset({ view = 'attention', id = 'card-a', projectId = 'project-b' } = {}) {
  const card = { id, projectId, columnId: 'column', session: `session-${id}`, title: id };
  cards.clear(); cards.set(id, card);
  store.cards = [card];
  store.projects = [
    { id: 'project-a', name: 'A', columns: [] },
    { id: 'project-b', name: 'B', columns: [] },
  ];
  state.view = view; state.projectId = 'project-a'; state.sessionId = null;
  ctx.attentionFilter = 'waiting'; ctx.attentionReturn = null;
  ids.get('attention-list').scrollTop = 73;
  snapshot = { alive: true, stale: false };
  ctx.attention = { get: () => snapshot };
  switches = []; opens = []; boardCards = [];
  pollImpl = async () => {};
  return card;
}

function boardCard(id) {
  const card = new NavElement();
  card.dataset.sid = id;
  const open = new NavElement('button');
  open.className = 'card-open';
  card.queries.set('.card-open', open);
  boardCards = [card];
  return { card, open };
}

const savedFor = focusId => ({
  projectId: 'project-a', filter: 'waiting', scroll: 73, focusId,
});

test('locate carries the Attention return point, focuses native open, and never starts', () => {
  const card = reset();
  const board = boardCard(card.id);
  locateAttentionCard(card.id);
  assert.deepEqual(switches, [{ pid: 'project-b', opts: { attentionReturn: savedFor(card.id) } }]);
  assert.deepEqual(ctx.attentionReturn, savedFor(card.id));
  assert.equal(document.activeElement, board.open);
  assert.deepEqual(board.card.scrolled, { block: 'nearest', inline: 'nearest' });
  assert.deepEqual(opens, []);
});

test('locating outside Attention passes null and cannot inherit another row', () => {
  const card = reset({ view: 'board', id: 'card-b' });
  ctx.attentionReturn = savedFor('card-a');
  boardCard(card.id);
  locateAttentionCard(card.id);
  assert.deepEqual(switches, [{ pid: 'project-b', opts: { attentionReturn: null } }]);
  assert.equal(ctx.attentionReturn, null);
  assert.deepEqual(opens, []);
});

test('a live Attention row opens without starting and keeps its exact return point', async () => {
  const card = reset();
  await openAttentionCard(card.id);
  assert.deepEqual(opens, [{ id: card.id, opts: { allowStart: false, attentionReturn: savedFor(card.id) } }]);
  assert.deepEqual(switches, []);
});

test('a row that becomes stale during re-poll locates with the same return point', async () => {
  const card = reset();
  const board = boardCard(card.id);
  pollImpl = async () => { snapshot = { alive: false, stale: true }; };
  await openAttentionCard(card.id);
  assert.deepEqual(opens, []);
  assert.deepEqual(switches, [{ pid: 'project-b', opts: { attentionReturn: savedFor(card.id) } }]);
  assert.deepEqual(ctx.attentionReturn, savedFor(card.id));
  assert.equal(document.activeElement, board.open);
});

test('session opens retain attention only for the matching located Board card', () => {
  const saved = savedFor('card-a');
  assert.equal(attentionReturnOnOpen('card-a', {}, 'board', saved), saved);
  assert.equal(attentionReturnOnOpen('card-b', {}, 'board', saved), null);
  assert.equal(attentionReturnOnOpen('card-b', {}, 'session', saved), saved);
  assert.equal(attentionReturnOnOpen('card-a', { attentionReturn: null }, 'attention', saved), null);
});
