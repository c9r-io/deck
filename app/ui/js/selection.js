// selection.js — direct cross-screen selection inside terminal panes.
// tmux copy-mode owns the selection and repaint; this module owns only the
// pointer lifecycle, public cell geometry, and stale async cancellation.
import { inv } from './state.js';
import { toast } from './dialogs.js';
import { createTerminalSelectionModel, terminalSelectionEdgeLines } from './pure.js';

let nextSelectionToken = 1;
const controllers = new Set();
let physicalPointerOwner = null;
let lateCompatibilityOwner = null;

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

function terminalSelectionController(pane, onModeChange) {
  const model = createTerminalSelectionModel();
  let gesture = null;
  let selected = false;
  let disposed = false;
  let previousDisableStdin = false;
  let opChain = Promise.resolve();
  let updateRunning = false;
  let updateDirty = false;
  let edgeTimer = null;
  let lastStatus = null;
  let limitNoticeShown = false;
  let token = 0;
  let replayingClick = false;
  let blockCompatibilityUntil = 0;
  let previousClick = null;
  let ownerTrace = {
    pointerDown: 0, promoted: 0, compatibilityBlocked: 0,
    clickReplayed: 0, ended: 0,
  };

  const queue = operation => {
    const pending = opChain.catch(() => {}).then(operation);
    opChain = pending;
    return pending;
  };

  const setInputSuppressed = suppress => {
    if (suppress) {
      previousDisableStdin = !!pane.term.options.disableStdin;
      pane.term.options.disableStdin = true;
    } else {
      pane.term.options.disableStdin = previousDisableStdin;
    }
  };

  const clearEdgeTimer = () => {
    if (edgeTimer != null) clearTimeout(edgeTimer);
    edgeTimer = null;
  };

  const setMode = value => {
    selected = value;
    if (onModeChange) onModeChange(value, lastStatus);
  };

  const releaseCapture = () => {
    if (!gesture || !pane.body.releasePointerCapture) return;
    try { pane.body.releasePointerCapture(gesture.pointerId); } catch (e) { /* already released */ }
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

  const requestUpdate = () => {
    if (!gesture || !gesture.promoted || disposed) return;
    updateDirty = true;
    if (updateRunning) return;
    updateRunning = true;
    const run = async () => {
      while (updateDirty && gesture && gesture.promoted && !disposed) {
        updateDirty = false;
        const point = { x: gesture.x, y: gesture.y };
        const cell = terminalCell(pane, point.x, point.y);
        if (!cell) break;
        model.move({ row: cell.row, col: cell.col });
        const edgeLines = terminalSelectionEdgeLines({
          pointerY: point.y, top: cell.rect.top, bottom: cell.rect.bottom,
        });
        const generation = model.snapshot().generation;
        try {
          const status = await queue(() => inv('terminal_selection_update', {
            name: pane.session, row: cell.row, col: cell.col, edgeLines,
          }));
          if (!model.apply(generation, status)) continue;
          lastStatus = status;
          setMode(true);
          if (status.history_at_limit && status.at_top && edgeLines < 0 && !limitNoticeShown) {
            limitNoticeShown = true;
            toast(`selection reached tmux’s ${status.history_limit.toLocaleString()}-row history limit; older output is no longer available`);
          }
        } catch (e) {
          if (model.snapshot().generation === generation) {
            await cancel(false);
            toast('terminal selection ended because its session changed');
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
    gesture.promoted = true;
    ownerTrace.promoted = 1;
    token = nextSelectionToken++;
    const generation = model.begin({ row: anchor.row, col: anchor.col });
    model.move({ row: active.row, col: active.col });
    try { pane.term.clearSelection(); } catch (e) { /* already empty */ }
    if (pane.body.setPointerCapture) {
      try { pane.body.setPointerCapture(gesture.pointerId); } catch (e) { /* document capture remains */ }
    }
    queue(() => inv('terminal_selection_start', {
      name: pane.session,
      anchorRow: anchor.row, anchorCol: anchor.col,
      activeRow: active.row, activeCol: active.col,
    })).then(status => {
      if (!model.apply(generation, status)) return;
      lastStatus = status;
      setMode(true);
      requestUpdate();
    }).catch(() => {
      if (model.snapshot().generation === generation) {
        cancel(false);
        toast('terminal selection could not start');
      }
    });
  };

  const pointerDown = event => {
    if (disposed || event.button !== 0) return;
    if (!terminalCell(pane, event.clientX, event.clientY)) return;
    /* Own the physical gesture before xterm sees pointerdown/mousedown.
       A drag is rendered by tmux; a sub-threshold click is replayed to xterm
       after pointerup. This prevents xterm's internal mouse service and the
       tmux coordinator from ever starting the same physical drag. */
    event.preventDefault();
    event.stopImmediatePropagation();
    if (selected) cancel(false);
    try { pane.term.clearSelection(); } catch (e) { /* already empty */ }
    clearEdgeTimer();
    ownerTrace = {
      pointerDown: 1, promoted: 0, compatibilityBlocked: 0,
      clickReplayed: 0, ended: 0,
    };
    gesture = {
      pointerId: event.pointerId,
      startX: event.clientX, startY: event.clientY,
      x: event.clientX, y: event.clientY,
      target: event.target,
      modifiers: {
        altKey: event.altKey, ctrlKey: event.ctrlKey,
        metaKey: event.metaKey, shiftKey: event.shiftKey,
      },
      promoted: false,
    };
    lateCompatibilityOwner = null;
    physicalPointerOwner = api;
    setInputSuppressed(true);
  };

  const pointerMove = event => {
    if (!gesture || event.pointerId !== gesture.pointerId) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    gesture.x = event.clientX;
    gesture.y = event.clientY;
    if (!gesture.promoted) {
      const distance = Math.hypot(gesture.x - gesture.startX, gesture.y - gesture.startY);
      if (distance < 4) return;
      promote();
    }
    if (gesture.promoted) {
      requestUpdate();
    }
  };

  const clickCount = ended => {
    const now = Date.now();
    const close = previousClick && now - previousClick.time <= 500
      && Math.hypot(ended.x - previousClick.x, ended.y - previousClick.y) <= 5;
    const count = close ? Math.min(3, previousClick.count + 1) : 1;
    previousClick = { time: now, x: ended.x, y: ended.y, count };
    return count;
  };

  const replayClick = ended => {
    const screen = pane.body.querySelector('.xterm-screen');
    const target = ended.target?.isConnected && pane.body.contains(ended.target)
      ? ended.target : screen;
    if (!target) return;
    const detail = clickCount(ended);
    const init = {
      bubbles: true, cancelable: true, composed: true, view: window,
      button: 0, buttons: 0, detail,
      clientX: ended.x, clientY: ended.y,
      ...ended.modifiers,
    };
    replayingClick = true;
    try {
      target.dispatchEvent(new MouseEvent('mousedown', { ...init, buttons: 1 }));
      target.dispatchEvent(new MouseEvent('mouseup', init));
      target.dispatchEvent(new MouseEvent('click', init));
      if (detail === 2) target.dispatchEvent(new MouseEvent('dblclick', init));
      ownerTrace.clickReplayed = 1;
    } finally {
      replayingClick = false;
    }
  };

  const pointerEnd = event => {
    if (!gesture || (event.pointerId != null && event.pointerId !== gesture.pointerId)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    const ended = {
      ...gesture,
      x: event.clientX ?? gesture.x,
      y: event.clientY ?? gesture.y,
    };
    const promoted = gesture.promoted;
    if (promoted) {
      model.finish();
    }
    clearEdgeTimer();
    releaseCapture();
    gesture = null;
    blockCompatibilityUntil = Date.now() + 100;
    if (physicalPointerOwner === api) physicalPointerOwner = null;
    lateCompatibilityOwner = api;
    setInputSuppressed(false);
    ownerTrace.ended = 1;
    if (!promoted) replayClick(ended);
  };
  const pointerCancel = event => {
    if (!gesture || (event.pointerId != null && event.pointerId !== gesture.pointerId)) return;
    event.preventDefault();
    event.stopImmediatePropagation();
    blockCompatibilityUntil = Date.now() + 100;
    lateCompatibilityOwner = api;
    cancel();
  };

  const compatibilityMouse = event => {
    const owns = physicalPointerOwner === api
      || (lateCompatibilityOwner === api && Date.now() < blockCompatibilityUntil);
    if (replayingClick || !owns) return;
    ownerTrace.compatibilityBlocked++;
    event.preventDefault();
    event.stopImmediatePropagation();
  };

  async function cancel(clearNative = true) {
    const hadTmuxSelection = selected || gesture?.promoted;
    if (gesture) {
      blockCompatibilityUntil = Date.now() + 100;
      lateCompatibilityOwner = api;
      ownerTrace.ended = 1;
    }
    clearEdgeTimer();
    releaseCapture();
    gesture = null;
    if (physicalPointerOwner === api) physicalPointerOwner = null;
    updateDirty = false;
    const cancelGeneration = model.cancel();
    setInputSuppressed(false);
    lastStatus = null;
    setMode(false);
    if (clearNative) {
      try { pane.term.clearSelection(); } catch (e) { /* pane may be disposing */ }
    }
    if (hadTmuxSelection) {
      await queue(() => inv('terminal_selection_cancel', { name: pane.session }).catch(() => {}));
    }
    if (model.snapshot().generation === cancelGeneration) model.reset();
  }

  const copy = async () => {
    if (!selected) return null;
    const copyToken = token;
    await opChain.catch(() => {});
    if (!selected || token !== copyToken || disposed) return null;
    const result = await inv('terminal_selection_copy', { name: pane.session, token: copyToken });
    if (!selected || token !== copyToken || disposed) return null;
    return result.text;
  };

  const resize = () => {
    if (gesture?.promoted) requestUpdate();
  };

  const dispose = () => {
    if (disposed) return;
    disposed = true;
    cancel(false);
    if (lateCompatibilityOwner === api) lateCompatibilityOwner = null;
    pane.body.removeEventListener('pointerdown', pointerDown, true);
    document.removeEventListener('pointermove', pointerMove, true);
    document.removeEventListener('pointerup', pointerEnd, true);
    document.removeEventListener('pointercancel', pointerCancel, true);
    for (const type of ['mousedown', 'mousemove', 'mouseup', 'click', 'dblclick']) {
      document.removeEventListener(type, compatibilityMouse, true);
    }
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
  for (const type of ['mousedown', 'mousemove', 'mouseup', 'click', 'dblclick']) {
    document.addEventListener(type, compatibilityMouse, true);
  }
  window.addEventListener('blur', blur);
  document.addEventListener('visibilitychange', visibility);

  const api = {
    copy, cancel, dispose, resize,
    hasSelection: () => selected,
    isDragging: () => !!gesture?.promoted,
    status: () => lastStatus,
    ownership: () => ({
      ...ownerTrace,
      owner: gesture ? 'coordinator' : 'none',
      xtermSelection: !!pane.term.hasSelection?.(),
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
