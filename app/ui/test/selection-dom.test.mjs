// Drives the production selection coordinator and copy router together. The
// event surface is deterministic; WK event delivery needs the separate smoke.
import test from 'node:test';
import assert from 'node:assert/strict';

class Surface {
  constructor() { this.listeners = new Map(); this.children = []; this.style = {}; this.dataset = {}; }
  addEventListener(type, fn) { const list = this.listeners.get(type) || []; list.push(fn); this.listeners.set(type, list); }
  removeEventListener(type, fn) { this.listeners.set(type, (this.listeners.get(type) || []).filter(f => f !== fn)); }
  fire(type, more = {}) {
    const event = { type, button: 0, buttons: 1, pointerId: 1, detail: 1,
      clientX: 15, clientY: 15, isTrusted: true, pointerType: 'mouse',
      preventDefault() { this.prevented = true; },
      stopImmediatePropagation() { this.stopped = true; }, ...more };
    for (const fn of this.listeners.get(type) || []) { fn(event); if (event.stopped) break; }
    return event;
  }
  appendChild(node) { this.children.push(node); return node; }
  replaceChildren(...nodes) { this.children = nodes; }
  remove() {}
  getBoundingClientRect() { return { left: 0, top: 0, right: 100, bottom: 100, width: 100, height: 100 }; }
  querySelectorAll(selector) { return selector === '.deck-selection-band'
    ? this.children.flatMap(child => child.querySelectorAll?.(selector) || []) : []; }
}
const documentSurface = new Surface();
documentSurface.hidden = false;
documentSurface.hasFocus = () => true;
documentSurface.createElement = () => { const el = new Surface(); el.className = ''; return el; };
documentSurface.getElementById = () => ({ appendChild() {} });
globalThis.document = documentSurface;
const windowSurface = new Surface();
const logs = [], calls = [];
let anchorCell = null, activeCell = null;
windowSurface.__TAURI__ = { core: { invoke: async (name, args) => {
  calls.push({ name, args });
  if (name === 'ui_event') { logs.push(args); return; }
  if (name === 'write_clipboard') return;
  if (name === 'terminal_selection_copy') return { text: 'selected bytes' };
  if (name === 'terminal_selection_finish' && harness?.finishError) throw new Error(harness.finishError);
  if (name === 'terminal_selection_start') {
    anchorCell = [args.anchorRow, args.anchorCol];
    activeCell = [args.activeRow, args.activeCol];
  }
  if (name === 'terminal_selection_update') activeCell = [args.row, args.col];
  if (name === 'terminal_selection_finish' && String(anchorCell) === String(activeCell)) {
    throw new Error('selection-missing-empty');
  }
  if (name === 'terminal_selection_start' || name === 'terminal_selection_update'
      || name === 'terminal_selection_finish') return {
    active: true, selection_present: true, selection_start_row: 1, selection_start_col: 1,
    selection_end_row: 1, selection_end_col: 4, frame_top: 0,
    history_rows: 10, scroll_position: 0, cursor_visible: false,
  };
  return {};
} } };
globalThis.window = windowSurface;
const { wireTerminalSelection } = await import('../js/selection.js');
const { createTerminalCopy } = await import('../js/terminal-clipboard.js');
let harness = null;
const tick = () => new Promise(resolve => setImmediate(resolve));

function fixture() {
  logs.length = 0; calls.length = 0;
  anchorCell = null; activeCell = null;
  const body = new Surface(), screen = new Surface();
  body.querySelector = selector => selector === '.xterm-screen' ? screen
    : selector === '.deck-selection-overlay' ? screen.children.find(x => x.className === 'deck-selection-overlay') : null;
  body.querySelectorAll = selector => selector === '.deck-selection-band'
    ? (screen.children.find(x => x.className === 'deck-selection-overlay')?.children || []) : [];
  screen.querySelector = () => screen.children.find(x => x.className === 'deck-selection-overlay') || null;
  const term = { rows: 10, cols: 10, options: {}, textarea: {}, buffer: { active: { type: 'normal', viewportY: 0 } },
    nativeText: '', listeners: [], hasSelection() { return !!this.nativeText; },
    getSelection() { return this.nativeText; }, getSelectionPosition() { return { start: { x: 1, y: 1 }, end: { x: 2, y: 1 } }; },
    clearSelection() { if (this.nativeText) { this.nativeText = ''; this.listeners.forEach(fn => fn()); } },
    select(text) { this.nativeText = text; this.listeners.forEach(fn => fn()); },
    onSelectionChange(fn) { this.listeners.push(fn); return { dispose: () => { this.listeners = this.listeners.filter(f => f !== fn); } }; },
    onData() { return { dispose() {} }; },
  };
  const pane = { body, term, session: 'private-test', syncSize: async () => true };
  wireTerminalSelection(pane, () => {});
  const copy = createTerminalCopy({ selection: pane.selection, term,
    copySelection: () => pane.selection.copy(), elsewhere: () => ({ count: 0, ageMs: -1 }),
    write: async text => { calls.push({ name: 'clipboard', text }); },
    log: (code, detail, a, b, context) => logs.push({ code, detail, a, b, context }),
    notice: () => {} });
  const key = () => copy({ type: 'keydown', key: 'c', metaKey: true, preventDefault() {} });
  const down = (id = 1, x = 15, detail = 1) => {
    body.fire('pointerdown', { pointerId: id, clientX: x, detail });
    body.fire('mousedown', { pointerId: id, clientX: x, detail });
  };
  const move = (source, id = 1, x = 45) => documentSurface.fire(source, { pointerId: id, clientX: x });
  const up = (id = 1, x = 45) => documentSurface.fire('pointerup', { pointerId: id, clientX: x });
  const settle = async () => { await pane.selection.idle(); await tick(); await pane.selection.idle(); await tick(); };
  harness = { pane, body, term, key, down, move, up, settle, finishError: null,
    bands: () => body.querySelectorAll('.deck-selection-band'),
    copyLogs: () => logs.filter(x => x.code === 'terminal-copy' || x.code === 'terminal-selection'),
    dispose: () => pane.selection.dispose() };
  return harness;
}

test('same-cell, pointermove, compatibility move and pointerup-only paths', async () => {
  const f = fixture();
  f.down(1); f.up(1, 15); await f.settle(); f.key();
  assert.equal(f.pane.selection.forensicReason(), 'same-cell');
  assert.equal(f.pane.selection.hasSelection(), false);
  for (const [id, source] of [[2, 'pointermove'], [3, 'mousemove'], [4, 'up']]) {
    f.down(id);
    if (source !== 'up') f.move(source, id);
    f.up(id); await f.settle();
    assert.equal(f.pane.selection.isFrozen(), true, source);
    assert.equal(f.pane.selection.forensicSnapshot().gesture.promotionSource,
      source === 'pointermove' ? 'pointer' : source === 'mousemove' ? 'compat' : 'up');
    assert.ok(f.bands().length > 0);
  }
  f.dispose();
});

test('frozen selection then ordinary click reports revoking gesture and never copies old bytes', async () => {
  const f = fixture(); f.down(5); f.move('pointermove', 5); f.up(5); await f.settle();
  const oldToken = f.pane.selection.traceContext().selection;
  assert.ok(f.bands().length > 0);
  f.down(6); f.up(6, 15); await f.settle(); f.key(); await tick();
  assert.equal(f.pane.selection.hasSelection(), false);
  assert.equal(f.bands().length, 0);
  assert.equal(f.pane.selection.forensicReason(), 'selection-revoked-pointer');
  const outcome = logs.find(x => x.detail === 'copy-selection-revoked-pointer');
  assert.equal(outcome.context.selection, oldToken);
  assert.equal(outcome.context.gesture, f.pane.selection.forensicSnapshot().gesture.id);
  assert.equal(calls.filter(x => x.name === 'clipboard').length, 0);
  f.dispose();
});

test('move out and back, delayed compatibility move, and immediate copy retain current behavior', async () => {
  const f = fixture();
  f.down(7); f.move('pointermove', 7); f.up(7, 15); await f.settle();
  assert.equal(f.pane.selection.hasSelection(), false);
  assert.equal(f.pane.selection.forensicReason(), 'promoted-empty');
  const before = f.pane.selection.forensicSnapshot().gesture;
  f.move('mousemove', 7); // after pointerup: physicalPointerOwner was released
  assert.equal(f.pane.selection.forensicSnapshot().gesture.id, before.id);
  assert.deepEqual(f.pane.selection.forensicSnapshot().gesture.postUpMousemove,
    { row: 0, col: 3 });
  assert.equal(f.pane.selection.hasSelection(), false);
  f.down(8); f.move('pointermove', 8); f.up(8); f.key(); await f.settle(); await tick();
  assert.equal(logs.filter(x => x.detail === 'keydown-deck').length, 1);
  assert.equal(calls.filter(x => x.name === 'clipboard').length, 1);
  f.dispose();
});

test('native copy and native takeover ending empty keep ownership observable', async () => {
  const f = fixture(); f.term.select('word'); f.key(); await tick();
  assert.equal(logs.filter(x => x.detail === 'keydown-native').length, 1);
  f.finishError = 'selection-missing-empty';
  f.down(9); f.move('pointermove', 9); f.up(9); await f.settle(); f.key();
  assert.equal(f.term.hasSelection(), false);
  assert.equal(f.pane.selection.hasSelection(), false);
  assert.equal(f.pane.selection.forensicReason(), 'promoted-empty');
  assert.ok(logs.some(x => x.detail === 'copy-native-native-end-pointer'));
  f.dispose();
});

test('focus, input, failure and disposal leave no Deck overlay bands', async () => {
  for (const reason of ['focus', 'input', 'dispose']) {
    const f = fixture(); f.down(10); f.move('pointermove', 10); f.up(10); await f.settle();
    assert.ok(f.bands().length > 0);
    if (reason === 'dispose') f.dispose(); else await f.pane.selection.cancel(true, reason);
    assert.equal(f.pane.selection.hasSelection(), false);
    assert.equal(f.bands().length, 0);
    if (reason !== 'dispose') f.dispose();
  }
  const f = fixture(); f.finishError = 'selection-missing-cleared';
  f.down(11); f.move('pointermove', 11); f.up(11); await f.settle();
  assert.equal(f.pane.selection.hasSelection(), false);
  assert.equal(f.bands().length, 0);
  f.dispose();
});

test('focus transfer does not copy the old pane selection', async () => {
  const f = fixture(); f.down(12); f.move('pointermove', 12); f.up(12); await f.settle();
  await f.pane.selection.cancel(true, 'focus');
  f.key(); await tick();
  assert.equal(f.pane.selection.forensicReason(), 'selection-revoked-focus');
  assert.equal(calls.filter(x => x.name === 'clipboard').length, 0);
  assert.equal(f.bands().length, 0);
  f.dispose();
});

test('double and triple click held drags stay native and route their own bytes', async () => {
  for (const detail of [2, 3]) {
    const f = fixture();
    f.down(detail + 20, 15, detail);
    f.term.select(detail === 2 ? 'word' : 'whole line');
    f.move('pointermove', detail + 20, 45);
    f.up(detail + 20, 45); await f.settle(); f.key(); await tick();
    assert.equal(f.pane.selection.hasSelection(), false);
    assert.equal(f.pane.selection.forensicSnapshot().gesture.nativeDragged, true);
    assert.equal(logs.filter(x => x.detail === 'keydown-native').length, 1);
    assert.equal(calls.filter(x => x.name === 'clipboard').length, 1);
    f.dispose();
  }
});
