// selection.js — one coordinator for pointer ownership and terminal selection.
// A physical click stays on xterm's trusted mouse/link path. Once movement
// crosses the drag threshold, ownership transfers to tmux. At pointerup the
// tmux range is frozen into an immutable token-bound backend snapshot and a
// public-geometry overlay; later wheel movement changes only the viewport.
import { inv, uev } from './state.js';
import { toast } from './dialogs.js';
import {
  createTerminalSelectionModel,
  terminalSelectionEdgeLines,
  terminalSelectionOverlayRows,
} from './pure.js';
import { formatNumber, t } from './i18n.js';

let nextSelectionToken = 1;
const controllers = new Set();
let physicalPointerOwner = null;

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
  let limitNoticeShown = false;
  let token = 0;
  let suppressLinkUntil = 0;
  let ownerTrace = {
    pointerDown: 0, promoted: 0, trustedClick: 0,
    compatibilityBlocked: 0, ended: 0,
  };

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
    if (onModeChange) onModeChange(value, lastStatus);
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
    if (!selected || !frozen || !lastStatus) return;
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
      throw new Error('selection-dimensions-changed');
    }
    if (!pane.term.cols || !pane.term.rows) throw new Error('selection-dimensions-changed');
    return { grid: { cols: pane.term.cols, rows: pane.term.rows } };
  };

  const invalidateSynchronizedSize = () => {
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
          if (status.history_at_limit && status.at_top && edgeLines < 0 && !limitNoticeShown) {
            limitNoticeShown = true;
            toast(t('selection.limit', { count: formatNumber(status.history_limit) }));
          }
        } catch (e) {
          if (currentToken === token && model.snapshot().generation === generation) {
            await cancel(false);
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
      requestUpdate();
    }).catch(() => {
      if (currentToken === token && model.snapshot().generation === generation) {
        cancel(false);
        toast(t('error.selectionStart'));
      }
    });
  };

  const pointerDown = event => {
    if (disposed || event.button !== 0) return;
    if (!terminalCell(pane, event.clientX, event.clientY)) return;
    // Keep the physical compatibility sequence trusted for click/link. A
    // later threshold crossing explicitly transfers ownership to tmux.
    cancel(false);
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
    if (!gesture.promoted) {
      const distance = Math.hypot(gesture.x - gesture.startX, gesture.y - gesture.startY);
      if (distance < 4) return;
      promote();
    }
    if (gesture.promoted) {
      event.preventDefault();
      event.stopImmediatePropagation();
      requestUpdate();
    }
  };

  const pointerEnd = event => {
    if (!gesture || (event.pointerId != null && event.pointerId !== gesture.pointerId)) return;
    const ended = gesture;
    ended.x = event.clientX ?? ended.x;
    ended.y = event.clientY ?? ended.y;
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
          renderOverlay();
          return;
        } catch (error) {
          if (!dimensionsChanged(error) || attempt === 2) throw error;
          invalidateSynchronizedSize();
        }
      }
    }).catch(error => {
      if (currentToken === token && !disposed) {
        uev('terminal-copy', copyFailureCode(error));
        cancel(false);
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
    cancel();
  };

  const compatibilityMove = event => {
    if (!gesture?.promoted || physicalPointerOwner !== api) return;
    ownerTrace.compatibilityBlocked++;
    event.preventDefault();
    event.stopImmediatePropagation();
  };

  async function cancel(clearNative = true) {
    const oldToken = token;
    const hadSelection = selected || !!gesture?.promoted;
    const currentGesture = gesture;
    clearEdgeTimer();
    releaseCapture(currentGesture);
    gesture = null;
    if (physicalPointerOwner === api) physicalPointerOwner = null;
    updateDirty = false;
    token = 0;
    frozen = false;
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

  const scroll = async lines => {
    const scrollToken = token;
    if (!selected || !frozen || !scrollToken) return null;
    const status = await queue(() => inv('terminal_selection_scroll', {
      name: pane.session, token: scrollToken, lines,
    }));
    if (!selected || token !== scrollToken || disposed) return null;
    lastStatus = status;
    renderOverlay();
    return status;
  };

  const prepareInput = () => {
    if (!selected && !gesture) {
      pane.term.options.disableStdin = false;
      return Promise.resolve();
    }
    return cancel();
  };

  const resize = () => {
    if (gesture?.promoted) requestUpdate();
    renderOverlay();
  };

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    cancel(false);
    pane.body.removeEventListener('pointerdown', pointerDown, true);
    document.removeEventListener('pointermove', pointerMove, true);
    document.removeEventListener('pointerup', pointerEnd, true);
    document.removeEventListener('pointercancel', pointerCancel, true);
    document.removeEventListener('mousemove', compatibilityMove, true);
    window.removeEventListener('blur', blur);
    document.removeEventListener('visibilitychange', visibility);
    controllers.delete(api);
  };
  const blur = () => cancel();
  const visibility = () => { if (document.hidden) cancel(); };

  pane.body.addEventListener('pointerdown', pointerDown, true);
  document.addEventListener('pointermove', pointerMove, true);
  document.addEventListener('pointerup', pointerEnd, true);
  document.addEventListener('pointercancel', pointerCancel, true);
  document.addEventListener('mousemove', compatibilityMove, true);
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', visibility);

  const api = {
    copy, cancel, dispose, prepareInput, resize, scroll, render: renderOverlay,
    hasSelection: () => selected,
    isDragging: () => !!gesture?.promoted,
    isFrozen: () => frozen,
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
export const cancelTerminalSelection = pane => pane?.selection?.cancel();
export const cancelAllTerminalSelections = () => {
  for (const controller of [...controllers]) controller.cancel();
};
