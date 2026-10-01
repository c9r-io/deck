// The session workspace's right-hand tool slot: one tool at a time, every
// owner's own close on replacement and on leave, popups closed with it, and
// focus returned to a terminal only by the user's own close. The second half
// drives the real Local Translation module through the slot (fake DOM and
// backend, as translation-lens-dom.test.mjs does) to show the slot runs the
// Lens's full cleanup and that a pane switch inside one layout is not a leave.
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';
import { createSessionToolSlot, SESSION_TOOLS } from '../js/session-tools.js';

/* three owners shaped like the real ones: open claims, close is idempotent,
   releases, and refocuses the terminal only for 'user' */
function harness() {
  const slot = createSessionToolSlot();
  const open = new Set(), reasons = [], focus = [];
  let popup = false;
  const owners = {};
  for (const name of SESSION_TOOLS) {
    const close = reason => {
      reasons.push(`${name}:${reason}`);
      const was = open.delete(name);
      slot.release(name);
      if (was && reason === 'user') focus.push(name);
    };
    slot.register(name, close);
    owners[name] = {
      open() { slot.claim(name); open.add(name); },
      toggle() { if (open.has(name)) close('user'); else this.open(); },
      close,
    };
  }
  slot.registerPopup(() => { popup = false; });
  return { slot, open, reasons, focus, owners, showPopup: () => { popup = true; }, popup: () => popup };
}

const only = (h, name) => {
  assert.deepEqual([...h.open], name ? [name] : []);
  assert.equal(h.slot.active(), name);
};

test('each tool opened alone is the only one visible', () => {
  for (const name of SESSION_TOOLS) {
    const h = harness();
    h.owners[name].open();
    only(h, name);
  }
});

test('queue → buffer → translation → queue: each switch leaves exactly the new tool', () => {
  const h = harness();
  h.owners.queue.open(); only(h, 'queue');
  h.owners.buffer.open(); only(h, 'buffer');
  h.owners.translation.open(); only(h, 'translation');
  h.owners.queue.open(); only(h, 'queue');
  assert.ok(h.reasons.every(r => r.endsWith(':replace')), 'a switch closes others as a replacement');
  assert.deepEqual(h.focus, [], 'a replacement never refocuses the terminal');
});

test('the active tool\'s button closes it and only that close returns focus', () => {
  const h = harness();
  h.owners.translation.toggle(); only(h, 'translation');
  h.owners.translation.toggle(); only(h, null);
  assert.deepEqual(h.focus, ['translation']);
});

test('leaving closes every owner, even one the slot did not record, plus popups, without focus', () => {
  const h = harness();
  h.owners.queue.open(); h.showPopup();
  h.open.add('buffer');   // a drawer opened behind the slot's back
  h.slot.closeAll('leave');
  only(h, null);
  assert.equal(h.popup(), false);
  assert.deepEqual(h.focus, []);
  assert.deepEqual(h.reasons.filter(r => r.endsWith(':leave')).sort(), ['buffer:leave', 'queue:leave', 'translation:leave']);
  h.slot.closeAll('leave');   // idempotent
  only(h, null);
});

test('switching tools closes an open popup (dropdown, template menu)', () => {
  const h = harness();
  h.owners.queue.open(); h.showPopup();
  h.owners.buffer.open();
  assert.equal(h.popup(), false);
});

test('only the three session tools may register', () => {
  assert.throws(() => createSessionToolSlot().register('board', () => {}), /unknown session tool/);
});

/* ---------- the real Lens through the slot ---------- */
const windowListeners = new Map();
globalThis.document = Object.assign(fakeDocument, { hidden: false, hasFocus: () => true, getSelection: () => ({ isCollapsed: true }) });
globalThis.window = {
  __TAURI__: null,
  addEventListener(type, fn) { const list = windowListeners.get(type) || []; list.push(fn); windowListeners.set(type, list); },
  dispatchEvent(event) { for (const fn of windowListeners.get(event.type) || []) fn(event); },
  setTimeout: (...args) => setTimeout(...args), clearTimeout: id => clearTimeout(id),
};
globalThis.CustomEvent ??= class extends Event { constructor(type, init = {}) { super(type); this.detail = init.detail; } };
globalThis.ResizeObserver = class { observe() {} };
globalThis.requestAnimationFrame = fn => setTimeout(fn, 16);
globalThis.__DECK_SMOKE_TRANSLATION = true;
mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'] });

const { ctx, state } = await import('../js/state.js');
const intelligence = await import('../js/local-intelligence.js');
const lens = await import('../js/translation-lens.js');
const tools = await import('../js/session-tools.js');

const flush = async () => { for (let i = 0; i < 6; i++) await new Promise(resolve => setImmediate(resolve)); };
const tick = async ms => { for (let left = ms; left > 0; left -= 10) { mock.timers.tick(Math.min(10, left)); await flush(); } };
const fire = type => window.dispatchEvent(new Event(type));
const $ = id => fakeDocument.getElementById(id);

const backend = { unloads: 0, disarms: 0, cancels: 0, armed: false, calls: [] };
intelligence.installTranslationSmokeBackend({
  capability: async () => ({ available: true, enabled: true, installed: true, loaded: false }),
  unload: async () => { backend.unloads++; },
  translate: (id, text) => new Promise(resolve => backend.calls.push({ id, text, resolve })),
  cancel: async () => { backend.cancels++; },
  arm: async () => { backend.armed = true; },
  disarm: async () => { backend.armed = false; backend.disarms++; },
  poll: async () => null,
  copy: async () => 1,
});
const viewports = { a: ['Alpha pane output.'], b: ['Beta pane output.'] };
const pane = name => ({ term: { rows: 2, buffer: { active: { viewportY: 0,
  getLine: row => ({ translateToString: () => viewports[name][row] || '' }) } } } });
const panes = new Map([['a', pane('a')], ['b', pane('b')]]);
ctx.settings.localIntelligence = { translation: { enabled: true, documentLimitBytes: 16384 } };
state.view = 'session'; ctx.attachedName = 'a';
const others = { queue: 0, buffer: 0 };
tools.registerSessionTool('queue', () => { others.queue++; tools.releaseSessionTool('queue'); });
tools.registerSessionTool('buffer', () => { others.buffer++; tools.releaseSessionTool('buffer'); });
lens.initTranslationLens({ panes });
await flush();

/* the Lens keeps at most one native request outstanding: answer them all */
const settle = async () => {
  for (const call of backend.calls) if (!call.done) { call.done = true; call.resolve({ requestId: call.id, text: `译:${call.text}` }); }
  await flush();
};
const openLens = async () => { $('translation-btn').onclick(); await flush(); await tick(400); await settle(); };
const lensOpen = () => !$('translation-panel').hidden && $('translation-btn')['aria-pressed'] === 'true';

test('opening the Lens claims the slot and closes the other tools', async () => {
  others.queue = others.buffer = 0;
  await openLens();
  assert.ok(lensOpen());
  assert.equal(tools.activeSessionTool(), 'translation');
  assert.deepEqual(others, { queue: 1, buffer: 1 });
  lens.closeTranslationLens(); await flush();
});

test('another tool taking the slot runs the Lens\'s full close, not a bare hide', async () => {
  await openLens();
  const unloads = backend.unloads;
  tools.claimSessionTool('queue');
  await settle();
  assert.ok(!lensOpen(), 'panel hidden and button released');
  assert.equal($('translation-btn')['aria-pressed'], 'false');
  assert.equal(backend.unloads, unloads + 1, 'the model is unloaded');
  const requests = backend.calls.length;
  viewports.a = ['Output after close.']; window.dispatchEvent(Object.assign(new Event('deck-terminal-changed'), { detail: 'a' }));
  await tick(1500);
  assert.equal(backend.calls.length, requests, 'the live cadence stopped');
  tools.closeSessionTools('leave');
});

test('a pane switch inside the layout retargets Live and keeps the Lens open', async () => {
  ctx.attachedName = 'a'; viewports.a = ['Alpha pane output.'];
  await openLens();
  ctx.attachedName = 'b'; fire('deck-pane-focused'); await tick(400); await settle(); await tick(400);
  assert.ok(lensOpen(), 'still open');
  assert.equal(tools.activeSessionTool(), 'translation');
  assert.ok(backend.calls.some(call => call.text === 'Beta pane output.'), 'Live follows the focused pane');
  tools.closeSessionTools('leave');
});

test('leaving the session closes the Lens through the slot and disarms copied-text observation', async () => {
  ctx.attachedName = 'a';
  await openLens();
  $('translation-tab-clipboard').onclick(); await tick(400); await settle(); await tick(400);
  assert.ok(backend.armed, 'copied-text observation armed');
  tools.closeSessionTools('leave'); await flush();
  assert.ok(!lensOpen());
  assert.equal(backend.armed, false, 'disarmed on leave');
  assert.equal(tools.activeSessionTool(), null);
});
