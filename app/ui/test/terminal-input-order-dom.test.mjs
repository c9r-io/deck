// Drives the production terminal input wiring (layout.js wireTerminalInput,
// goLive, ensureAttached, leaveSessionView, focusPane) with the production
// selection coordinator and the vendored xterm engine. Only the completion
// time of IPC replies is controlled; pane.liveQ is never assigned here.
// What is observed is the order and arguments of pty_write CALLS. Nothing in
// this file is a backend: delivery is covered by the pty.rs tests, and real
// WKWebView event delivery by the smoke.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Terminal } = createRequire(import.meta.url)('../vendor/xterm.js');

class Surface {
  constructor() { this.listeners = new Map(); this.children = []; this.style = {}; this.dataset = {};
    this.classList = { add() {}, remove() {}, toggle() {}, contains() { return false; } }; this.isConnected = true; }
  addEventListener(type, fn) { const l = this.listeners.get(type) || []; l.push(fn); this.listeners.set(type, l); }
  removeEventListener(type, fn) { this.listeners.set(type, (this.listeners.get(type) || []).filter(f => f !== fn)); }
  fire(type, more = {}) {
    const event = { type, button: 0, buttons: 1, pointerId: 1, detail: 1, clientX: 15, clientY: 15,
      isTrusted: true, pointerType: 'mouse', preventDefault() {}, stopPropagation() {},
      stopImmediatePropagation() { this.stopped = true; }, ...more };
    for (const fn of this.listeners.get(type) || []) { fn(event); if (event.stopped) break; }
    return event;
  }
  appendChild(n) { this.children.push(n); return n; }
  replaceChildren(...n) { this.children = n; }
  remove() {} closest() { return null; } contains() { return false; }
  getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 100, width: 100, height: 100 }; }
  querySelector() { return null; }
  querySelectorAll(sel) { return sel === '.deck-selection-band' ? this.children.flatMap(c => c.querySelectorAll?.(sel) || []) : []; }
  dispatchEvent(e) { for (const fn of this.listeners.get(e.type) || []) fn(e); return true; }
}
const doc = new Surface(); doc.hidden = false; doc.hasFocus = () => true;
doc.createElement = () => { const el = new Surface(); el.className = ''; return el; };
doc.getElementById = () => new Surface(); doc.documentElement = new Surface(); doc.body = new Surface();
globalThis.document = doc;
const win = new Surface();
globalThis.window = win;
globalThis.requestAnimationFrame = fn => setImmediate(fn);
globalThis.CustomEvent = globalThis.CustomEvent || class { constructor(type, init) { this.type = type; this.detail = init?.detail; } };

// ---- controllable IPC -------------------------------------------------
let trace, gates, anchorCell, activeCell, attachGen = 100;
const rejections = []; process.on('unhandledRejection', e => rejections.push(String(e)));
const b64 = s => Buffer.from(s, 'base64').toString('utf8');
const listeners = {};
win.__TAURI__ = { event: { listen: async (ev, cb) => { listeners[ev] = cb; return () => {}; } }, core: { invoke: (name, args) => {
  if (name === 'ui_event') { if (args.code === 'pty-write-cancel') trace.push({ layer: 'cancel', reason: args.detail }); if (args.code === 'pty-write-fail') trace.push({ layer: 'write-fail' }); return Promise.resolve(); }
  if (name === 'pty_write') {            // layer 2: order of production write calls
    const text = b64(args.dataB64);
    // Order and arguments of production write calls. Nothing after this line is a backend.
    trace.push({ layer: 'pty_write-invoke', session: args.name, gen: args.gen ?? null, text });
    if (gates.writeFail) return Promise.reject(new Error('probe: write refused'));
    return Promise.resolve();
  }
  if (name === 'attach_session' && !gates[name]) return Promise.resolve(++attachGen);
  if (gates[name]) {                      // held reply, released by the scenario
    trace.push({ layer: 'ipc-held', name });
    return new Promise((resolve, reject) => gates[name].push({ resolve, reject, name }));
  }
  if (name === 'terminal_selection_start') { anchorCell = [args.anchorRow, args.anchorCol]; activeCell = [args.activeRow, args.activeCol]; }
  if (name === 'terminal_selection_update') activeCell = [args.row, args.col];
  if (name === 'terminal_selection_start' || name === 'terminal_selection_update' || name === 'terminal_selection_finish') return Promise.resolve({
    active: true, selection_present: true, selection_start_row: 1, selection_start_col: 1,
    selection_end_row: 1, selection_end_col: 4, frame_top: 0, history_rows: 10, scroll_position: 0, cursor_visible: false });
  trace.push({ layer: 'ipc', name });
  return Promise.resolve({});
} } };

const layout = await import('../js/layout.js');
const { wireTerminalSelection } = await import('../js/selection.js');
const { panes } = await import('../js/board.js');
const { store } = await import('../js/state.js');
const tick = () => new Promise(r => setImmediate(r));
// initLayout registers the production pty-data listener the frames below go through.
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
layout.initLayout();
await tick();
assert.equal(typeof listeners['pty-data'], 'function');
let n = 0;

function fixture({ scrolled = false, session = 'ime-probe-' + (++n), keep = false } = {}) {
  if (!keep) { trace = []; gates = {}; rejections.length = 0; }
  anchorCell = activeCell = null;
  const sid = 'card-' + session;
  const engine = new Terminal({ allowProposedApi: true, cols: 10, rows: 10 });
  const textarea = new Surface();
  const body = new Surface(), screen = new Surface();
  const overlay = () => screen.children.find(x => x.className === 'deck-selection-overlay');
  body.querySelector = s => s === '.xterm-screen' ? screen : s === '.deck-selection-overlay' ? overlay() : null;
  body.querySelectorAll = s => s === '.deck-selection-band' ? (overlay()?.children || []) : [];
  screen.querySelector = () => overlay() || null;
  // The real engine supplies onData/input/selection/parser; only the DOM-bound
  // members a headless engine lacks are substituted.
  const term = new Proxy(engine, { get(t, k) {
    if (k === 'textarea') return textarea;
    if (k === 'element') return body;
    const v = t[k]; return typeof v === 'function' ? v.bind(t) : v; },
    set(t, k, v) { t[k] = v; return true; } });
  const pane = { sid, session, body, el: new Surface(), term, seps: [], syncSize: async () => true, fit: { fit() {} }, invalidateSize() {} };
  if (!store.cards.find(c => c.id === sid)) store.cards.push({ id: sid, session, scrolled, status: 'running', fg: 'zsh', dir: '/tmp' });
  else store.cards.find(c => c.id === sid).scrolled = scrolled;
  panes.set(session, pane);
  engine.onData(text => trace.push({ layer: 'onData-entry', text }));   // layer 1, registered first
  wireTerminalSelection(pane, () => {});
  layout.wireTerminalInput(pane, term, body);                            // production wiring
  const settle = async () => { await pane.selection.idle(); await tick(); await pane.selection.idle(); await tick(); };
  return { pane, engine, textarea, body, session, settle,
    key: text => engine.input(text, true),
    compositionstart: () => { trace.push({ layer: 'dom-event', type: 'compositionstart' }); textarea.fire('compositionstart'); },
    async select() { body.fire('pointerdown', { clientX: 15 }); body.fire('mousedown', { clientX: 15 });
      doc.fire('pointermove', { clientX: 45 }); doc.fire('pointerup', { clientX: 45 }); await settle();
      assert.equal(pane.selection.hasSelection(), true, 'fixture: production selection exists'); },
    hold(name) { gates[name] = []; },
    async release(name, how = 'resolve') { trace.push({ layer: 'ipc-released', name, how });
      for (const g of gates[name].splice(0)) g[how](how === 'resolve' ? (name === 'attach_session' ? ++attachGen : {}) : new Error('probe')); delete gates[name]; await tick(); await tick(); },
    writes: () => trace.filter(x => x.layer === 'pty_write-invoke' && x.session === session).map(x => x.text),
    entries: () => trace.filter(x => x.layer === 'onData-entry').map(x => x.text),
    calls: () => trace.filter(x => x.layer === 'pty_write-invoke' && x.session === session).map(x => [x.text, x.gen]),
    cancels: () => trace.filter(x => x.layer === 'cancel').map(x => x.reason),
    attach: () => layout.ensureAttached(pane),
    frame: (gen, text, seq = 1) => listeners['pty-data']({ payload: { name: session, gen, seq, data: Buffer.from(text).toString('base64') } }),
    parsed: () => new Promise(resolve => engine.write('', resolve)),
    done() { store.cards.length = 0; panes.clear(); } };
}

const input = await import('../js/terminal-input.js');
const done = () => { store.cards.length = 0; panes.clear(); };

for (const [name, opts, cleanup, second, composition] of [
  ['selection cleanup pending, then plain input', {}, 'terminal_selection_cancel', 'b', false],
  ['selection cleanup pending, compositionstart, then committed text', {}, 'terminal_selection_cancel', '中文', true],
  ['return to live pending, then plain input', { scrolled: true }, 'scroll_bottom', 'b', false],
  ['return to live pending, compositionstart, then committed text', { scrolled: true }, 'scroll_bottom', '中文', true],
]) test(`write order equals entry order: ${name}`, async () => {
  const f = fixture(opts);
  try {
    await f.attach();
    if (cleanup === 'terminal_selection_cancel') await f.select();
    f.hold(cleanup);
    f.key('a'); await tick();
    if (composition) { f.compositionstart(); await tick(); }
    f.key(second); await tick();
    assert.deepEqual(f.writes(), [], 'Nothing passes the pending cleanup');
    await f.release(cleanup);
    assert.deepEqual(f.writes(), f.entries());
    assert.deepEqual(f.writes(), ['a', second]);
  } finally { done(); }
});

test('a composition does not pass an unfinished cleanup, and release writes everything in order', async () => {
  const f = fixture({ scrolled: true });
  try {
    await f.attach();
    f.hold('scroll_bottom');
    for (const k of ['a', 'b', 'c']) { f.key(k); await tick(); }
    f.compositionstart(); await tick(); f.key('中文'); await tick(); f.key('d'); await tick();
    assert.deepEqual(f.writes(), [], 'The barrier holds every later input');
    await f.release('scroll_bottom');
    assert.deepEqual(f.writes(), ['a', 'b', 'c', '中文', 'd']);
    assert.equal(f.pane.liveQ, null);
  } finally { done(); }
});

test('a failed cleanup reply or a failed write is attempted once and leaves the tail usable', async () => {
  let f = fixture({ scrolled: true });
  try {
    await f.attach();
    f.hold('scroll_bottom');
    f.key('a'); await tick(); await f.release('scroll_bottom', 'reject'); f.key('b'); await tick();
    assert.deepEqual(f.writes(), ['a', 'b']);
    assert.equal(f.pane.liveQ, null);
    done();
    f = fixture({ scrolled: true });
    await f.attach();
    f.hold('scroll_bottom'); gates.writeFail = true;
    f.key('a'); await tick(); await f.release('scroll_bottom'); gates.writeFail = false; f.key('b'); await tick();
    assert.deepEqual(f.writes(), ['a', 'b'], 'A failed write is not retried');
    assert.equal(trace.filter(x => x.layer === 'write-fail').length, 1);
    assert.equal(f.pane.liveQ, null);
    assert.deepEqual(rejections, []);
  } finally { done(); }
});

test('input waiting on a removed pane is cancelled, not written', async () => {
  const f = fixture({ scrolled: true });
  try {
    await f.attach();
    f.hold('scroll_bottom');
    f.key('旧'); await tick();
    layout.leaveSessionView({ switchingSession: true });
    await f.release('scroll_bottom');
    assert.deepEqual(f.writes(), []);
    assert.deepEqual(f.cancels(), ['pane']);
  } finally { done(); }
});

test('a new pane and attachment of the same session never receive the old pane\'s input', async () => {
  const f = fixture({ scrolled: true });
  try {
    await f.attach(); f.hold('scroll_bottom');
    f.key('旧'); await tick();
    layout.leaveSessionView({ switchingSession: true });
    const g = fixture({ session: f.session, keep: true });
    await g.attach(); g.key('新'); await tick();
    await f.release('scroll_bottom');
    assert.deepEqual(g.calls(), [['新', g.pane.inputGen]]);
    assert.deepEqual(g.cancels(), ['pane']);
  } finally { done(); }
});

test('input bound to a replaced attachment of the same pane is cancelled, never re-bound', async () => {
  const f = fixture({ scrolled: true });
  try {
    await f.attach(); const old = f.pane.inputGen; f.hold('scroll_bottom');
    f.key('旧'); await tick();
    await f.attach(); assert.notEqual(f.pane.inputGen, old);
    f.key('新'); await tick();
    await f.release('scroll_bottom');
    assert.deepEqual(f.calls(), [['新', f.pane.inputGen]]);
    assert.deepEqual(f.cancels(), ['attachment']);
  } finally { done(); }
});

test('focusing another split cancels nothing on the pane that keeps its attachment', async () => {
  const f = fixture({ scrolled: true });
  try {
    await f.attach();
    const other = fixture({ keep: true }); await other.attach();
    f.hold('scroll_bottom');
    f.key('a'); await tick();
    layout.focusPane(other.session); f.textarea.fire('blur'); await tick();
    await f.release('scroll_bottom');
    assert.deepEqual(f.calls(), [['a', f.pane.inputGen]]);
    assert.deepEqual(f.cancels(), []);
  } finally { done(); }
});

// Attach start. Terminal input never names "whichever attachment has this
// session name": every call below carries one generation.
test('first attach: early keyboard input waits for its own attach request, the reply follows it, each written once', async () => {
  const f = fixture();
  try {
    f.hold('attach_session'); const attaching = f.attach(); await tick();
    const gen = attachGen + 1;
    f.key('x'); await tick();
    f.frame(gen, 'hello\x1b[5n'); await f.parsed(); await tick();
    assert.deepEqual(f.calls(), [], 'Nothing is written before the request names its attachment');
    await f.release('attach_session'); await attaching; await tick();
    f.key('y'); await tick();
    assert.deepEqual(f.calls(), [['x', gen], ['\x1b[0n', gen], ['y', gen]]);
    assert.deepEqual(f.cancels(), []);
  } finally { done(); }
});

test('an automatic reply to the first frame is bound to that frame and does not wait for the attach reply', async () => {
  const f = fixture();
  try {
    f.hold('attach_session'); const attaching = f.attach(); await tick();
    const gen = attachGen + 1;
    f.frame(gen, '\x1b[5n'); await f.parsed(); await tick();
    assert.deepEqual(f.calls(), [['\x1b[0n', gen]]);
    await f.release('attach_session'); await attaching;
    assert.deepEqual(f.cancels(), []);
  } finally { done(); }
});

test('early input whose pane is replaced before its attach reply never reaches the replacement', async () => {
  const f = fixture();
  try {
    f.hold('attach_session'); const attaching = f.attach(); await tick();
    f.key('旧'); await tick();
    layout.leaveSessionView({ switchingSession: true });
    const pending = gates.attach_session.splice(0); delete gates.attach_session;
    const g = fixture({ session: f.session, keep: true });
    await g.attach(); g.key('新'); await tick();
    for (const reply of pending) reply.resolve(g.pane.inputGen - 1);
    await attaching; await tick(); await tick();
    assert.deepEqual(g.calls(), [['新', g.pane.inputGen]]);
    assert.deepEqual(g.cancels(), ['pane']);
  } finally { done(); }
});

test('early input whose attach names no attachment is cancelled once, never written', async () => {
  const f = fixture();
  try {
    f.hold('attach_session'); const attaching = f.attach(); await tick();
    f.key('x'); await tick();
    await f.release('attach_session', 'reject'); await attaching; await tick();
    assert.deepEqual(f.calls(), []);
    assert.deepEqual(f.cancels(), ['attachment']);
    f.key('y'); await tick();
    assert.deepEqual(f.calls(), [], 'A pane with no attachment has no target');
    assert.deepEqual(f.cancels(), ['attachment', 'attachment']);
  } finally { done(); }
});

test('a reply to a frame of the previous attachment keeps that generation on the new pane', async () => {
  const f = fixture();
  try {
    await f.attach(); const old = f.pane.inputGen;
    layout.leaveSessionView({ switchingSession: true });
    const g = fixture({ session: f.session, keep: true });
    g.hold('attach_session'); const attaching = g.attach(); await tick();
    g.frame(old, '\x1b[5n', 9); await g.parsed(); await tick();
    assert.deepEqual(g.calls(), [['\x1b[0n', old]], 'pty_write refuses it once the attachment is replaced');
    await g.release('attach_session'); await attaching;
    assert.notEqual(g.pane.inputGen, old);
  } finally { done(); }
});

test('an input written before its attachment is replaced is neither recalled nor reported cancelled', async () => {
  const f = fixture();
  try {
    await f.attach(); const gen = f.pane.inputGen;
    f.key('a'); await tick();
    layout.leaveSessionView({ switchingSession: true });
    const g = fixture({ session: f.session, keep: true }); await g.attach(); await tick();
    assert.deepEqual(f.calls(), [['a', gen]]);
    assert.deepEqual(f.cancels(), []);
  } finally { done(); }
});

test('ordinary input with no selection and no scroll is written directly, in order', async () => {
  const f = fixture();
  try {
    await f.attach();
    for (const k of ['a', '中文', 'b']) f.key(k);
    assert.deepEqual(f.writes(), ['a', '中文', 'b'], 'No queue: the calls are synchronous');
    assert.equal(f.pane.liveQ ?? null, null);
  } finally { done(); }
});
