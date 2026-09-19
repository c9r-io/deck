import test from 'node:test';
import assert from 'node:assert/strict';
import { wireTerminalLinks } from '../js/terminal-links.js';
import { terminalLogicalLine } from '../js/terminal-links-model.js';

class Surface {
  listeners = new Map();
  addEventListener(type, fn) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(fn); }
  removeEventListener(type, fn) { this.listeners.get(type)?.delete(fn); }
  fire(type, values = {}) {
    const event = { type, button: 0, detail: 1, clientX: 25, clientY: 5, ...values };
    for (const fn of this.listeners.get(type) || []) fn(event);
    return event;
  }
}
function row(text, wrapped = false) {
  const cells = [...text].map(chars => ({ getChars: () => chars, getWidth: () => 1 }));
  return { isWrapped: wrapped, getCell: x => cells[x] || { getChars: () => '', getWidth: () => 1 } };
}
function fixture(text = 'src/main.rs') {
  const doc = new Surface(), win = new Surface(), host = new Surface();
  globalThis.document = doc; globalThis.window = win;
  let cursor = false, allowed = true, provider, disposed = false;
  let lines = [row(text)];
  const screen = { getBoundingClientRect: () => ({ left: 0, top: 0, right: 400, bottom: 30, width: 400, height: 30 }),
    classList: { contains: () => cursor } };
  host.querySelector = () => screen;
  const term = {
    cols: 40, rows: 3, clears: 0,
    buffer: { active: { viewportY: 0, length: 1, getLine: i => lines[i] } },
    registerLinkProvider: p => { provider = p; return { dispose: () => { disposed = true; } }; },
    clearSelection() { this.clears++; },
  };
  const pane = { term, body: host, selection: { traceContext: () => ({ run: 1, pane: 2, selection: 3 }), allowLinkActivation: () => allowed } };
  const opened = [], logs = [];
  wireTerminalLinks(pane, { openLink: (event, link, trace) => opened.push({ link, trace }), logEvent: (...args) => logs.push(args) });
  return { doc, win, host, term, pane, opened, logs, screen,
    repaint: text => { lines = [row(text)]; },
    allow: value => { allowed = value; }, cursor: value => { cursor = value; },
    disposed: () => disposed,
    query: () => { let links; provider.provideLinks(1, value => { links = value; }); return links; },
    down: values => host.fire('mousedown', values), up: values => doc.fire('mouseup', values),
  };
}

test('a same-text repaint keeps the physical link press and opens only once across both release routes', () => {
  const f = fixture(); const [old] = f.query(); old.hover(); f.down();
  f.repaint('src/main.rs'); const [fresh] = f.query(); fresh.hover(); old.leave();
  const release = { type: 'mouseup', button: 0, clientX: 25, clientY: 5 };
  fresh.activate(release); f.up();
  assert.equal(f.opened.length, 1); assert.equal(f.term.clears, 1);
  assert.equal(f.opened[0].link.text, 'src/main.rs');
  assert.deepEqual(f.opened[0].trace, { run: 1, pane: 2, selection: 0, attempt: 1 });
  assert.ok(!JSON.stringify(f.logs).includes('src/main.rs'), 'diagnostics contain no link text');
  f.pane.disposeLinks(); assert.equal(f.disposed(), true);
  f.down(); f.up(); assert.equal(f.opened.length, 1);
  assert.ok([...f.doc.listeners.values()].every(set => set.size === 0));
});

test('changed content, buffer, viewport, grid and release cell revoke the old link press', () => {
  for (const change of ['text', 'buffer', 'viewport', 'cols', 'rows', 'outside', 'no-screen', 'zero-grid']) {
    const f = fixture(); f.down();
    if (change === 'text') f.repaint('src/else.rs');
    if (change === 'buffer') f.term.buffer.active = { ...f.term.buffer.active };
    if (change === 'viewport') f.term.buffer.active.viewportY++;
    if (change === 'cols') f.term.cols++;
    if (change === 'rows') f.term.rows++;
    if (change === 'no-screen') f.host.querySelector = () => null;
    if (change === 'zero-grid') f.screen.getBoundingClientRect = () => ({ width: 0, height: 0 });
    f.up(change === 'outside' ? { clientX: 500 } : {});
    assert.deepEqual(f.opened, [], change); f.pane.disposeLinks();
  }
});

test('drag, selection ownership, window loss and pointer cancellation cannot open a menu', () => {
  for (const change of ['drag', 'selection', 'blur', 'hidden', 'cancel']) {
    const f = fixture(); f.down();
    if (change === 'drag') { f.doc.fire('mousemove', { clientX: 55 }); f.doc.fire('mousemove'); }
    if (change === 'selection') f.allow(false);
    if (change === 'blur') f.win.fire('blur');
    if (change === 'hidden') { f.doc.hidden = true; f.doc.fire('visibilitychange'); }
    if (change === 'cancel') f.doc.fire('pointercancel');
    f.up(); assert.deepEqual(f.opened, [], change); f.pane.disposeLinks();
  }
});

test('OSC 8 keeps ownership; observational queries do not steal the active path hover', () => {
  const f = fixture(); f.cursor(true); f.down(); f.up(); assert.deepEqual(f.opened, []);
  const [link] = f.query(); link.hover();
  f.pane.linkProvider.provideLinks(1, links => assert.equal(links.length, 1));
  f.down(); f.up(); assert.equal(f.opened.length, 1);
  link.leave(); f.down(); f.up(); assert.equal(f.opened.length, 1);
  f.pane.disposeLinks();
});

test('plain text and non-primary/outside presses are harmless; URLs use the same lifecycle', () => {
  const f = fixture('plain text'); assert.equal(f.query(), undefined);
  f.down(); f.up(); f.down({ button: 2 }); f.up({ button: 2 });
  f.down({ clientX: -1 }); f.up(); assert.deepEqual(f.opened, []);
  f.repaint('https://example.com/a'); f.doc.fire('visibilitychange'); f.down(); f.up();
  assert.equal(f.opened[0].link.kind, 'url'); f.pane.disposeLinks();
});

test('logical lines join wraps, map UTF-16 and wide glyph cells, and bound dense screens', () => {
  const lines = [row('abc'), row('def', true), row('last')];
  const term = { cols: 3, buffer: { active: { length: lines.length, getLine: i => lines[i] } } };
  assert.equal(terminalLogicalLine(term, 2).text, 'abcdeflas');
  const wide = [
    { getChars: () => '😀', getWidth: () => 2 },
    { getChars: () => '', getWidth: () => 0 },
    { getChars: () => 'x', getWidth: () => 1 },
  ];
  term.buffer.active = { length: 1, getLine: () => ({ getCell: i => wide[i] }) };
  const logical = terminalLogicalLine(term, 1);
  assert.equal(logical.text, '😀x');
  assert.deepEqual(logical.positions.map(p => [p.x, p.endX]), [[1, 2], [1, 2], [3, 3]]);
  let calls = 0;
  term.buffer.active = { length: 1000, getLine: () => { calls++; return row('abc', true); } };
  assert.equal(terminalLogicalLine(term, 500).text.length, 32 * 3);
  assert.ok(calls < 100, 'one hover reads only the bounded logical line');
});
