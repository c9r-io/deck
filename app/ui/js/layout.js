// layout.js — split-tree layout, pane lifecycle, terminal creation, session view
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, dotTitle, duev, inv, listen, setMemChip, state, store, uev } from './state.js';
import { inlineRename, toast } from './dialogs.js';
import { t } from './i18n.js';
import { panes, pollNow, provider, render, renderSidebar, updateSidebarSelection, activeProject } from './board.js';
import { SHELL_FG, acceptGhost, feedMirror, maybeRecordCommand, mountQuickBar, nextShellTitle, renderSuggest, resetSuggest, showLinkCtx, updateGhost, writeClipboard } from './terminal.js';
import { AGENT_HISTORY_VERTICAL_UP, createTerminalPasteTrace, createTerminalResizeCoordinator, createTerminalWheelAccumulator, createTerminalWheelFrameScheduler, isComposingKeyEvent, isPlainShiftKeydown, shouldRouteImeKeydownThroughInput, shQuote, terminalAgentComposerGeometry, terminalAgentHistoryUpRoute, terminalCopyRoute, terminalSelectionWheelRoute, tokenizeTerminalLinks, terminalWheelLines } from './pure.js';
import { toggleQueuePanel } from './scheduler.js';
import { cancelAllTerminalSelections, cancelTerminalSelection, copyTerminalSelection, hasTerminalSelection, terminalSelectionElsewhere, wireTerminalSelection } from './selection.js';
import { getTerminalTheme, onThemeChange, syncThemeIntegrations } from './theme.js';
import { getFontScale, onFontScaleChange, TERMINAL_BASE_FONT_SIZE } from './font-scale.js';
import { registerShortcutAction } from './shortcuts.js';

onThemeChange(({ terminal }) => panes.forEach(pane => { pane.term.options.theme = terminal; }));
onFontScaleChange(scale => {
  panes.forEach(pane => { pane.term.options.fontSize = TERMINAL_BASE_FONT_SIZE * scale; });
  fitAll();
});

/* ----- layout tree helpers ----- */
export const leafOf = sid => ({ type: 'leaf', sid });

export function collectLeaves(n, out = []) {
  if (!n) return out;
  if (n.type === 'leaf') out.push(n.sid);
  else { collectLeaves(n.a, out); collectLeaves(n.b, out); }
  return out;
}

export function splitAt(n, targetSid, dir, newSid, before) {
  if (!n) return n;
  if (n.type === 'leaf') {
    if (n.sid !== targetSid) return n;
    const fresh = leafOf(newSid);
    return {
      type: 'split', dir, ratio: 0.5,
      a: before ? fresh : n,
      b: before ? n : fresh,
    };
  }
  return { ...n, a: splitAt(n.a, targetSid, dir, newSid, before), b: splitAt(n.b, targetSid, dir, newSid, before) };
}

export function removeFromLayout(n, sid) {
  if (!n) return null;
  if (n.type === 'leaf') return n.sid === sid ? null : n;
  const a = removeFromLayout(n.a, sid);
  const b = removeFromLayout(n.b, sid);
  if (!a) return b;
  if (!b) return a;
  return { ...n, a, b };
}

export function b64ToU8(b64) {
  const bin = atob(b64);
  const u8 = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
  return u8;
}
export function strToB64(str) {
  const u8 = new TextEncoder().encode(str);
  let bin = '';
  for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]);
  return btoa(bin);
}

const MAX_LINK_LOGICAL_ROWS = 32;

function terminalLineFillsWidth(line, cols) {
  if (!line || cols < 1) return false;
  const tail = line.getCell(cols - 1);
  if (!tail) return false;
  if (tail.getChars()) return true;
  // The final cell of a width-2 glyph is a zero-width continuation cell.
  if (tail.getWidth() === 0 && cols > 1) {
    const lead = line.getCell(cols - 2);
    return !!lead?.getChars() && lead.getWidth() > 1;
  }
  return false;
}

/* Build one visual logical line with UTF-16-offset → terminal-cell mapping,
   using only xterm's public BufferLine/BufferCell APIs. Live output carries
   xterm's isWrapped bit. A tmux attach/history redraw can lose that bit and
   repaint the same wrap as ordinary rows; a content cell in the final column
   is then the only public continuation signal. Bound the fallback so a dense
   full-screen TUI can never turn one hover into an unbounded buffer scan. */
export function terminalLogicalLine(term, requestedLine) {
  const buffer = term.buffer.active;
  let first = requestedLine - 1;
  while (first > 0 && requestedLine - first < MAX_LINK_LOGICAL_ROWS) {
    const current = buffer.getLine(first);
    const previous = buffer.getLine(first - 1);
    if (!current?.isWrapped && !terminalLineFillsWidth(previous, term.cols)) break;
    first--;
  }
  let last = requestedLine - 1;
  while (last + 1 < buffer.length && last - first + 1 < MAX_LINK_LOGICAL_ROWS) {
    const current = buffer.getLine(last);
    const next = buffer.getLine(last + 1);
    if (!next?.isWrapped && !terminalLineFillsWidth(current, term.cols)) break;
    last++;
  }
  let text = '';
  const positions = [];
  for (let y = first; y <= last; y++) {
    const line = buffer.getLine(y);
    if (!line) continue;
    for (let x = 0; x < term.cols; x++) {
      const cell = line.getCell(x);
      if (!cell || cell.getWidth() === 0) continue;
      const chars = cell.getChars() || ' ';
      const pos = { x: x + 1, endX: x + Math.max(1, cell.getWidth()), y: y + 1 };
      text += chars;
      for (let i = 0; i < chars.length; i++) positions.push(pos);
    }
  }
  const trimmed = text.trimEnd();
  positions.length = trimmed.length;
  return { text: trimmed, positions };
}

/* ----- file drop / image paste → path insertion (Warp-style) ----- */

/* external drag from Finder / the screenshot thumbnail — never true for
   deck's own card/pane drags (those use text/deck-session) */
export function isFileDrag(dt) {
  return !!dt && Array.from(dt.types || []).includes('Files');
}

const MAX_DROP_BYTES = 32 * 1024 * 1024;

/* WKWebView surfaces dropped/pasted files as CONTENT (no usable path), so:
   read the bytes → backend saves them 0600 under ~/.deck/drops → the saved
   path is typed into the pane's session (quoted, no Enter — the user still
   owns submission). */
async function insertDroppedFiles(pane, fileList) {
  const files = Array.from(fileList).slice(0, 4);
  const paths = [];
  for (const f of files) {
    if (f.size > MAX_DROP_BYTES) { toast(t('error.fileLarge')); continue; }
    try {
      const b64 = await new Promise((res, rej) => {
        const r = new FileReader();
        r.onload = () => res(String(r.result).split(',')[1] || '');
        r.onerror = () => rej(new Error('read failed'));
        r.readAsDataURL(f);
      });
      paths.push(await inv('save_dropped_file', { name: f.name || 'pasted.png', dataB64: b64 }));
    } catch (e) {
      toast(t('error.fileAttach'));
    }
  }
  if (!paths.length) return;
  focusPane(pane.session);
  const text = paths.map(shQuote).join(' ') + ' ';
  inv('pty_write', { name: pane.session, dataB64: strToB64(text) })
    .catch(() => uev('pty-write-fail'));
}

/* tmux copy-mode always exposes a copy cursor, even after the live input cell
   has moved below the viewport. xterm's DOM renderer marks that one cell with
   a public CSS class; remove only the cursor marker so the cell's original
   foreground/background classes remain intact. Observe while hidden because
   focus and blink refreshes can rebuild the row without parsing PTY bytes. */
function setScrollCursorVisible(pane, visible) {
  const show = visible !== false;
  if (pane.scrollCursorVisible === show) {
    if (!show) pane.stripScrollCursor?.();
    return;
  }
  pane.scrollCursorVisible = show;
  pane.scrollCursorObserver?.disconnect();
  pane.scrollCursorObserver = null;
  if (show) {
    if (pane.body.isConnected) {
      requestAnimationFrame(() => {
        if (!pane.body.isConnected || pane.scrollCursorVisible === false) return;
        try { pane.term.refresh(pane.term.buffer.active.cursorY, pane.term.buffer.active.cursorY); }
        catch (_) { /* pane was disposed during the frame */ }
      });
    }
    return;
  }
  pane.stripScrollCursor = () => {
    pane.body.querySelectorAll('.xterm-cursor')
      .forEach(cell => cell.classList.remove('xterm-cursor'));
  };
  const rows = pane.body.querySelector('.xterm-rows');
  if (rows) {
    pane.scrollCursorObserver = new MutationObserver(pane.stripScrollCursor);
    pane.scrollCursorObserver.observe(rows, {
      subtree: true, childList: true, attributes: true, attributeFilter: ['class'],
    });
  }
  pane.stripScrollCursor();
}

export function createPane(card) {
  const el = document.createElement('div');
  el.className = 'spane';
  el.innerHTML = `
    <div class="spane-head"><span class="dot ${card.status}"></span><span class="name"></span><span class="hspace"></span><button class="px">✕</button></div>
    <div class="spane-body"></div>`;
  el.querySelector('.name').textContent = card.title;
  el.querySelector('.dot').title = dotTitle(card.status);
  el.querySelector('.px').title = t('session.closePane');
  const body = el.querySelector('.spane-body');
  const session = card.session;

  const term = new Terminal({
    fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace',
    fontSize: TERMINAL_BASE_FONT_SIZE * getFontScale(),
    lineHeight: 1.7,
    cursorBlink: true,
    // Preserve macOS text-input/dead-key semantics. Option is not rewritten
    // to Meta/ESC; terminal Meta remains available through Command shortcuts.
    macOptionIsMeta: false,
    scrollback: 5000,
    allowProposedApi: true,   // registerDecoration (input separators)
    theme: getTerminalTheme(),
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  try {
    /* OSC52: tmux mouse selections land in the system clipboard */
    term.loadAddon(new ClipboardAddon.ClipboardAddon());
  } catch (e) { uev('clipboard-addon-fail'); }
  term.open(body);

  if (!ghostEl) {
    ghostEl = document.createElement('div');
    ghostEl.id = 'ghost';
  }
  /* echo arrives asynchronously — reposition after each parsed write */
  const pane = { sid: card.id, session, el, body, term, fit, seps: [] };
  pane.pasteTrace = createTerminalPasteTrace({
    emit: (detail, length, attempt) => uev('terminal-paste', detail, length, attempt),
  });
  const resize = createTerminalResizeCoordinator((cols, rows) =>
    inv('pty_resize', { name: pane.session, cols, rows }));
  pane.syncSize = () => resize.sync(pane.term.cols, pane.term.rows);
  pane.invalidateSize = () => resize.invalidate();
  wireTerminalSelection(pane, (active, status, phase = {}) => {
    // Copy-mode reaches an endpoint through several cursor motions. Keep
    // xterm's cursor hidden for that short drag window so those internal
    // positions never leak into the user-visible frame.
    setScrollCursorVisible(pane,
      !active || (!phase.dragging && status?.cursor_visible !== false));
    const current = provider.get(pane.sid);
    if (!current) return;
    current.scrolled = active;
    updatePaneChrome(current);
  });

  term.onWriteParsed(() => {
    if (pane.scrollCursorVisible === false) pane.stripScrollCursor?.();
    if (ghostRemainder && attachedName === session) updateGhost();
    positionSeparators(pane);
    pane.selection?.writeParsed();
  });
  term.onScroll(() => positionSeparators(pane));
  panes.set(session, pane);
  // A first session can start after the boot-time theme application, so sync
  // tmux's transient copy-mode highlight once its private server exists.
  syncThemeIntegrations();

  const head = el.querySelector('.spane-head');
  head.addEventListener('mousedown', () => focusPane(session));
  /* pane headers are drag sources: drop on another pane's edge to MOVE it */
  head.draggable = true;
  head.addEventListener('dragstart', ev => {
    ev.dataTransfer.setData('text/deck-session', pane.sid);
  });
  head.addEventListener('dragend', () => { $('dropzone').style.display = 'none'; });
  el.querySelector('.px').onclick = e => {
    e.stopPropagation();
    closePaneBySid(pane.sid);
  };
  body.addEventListener('mousedown', () => { if (attachedName !== session) focusPane(session); });

  /* drag a card (from sidebar or board) onto a pane edge to split (方案 A);
     an EXTERNAL file drag (Finder, the screenshot floating thumbnail) is a
     different gesture: drop anywhere on the pane to attach the file — its
     path is typed into the session, Warp-style */
  el.addEventListener('dragover', e => {
    e.preventDefault();
    if (isFileDrag(e.dataTransfer)) {
      e.dataTransfer.dropEffect = 'copy';
      $('dropzone').style.display = 'none';
      el.classList.add('file-drop');
      return;
    }
    const r = el.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    let zone = null;
    if (x < 0.3) zone = { dir: 'row', before: true, box: [r.left, r.top, r.width / 2, r.height] };
    else if (x > 0.7) zone = { dir: 'row', before: false, box: [r.left + r.width / 2, r.top, r.width / 2, r.height] };
    else if (y < 0.35) zone = { dir: 'col', before: true, box: [r.left, r.top, r.width, r.height / 2] };
    else if (y > 0.65) zone = { dir: 'col', before: false, box: [r.left, r.top + r.height / 2, r.width, r.height / 2] };
    const dz = $('dropzone');
    if (zone) {
      dz.style.display = 'block';
      dz.style.left = zone.box[0] + 'px';
      dz.style.top = zone.box[1] + 'px';
      dz.style.width = zone.box[2] + 'px';
      dz.style.height = zone.box[3] + 'px';
      dz.dataset.dir = zone.dir;
      dz.dataset.before = zone.before;
      dz.dataset.target = pane.sid;
    } else {
      dz.style.display = 'none';
    }
  });
  el.addEventListener('dragleave', () => {
    $('dropzone').style.display = 'none';
    el.classList.remove('file-drop');
  });
  el.addEventListener('drop', e => {
    e.preventDefault();
    el.classList.remove('file-drop');
    const dz = $('dropzone');
    dz.style.display = 'none';
    if (e.dataTransfer.files && e.dataTransfer.files.length) {
      insertDroppedFiles(pane, e.dataTransfer.files);
      return;
    }
    const droppedSid = e.dataTransfer.getData('text/deck-session');
    if (!droppedSid || dz.dataset.target !== pane.sid) return;
    addSplit(pane.sid, dz.dataset.dir, dz.dataset.before === 'true', droppedSid);
  });
  /* ⌘V with an IMAGE on the clipboard (⌃⌘⇧4 screenshots): the native paste
     event carries a file, which xterm's text path would silently drop —
     save it and type its path instead. Text pastes pass through untouched. */
  body.addEventListener('paste', e => {
    const files = e.clipboardData && e.clipboardData.files;
    if (files && files.length) {
      pane.pasteTrace.event('event-file', files.length);
      e.preventDefault();
      e.stopPropagation();
      insertDroppedFiles(pane, files);
      return;
    }
    if (!e.clipboardData) {
      pane.pasteTrace.event('event-unavailable');
      return;
    }
    let textLength = null;
    try { textLength = e.clipboardData.getData('text/plain').length; } catch (_) { /* unavailable */ }
    const detail = textLength == null ? 'event-unavailable'
      : textLength > 0 ? 'event-text' : 'event-empty';
    pane.pasteTrace.event(detail, textLength || 0);
  }, true);

  wireTerminalInput(pane, term, body);
  return pane;
}

/* hairline above each submitted input — separates the user's commands /
   messages from surrounding output. Anchored to the buffer line via a
   marker, so it scrolls with the content. */
/* We draw the lines ourselves: xterm markers track the buffer line (stable
   API), and a plain absolutely-positioned 1px div per marker is placed with
   the same geometry math as the ghost suggestion. xterm's decoration
   renderer proved unreliable (registered but never painted). */
export function addInputSeparator(pane) {
  try {
    const t = pane.term;
    const marker = t.registerMarker(0);
    if (!marker) { if (sepLogged < 5) { sepLogged++; uev('separator', 'no-marker'); } return; }
    const el = document.createElement('div');
    el.style.cssText = 'position:absolute; left:0; right:0; height:1px;' +
      'background:var(--input-separator); pointer-events:none; z-index:4; display:none;';
    pane.body.appendChild(el);
    const entry = { marker, el };
    pane.seps.push(entry);
    if (pane.seps.length > 200) {
      const old = pane.seps.shift();
      old.el.remove();
      try { old.marker.dispose(); } catch (e2) { /* fine */ }
    }
    marker.onDispose(() => {
      el.remove();
      const i = pane.seps.indexOf(entry);
      if (i >= 0) pane.seps.splice(i, 1);
    });
    if (sepLogged < 5) { sepLogged++; uev('separator', 'at', marker.line); }
    positionSeparators(pane);
  } catch (e) {
    if (sepLogged < 5) { sepLogged++; uev('separator', 'fail'); }
  }
}

export function clearSeparators(pane) {
  for (const s of [...pane.seps]) {
    try { s.marker.dispose(); } catch (e) { s.el.remove(); }
  }
  pane.seps.length = 0;
}

export function positionSeparators(pane) {
  if (!pane.seps.length) return;
  const t = pane.term;
  const screen = pane.body.querySelector('.xterm-screen');
  if (!screen) return;
  const bodyRect = pane.body.getBoundingClientRect();
  const sRect = screen.getBoundingClientRect();
  const top0 = sRect.top - bodyRect.top;
  const h = sRect.height / t.rows;
  const viewportY = t.buffer.active.viewportY;
  for (const s of pane.seps) {
    if (s.marker.isDisposed) continue;
    const row = s.marker.line - viewportY;
    if (row < 0 || row >= t.rows) {
      s.el.style.display = 'none';
    } else {
      /* Warp-style breathing room, prompt-aware: prompts that pad with a
         blank line get the hairline centered in that blank band; tight
         prompts (default zsh — text on every row) get it on the row seam,
         inside the lineHeight leading, so it never crosses glyphs. */
      const above = t.buffer.active.getLine(s.marker.line - 1);
      const blankAbove = !above || above.translateToString(true).trim() === '';
      s.el.style.display = 'block';
      s.el.style.top = (top0 + row * h - (blankAbove ? Math.round(h * 0.5) : 1)) + 'px';
    }
  }
}

export function wireTerminalInput(pane, term, host) {
  const session = pane.session;
  const card = () => provider.get(pane.sid);
  let agentHistoryBrowsing = false;

  const agentComposerGeometry = () => {
    const buffer = term.buffer.active;
    const viewport = buffer.viewportY;
    const lines = Array.from({ length: term.rows }, (_, row) =>
      buffer.getLine(viewport + row)?.translateToString(true, 0, 5) || '');
    return terminalAgentComposerGeometry({
      lines, cursorRow: buffer.cursorY, cursorCol: buffer.cursorX,
    });
  };

  /* Apple Pinyin and other macOS IMEs may deliver a printable punctuation
     keydown as keyCode=229 before OR after the corresponding InputEvent.
     xterm 5.5's target keydown handler then enters its deferred textarea-diff
     fallback and can suppress the first committed character. A modifier-only
     Shift keydown triggers the same flag despite carrying no terminal bytes.
     Stop those two non-byte events before xterm, without preventDefault:
     WebKit still performs the native edit and xterm consumes final
     InputEvent.data. The actual Shift+key event retains its shiftKey. Host
     capture runs before xterm's target listener and disappears with the DOM. */
  host.addEventListener('keydown', event => {
    if (event.target !== term.textarea) return;
    if (event.type === 'keydown' && event.metaKey) {
      const key = String(event.key || '').toLowerCase();
      if (key === 'c') uev('terminal-copy', 'key-capture');
      if (key === 'v') pane.pasteTrace.keyCapture();
    }
    const imePrintable = shouldRouteImeKeydownThroughInput(event);
    const plainShift = isPlainShiftKeydown(event);
    if (imePrintable || plainShift) {
      event.stopPropagation();
    }
  }, true);

  /* xterm auto-answers terminal queries (DA `ESC[?..c`, DSR `ESC[..R`,
     OSC `ESC]..BEL`) through onData. Those are not user input — without
     this filter they desync the input mirror on every attach, and the
     first command typed after attaching is never captured. */
  /* Auto-replies, not user input:
     - CSI with a ?/> prefix (DA1/DA2, DECRPM `$y`, mode reports …) — user
       keys never carry those prefixes
     - DSR cursor reports `ESC[..R`, focus events `ESC[I`/`ESC[O`
     - OSC / DCS responses */
  const AUTO_REPLY = /^(?:\x1b\[[?>][0-9;$]*[a-zA-Z]|\x1b\[[0-9;]*R|\x1b\[[IO]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP[^\x1b]*\x1b\\)+$/;
  let odLogged = 0, escLogged = 0;
  term.onData(d => {
    const isAutoReply = AUTO_REPLY.test(d);
    const pasteAttempt = isAutoReply ? null : pane.pasteTrace.onData(d.length);
    /* the input mirror / completion only tracks the focused pane */
    if (!isAutoReply && attachedName === session) {
      if (d.includes('\x1b') && escLogged < 5) {
        escLogged++;
        /* control replies (ESC-prefixed) are loggable; anything else could be
           typed/pasted user text — length only */
        duev('mirror-desync', d.startsWith('\x1b') ? 'esc' : 'plain', d.length);
      }
      const preDesynced = lineBuf === null;
      const completed = feedMirror(d);
      if (completed) maybeRecordCommand(completed);
      /* separator on submit — SHELLS ONLY: markers assume append-scroll
         output; agent TUIs (claude) repaint in place, so a line anchored
         to "the input row" ends up crossing arbitrary repainted content.
         Agent UIs already delineate messages with their own ❯ rows. */
      if (completed || (preDesynced && (d.includes('\r') || d.includes('\n')))) {
        const c = card();
        if (c && SHELL_FG.test(c.fg || '')) addInputSeparator(pane);
      }
      renderSuggest();
      if (odLogged < 3) {
        odLogged++;
        duev('ondata', lineBuf === null ? 'desync' : 'ok', d.length, lineBuf === null ? -1 : lineBuf.length);
      }
    }
    /* typing while the view is frozen in scrollback: leave copy-mode FIRST
       (otherwise tmux eats the keys as copy-mode commands), then write —
       chained so keystroke order is preserved; once the chain drains,
       writes go direct again. Terminal auto-replies never trigger this. */
    const doWrite = bytes => inv('pty_write', { name: session, dataB64: strToB64(bytes) })
      .then(result => {
        if (pasteAttempt) pane.pasteTrace.write(pasteAttempt, true);
        return result;
      })
      .catch(() => {
        uev('pty-write-fail');
        if (pasteAttempt) pane.pasteTrace.write(pasteAttempt, false);
      });
    const cc = card();
    if (!isAutoReply && hasTerminalSelection(pane)) {
      pane.liveQ = cancelTerminalSelection(pane, 'input');
    }
    if (!isAutoReply && cc && cc.scrolled) {
      pane.liveQ = goLive(session);
    }
    if (pane.liveQ) {
      const q = pane.liveQ.then(() => doWrite(d));
      pane.liveQ = q;
      q.then(() => { if (pane.liveQ === q) pane.liveQ = null; });
    } else {
      doWrite(d);
    }
  });
  /* app shortcuts pass through; ⌘C/⌘V are handled here because a menu-less
     macOS app gets no standard edit actions in the webview */
  term.attachCustomKeyEventHandler(e => {
    if (isComposingKeyEvent(e)) return true;
    if (e.type === 'keydown' && e.key === 'ArrowUp'
        && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      const route = terminalAgentHistoryUpRoute({
        foreground: card()?.fg,
        browsing: agentHistoryBrowsing,
        composer: agentComposerGeometry(),
      });
      if (route === 'vertical') {
        e.preventDefault();
        agentHistoryBrowsing = false;
        // Feed the public xterm input path so selection/live-view cleanup and
        // PTY write ordering remain identical to an ordinary physical key.
        term.input(AGENT_HISTORY_VERTICAL_UP);
        return false;
      }
      if (route === 'history') agentHistoryBrowsing = true;
    } else if (e.type === 'keydown' && !/^(?:ArrowUp|ArrowDown|Shift|Control|Alt|Meta)$/.test(e.key)) {
      agentHistoryBrowsing = false;
    }
    /* ⌘V: returning false skips xterm's key handling; the browser then fires
       a native paste event, which xterm's textarea handler feeds into the
       PTY. (navigator.clipboard.readText is permission-blocked in WKWebView —
       the native paste event is the reliable path.) */
    if (e.type === 'keydown' && e.metaKey && String(e.key || '').toLowerCase() === 'v') {
      pane.pasteTrace.keyHandler();
      return false;
    }
    if (e.type === 'keydown' && e.key === 'Escape' && hasTerminalSelection(pane)) {
      e.preventDefault();
      cancelTerminalSelection(pane, 'escape');
      return false;
    }
    const copyRoute = terminalCopyRoute(e, hasTerminalSelection(pane), term.hasSelection());
    if (e.type === 'keydown' && e.metaKey && String(e.key || '').toLowerCase() === 'c') {
      if (copyRoute === 'deck' || copyRoute === 'native') {
        uev('terminal-copy', copyRoute === 'deck' ? 'keydown-deck' : 'keydown-native');
      } else {
        /* Empty-handed ⌘C: say whether a live Deck selection exists in some
           OTHER pane (focus never followed the drag) or nowhere at all.
           `a` counts the other panes holding one, `b` is the youngest one's
           age in ms. Attribution only — the copy still does nothing. */
        const elsewhere = terminalSelectionElsewhere(pane);
        if (elsewhere.count > 0) {
          uev('terminal-copy', 'keydown-elsewhere', elsewhere.count, elsewhere.ageMs);
        } else {
          uev('terminal-copy', 'keydown-none');
        }
      }
    }
    if (copyRoute === 'deck') {
      e.preventDefault();
      copyTerminalSelection(pane)
        .then(text => {
          if (text == null) {
            uev('terminal-copy', 'selection-vanished');
            return null;
          }
          return writeClipboard(text)
            .then(() => uev('terminal-copy', 'success'))
            .catch(error => {
              uev('terminal-copy', 'clipboard-write-failed');
              throw error;
            });
        })
        .catch(() => toast(t('error.copy')));
      return false;
    }
    if (copyRoute === 'native') {
      e.preventDefault();
      const text = term.getSelection();
      if (!text) uev('terminal-copy', 'selection-vanished');
      writeClipboard(text)
        .then(() => uev('terminal-copy', 'success'))
        .catch(() => {
          uev('terminal-copy', 'clipboard-write-failed');
          toast(t('error.copy'));
        });
      return false;
    }
    /* ghost suggestion: Tab or → applies it in place; Esc dismisses */
    if (e.type === 'keydown' && ghostRemainder) {
      if (e.key === 'Tab' || e.key === 'ArrowRight') {
        e.preventDefault();
        acceptGhost();
        return false;
      }
      if (e.key === 'Escape') {
        lineBuf = null;
        freshShell = false;
        renderSuggest();
        return false;
      }
    }
    if (e.type === 'keydown' && e.key === 'Escape'
        && $('quick-bar').style.display === 'flex') {
      lineBuf = null;
      freshShell = false;
      renderSuggest();
      return false;
    }
    return true;
  });

  /* Composition owns the complete preedit→commit chain. Cancel selection
     synchronously in the frontend, serialize backend cleanup before onData,
     and never derive committed characters from KeyboardEvent.key. */
  term.textarea.addEventListener('compositionstart', () => {
    pane.liveQ = pane.selection?.prepareInput() || Promise.resolve();
  }, true);

  /* clickable paths and URLs in terminal output */
  const linkProvider = {
    provideLinks(lineNo, cb) {
      const logical = terminalLogicalLine(term, lineNo);
      const { text, positions } = logical;
      if (!text) return cb(undefined);
      const matches = tokenizeTerminalLinks(text);
      if (!matches.length) return cb(undefined);
      const c = card();
      const cwd = c ? c.dir : HOME;
      const pathMatches = matches.filter(match => match.kind === 'path');
      const validation = pathMatches.length
        ? inv('terminal_paths_exist', { values: pathMatches.map(match => match.value), cwd })
          .catch(() => pathMatches.map(() => false))
        : Promise.resolve([]);
      validation.then(pathResults => {
        if (!host.isConnected) return cb(undefined);
        const validPaths = new Map(pathMatches.map((match, index) => [match, !!pathResults[index]]));
        const links = [];
        for (const match of matches) {
          if (match.kind === 'path' && !validPaths.get(match)) continue;
          const { value, kind } = match;
          const start = positions[match.index];
          const end = positions[match.index + value.length - 1];
          if (!start || !end) continue;
          if (lineNo < start.y || lineNo > end.y) continue;
          links.push({
            range: { start: { x: start.x, y: start.y }, end: { x: end.endX, y: end.y } },
            text: value,
            activate: (e, txt) => {
              if (!pane.selection?.allowLinkActivation()) return;
              e.stopPropagation();
              try { term.clearSelection(); } catch (e2) { /* fine */ }
              const c = card();
              showLinkCtx(e, kind, txt, c ? c.dir : HOME, c ? c.id : null);
            },
          });
        }
        cb(links.length ? links : undefined);
      });
    },
  };
  pane.linkProvider = linkProvider; // public smoke seam: provider activate, not menu helper
  term.registerLinkProvider(linkProvider);

  /* Wheel handling, deck-driven: tmux mouse mode stays OFF. xterm owns
     click/double/triple-click selection; the coordinator owns promoted drags.
     Fractional trackpad deltas are consumed on display frames, with one
     backend request in flight; tmux remains the scrollback authority without
     imposing the old 50ms/20fps timer or dropping each batch's remainder. */
  const wheel = createTerminalWheelAccumulator();
  const wheelFrames = createTerminalWheelFrameScheduler({
    requestFrame: callback => requestAnimationFrame(callback),
    ready: wheel.ready,
    take: wheel.take,
    active: () => host.isConnected,
    run: lines => {
      const route = terminalSelectionWheelRoute({
        tokenSelected: hasTerminalSelection(pane),
        frozen: pane.selection.isFrozen(),
        nativeSelected: term.hasSelection(),
      });
      const request = route === 'frozen'
        ? pane.selection.scroll(lines)
        : route === 'native'
          ? pane.selection.freezeNative().then(adopted => adopted
            ? pane.selection.scroll(lines)
            : inv('scroll_session', { name: session, lines }))
          : inv('scroll_session', { name: session, lines });
      request.then(result => {
        const inMode = typeof result === 'object' ? result?.active : result;
        const cursorVisible = typeof result === 'object' ? result?.cursor_visible : true;
        setScrollCursorVisible(pane, !inMode || cursorVisible !== false);
        const c = card();
        if (c && !!c.scrolled !== !!inMode) { c.scrolled = !!inMode; updatePaneChrome(c); }
      }).catch(() => {});
    },
  });
  host.addEventListener('wheel', e => {
    const mode = term.modes && term.modes.mouseTrackingMode;
    if (mode && mode !== 'none') return;   // app owns the mouse
    e.preventDefault();
    e.stopPropagation();
    wheel.add(terminalWheelLines(e.deltaY, e.deltaMode, term.rows));
    wheelFrames.schedule();
  }, { passive: false, capture: true });

}

/* ----- layout rendering & pane lifecycle ----- */
export function fitAll() {
  requestAnimationFrame(() => {
    panes.forEach(p => {
      try {
        p.fit.fit();
        p.syncSize().catch(() => {});
        if (p.selection) p.selection.resize();
      } catch (e) { /* pane mid-teardown */ }
    });
    if (ghostRemainder) updateGhost();
    panes.forEach(positionSeparators);
  });
}
new ResizeObserver(() => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(fitAll, 80);
}).observe(document.getElementById('terminal'));

export function buildNode(node, parent, grow) {
  if (node.type === 'leaf') {
    const card = provider.get(node.sid);
    const p = card && panes.get(card.session);
    if (!p) return;
    p.el.style.flex = `${grow} 1 0%`;
    parent.appendChild(p.el);
    return;
  }
  const box = document.createElement('div');
  box.style.cssText = `display:flex; min-width:0; min-height:0; flex:${grow} 1 0%;` +
    (node.dir === 'col' ? 'flex-direction:column;' : '');
  const divider = document.createElement('div');
  divider.className = node.dir === 'col' ? 'split-divider-h' : 'split-divider-v';
  buildNode(node.a, box, node.ratio);
  box.appendChild(divider);
  buildNode(node.b, box, 1 - node.ratio);
  /* drag the divider to resize */
  divider.addEventListener('mousedown', e => {
    e.preventDefault();
    const horiz = node.dir !== 'col';
    const move = ev => {
      const r = box.getBoundingClientRect();
      let ratio = horiz ? (ev.clientX - r.left) / r.width : (ev.clientY - r.top) / r.height;
      ratio = Math.min(0.85, Math.max(0.15, ratio));
      node.ratio = ratio;
      box.children[0].style.flex = `${ratio} 1 0%`;
      box.children[2].style.flex = `${1 - ratio} 1 0%`;
    };
    const up = () => {
      document.removeEventListener('mousemove', move);
      document.removeEventListener('mouseup', up);
      fitAll();
    };
    document.addEventListener('mousemove', move);
    document.addEventListener('mouseup', up);
  });
  parent.appendChild(box);
}

export function renderLayout() {
  const host = $('terminal');
  host.innerHTML = '';
  if (layout) buildNode(layout, host, 1);
  fitAll();
}

/* keep pane mini-headers in sync with polling / renames */
export function updatePaneChrome(card) {
  const p = card && panes.get(card.session);
  if (!p) return;
  const dot = p.el.querySelector('.spane-head .dot');
  if (dot) { dot.className = 'dot ' + card.status; dot.title = dotTitle(card.status); }
  const name = p.el.querySelector('.spane-head .name');
  if (name && name.textContent !== card.title) name.textContent = card.title;
  /* scrollback chip: the ONLY visual clue that the view is frozen history
     (the tmux position badge is deliberately off) */
  let chip = p.el.querySelector('.spane-head .scrollchip');
  if (card.scrolled) {
    if (!chip) {
      chip = document.createElement('button');
      chip.className = 'scrollchip';
      chip.textContent = t('session.scrollback');
      chip.title = t('session.scrollbackTitle');
      chip.onclick = e => { e.stopPropagation(); goLive(card.session); };
      p.el.querySelector('.spane-head .px').before(chip);
    }
    chip.textContent = t('session.scrollback');
    chip.title = t('session.scrollbackTitle');
  } else if (chip) {
    chip.remove();
  }
}

/* leave copy-mode → live view; clears the chip immediately (the poll would
   confirm within 2.5s anyway) */
export function goLive(session) {
  const p = panes.get(session);
  if (p) setScrollCursorVisible(p, true);
  const c = p && provider.get(p.sid);
  if (c && c.scrolled) { c.scrolled = false; updatePaneChrome(c); }
  if (p && hasTerminalSelection(p)) return cancelTerminalSelection(p, 'live');
  return inv('scroll_bottom', { name: session }).catch(() => {});
}

export function focusPane(session) {
  const p = panes.get(session);
  if (!p) return;
  const changed = attachedName !== session;
  const previous = changed && attachedName ? panes.get(attachedName) : null;
  if (previous && hasTerminalSelection(previous)) cancelTerminalSelection(previous, 'focus');
  attachedName = session;
  term = p.term;
  panes.forEach(q => q.el.classList.toggle('focus', q === p));
  state.sessionId = p.sid;
  if (changed) resetSuggest(p);
  else mountQuickBar(p);
  if (ghostEl && ghostEl.parentElement !== p.body) p.body.appendChild(ghostEl);
  renderSessionView();
  updateSidebarSelection();
  p.term.focus();
}

/* Returns explicit created/restored state (backend is idempotent: an
   already-live session is success, not an error). Callers use it to decide
   fresh-shell cleanup — clearing history on a restored or merely-live session
   would eat real scrollback. */
export async function ensureAttached(pane) {
  const card = provider.get(pane.sid);
  const outcome = { created: false, restored: false };
  if (!card) return outcome;
  try {
    if (card.status === 'stopped') {
      const started = await inv('start_session', {
        name: card.session, dir: card.dir, cmd: card.cmd,
        restoreShell: !!settings.sessionRestore,
      });
      outcome.created = !!started.created;
      outcome.restored = !!started.restored;
    }
    const gen = await inv('attach_session', { name: card.session, cols: pane.term.cols, rows: pane.term.rows });
    /* max(): the first pty-data event can arrive BEFORE this invoke resolves;
       the handler below already advanced ptyGens then, and regressing it
       would make us drop (and never ACK) the current stream */
    ptyGens.set(card.session, Math.max(ptyGens.get(card.session) || 0, gen));
    if (outcome.restored) toast(t('session.restored'));
  } catch (e) {
    toast(t('error.attach'));
  }
  return outcome;
}

export async function addSplit(targetSid, dir, before, newSid) {
  const card = provider.get(newSid);
  if (!card || state.view !== 'session' || !layout) return;
  if (newSid === targetSid) return;
  /* already open in a pane → this is a MOVE: pluck the leaf and re-insert
     at the drop position; the terminal instance is reused untouched */
  if (panes.has(card.session)) {
    if (!collectLeaves(layout).includes(newSid)) { focusPane(card.session); return; }
    layout = removeFromLayout(layout, newSid);
    layout = splitAt(layout, targetSid, dir, newSid, before);
    renderLayout();
    focusPane(card.session);
    return;
  }
  const pane = createPane(card);
  layout = splitAt(layout, targetSid, dir, newSid, before);
  renderLayout();
  const { created, restored } = await ensureAttached(pane);
  if (created && !restored) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  freshShell = created && !card.cmd.trim();
  focusPane(card.session);
  pollNow();
}

/* close one pane; the session keeps running unless the card itself closes */
export function closePaneBySid(sid, opts = {}) {
  const entry = [...panes.values()].find(p => p.sid === sid);
  if (!entry) return;
  if ($('quick-bar').closest('.spane') === entry.el) resetSuggest();
  if (entry.selection) entry.selection.dispose();
  entry.pasteTrace?.dispose();
  entry.scrollCursorObserver?.disconnect();
  if (opts.detach !== false) inv('detach_session', { name: entry.session }).catch(() => {});
  try { entry.term.dispose(); } catch (e) { /* already gone */ }
  const quickBar = $('quick-bar');
  if (quickBar && entry.el.contains(quickBar)) $('session-view').appendChild(quickBar);
  entry.el.remove();
  panes.delete(entry.session);
  ptyGens.delete(entry.session);
  layout = removeFromLayout(layout, sid);
  if (!layout || !collectLeaves(layout).length) {
    backToBoard();
    return;
  }
  renderLayout();
  if (attachedName === entry.session) {
    const nextSid = collectLeaves(layout)[0];
    const c = provider.get(nextSid);
    if (c) focusPane(c.session);
  }
}

/* 方案 B: split button / ⌘D — pick a session for the new pane */
export function showSplitPicker(dir) {
  if (state.view !== 'session' || !state.sessionId) return;
  const targetSid = state.sessionId;
  const openSids = new Set(collectLeaves(layout));
  const candidates = store.cards.filter(c => !openSids.has(c.id));
  const order = { attention: 0, done: 1, running: 1, waiting: 1, stopped: 2 };
  candidates.sort((a, b) => order[a.status] - order[b.status]);
  const ctx = $('ctx');
  ctx.replaceChildren();
  const label = document.createElement('div');
  label.className = 'ctx-label';
  label.textContent = t('split.choose', { direction: t(dir === 'col' ? 'split.down' : 'split.right') });
  ctx.appendChild(label);
  for (const c of candidates.slice(0, 12)) {
    const button = document.createElement('button');
    button.dataset.sid = c.id;
    const status = document.createElement('span');
    status.className = `split-status ${c.status}`;
    status.textContent = c.status === 'stopped' ? '○'
      : (c.status === 'waiting' || c.status === 'attention') ? '◆' : '●';
    status.title = dotTitle(c.status);
    button.append(status, document.createTextNode(' ' + c.title));
    ctx.appendChild(button);
  }
  ctx.appendChild(document.createElement('hr'));
  const newButton = document.createElement('button');
  newButton.dataset.new = '1';
  newButton.textContent = t('session.newShellHere');
  ctx.appendChild(newButton);
  ctx.onclick = async ev => {
    const sid = ev.target.closest('button') && ev.target.closest('button').dataset.sid;
    const isNew = ev.target.closest('button') && ev.target.closest('button').dataset.new;
    ctx.style.display = 'none';
    if (sid) addSplit(targetSid, dir, false, sid);
    if (isNew) {
      const p = activeProject();
      const focused = provider.get(targetSid);
      const c = await provider.create({
        projectId: p.id,
        columnId: (p.columns.find(x => x.semantic === 'working') || p.columns[0]).id,
        title: nextShellTitle(p),
        cmd: '',
        dir: focused ? focused.dir : HOME,
      });
      addSplit(targetSid, dir, false, c.id);
    }
  };
  const btn = $(dir === 'col' ? 'split-down' : 'split-right');
  const r = btn.getBoundingClientRect();
  ctx.style.display = 'block';
  ctx.style.left = Math.min(r.left, innerWidth - ctx.offsetWidth - 8) + 'px';
  ctx.style.top = (r.bottom + 6) + 'px';
}
$('split-right').onclick = e => { e.stopPropagation(); showSplitPicker('row'); };
$('split-down').onclick = e => { e.stopPropagation(); showSplitPicker('col'); };
registerShortcutAction('splitRight', () => showSplitPicker('row'));
registerShortcutAction('splitDown', () => showSplitPicker('col'));

/* NOTE: listen() requires the core:event permission in
   src-tauri/capabilities/default.json — without it registration is refused
   with a silent promise rejection and the terminal never receives output. */
listen('pty-data', ev => {
  const { name, gen, seq, data } = ev.payload;
  /* flow control: drop a stale attachment's tail (its gate is already
     closed backend-side — no ACK owed); a NEWER gen means our attach invoke
     hasn't resolved yet — accept it and advance, or the first paint is lost */
  const cur = ptyGens.get(name) || 0;
  if (gen < cur) return;
  if (gen > cur) ptyGens.set(name, gen);
  const p = panes.get(name);
  if (p) {
    const u8 = b64ToU8(data);
    rxBytes += u8.length;
    if (rxLogged < 3 || rxLogged % 200 === 0) uev('pty-rx', null, u8.length, rxBytes);
    rxLogged++;
    /* ACK only after xterm has actually consumed the bytes — this is what
       bounds the backend's in-flight window (see pty.rs) */
    p.term.write(u8, () => inv('pty_ack', { name, gen, seq }).catch(() => {}));
  } else {
    /* pane already gone but the stream still current: ACK so the emitter
       reaches its natural end instead of waiting on a window we'll never fill */
    inv('pty_ack', { name, gen, seq }).catch(() => {});
  }
}).catch(() => uev('listen-fail', 'pty-data'));
listen('pty-exit', ev => {
  const pane = panes.get(ev.payload.name);
  if (pane) {
    cancelTerminalSelection(pane, 'exit');
    toast(t('session.ended'));
    pollNow();
  }
}).catch(() => uev('listen-fail', 'pty-exit'));

/* ---------- session view ---------- */
export async function openSession(sid) {
  const card = provider.get(sid);
  if (!card) return;
  /* already open in a pane → just focus it */
  if (state.view === 'session' && panes.has(card.session)) {
    focusPane(card.session);
    return;
  }
  leaveSessionView();
  state.projectId = card.projectId;
  state.view = 'session';
  state.sessionId = sid;
  toggleQueuePanel(false);
  render();
  const pane = createPane(card);
  layout = leafOf(sid);
  renderLayout();
  const { created, restored } = await ensureAttached(pane);
  if (created && !restored) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  freshShell = created && !card.cmd.trim();
  focusPane(card.session);
  /* history feeds both the fresh-shell chips and typed-prefix completion */
  inv('recent_commands', { limit: 50 })
    .then(c => { histCache = c; renderSuggest(); })
    .catch(() => { histCache = []; });
  pollNow();
}

export function leaveSessionView() {
  cancelAllTerminalSelections('leave');
  resetSuggest(null);
  toggleQueuePanel(false);
  const quickBar = $('quick-bar');
  if (quickBar && quickBar.closest('.spane')) $('session-view').appendChild(quickBar);
  panes.forEach(p => {
    if (p.selection) p.selection.dispose();
    p.pasteTrace?.dispose();
    p.scrollCursorObserver?.disconnect();
    inv('detach_session', { name: p.session }).catch(() => {});
    try { p.term.dispose(); } catch (e) { /* fine */ }
    p.el.remove();
  });
  panes.clear();
  layout = null;
  attachedName = null;
  term = null;
}

export function backToBoard() {
  leaveSessionView();
  state.view = 'board';
  state.sessionId = null;
  render();
  pollNow();
}

export function renderSessionView() {
  const s = provider.get(state.sessionId);
  if (!s) { backToBoard(); return; }
  $('sess-dot').className = 'dot ' + s.status;
  $('sess-dot').title = dotTitle(s.status);
  /* back button names the board this card lives on */
  const proj0 = activeProject();
  const col0 = proj0 && proj0.columns.find(c => c.id === s.columnId);
  $('back-label').textContent = col0 ? col0.name : t('app.board');
  const nameEl = $('sess-name');
  nameEl.textContent = s.title;
  nameEl.title = t('session.renameTitle');
  nameEl.ondblclick = () => {
    inlineRename(nameEl, s.title, async v => {
      if (v) await provider.rename(s.id, v);
      renderSessionView();
      renderSidebar();
    });
  };
  setMemChip($('sess-mem'), s);
  $('sess-path').textContent = (s.cmd ? '$ ' + s.cmd + '  ·  ' : '') + s.dir;
}
