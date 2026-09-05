// DOM-contract tests for the Board-level template manager (ui/js/templates.js).
// The real handlers run against a fake document, so what is asserted here is
// what the production module does: one Board transaction per user action, a
// refused edit writing nothing, and rules that name a template being reported
// before the name they point at changes.
import test from 'node:test';
import assert from 'node:assert/strict';

class FakeElement {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.listeners = new Map();
    this.style = {};
    this.classList = { add() {}, remove() {}, toggle() {}, contains() { return false; } };
    this.isConnected = true;
    this.value = '';
    this.disabled = false;
    this.hidden = false;
    this.dataset = {};
    this._textContent = '';
    this._query = new Map();
  }
  addEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    list.push(fn); this.listeners.set(type, list);
  }
  fire(type, extra = {}) {
    const event = {
      key: '', keyCode: 0, isComposing: false, target: this,
      prevented: 0, stopped: 0,
      preventDefault() { this.prevented++; },
      stopPropagation() { this.stopped++; },
      ...extra,
    };
    for (const fn of this.listeners.get(type) || []) fn(event);
    if (typeof this[`on${type}`] === 'function') this[`on${type}`](event);
    return event;
  }
  append(...nodes) { this.children.push(...nodes); }
  appendChild(node) { this.children.push(node); return node; }
  replaceChildren(...nodes) { this.children = nodes; }
  querySelectorAll() { return []; }
  /* board.js renders through innerHTML + querySelector; the manager never
     inspects that markup, so a stable stand-in per selector is enough */
  querySelector(selector) {
    if (!this._query.has(selector)) this._query.set(selector, new FakeElement());
    return this._query.get(selector);
  }
  set innerHTML(value) { this._html = String(value); this.children = []; }
  get innerHTML() { return this._html || ''; }
  remove() { this.isConnected = false; }
  focus() { fakeDocument.activeElement = this; }
  select() {}
  closest() { return null; }
  getClientRects() { return []; }
  setAttribute(name, value) { this[name] = value; }
  set textContent(value) { this._textContent = String(value); this.children = []; }
  get textContent() { return this._textContent; }
}

const ids = new Map();
const fakeDocument = {
  activeElement: null,
  addEventListener() {},
  body: new FakeElement('body'),
  documentElement: new FakeElement('html'),
  createElement: tag => new FakeElement(tag),
  getElementById(id) {
    if (!ids.has(id)) ids.set(id, new FakeElement());
    return ids.get(id);
  },
  querySelectorAll() { return []; },
  querySelector() { return null; },
};
globalThis.document = fakeDocument;
globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} };
globalThis.requestAnimationFrame = fn => setTimeout(fn, 0);
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false, addEventListener() {}, matchMedia: null };

const { cfmDone } = await import('../js/dialogs.js');
const { openTemplates, closeTemplates, initTemplates } = await import('../js/templates.js');
initTemplates();
const { state, store } = await import('../js/state.js');
const { flushBoardMutations } = await import('../js/persistence.js');

const $ = id => fakeDocument.getElementById(id);
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
/* a user action is one queued Board transaction plus the re-render that
   follows it; settle so the next gesture sees what the app would show */
const settle = async () => {
  for (let i = 0; i < 4; i++) { await tick(); await flushBoardMutations(); }
};
let writes = [];

function seed(templates, rules = []) {
  writes = [];
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd !== 'save_board') return null;
    writes.push(JSON.parse(args.data));
  } } };
  globalThis.settings = { inbound: { sources: {}, rules } };
  store.projects = [{ id: 'P1', name: 'deck', columns: [{ id: 'C1', name: 'Working' }], templates }];
  store.cards = [];
  state.projectId = 'P1';
  state.view = 'board';
  fakeDocument.activeElement = null;
  openTemplates();
}

const savedTemplates = () => writes.at(-1).projects[0].templates;
const stepRows = () => $('tpl-steps').children.filter(child => child.children.length >= 5);
const stepInput = index => stepRows()[index].children[1];

test('editing a step persists the whole ordered list in one Board transaction', async () => {
  seed([{ name: 'morning', steps: ['read the notes', 'plan the work'] }]);
  const input = stepInput(0);
  input.value = '  read the notes\n and the diff  ';
  input.fire('change');
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 1, 'one write per edit');
  assert.deepEqual(savedTemplates()[0].steps, ['read the notes and the diff', 'plan the work'],
    'a step reaches the queue as one line');

  stepRows()[0].children[3].fire('click');   // ↓ move later
  await tick();
  await flushBoardMutations();
  assert.deepEqual(savedTemplates()[0].steps, ['plan the work', 'read the notes and the diff']);

  stepRows()[1].children[4].fire('click');   // ✕ remove
  await tick();
  await flushBoardMutations();
  assert.deepEqual(savedTemplates()[0].steps, ['plan the work']);
  closeTemplates();
});

test('an emptied step and an added blank step write nothing', async () => {
  seed([{ name: 'morning', steps: ['read the notes'] }]);
  const input = stepInput(0);
  input.value = '   ';
  input.fire('change');
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 0, 'a step is never silently deleted by clearing its text');
  assert.equal(input.value, 'read the notes', 'the visible row returns to what is on disk');

  $('tpl-step-text').value = '  ';
  $('tpl-step-add').fire('click');
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 0);

  $('tpl-step-text').value = 'run the tests';
  $('tpl-step-text').fire('keydown', { key: 'Enter', keyCode: 229, isComposing: true });
  await tick();
  assert.equal(writes.length, 0, 'an IME commit is not a submit');
  $('tpl-step-text').fire('keydown', { key: 'Enter' });
  await tick();
  await flushBoardMutations();
  assert.deepEqual(savedTemplates()[0].steps, ['read the notes', 'run the tests']);
  closeTemplates();
});

test('a duplicate or empty template name is refused and nothing is written', async () => {
  seed([{ name: 'morning', steps: ['a'] }, { name: 'release', steps: ['b'] }]);
  $('tpl-name').value = 'release';
  $('tpl-name').fire('change');
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 0);
  assert.equal($('tpl-name').value, 'morning', 'the editor returns to the stored name');

  $('tpl-name').value = '   ';
  $('tpl-name').fire('change');
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 0);
  assert.equal($('tpl-name').value, 'morning');
  closeTemplates();
});

test('renaming a template an inbound rule names is confirmed before the link breaks', async () => {
  const rules = [{ id: 'r1', source: 'slack', projectId: 'P1', template: 'morning' }];
  seed([{ name: 'morning', steps: ['a'] }], rules);
  $('tpl-name').value = 'morning review';
  $('tpl-name').fire('change');
  await tick();
  assert.equal($('cfm').style.display, 'flex', 'the affected rules are reported first');
  cfmDone(false);
  await tick();
  await flushBoardMutations();
  assert.equal(writes.length, 0, 'declining keeps the name the rule still matches');

  $('tpl-name').value = 'morning review';
  $('tpl-name').fire('change');
  await tick();
  cfmDone(true);
  await tick();
  await flushBoardMutations();
  assert.deepEqual(savedTemplates().map(tp => tp.name), ['morning review']);
  assert.deepEqual(rules[0], { id: 'r1', source: 'slack', projectId: 'P1', template: 'morning' },
    'deck never rewrites the user’s rules for them');
  closeTemplates();
});

test('creating and deleting a template goes through the ordinary Board write', async () => {
  seed([{ name: 'morning', steps: ['a'] }]);
  $('tpl-new').fire('click');
  await settle();
  assert.equal(savedTemplates().length, 2);
  assert.equal($('tpl-name').value, 'new template',
    'the editor follows the created template even while the name field holds focus');
  assert.deepEqual(savedTemplates()[1].steps, [], 'a new template starts empty');

  $('tpl-delete').fire('click');
  await tick();
  cfmDone(true);
  await settle();
  assert.deepEqual(savedTemplates().map(tp => tp.name), ['morning'],
    'only the selected template is removed');
  closeTemplates();
  assert.equal($('tpl-modal').style.display, 'none');
});
