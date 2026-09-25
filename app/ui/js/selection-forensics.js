// Content-free, bounded observations of terminal selection ownership. This
// model never decides whether a gesture selects or whether a copy may write.
const bound = (value, limit) => Math.max(-limit, Math.min(limit, Math.trunc(value) || 0));
const delta = (start, cell) => cell && start
  ? { row: bound(cell.row - start.row, 99), col: bound(cell.col - start.col, 999) }
  : null;

export const COPY_NO_SELECTION_REASONS = Object.freeze([
  'no-gesture', 'gesture-active', 'gesture-cancelled', 'same-cell', 'native-gesture-no-range',
  'native-range-ended', 'promoted-empty', 'promoted-start-failed',
  'promoted-finish-failed', 'selection-revoked-pointer',
  'selection-revoked-input', 'selection-revoked-focus',
  'selection-revoked-lifecycle', 'selection-revoked-other',
]);
export const PROMOTION_SOURCES = Object.freeze(['pointer', 'compat', 'up']);

export function createSelectionForensics(now = Date.now) {
  let current = null, lastGesture = null, lastSelectionOutcome = null, lastNativeOutcome = null;
  let lastStartCell = null;
  let sequence = 0;
  const stamp = () => ({ at: now(), order: ++sequence });
  const begin = ({ id, cell, detail, native }) => {
    current = { id, start: cell && { row: cell.row, col: cell.col },
      detail: Math.max(0, Math.min(9, Math.trunc(detail) || 0)), native: !!native,
      nativeDragged: false, pointer: null, compat: null, up: null,
      sawPointerMove: false, sawCompatibilityMove: false,
      upCoordinatesPresent: false, postUpMousemove: null,
      pointerCrossed: false, compatCrossed: false, upCrossed: false,
      promoted: false, promotionSource: null, outcome: null, ...stamp() };
    return current;
  };
  const observe = (source, cell) => {
    if (!current) return;
    const key = source === 'pointer' ? 'pointer' : source === 'compat' ? 'compat' : 'up';
    current[key] = delta(current.start, cell);
    if (key === 'pointer') current.sawPointerMove = true;
    if (key === 'compat') current.sawCompatibilityMove = true;
    if (current[key]) current[`${key}Crossed`] ||= current[key].row !== 0 || current[key].col !== 0;
  };
  const nativeDetail = (detail, native) => {
    if (!current) return;
    current.detail = Math.max(0, Math.min(9, Math.trunc(detail) || 0));
    current.native = !!native;
  };
  const promote = source => {
    if (!current) return;
    current.promoted = true;
    current.promotionSource = PROMOTION_SOURCES.includes(source) ? source : null;
  };
  const upCoordinates = present => { if (current) current.upCoordinatesPresent = !!present; };
  const end = nativeDragged => {
    if (!current) return;
    current.nativeDragged = !!nativeDragged;
    current.outcome = current.promoted ? 'promoted-pending'
      : current.native ? 'native-gesture-no-range' : 'same-cell';
    const { start, ...summary } = current;
    lastStartCell = start;
    lastGesture = { ...summary, ...stamp() };
    current = null;
  };
  const abort = () => {
    if (!current) return;
    current.outcome = 'gesture-cancelled';
    const { start, ...summary } = current;
    lastStartCell = start;
    lastGesture = { ...summary, ...stamp() };
    current = null;
  };
  const gestureOutcome = (id, outcome) => {
    if (lastGesture?.id === id) lastGesture = { ...lastGesture, outcome, ...stamp() };
  };
  const postUpMousemove = cell => {
    if (current || !lastGesture || now() - lastGesture.at > 250) return null;
    const moved = delta(lastStartCell, cell);
    if (moved) lastGesture = { ...lastGesture, postUpMousemove: moved };
    return moved;
  };
  const selectionOutcome = (kind, token, gestureId = 0, revokerGestureId = 0) => {
    lastSelectionOutcome = { kind, token, gestureId, revokerGestureId, ...stamp() };
  };
  const nativeOutcome = (kind, token, gestureId = 0) => {
    lastNativeOutcome = { kind, token, gestureId, ...stamp() };
  };
  const snapshot = () => ({ gesture: current || lastGesture, active: !!current,
    selection: lastSelectionOutcome, native: lastNativeOutcome });
  const reason = () => {
    if (current) return 'gesture-active';
    if (lastSelectionOutcome?.kind === 'revoked-pointer'
        && lastSelectionOutcome.revokerGestureId === lastGesture?.id) return 'selection-revoked-pointer';
    const selectionReason = lastSelectionOutcome?.kind.startsWith('revoked-')
      ? ['revoked-pointer', 'revoked-input', 'revoked-focus'].includes(lastSelectionOutcome.kind)
        ? `selection-${lastSelectionOutcome.kind}`
        : ['revoked-live', 'revoked-exit', 'revoked-dispose', 'revoked-leave',
          'revoked-blur', 'revoked-hidden'].includes(lastSelectionOutcome.kind)
          ? 'selection-revoked-lifecycle' : 'selection-revoked-other'
      : lastSelectionOutcome?.kind;
    const candidates = [
      lastGesture && { order: lastGesture.order, reason: lastGesture.outcome },
      lastSelectionOutcome && { order: lastSelectionOutcome.order, reason: selectionReason },
      lastNativeOutcome?.kind !== 'live' && lastNativeOutcome
        && { order: lastNativeOutcome.order, reason: 'native-range-ended' },
    ].filter(Boolean).sort((a, b) => b.order - a.order);
    const found = candidates[0]?.reason;
    return COPY_NO_SELECTION_REASONS.includes(found) ? found : 'no-gesture';
  };
  return { begin, observe, nativeDetail, promote, upCoordinates, postUpMousemove,
    end, abort, gestureOutcome, selectionOutcome,
    nativeOutcome, snapshot, reason };
}
