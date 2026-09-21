// layout.js — split-tree layout, pane lifecycle, terminal creation, session view
// Leaving or refocusing ends the voice recording; each voice paste briefly
// owns the target's user input.
// Part of deck's no-build frontend: native ES modules, no bundler.
// Read receipts require a successful attach to the still-visible pane.
// Split buttons and shortcuts open the shared menu before start/attach; its
// DOM element must stay distinct from the imported runtime state (`ctx`).
// Attention navigation passes allowStart:false: attaching never creates a shell.
// A pane that is already open is only focused: a detached pane (shell exited,
// attach failed) is never re-attached or restarted by a click; exit retirement
// in board.js owns it. A pty-exit may land before its attach reply — the reply
// never marks a pane attached once its generation has exited. Each pane is
// synchronously fitted before attach and confirms that grid afterwards;
// the asynchronous layout RAF must never attach tmux at xterm's 80x24 default.
// A stopped card
// reopens as its shell: the launch command is sent only while the card's
// durable `launched` flag is false (pure.js startCommand). A caller that
// already started the session (provider.createStarted) passes that outcome
// as `opts.started`, so fresh-shell chips and the first-prompt cleanup still
// follow the real start and not the idempotent re-attach. The fresh-shell
// flag is set AFTER focusPane (which resets suggestion state on a focus
// change): a new empty shell offers its recent-command chips.
// This module composes terminal-clipboard.js and terminal-links.js with pane
// selection, diagnostics and context-menu callbacks; those adapters own copy
// routing and link gestures. Pane teardown disposes their link listeners.
import { $, ctx, dotTitle, duev, inv, listen, setMemChip, state, store, uev } from './state.js';
import { choiceDialog, confirmDialog, inlineRename, toast } from './dialogs.js';
import { t } from './i18n.js';
import { markSessionSeen, panes, pollNow, provider, render, renderSidebar, updateSidebarSelection, activeProject } from './board.js';
import { SHELL_FG, acceptGhost, feedMirror, maybeRecordCommand, mountQuickBar, nextShellTitle, renderSuggest, resetSuggest, showLinkCtx, updateGhost } from './terminal.js';
import { AGENT_HISTORY_VERTICAL_UP, collapseHome, isNotDirectoryError, mcpErrorKey, newSessionColumn, startCommand, createTerminalResizeCoordinator, createTerminalWheelAccumulator, createTerminalWheelFrameScheduler, isComposingKeyEvent, isPlainShiftKeydown, isTerminalAutoReply, scrollResultView, shouldRouteImeKeydownThroughInput, shQuote, terminalAgentComposerGeometry, terminalAgentHistoryUpRoute, terminalSelectionWheelRoute, terminalWheelLines } from './pure.js';
import { toggleQueuePanel } from './scheduler.js';
import { cancelAllTerminalSelections, cancelTerminalSelection, copyTerminalSelection, hasTerminalSelection, terminalSelectionElsewhere, wireTerminalSelection } from './selection.js';
import { getTerminalTheme, onThemeChange, syncThemeIntegrations } from './theme.js';
import { b64ToU8, strToB64 } from './terminal-bytes.js';
import { wireTerminalLinks } from './terminal-links.js';
import { createTerminalCopy, writeClipboard } from './terminal-clipboard.js';
import { getFontScale, onFontScaleChange, TERMINAL_BASE_FONT_SIZE } from './font-scale.js';
import { registerShortcutAction } from './shortcuts.js';
import { showAttention } from './attention.js';

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

  if (!ctx.ghostEl) {
    ctx.ghostEl = document.createElement('div');
    ctx.ghostEl.id = 'ghost';
  }
  /* echo arrives asynchronously — reposition after each parsed write */
  const pane = { sid: card.id, session, el, body, term, fit, seps: [] };
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
    if (ctx.ghostRemainder && ctx.attachedName === session) updateGhost();
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
  body.addEventListener('mousedown', () => { if (ctx.attachedName !== session) focusPane(session); });

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
      e.preventDefault();
      e.stopPropagation();
      insertDroppedFiles(pane, files);
    }
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
    if (!marker) { if (ctx.sepLogged < 5) { ctx.sepLogged++; uev('separator', 'no-marker'); } return; }
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
    positionSeparators(pane);
  } catch (e) {
    if (ctx.sepLogged < 5) { ctx.sepLogged++; uev('separator', 'fail'); }
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
    const imePrintable = shouldRouteImeKeydownThroughInput(event);
    const plainShift = isPlainShiftKeydown(event);
    if (imePrintable || plainShift) {
      event.stopPropagation();
    }
  }, true);

  let odLogged = 0, escLogged = 0;
  term.onData(d => {
    /* xterm's auto-answers to terminal queries are not user input */
    const isAutoReply = isTerminalAutoReply(d);
    if (!isAutoReply && ctx.voiceDelivering === session) return;
    /* the input mirror / completion only tracks the focused pane */
    if (!isAutoReply && ctx.attachedName === session) {
      if (d.includes('\x1b') && escLogged < 5) {
        escLogged++;
        /* control replies (ESC-prefixed) are loggable; anything else could be
           typed/pasted user text — length only */
        duev('mirror-desync', d.startsWith('\x1b') ? 'esc' : 'plain', d.length);
      }
      const preDesynced = ctx.lineBuf === null;
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
        duev('ondata', ctx.lineBuf === null ? 'desync' : 'ok', d.length, ctx.lineBuf === null ? -1 : ctx.lineBuf.length);
      }
    }
    /* typing while the view is frozen in scrollback: leave copy-mode FIRST
       (otherwise tmux eats the keys as copy-mode commands), then write —
       chained so keystroke order is preserved; once the chain drains,
       writes go direct again. Terminal auto-replies never trigger this. */
    const doWrite = bytes => inv('pty_write', { name: session, dataB64: strToB64(bytes) })
      .catch(() => { uev('pty-write-fail'); });
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
  const copyKey = createTerminalCopy({
    selection: pane.selection, term,
    copySelection: () => copyTerminalSelection(pane),
    elsewhere: () => terminalSelectionElsewhere(pane),
    write: writeClipboard, log: uev, notice: key => toast(t(key)),
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
      return false;
    }
    if (e.type === 'keydown' && e.key === 'Escape' && hasTerminalSelection(pane)) {
      e.preventDefault();
      cancelTerminalSelection(pane, 'escape');
      return false;
    }
    if (copyKey(e)) return false;
    /* ghost suggestion: Tab or → applies it in place; Esc dismisses */
    if (e.type === 'keydown' && ctx.ghostRemainder) {
      if (e.key === 'Tab' || e.key === 'ArrowRight') {
        e.preventDefault();
        acceptGhost();
        return false;
      }
      if (e.key === 'Escape') {
        ctx.lineBuf = null;
        ctx.freshShell = false;
        renderSuggest();
        return false;
      }
    }
    if (e.type === 'keydown' && e.key === 'Escape'
        && $('quick-bar').style.display === 'flex') {
      ctx.lineBuf = null;
      ctx.freshShell = false;
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

  wireTerminalLinks(pane, {
    logEvent: uev,
    openLink: (event, link, trace) => {
      const c = card();
      showLinkCtx(event, link.kind, link.text, c ? c.dir : ctx.HOME, c ? c.id : null, link.lookback, trace);
    },
  });

  /* Wheel handling, deck-driven: tmux mouse mode stays OFF. xterm owns
     double/triple-click selection and held multi-click drags; the coordinator
     owns promoted single-click drags. A held native drag must not be frozen
     halfway through by wheel adoption.
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
      if (pane.selection.isNativeDragging()) {
        term.scrollLines(lines);
        return;
      }
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
        const { inMode, cursorVisible } = scrollResultView(result);
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
    if (ctx.ghostRemainder) updateGhost();
    panes.forEach(positionSeparators);
  });
}

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
  if (ctx.layout) buildNode(ctx.layout, host, 1);
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
  const changed = ctx.attachedName !== session;
  const previous = changed && ctx.attachedName ? panes.get(ctx.attachedName) : null;
  if (previous && hasTerminalSelection(previous)) cancelTerminalSelection(previous, 'focus');
  ctx.attachedName = session;
  ctx.term = p.term;
  panes.forEach(q => q.el.classList.toggle('focus', q === p));
  state.sessionId = p.sid;
  if (changed) resetSuggest(p);
  else mountQuickBar(p);
  if (ctx.ghostEl && ctx.ghostEl.parentElement !== p.body) p.body.appendChild(ctx.ghostEl);
  renderSessionView();
  updateSidebarSelection();
  p.term.focus();
  window.dispatchEvent(new Event('deck-voice-session-changed'));
}

/* the attachment's stream ended: the pane keeps its transcript but is no
   longer viewing; the poll decides whether the card retires */
function paneExited(pane, gen) {
  window.dispatchEvent(new CustomEvent('deck-voice-target-exit', { detail: pane.session }));
  pane.exitedGen = gen;
  pane.attached = false;
  cancelTerminalSelection(pane, 'exit');
  toast(t('session.ended'));
  pollNow();
}

/* Returns explicit created/restored state (backend is idempotent: an
   already-live session is success, not an error). Callers use it to decide
   fresh-shell cleanup — clearing history on a restored or merely-live session
   would eat real scrollback. */
export function ensureAttached(pane, opts = {}) {
  if (!pane.attachPromise) pane.attachPromise = attachPane(pane, opts).finally(() => { pane.attachPromise = null; });
  return pane.attachPromise;
}
async function attachPane(pane, { allowStart = true } = {}) {
  const card = provider.get(pane.sid);
  const outcome = { created: false, restored: false, attached: false, commandSent: false };
  if (!card || ctx.tmuxRestarting) return outcome;
  try {
    if (card.status === 'stopped' && allowStart) {
      // A managed runner may only be created through the durable MCP/Board
      // transaction. Never turn a stopped MCP card into an ordinary shell.
      if (card.origin?.source === 'mcp') return outcome;
      const cmd = startCommand(card);
      const started = await inv('start_session', {
        name: card.session, dir: card.dir, cmd,
        restoreShell: !!ctx.settings.sessionRestore,
      });
      outcome.created = !!started.created;
      outcome.restored = !!started.restored;
      outcome.commandSent = outcome.created && !!cmd;
      if (outcome.commandSent) await provider.markLaunched(card.id);
    }
    if (ctx.tmuxRestarting || panes.get(card.session) !== pane) return outcome;
    /* renderLayout schedules its fit on RAF, but a fast restore/start can
       reach attach first and otherwise shrink tmux to xterm's 80x24 default.
       Fit synchronously while the mounted pane is still the intended owner,
       then confirm the same grid after attach so an earlier pre-attach resize
       rejection cannot remain the resize coordinator's last word. */
    pane.fit.fit();
    const gen = await inv('attach_session', { name: card.session, cols: pane.term.cols, rows: pane.term.rows });
    pane.invalidateSize();
    await pane.syncSize();
    /* max(): the first pty-data event can arrive BEFORE this invoke resolves;
       the handler below already advanced ptyGens then, and regressing it
       would make us drop (and never ACK) the current stream */
    ctx.ptyGens.set(card.session, Math.max(ctx.ptyGens.get(card.session) || 0, gen));
    /* the stream can END before this invoke resolves too (shell exits at
       once): marking the pane attached now would grant a read receipt on a
       dead stream. Either the exit already ran (its gen was current) or it
       is parked on the pane waiting for this reply to name its generation */
    if (pane.exitedGen === gen) return outcome;
    const parked = pane.pendingExit;
    pane.pendingExit = null;
    if (parked === gen) { paneExited(pane, gen); return outcome; }
    if (panes.get(card.session) === pane && state.view === 'session') {
      pane.attached = true;
      pane.attachedGen = gen;
      outcome.attached = true;
      markSessionSeen(card.id);
    }
    if (outcome.restored) toast(t('session.restored'));
  } catch (e) {
    toast(t('error.attach'));
  }
  return outcome;
}

export async function addSplit(targetSid, dir, before, newSid, opts = {}) {
  if (ctx.tmuxRestarting) return;
  const card = provider.get(newSid);
  if (!card || state.view !== 'session' || !ctx.layout) return;
  if (newSid === targetSid) return;
  /* already open in a pane → this is a MOVE: pluck the leaf and re-insert
     at the drop position; the terminal instance is reused untouched */
  if (panes.has(card.session)) {
    markSessionSeen(newSid);
    if (!collectLeaves(ctx.layout).includes(newSid)) { focusPane(card.session); return; }
    ctx.layout = removeFromLayout(ctx.layout, newSid);
    ctx.layout = splitAt(ctx.layout, targetSid, dir, newSid, before);
    renderLayout();
    focusPane(card.session);
    return;
  }
  const pane = createPane(card);
  ctx.layout = splitAt(ctx.layout, targetSid, dir, newSid, before);
  renderLayout();
  const { created, restored, commandSent } = startOutcome(await ensureAttached(pane), opts.started);
  if (created && !restored) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  focusPane(card.session);
  /* AFTER focusPane: focusing a new pane resets the suggestion state, so the
     fresh-shell flag must be the last word (it was cleared here since v0.4.0) */
  ctx.freshShell = created && !commandSent;
  renderSuggest();
  pollNow();
}

/* the attach's own outcome merged with a start the caller already did */
function startOutcome(attach, started) {
  const s = started || {};
  return {
    ...attach,
    created: !!(attach.created || s.created),
    restored: !!(attach.restored || s.restored),
    commandSent: !!(attach.commandSent || s.commandSent),
  };
}

/* the split picker's "new shell here": the focused pane's directory, never
   a command; started before the card exists, like every creation */
async function newShellInSplit(targetSid, dir, cwd) {
  const p = activeProject();
  if (!p) return;
  const start = async where => provider.createStarted({
    projectId: p.id, columnId: newSessionColumn(p).id, title: nextShellTitle(p), cmd: '', dir: where,
  });
  try {
    const { card, started } = await start(cwd);
    addSplit(targetSid, dir, false, card.id, { started });
  } catch (error) {
    if (!isNotDirectoryError(error)) { toast(t('terminal.createFailed')); return; }
    const choice = await choiceDialog(t('terminal.dirUnavailable', { dir: collapseHome(cwd, ctx.HOME) }),
      [{ id: 'home', label: t('terminal.newHomeShell'), primary: true }]);
    if (choice !== 'home') return;
    try {
      const { card, started } = await start(ctx.HOME);
      addSplit(targetSid, dir, false, card.id, { started });
    } catch (_) { toast(t('terminal.createFailed')); }
  }
}

/* close one pane; the session keeps running unless the card itself closes */
/* whether a pane currently shows `session` (the user can see and type
   into it) */
export const hasPane = session => panes.has(session);

export function closePaneBySid(sid, opts = {}) {
  const entry = [...panes.values()].find(p => p.sid === sid);
  if (!entry) return;
  window.dispatchEvent(new CustomEvent('deck-voice-target-exit', { detail: entry.session }));
  if ($('quick-bar').closest('.spane') === entry.el) resetSuggest();
  entry.disposeLinks?.();
  if (entry.selection) entry.selection.dispose();
  entry.scrollCursorObserver?.disconnect();
  if (opts.detach !== false) inv('detach_session', { name: entry.session }).catch(() => {});
  try { entry.term.dispose(); } catch (e) { /* already gone */ }
  const quickBar = $('quick-bar');
  if (quickBar && entry.el.contains(quickBar)) $('terminal-host').appendChild(quickBar);
  entry.el.remove();
  panes.delete(entry.session);
  ctx.ptyGens.delete(entry.session);
  ctx.layout = removeFromLayout(ctx.layout, sid);
  if (!ctx.layout || !collectLeaves(ctx.layout).length) {
    backToBoard();
    return;
  }
  renderLayout();
  if (ctx.attachedName === entry.session) {
    const nextSid = collectLeaves(ctx.layout)[0];
    const c = provider.get(nextSid);
    if (c) focusPane(c.session);
  }
}

/* 方案 B: split button / ⌘D — pick a session for the new pane */
export function showSplitPicker(dir) {
  if (state.view !== 'session' || !state.sessionId) return;
  const targetSid = state.sessionId;
  const openSids = new Set(collectLeaves(ctx.layout));
  const candidates = store.cards.filter(c => !openSids.has(c.id));
  const order = { attention: 0, done: 1, running: 1, waiting: 1, stopped: 2 };
  candidates.sort((a, b) => order[a.status] - order[b.status]);
  const home = ctx.HOME;
  const menu = $('ctx');
  menu.replaceChildren();
  menu.onkeydown = null;
  const label = document.createElement('div');
  label.className = 'ctx-label';
  label.textContent = t('split.choose', { direction: t(dir === 'col' ? 'split.down' : 'split.right') });
  menu.appendChild(label);
  for (const c of candidates.slice(0, 12)) {
    const button = document.createElement('button');
    button.dataset.sid = c.id;
    const status = document.createElement('span');
    status.className = `split-status ${c.status}`;
    status.textContent = c.status === 'stopped' ? '○'
      : (c.status === 'waiting' || c.status === 'attention') ? '◆' : '●';
    status.title = dotTitle(c.status);
    button.append(status, document.createTextNode(' ' + c.title));
    menu.appendChild(button);
  }
  menu.appendChild(document.createElement('hr'));
  const newButton = document.createElement('button');
  newButton.dataset.new = '1';
  newButton.textContent = t('session.newShellHere');
  menu.appendChild(newButton);
  menu.onclick = async ev => {
    const sid = ev.target.closest('button') && ev.target.closest('button').dataset.sid;
    const isNew = ev.target.closest('button') && ev.target.closest('button').dataset.new;
    menu.style.display = 'none';
    if (sid) addSplit(targetSid, dir, false, sid);
    if (isNew) {
      const focused = provider.get(targetSid);
      newShellInSplit(targetSid, dir, focused ? focused.dir : home);
    }
  };
  const btn = $(dir === 'col' ? 'split-down' : 'split-right');
  const r = btn.getBoundingClientRect();
  menu.style.display = 'block';
  menu.style.left = Math.min(r.left, innerWidth - menu.offsetWidth - 8) + 'px';
  menu.style.top = (r.bottom + 6) + 'px';
}

/* NOTE: listen() requires the core:event permission in
   src-tauri/capabilities/default.json — without it registration is refused
   with a silent promise rejection and the terminal never receives output. */

/* ---------- session view ---------- */
export async function openSession(sid, opts = {}) {
  if (ctx.tmuxRestarting) return false;
  const card = provider.get(sid);
  if (!card) return;
  ctx.attentionReturn = opts.attentionReturn || (state.view === 'session' ? ctx.attentionReturn : null);
  /* already open in a pane → just focus it. A detached pane stays detached:
     re-attaching a dead session or restarting a card mid-retirement is not
     what a click means */
  if (state.view === 'session' && panes.has(card.session)) {
    const pane = panes.get(card.session);
    markSessionSeen(sid);
    focusPane(card.session);
    return !!pane.attached;
  }
  leaveSessionView({ switchingSession: true });
  state.projectId = card.projectId;
  state.view = 'session';
  state.sessionId = sid;
  toggleQueuePanel(false);
  render();
  const pane = createPane(card);
  ctx.layout = leafOf(sid);
  renderLayout();
  const { created, restored, attached, commandSent } = startOutcome(await ensureAttached(pane, opts), opts.started);
  if (created && !restored) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  if (panes.get(card.session) !== pane || state.view !== 'session') return false;
  focusPane(card.session);
  /* AFTER focusPane: focusing a new pane resets the suggestion state, so the
     fresh-shell flag must be the last word (it was cleared here since v0.4.0) */
  ctx.freshShell = created && !commandSent;
  /* history feeds both the fresh-shell chips and typed-prefix completion */
  inv('recent_commands', { limit: 50 })
    .then(c => { ctx.histCache = c; renderSuggest(); })
    .catch(() => { ctx.histCache = []; });
  pollNow();
  return attached;
}

export function leaveSessionView({ switchingSession = false, detach = true } = {}) {
  window.dispatchEvent(new CustomEvent('deck-session-leave', { detail: { switchingSession } }));
  cancelAllTerminalSelections('leave');
  resetSuggest(null);
  toggleQueuePanel(false);
  const quickBar = $('quick-bar');
  if (quickBar && quickBar.closest('.spane')) $('terminal-host').appendChild(quickBar);
  panes.forEach(p => {
    p.disposeLinks?.();
    if (p.selection) p.selection.dispose();
    p.scrollCursorObserver?.disconnect();
    if (detach) inv('detach_session', { name: p.session }).catch(() => {});
    try { p.term.dispose(); } catch (e) { /* fine */ }
    p.el.remove();
  });
  panes.clear();
  ctx.layout = null;
  ctx.attachedName = null;
  ctx.term = null;
}

export function backToBoard(opts = {}) {
  if (!opts.home && ctx.attentionReturn) { showAttention(true); return; }
  ctx.attentionReturn = null;
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
  $('back-label').textContent = ctx.attentionReturn ? t('attention.title') : col0 ? col0.name : t('app.board');
  $('back-btn').title = t(ctx.attentionReturn ? 'attention.back' : 'session.back');
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
  $('queue-btn').disabled = s.origin?.source === 'mcp';
  $('voice-btn').disabled = s.origin?.source === 'mcp';
  const mcp = $('mcp-control-btn');
  const grant = $('mcp-grant-btn');
  mcp.hidden = s.origin?.source !== 'mcp';
  grant.hidden = mcp.hidden;
  if (!mcp.hidden) {
    const cardId = s.id;
    inv('mcp_session_ui', { cardId }).then(status => {
      if (provider.get(cardId) !== s || state.sessionId !== cardId || !status.managed) return;
      mcp.dataset.human = String(status.humanControl === true);
      const action = t(status.humanControl ? 'mcp.return' : 'mcp.takeover');
      const task = status.stale ? t('mcp.stale') : (status.jobState || t('mcp.noJob'));
      mcp.textContent = t('mcp.sessionStatus', { client: status.clientName || 'MCP', task, action });
      mcp.title = [
        t(status.stale ? 'mcp.staleHint' : (status.activeJob ? 'mcp.activeJob' : 'mcp.idle')),
        status.recentError || '',
      ].filter(Boolean).join(' · ');
      grant.dataset.active = String(status.executionGrantActive === true);
      grant.textContent = status.executionGrantActive
        ? t('mcp.revokeExecution')
        : t('mcp.approveExecution');
      grant.title = status.executionGrantActive && status.executionExpiresAt
        ? t('mcp.executionUntil', { time: new Date(status.executionExpiresAt).toLocaleTimeString() })
        : t('mcp.executionRequired');
    }).catch(() => {});
  }
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initLayout() {
  onThemeChange(({ terminal }) => panes.forEach(pane => { pane.term.options.theme = terminal; }));

  onFontScaleChange(scale => {
    panes.forEach(pane => { pane.term.options.fontSize = TERMINAL_BASE_FONT_SIZE * scale; });
    fitAll();
  });

  new ResizeObserver(() => {
    clearTimeout(ctx.resizeTimer);
    ctx.resizeTimer = setTimeout(fitAll, 80);
  }).observe(document.getElementById('terminal'));

  $('split-right').onclick = e => { e.stopPropagation(); showSplitPicker('row'); };

  $('split-down').onclick = e => { e.stopPropagation(); showSplitPicker('col'); };

  $('mcp-control-btn').onclick = async e => {
    e.stopPropagation();
    const cardId = state.sessionId;
    if (!cardId) return;
    const human = $('mcp-control-btn').dataset.human === 'true';
    if (!human && !(await confirmDialog(t('mcp.takeoverConfirm')))) return;
    try {
      await inv(human ? 'mcp_return_control' : 'mcp_takeover', { sessionId: cardId });
    } catch (error) {
      // One sentence per stable machine code; never the raw text.
      toast(t(mcpErrorKey(error)));
    }
    renderSessionView();
  };

  $('mcp-grant-btn').onclick = async e => {
    e.stopPropagation();
    const sessionId = state.sessionId;
    if (!sessionId) return;
    const active = $('mcp-grant-btn').dataset.active === 'true';
    if (!active && !(await confirmDialog(t('mcp.approveExecutionConfirm')))) return;
    try {
      if (active) await inv('mcp_execution_revoke', { sessionId });
      else {
        const minutes = await choiceDialog(t('mcp.executionDuration'), [5, 15, 30, 60].map(value => ({
          id: String(value), label: t('mcp.minutes', { value }), primary: value === 15,
        })));
        if (!minutes) return;
        const allowStdin = await confirmDialog(t('mcp.allowStdinConfirm'));
        const allowOutput = await confirmDialog(t('mcp.allowOutputConfirm'));
        await inv('mcp_execution_grant', {
          sessionId, durationMs: Number(minutes) * 60 * 1000, allowStdin, allowOutput,
        });
      }
      renderSessionView();
    } catch (_) { toast(t('mcp.actionFailed')); }
  };

  registerShortcutAction('splitRight', () => showSplitPicker('row'));

  registerShortcutAction('splitDown', () => showSplitPicker('col'));

  listen('pty-data', ev => {
    const { name, gen, seq, data } = ev.payload;
    /* flow control: drop a stale attachment's tail (its gate is already
       closed backend-side — no ACK owed); a NEWER gen means our attach invoke
       hasn't resolved yet — accept it and advance, or the first paint is lost */
    const cur = ctx.ptyGens.get(name) || 0;
    if (gen < cur) return;
    if (gen > cur) ctx.ptyGens.set(name, gen);
    const p = panes.get(name);
    if (p) {
      const u8 = b64ToU8(data);
      ctx.rxBytes += u8.length;
      if (ctx.rxLogged < 3 || ctx.rxLogged % 200 === 0) uev('pty-rx', null, u8.length, ctx.rxBytes);
      ctx.rxLogged++;
      /* ACK only after xterm has actually consumed the bytes — this is what
         bounds the backend's in-flight window (see pty.rs) */
      p.term.write(u8, () => {
        inv('pty_ack', { name, gen, seq }).catch(() => {});
        // The first consumed frame may precede the attach reply. Both must
        // name the same generation before a turn can be marked viewed.
        if (panes.get(name) === p && ctx.ptyGens.get(name) === gen) {
          p.renderedGen = gen;
          markSessionSeen(p.sid);
        }
      });
    } else {
      /* pane already gone but the stream still current: ACK so the emitter
         reaches its natural end instead of waiting on a window we'll never fill */
      inv('pty_ack', { name, gen, seq }).catch(() => {});
    }
  }).catch(() => uev('listen-fail', 'pty-data'));

  listen('pty-exit', ev => {
    const { name, gen } = ev.payload;
    const cur = ctx.ptyGens.get(name) || 0;
    const pane = panes.get(name);
    /* a gen we have not seen yet is EITHER the shell exiting before our
       attach reply named its generation OR a queued exit from a pane closed
       an instant ago (ptyGens was cleared). Only the reply can tell them
       apart: park it on the pane and let attachPane compare */
    if (gen > cur) { if (pane) pane.pendingExit = gen; return; }
    if (gen < cur) return;
    if (pane) paneExited(pane, gen);
  }).catch(() => uev('listen-fail', 'pty-exit'));
}
