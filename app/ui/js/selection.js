// selection.js — one coordinator for pointer ownership and terminal selection.
// A physical click stays on xterm's trusted mouse/link path. Once movement
// crosses the drag threshold, ownership transfers to tmux. At pointerup the
// tmux range is frozen into an immutable token-bound backend snapshot and a
// public-geometry overlay; later wheel movement changes only the viewport.
//
// # Contract
// Terminal gesture and selection authority is explicit. A sub-threshold
// physical gesture stays on xterm's trusted mouse/link path; no synthetic
// compatibility click is replayed. Crossing the threshold transfers the drag
// to `selection.js`/tmux and clears speculative xterm selection. tmux owns the
// directional, end-exclusive endpoints only while dragging. Endpoints are
// placed with `top-line` + `cursor-down` + `cursor-right` ONLY
// (`copy_cursor_moves`): `start-of-line`/`end-of-line`/`back-to-indentation`
// walk to the ends of the WRAPPED logical line and `cursor-left` lands on a
// wide grapheme's trailing column, so all four leave the visible row. Because
// `cursor-down` snaps the column to a line end until the walk first steps off
// a NON-EMPTY line, the plan descends to the last blank row above the frame's
// first text, wraps out of it with one `cursor-right`, then descends the rest
// — without that, a full-screen agent frame (blank rows on top) selected rows
// the pointer never touched while a shell pane looked fine. Pointerup queues
// the final update, atomically snapshots tmux into a unique buffer, validates
// the pane token, clears tmux's cursor-bound highlight without moving the
// viewport, and installs one immutable backend lease (bytes + absolute content
// coordinates). A plain overlay derived from public `.xterm-screen`, cols and
// rows renders that lease; selection wheel commands only update viewport
// status, so endpoints and copy bytes cannot drift. Pointerup positions the
// final cell with edge scrolling disabled. Every pointer coordinate uses a
// frontend grid confirmed by `pty_resize`; the backend serializes resize reflow
// with selection operations and rejects stale dimensions instead of clamping.
// A completed-selection scroll treats tmux status and xterm `onWriteParsed` as
// unordered: if the frame arrived first, the status completion renders; if the
// status arrived first, the next parsed frame renders. Because the immutable
// lease no longer depends on tmux's copy cursor, scrolling re-anchors that
// cursor to the live input row and publishes `cursor_visible`; xterm removes
// its cursor marker once the input row is outside the viewport instead of
// leaving it fixed on the selected cell. While Deck owns a
// promoted drag, `onSelectionChange` clears any late compatibility-mouse xterm
// selection so a second viewport-fixed highlight cannot survive.
// ⌘C waits for the whole
// chain and reads only the current token. Escape, input/composition, blur,
// visibility, focus change, detach and disposal revoke the lease. Never add a
// transparent textarea or dependency on xterm private internals.
// Gesture promotion is based on crossing a public terminal cell, never an
// arbitrary CSS-pixel distance, and pointerup rechecks the final cell because
// WebKit may coalesce the last pointermove. This is what keeps short one-row
// drags on the same tmux/overlay path as multi-row drags while same-cell and
// double/triple clicks remain native xterm operations. If a native xterm
// word/line range survives until the first wheel frame, read only its public
// `getSelectionPosition()` coordinates, convert visible absolute buffer rows
// with `terminalNativeSelectionCells`, and freeze it in tmux before scrolling.
// Wheel routing keeps an existing Deck token authoritative, adopts an idle
// native range, and otherwise uses ordinary session scrolling.
// Selection never sets `disableStdin`; composition/dead-key events bypass all
// Deck shortcuts, and `macOptionIsMeta` is false so Option remains owned by
// macOS text input. Codex/Claude Up-arrow compatibility is narrowly armed by a
// history recall at the agent prompt and requires a visible continuation row
// located from the first five public xterm cells; it re-enters `term.input` and
// must never capture shell/editor keys or terminal text.
// Terminal links use `tokenizeTerminalLinks`, not an overlapping global regex:
// an HTTP(S) URL consumes its whole logical-line interval before path candidates
// are considered. `terminal_paths_exist` then resolves candidates against the
// pane cwd in one bounded backend call; nonexistent or inaccessible local paths
// never become interactive. Link actions resolve again before opening.
// History is 50,000 rows and clipboard extraction is explicitly capped at
// 64 MiB without truncation. During selection tmux freezes the reading frame
// while the PTY stream continues through its bounded ACK gate.
import { inv, uev } from './state.js';
import { toast } from './dialogs.js';
import {
  createTerminalSelectionModel,
  terminalNativeSelectionCells,
  terminalSelectionEdgeLines,
  terminalSelectionOverlayRows,
} from './pure.js';
import { formatNumber, t } from './i18n.js';

let nextSelectionToken = 1;
const controllers = new Set();
let physicalPointerOwner = null;
/* Forensics for `revoker-*`: when the last pointerup was seen anywhere. A
   trailing pointerdown that kills a frozen selection is classified by how
   long after the finishing release it arrived — a replayed/synthetic event
   lands within tens of ms, a trackpad lift-off tap within a few hundred, a
   deliberate re-click later. */
let lastPointerUpAt = 0;

function terminalCell(pane, clientX, clientY) {
  const screen = pane.body.querySelector('.xterm-screen');
  if (!screen || !pane.term.cols || !pane.term.rows) return null;
  const rect = screen.getBoundingClientRect();
  if (!rect.width || !rect.height) return null;
  const x = Math.max(rect.left, Math.min(rect.right - 0.01, clientX));
  const y = Math.max(rect.top, Math.min(rect.bottom - 0.01, clientY));
  return {
    row: Math.max(0, Math.min(pane.term.rows - 1,
      Math.floor((y - rect.top) / (rect.height / pane.term.rows)))),
    col: Math.max(0, Math.min(pane.term.cols - 1,
      Math.floor((x - rect.left) / (rect.width / pane.term.cols)))),
    rect,
  };
}

const copyFailureCode = error => {
  const value = String(error || '');
  if (value.includes('selection-missing')) return 'selection-missing';
  return 'snapshot-failed';
};

/* `finish-failed` reason code for the log's `a` slot: 1 = the pane had left
   tmux copy-mode, 2 = copy-mode survived but its selection was cleared,
   0 = anything else. Derived from the backend's closed error suffix, never
   from error text. */
const finishFailureReason = error => {
  const value = String(error || '');
  if (value.includes('selection-missing-inactive')) return 1;
  if (value.includes('selection-missing-cleared')) return 2;
  return 0;
};

const dimensionsChanged = error => String(error || '').includes('selection-dimensions-changed');

function terminalSelectionController(pane, onModeChange) {
  const model = createTerminalSelectionModel();
  let gesture = null;
  let selected = false;
  let frozen = false;
  let disposed = false;
  let opChain = Promise.resolve();
  let updateRunning = false;
  let updateDirty = false;
  let edgeTimer = null;
  let lastStatus = null;
  let parsedFrame = 0;
  let nativeSelectionDisposable = null;
  let limitNoticeShown = false;
  let token = 0;
  let suppressLinkUntil = 0;
  let promotedAt = 0;
  let ownerTrace = {
    pointerDown: 0, promoted: 0, trustedClick: 0,
    compatibilityBlocked: 0, ended: 0,
  };

  /* Selection lifecycle diagnostics. ⌘C only ever reports what it FOUND;
     without these, a copy that logs `terminal-copy keydown-none` cannot be
     told apart from a drag that never promoted, a selection tmux refused to
     start, and a live selection some later event revoked. Same contract as
     the rest of frontend logging: a closed label plus two integers —
     `a` is a per-label count (rows spanned, or 1 when a FROZEN selection was
     destroyed) and `b` is milliseconds since promotion, -1 when never
     promoted. No terminal text, session name or error text can enter. */
  const sev = (detail, a = 0) =>
    uev('terminal-selection', detail, a, promotedAt ? Date.now() - promotedAt : -1);
  const statusRows = status => (status
    ? Math.abs(status.selection_end_row - status.selection_start_row) + 1
    : 0);

  const queue = operation => {
    const pending = opChain.catch(() => {}).then(operation);
    opChain = pending;
    return pending;
  };

  const clearEdgeTimer = () => {
    if (edgeTimer != null) clearTimeout(edgeTimer);
    edgeTimer = null;
  };

  const setMode = value => {
    selected = value;
    if (onModeChange) onModeChange(value, lastStatus, { dragging: !!gesture?.promoted, frozen });
  };

  const releaseCapture = current => {
    if (!current || !pane.body.releasePointerCapture) return;
    try { pane.body.releasePointerCapture(current.pointerId); } catch (e) { /* already released */ }
  };

  const overlay = () => {
    const screen = pane.body.querySelector('.xterm-screen');
    if (!screen) return null;
    let layer = screen.querySelector(':scope > .deck-selection-overlay');
    if (!layer) {
      layer = document.createElement('div');
      layer.className = 'deck-selection-overlay';
      screen.appendChild(layer);
    }
    return layer;
  };

  const clearOverlay = () => {
    const layer = pane.body.querySelector('.deck-selection-overlay');
    if (layer) layer.replaceChildren();
  };

  const renderOverlay = () => {
    const layer = overlay();
    if (!layer) return;
    layer.replaceChildren();
    if (!selected || !lastStatus) return;
    const screen = pane.body.querySelector('.xterm-screen');
    const rect = screen.getBoundingClientRect();
    const viewportTop = lastStatus.history_rows - lastStatus.scroll_position;
    const spans = terminalSelectionOverlayRows({
      startRow: lastStatus.selection_start_row,
      startCol: lastStatus.selection_start_col,
      endRow: lastStatus.selection_end_row,
      endCol: lastStatus.selection_end_col,
      viewportTop,
      rows: pane.term.rows,
      cols: pane.term.cols,
    });
    const cellWidth = rect.width / pane.term.cols;
    const cellHeight = rect.height / pane.term.rows;
    for (const span of spans) {
      const band = document.createElement('div');
      band.className = 'deck-selection-band';
      band.dataset.absoluteRow = String(span.absoluteRow);
      band.style.left = `${span.col * cellWidth}px`;
      band.style.top = `${span.row * cellHeight}px`;
      band.style.width = `${span.width * cellWidth}px`;
      band.style.height = `${cellHeight}px`;
      layer.appendChild(band);
    }
  };

  const scheduleEdge = () => {
    clearEdgeTimer();
    if (!gesture || !gesture.promoted || disposed) return;
    const cell = terminalCell(pane, gesture.x, gesture.y);
    if (!cell) return;
    const edge = terminalSelectionEdgeLines({
      pointerY: gesture.y, top: cell.rect.top, bottom: cell.rect.bottom,
    });
    const stopped = edge < 0 ? lastStatus?.at_top : edge > 0 ? lastStatus?.at_bottom : false;
    if (!edge || stopped) return;
    edgeTimer = setTimeout(() => {
      edgeTimer = null;
      requestUpdate();
    }, 45);
  };

  const synchronizeSize = async () => {
    if (pane.syncSize && !(await pane.syncSize())) {
      sev('dimensions-changed');
      throw new Error('selection-dimensions-changed');
    }
    if (!pane.term.cols || !pane.term.rows) {
      sev('dimensions-changed');
      throw new Error('selection-dimensions-changed');
    }
    return { grid: { cols: pane.term.cols, rows: pane.term.rows } };
  };

  const invalidateSynchronizedSize = () => {
    sev('dimensions-changed');
    pane.invalidateSize?.();
  };

  const startAt = async (currentToken, anchorPoint, activePoint) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      const dimensions = await synchronizeSize();
      const anchor = terminalCell(pane, anchorPoint.x, anchorPoint.y);
      const active = terminalCell(pane, activePoint.x, activePoint.y);
      if (!anchor || !active) throw new Error('selection-missing');
      try {
        const status = await inv('terminal_selection_start', {
          name: pane.session, token: currentToken,
          anchorRow: anchor.row, anchorCol: anchor.col,
          activeRow: active.row, activeCol: active.col,
          ...dimensions,
        });
        return { status, active };
      } catch (error) {
        if (!dimensionsChanged(error) || attempt === 2) throw error;
        invalidateSynchronizedSize();
      }
    }
    throw new Error('selection-dimensions-changed');
  };

  const startCells = async (currentToken, anchor, active) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      const dimensions = await synchronizeSize();
      try {
        const status = await inv('terminal_selection_start', {
          name: pane.session, token: currentToken,
          anchorRow: anchor.row, anchorCol: anchor.col,
          activeRow: active.row, activeCol: active.col,
          ...dimensions,
        });
        return { status, dimensions };
      } catch (error) {
        if (!dimensionsChanged(error) || attempt === 2) throw error;
        invalidateSynchronizedSize();
      }
    }
    throw new Error('selection-dimensions-changed');
  };

  const updateAt = async (currentToken, point, allowEdgeScroll) => {
    for (let attempt = 0; attempt < 3; attempt++) {
      const dimensions = await synchronizeSize();
      const cell = terminalCell(pane, point.x, point.y);
      if (!cell) throw new Error('selection-missing');
      const edgeLines = allowEdgeScroll ? terminalSelectionEdgeLines({
        pointerY: point.y, top: cell.rect.top, bottom: cell.rect.bottom,
      }) : 0;
      try {
        const status = await inv('terminal_selection_update', {
          name: pane.session, token: currentToken,
          row: cell.row, col: cell.col, edgeLines,
          ...dimensions,
        });
        return { status, cell, edgeLines, dimensions };
      } catch (error) {
        if (!dimensionsChanged(error) || attempt === 2) throw error;
        invalidateSynchronizedSize();
      }
    }
    throw new Error('selection-dimensions-changed');
  };

  const requestUpdate = () => {
    if (!gesture || !gesture.promoted || disposed) return;
    updateDirty = true;
    if (updateRunning) return;
    updateRunning = true;
    const run = async () => {
      while (updateDirty && gesture && gesture.promoted && !disposed) {
        updateDirty = false;
        const current = gesture;
        const point = { x: current.x, y: current.y };
        const generation = model.snapshot().generation;
        const currentToken = token;
        try {
          const result = await queue(() => updateAt(currentToken, point, true));
          const { status, cell, edgeLines } = result;
          model.move({ row: cell.row, col: cell.col });
          if (currentToken !== token || !model.apply(generation, status)) continue;
          lastStatus = status;
          // tmux retains the byte-accurate selection, but its native
          // selection paint is disabled: drawing one settled DOM overlay
          // avoids exposing every intermediate top-line/cursor motion as a
          // full terminal flash on restored, history-heavy panes.
          renderOverlay();
          if (status.history_at_limit && status.at_top && edgeLines < 0 && !limitNoticeShown) {
            limitNoticeShown = true;
            toast(t('selection.limit', { count: formatNumber(status.history_limit) }));
          }
        } catch (e) {
          if (currentToken === token && model.snapshot().generation === generation) {
            sev('update-failed');
            await cancel(false, null);
            toast(t('error.selectionChanged'));
          }
          break;
        }
      }
      updateRunning = false;
      scheduleEdge();
    };
    run();
  };

  const promote = () => {
    if (!gesture || gesture.promoted || disposed) return;
    const anchor = terminalCell(pane, gesture.startX, gesture.startY);
    const active = terminalCell(pane, gesture.x, gesture.y);
    if (!anchor || !active) return;
    if (anchor.row === active.row && anchor.col === active.col) return;
    gesture.promoted = true;
    ownerTrace.promoted = 1;
    token = nextSelectionToken++;
    frozen = false;
    promotedAt = Date.now();
    sev('promote', Math.abs(active.row - anchor.row) + 1);
    const currentToken = token;
    const generation = model.begin({ row: anchor.row, col: anchor.col });
    model.move({ row: active.row, col: active.col });
    setMode(true);
    try { pane.term.clearSelection(); } catch (e) { /* already empty */ }
    if (pane.body.setPointerCapture) {
      try { pane.body.setPointerCapture(gesture.pointerId); } catch (e) { /* document capture remains */ }
    }
    const anchorPoint = { x: gesture.startX, y: gesture.startY };
    const activePoint = { x: gesture.x, y: gesture.y };
    queue(() => startAt(currentToken, anchorPoint, activePoint)).then(result => {
      const { status, active: synchronizedActive } = result;
      model.move({ row: synchronizedActive.row, col: synchronizedActive.col });
      if (currentToken !== token || !model.apply(generation, status)) return;
      lastStatus = status;
      sev('start-ok', statusRows(status));
      renderOverlay();
      requestUpdate();
    }).catch(() => {
      if (currentToken === token && model.snapshot().generation === generation) {
        sev('start-failed');
        cancel(false, null);
        toast(t('error.selectionStart'));
      }
    });
  };

  const pointerDown = event => {
    if (disposed || event.button !== 0) return;
    if (!terminalCell(pane, event.clientX, event.clientY)) return;
    /* This pointerdown is about to revoke a live selection (the paired
       `cancel-pointer` follows). Attribute WHERE it came from so a failed
       ⌘C can be traced to a synthetic/replayed event, a trackpad lift-off
       tap, or a real re-click: the label classifies isTrusted + pointerType,
       `a` is the click count, `b` is ms since the last pointerup anywhere.
       Ordinary clicks with nothing to destroy stay silent, like cancel. */
    if (selected || gesture?.promoted) {
      const label = !event.isTrusted ? 'revoker-synthetic'
        : event.pointerType === 'mouse' ? 'revoker-mouse'
          : event.pointerType === 'touch' ? 'revoker-touch'
            : event.pointerType === 'pen' ? 'revoker-pen' : 'revoker-unknown';
      const sinceUp = lastPointerUpAt
        ? Math.min(Date.now() - lastPointerUpAt, 3600000) : -1;
      uev('terminal-selection', label,
        Math.max(0, Math.min(9, event.detail || 0)), sinceUp);
    }
    // Keep the physical compatibility sequence trusted for click/link. A
    // later terminal-cell transition explicitly transfers ownership to tmux.
    cancel(false, 'pointer');
    ownerTrace = {
      pointerDown: 1, promoted: 0, trustedClick: 0,
      compatibilityBlocked: 0, ended: 0,
    };
    gesture = {
      pointerId: event.pointerId,
      startX: event.clientX, startY: event.clientY,
      x: event.clientX, y: event.clientY,
      promoted: false,
    };
    physicalPointerOwner = api;
  };

  const pointerMove = event => {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    gesture.x = event.clientX;
    gesture.y = event.clientY;
    // xterm selects terminal cells, not CSS-pixel distances. Promote as soon
    // as the pointer enters a different cell so a short horizontal drag near
    // a glyph boundary follows the exact same tmux path as a multi-row drag.
    if (!gesture.promoted) promote();
    if (gesture.promoted) {
      event.preventDefault();
      event.stopImmediatePropagation();
      requestUpdate();
    }
  };

  const pointerEnd = event => {
    lastPointerUpAt = Date.now();
    if (!gesture || (event.pointerId != null && event.pointerId !== gesture.pointerId)) return;
    const ended = gesture;
    ended.x = event.clientX ?? ended.x;
    ended.y = event.clientY ?? ended.y;
    // WebKit can coalesce the final pointermove of a quick drag. Re-evaluate
    // the pointerup cell before deciding this was a click; promote() already
    // leaves same-cell clicks and native double/triple-click selection alone.
    if (!ended.promoted) promote();
    clearEdgeTimer();
    releaseCapture(ended);
    gesture = null;
    if (physicalPointerOwner === api) physicalPointerOwner = null;
    ownerTrace.ended = 1;
    if (!ended.promoted) {
      ownerTrace.trustedClick = 1;
      return;
    }

    suppressLinkUntil = Date.now() + 250;
    model.finish();
    const generation = model.snapshot().generation;
    const currentToken = token;
    const finalPoint = { x: ended.x, y: ended.y };
    queue(async () => {
      for (let attempt = 0; attempt < 3; attempt++) {
        const finalUpdate = await updateAt(currentToken, finalPoint, false);
        model.move({ row: finalUpdate.cell.row, col: finalUpdate.cell.col });
        if (currentToken !== token || !model.apply(generation, finalUpdate.status)) {
          throw new Error('selection-missing');
        }
        lastStatus = finalUpdate.status;
        try {
          const status = await inv('terminal_selection_finish', {
            name: pane.session, token: currentToken,
            ...finalUpdate.dimensions,
          });
          if (currentToken !== token || disposed || model.snapshot().generation !== generation) return;
          lastStatus = status;
          frozen = true;
          sev('finish-ok', statusRows(status));
          renderOverlay();
          if (onModeChange) onModeChange(true, lastStatus, { dragging: false, frozen: true });
          return;
        } catch (error) {
          if (!dimensionsChanged(error) || attempt === 2) throw error;
          invalidateSynchronizedSize();
        }
      }
    }).catch(error => {
      if (currentToken === token && !disposed) {
        uev('terminal-copy', copyFailureCode(error));
        sev('finish-failed', finishFailureReason(error));
        cancel(false, null);
        toast(t('error.selectionChanged'));
      }
    });
    setTimeout(() => {
      if (currentToken === token) {
        try { pane.term.clearSelection(); } catch (e) { /* disposed */ }
      }
    }, 0);
  };

  const pointerCancel = event => {
    if (!gesture || (event.pointerId != null && event.pointerId !== gesture.pointerId)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    suppressLinkUntil = Date.now() + 250;
    cancel(true, 'pointer-cancel');
  };

  const compatibilityMove = event => {
    if (!gesture || physicalPointerOwner !== api) return;
    // macOS WKWebView can emit horizontal drag motion only as compatibility
    // mousemove events even though pointerdown/pointerup were delivered. Use
    // the same public cell transition as pointerMove before xterm's bubbling
    // listener sees the event, so one-row and multi-row drags share ownership.
    gesture.x = event.clientX;
    gesture.y = event.clientY;
    if (!gesture.promoted) promote();
    if (!gesture.promoted) return;
    ownerTrace.compatibilityBlocked++;
    event.preventDefault();
    event.stopImmediatePropagation();
    requestUpdate();
  };

  /* `reason` is a closed label naming what revoked the selection. It is
     logged ONLY when something real was destroyed, so an ordinary click —
     which cancels an empty controller on every pointerdown — stays silent.
     Callers that already logged a more specific failure pass null. */
  async function cancel(clearNative = true, reason = 'other') {
    const oldToken = token;
    const hadSelection = selected || !!gesture?.promoted;
    if (hadSelection && reason) sev(`cancel-${reason}`, frozen ? 1 : 0);
    const currentGesture = gesture;
    clearEdgeTimer();
    releaseCapture(currentGesture);
    gesture = null;
    if (physicalPointerOwner === api) physicalPointerOwner = null;
    updateDirty = false;
    token = 0;
    frozen = false;
    /* with the rest of the synchronous teardown, never after the awaited
       backend cancel below: a promote that lands while this cancel is still
       in flight owns `promotedAt`, and a late reset would report the new
       selection's whole life as "never promoted". */
    promotedAt = 0;
    const cancelGeneration = model.cancel();
    lastStatus = null;
    setMode(false);
    clearOverlay();
    // Repair disableStdin left by an older controller, but pointer selection
    // itself never owns keyboard input in this state machine.
    pane.term.options.disableStdin = false;
    if (clearNative) {
      try { pane.term.clearSelection(); } catch (e) { /* pane may be disposing */ }
    }
    if (hadSelection && oldToken) {
      await queue(() => inv('terminal_selection_cancel', {
        name: pane.session, token: oldToken,
      }).catch(() => {}));
    }
    if (model.snapshot().generation === cancelGeneration) model.reset();
  }

  const copy = async () => {
    const copyToken = token;
    if (!selected || !copyToken) return null;
    await opChain.catch(() => {});
    if (!selected || token !== copyToken || disposed) return null;
    try {
      const result = await inv('terminal_selection_copy', {
        name: pane.session, token: copyToken,
      });
      if (!selected || token !== copyToken || disposed) return null;
      return result.text;
    } catch (error) {
      uev('terminal-copy', copyFailureCode(error));
      throw error;
    }
  };

  const freezeNative = async () => {
    if (selected) return frozen;
    const position = pane.term.getSelectionPosition?.();
    const viewport = pane.term.buffer.active.viewportY;
    const cells = terminalNativeSelectionCells({
      position, viewportY: viewport, rows: pane.term.rows, cols: pane.term.cols,
    });
    if (!cells) return false;
    const { anchor, active } = cells;

    token = nextSelectionToken++;
    frozen = false;
    promotedAt = Date.now();
    const currentToken = token;
    const generation = model.begin(anchor);
    model.move(active);
    model.finish();
    setMode(true);
    try {
      const status = await queue(async () => {
        const started = await startCells(currentToken, anchor, active);
        if (currentToken !== token || disposed
            || !model.apply(generation, started.status)) throw new Error('selection-missing');
        return inv('terminal_selection_finish', {
          name: pane.session, token: currentToken,
          ...started.dimensions,
        });
      });
      if (currentToken !== token || disposed
          || model.snapshot().generation !== generation) return false;
      lastStatus = status;
      frozen = true;
      sev('freeze-ok', statusRows(status));
      pane.term.clearSelection();
      renderOverlay();
      return true;
    } catch (error) {
      if (currentToken === token && !disposed) {
        sev('freeze-failed');
        await cancel(true, null);
      }
      return false;
    }
  };

  const scroll = async lines => {
    const scrollToken = token;
    if (!selected || !frozen || !scrollToken) return null;
    let frameAtRequest = parsedFrame;
    const status = await queue(() => {
      // Capture inside the serialized operation: writes which arrived while a
      // preceding selection command was queued cannot satisfy this scroll.
      frameAtRequest = parsedFrame;
      return inv('terminal_selection_scroll', {
        name: pane.session, token: scrollToken, lines,
      });
    });
    if (!selected || token !== scrollToken || disposed) return null;
    lastStatus = status;
    // Selection scrolls share the ordinary scroll cursor contract. Notify
    // even though selection mode itself did not change so the host can hide
    // xterm's copy cursor after the real input row leaves the viewport.
    if (onModeChange) onModeChange(true, lastStatus, { dragging: false, frozen: true });
    // The tmux status reply and its PTY repaint have no fixed ordering. If a
    // frame was already parsed, render the new coordinates now; otherwise
    // writeParsed() will render them when the repaint reaches xterm.
    if (parsedFrame !== frameAtRequest) renderOverlay();
    return status;
  };

  const writeParsed = () => {
    parsedFrame++;
    renderOverlay();
  };

  const prepareInput = () => {
    if (!selected && !gesture) {
      pane.term.options.disableStdin = false;
      return Promise.resolve();
    }
    return cancel(true, 'input');
  };

  const resize = () => {
    if (gesture?.promoted) requestUpdate();
    renderOverlay();
  };

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    cancel(false, 'dispose');
    nativeSelectionDisposable?.dispose();
    nativeSelectionDisposable = null;
    pane.body.removeEventListener('pointerdown', pointerDown, true);
    document.removeEventListener('pointermove', pointerMove, true);
    document.removeEventListener('pointerup', pointerEnd, true);
    document.removeEventListener('pointercancel', pointerCancel, true);
    document.removeEventListener('mousemove', compatibilityMove, true);
    window.removeEventListener('blur', blur);
    document.removeEventListener('visibilitychange', visibility);
    controllers.delete(api);
  };
  const blur = () => cancel(true, 'blur');
  const visibility = () => { if (document.hidden) cancel(true, 'hidden'); };

  pane.body.addEventListener('pointerdown', pointerDown, true);
  document.addEventListener('pointermove', pointerMove, true);
  document.addEventListener('pointerup', pointerEnd, true);
  document.addEventListener('pointercancel', pointerCancel, true);
  document.addEventListener('mousemove', compatibilityMove, true);
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', visibility);

  const api = {
    copy, cancel, dispose, freezeNative, prepareInput, resize, scroll,
    render: renderOverlay, writeParsed,
    hasSelection: () => selected,
    isDragging: () => !!gesture?.promoted,
    isFrozen: () => frozen,
    ageMs: () => (promotedAt ? Date.now() - promotedAt : -1),
    allowLinkActivation: () => !gesture?.promoted && Date.now() >= suppressLinkUntil,
    status: () => lastStatus,
    ownership: () => ({
      ...ownerTrace,
      owner: gesture?.promoted ? 'drag-selection'
        : gesture ? 'pointer-pending'
          : selected ? 'frozen-selection' : 'xterm',
      xtermSelection: !!pane.term.hasSelection?.(),
      frozen,
    }),
    idle: () => opChain.catch(() => {}),
  };
  // A physical drag begins on xterm so clicks and double/triple-clicks stay
  // native. Once Deck promotes that gesture, any late compatibility mouse
  // event must not leave a second, viewport-fixed xterm selection behind.
  nativeSelectionDisposable = pane.term.onSelectionChange?.(() => {
    if ((gesture?.promoted || selected) && pane.term.hasSelection?.()) {
      /* Direct evidence that WKWebView replayed the drag as late
         compatibility mouse events: an xterm selection appeared while Deck
         still owns one. `a` is the rows the doomed native range spanned. */
      const position = pane.term.getSelectionPosition?.();
      const span = Number.isFinite(position?.start?.y) && Number.isFinite(position?.end?.y)
        ? Math.abs(position.end.y - position.start.y) + 1 : 0;
      sev('native-cleared', span);
      try { pane.term.clearSelection(); } catch (e) { /* pane may be disposing */ }
    }
  });
  controllers.add(api);
  return api;
}

export function wireTerminalSelection(pane, onModeChange) {
  const controller = terminalSelectionController(pane, onModeChange);
  pane.selection = controller;
  return controller;
}

export const hasTerminalSelection = pane => !!pane?.selection?.hasSelection();
export const copyTerminalSelection = pane => pane?.selection?.copy();
export const cancelTerminalSelection = (pane, reason = 'other') =>
  pane?.selection?.cancel(true, reason);
export const cancelAllTerminalSelections = (reason = 'leave') => {
  for (const controller of [...controllers]) controller.cancel(true, reason);
};

/* ⌘C forensics: a `keydown-none` in the focused pane is ambiguous while
   another pane still holds a live Deck selection — that split is what
   separates "the selection was revoked" from "⌘C went to the wrong pane
   because the drag never moved keyboard focus". Returns how many OTHER
   panes hold one and the youngest one's age in ms (-1 when none). */
export const terminalSelectionElsewhere = pane => {
  let count = 0;
  let ageMs = -1;
  for (const controller of controllers) {
    if (controller === pane?.selection || !controller.hasSelection()) continue;
    count++;
    const age = controller.ageMs();
    if (age >= 0 && (ageMs < 0 || age < ageMs)) ageMs = age;
  }
  return { count, ageMs };
};
