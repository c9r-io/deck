// Public xterm link adapter; opening actions are supplied by the view owner.
import { terminalLogicalLine, terminalLinkRanges, tokenizeTerminalLinks } from './terminal-links-model.js';

/* A physical press owns a value/range snapshot, not xterm's transient hover
   object. xterm rebuilds that object on every row repaint, even if the text
   is identical. At release we verify the live public buffer, grid, viewport
   and selection guard before opening exactly once. No input is replayed and
   no xterm private API is used. Hover stays synchronous and filesystem-free. */
export function wireTerminalLinks(pane, { openLink, logEvent }) {
  const term = pane.term, host = pane.body;
  let press = null, hovered = null, attempt = 0, lastSlowScan = -Infinity;
  const context = () => ({ ...pane.selection.traceContext(), selection: 0, attempt });
  const log = (detail, a, b, trace = context()) => logEvent('terminal-link', detail, a, b, trace);
  const scan = lineNo => {
    const started = performance.now();
    const { text, positions } = terminalLogicalLine(term, lineNo);
    const links = terminalLinkRanges({ matches: tokenizeTerminalLinks(text), positions, lineNo });
    const elapsed = performance.now() - started;
    // Normal hover is silent. A slow scan is useful in ordinary exports,
    // but continuous repaint cannot flood the log with the same symptom.
    if (elapsed >= 16 && started - lastSlowScan >= 5000) {
      lastSlowScan = started;
      log('scan-slow', Math.ceil(elapsed), text.length);
    }
    return links;
  };
  const cellAt = e => {
    const screen = host.querySelector('.xterm-screen');
    const rect = screen?.getBoundingClientRect();
    if (!rect || rect.width <= 0 || rect.height <= 0
        || e.clientX < rect.left || e.clientX >= rect.right
        || e.clientY < rect.top || e.clientY >= rect.bottom) return null;
    return {
      x: Math.floor((e.clientX - rect.left) * term.cols / rect.width) + 1,
      y: Math.floor((e.clientY - rect.top) * term.rows / rect.height) + 1 + term.buffer.active.viewportY,
    };
  };
  const at = cell => cell && scan(cell.y).find(link => {
    const { start, end } = link.range;
    return (cell.y > start.y || cell.y === start.y && cell.x >= start.x)
      && (cell.y < end.y || cell.y === end.y && cell.x <= end.x);
  });
  const cancelPress = detail => {
    const ended = press;
    press = null;
    if (ended?.link) log(detail, ended.link.text.length, Date.now() - ended.at, ended.trace);
  };
  const down = e => {
    if (e.button !== 0) return;
    cancelPress('cancelled');
    // xterm's higher-priority OSC 8 provider owns an explicit hyperlink.
    // Do not also open a path menu for its display text.
    if (!hovered && host.querySelector('.xterm-screen')?.classList.contains('xterm-cursor-pointer')) return;
    const cell = cellAt(e);
    if (!cell) return;
    attempt++;
    const link = at(cell);
    press = { cell, link, at: Date.now(), trace: context(), moved: false,
      buffer: term.buffer.active, viewport: term.buffer.active.viewportY, cols: term.cols, rows: term.rows };
    if (link) log(link.kind === 'url' ? 'press-url' : 'press-path', link.text.length, e.detail, press.trace);
  };
  const move = e => {
    if (!press) return;
    const cell = cellAt(e);
    if (!cell || cell.x !== press.cell.x || cell.y !== press.cell.y) press.moved = true;
  };
  const up = e => {
    if (e.button !== 0 || !press) return;
    const ended = press;
    press = null; // xterm activate and the document mouseup share one release
    const age = Date.now() - ended.at;
    const outcome = detail => log(detail, ended.link?.text.length || 0, age, ended.trace);
    if (ended.moved || !pane.selection.allowLinkActivation()) {
      if (ended.link) outcome('drag');
      return;
    }
    if (!ended.link) { outcome('miss'); return; }
    if (term.buffer.active !== ended.buffer || term.buffer.active.viewportY !== ended.viewport
        || term.cols !== ended.cols || term.rows !== ended.rows) { outcome('viewport'); return; }
    const cell = cellAt(e);
    if (!cell || cell.x !== ended.cell.x || cell.y !== ended.cell.y) { outcome('outside'); return; }
    const current = at(cell), previous = ended.link;
    if (!current || current.text !== previous.text || current.kind !== previous.kind
        || current.lookback !== previous.lookback
        || current.range.start.x !== previous.range.start.x || current.range.start.y !== previous.range.start.y
        || current.range.end.x !== previous.range.end.x || current.range.end.y !== previous.range.end.y) {
      outcome('changed'); return;
    }
    term.clearSelection();
    openLink(e, current, ended.trace);
    outcome(current.kind === 'url' ? 'menu-url' : 'menu-path');
  };
  const blur = () => cancelPress('cancelled');
  const hidden = () => { if (document.hidden) blur(); };
  const provider = {
    provideLinks(lineNo, cb) {
      // A higher-priority provider can replace a hover without calling the
      // old provider's leave callback. Only this query may establish ownership.
      hovered = null;
      const links = scan(lineNo).map(link => ({ ...link, activate: up,
        hover: () => { hovered = link; },
        leave: () => { if (hovered === link) hovered = null; },
      }));
      cb(links.length ? links : undefined);
    },
  };
  // An observational range query must not mutate the live hover ownership.
  pane.linkProvider = { provideLinks: (lineNo, cb) => cb(scan(lineNo)) };
  const registered = term.registerLinkProvider(provider);
  host.addEventListener('mousedown', down, true);
  document.addEventListener('mousemove', move, true);
  document.addEventListener('mouseup', up);
  document.addEventListener('pointercancel', blur);
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', hidden);
  pane.disposeLinks = () => {
    blur(); registered.dispose();
    host.removeEventListener('mousedown', down, true);
    document.removeEventListener('mousemove', move, true);
    document.removeEventListener('mouseup', up);
    document.removeEventListener('pointercancel', blur);
    window.removeEventListener('blur', blur);
    document.removeEventListener('visibilitychange', hidden);
  };
}
