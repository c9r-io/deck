// Debug-only real-WKWebView smoke. main.rs imports this module only when the
// app was launched with --smoke-wkwebview and an isolated --smoke-data-dir.
// The release gate also runs `node --test app/ui/test/*.mjs`; defer production
// DOM imports so Node can load this carrier without fabricating a browser.
let $, inv, state, store, panes, provider, render, pollNow, boardData;
let renameCardInline, renderSuggest, resetSuggest;
let showLinkCtx, toggleSidebar, addSplit, backToBoard, openSession, strToB64;
let closePaneBySid, focusPane, cancelTerminalSelection, copyTerminalSelection;
let refreshQueue, toggleQueuePanel;
let activateTheme, persistThemeChoice;
let terminalLogicalLine, tokenizeTerminalLinks;
if (typeof window !== 'undefined') {
  ({ $, inv, state, store } = await import('../js/state.js'));
  ({ panes, provider, render, pollNow } = await import('../js/board.js'));
  ({ boardData } = await import('../js/persistence.js'));
  ({
    renameCardInline, renderSuggest, resetSuggest,
    showLinkCtx, toggleSidebar,
  } = await import('../js/terminal.js'));
  ({ addSplit, backToBoard, closePaneBySid, focusPane, openSession, strToB64, terminalLogicalLine } = await import('../js/layout.js'));
  ({ tokenizeTerminalLinks } = await import('../js/pure.js'));
  ({ cancelTerminalSelection, copyTerminalSelection } = await import('../js/selection.js'));
  ({ refreshQueue, toggleQueuePanel } = await import('../js/scheduler.js'));
  ({ activateTheme } = await import('../js/theme.js'));
  ({ persistThemeChoice } = await import('../js/dialogs.js'));
}

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
let smokeFailed = false;
async function waitFor(test, timeout = 8000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await test()) return true;
    await pause(50);
  }
  return false;
}
const report = (name, ok, a = 0, b = 0) => {
  if (!ok) smokeFailed = true;
  return inv('ui_event', {
    code: 'smoke-check', detail: name,
    a: ok ? Math.trunc(a || 1) : -Math.abs(Math.trunc(a || 1)), b: Math.trunc(b || 0),
  });
};
const metric = (name, a = 0, b = 0) => inv('ui_event', {
  code: 'smoke-check', detail: name, a: Math.trunc(a), b: Math.trunc(b),
});
const eventAt = (x = 20, y = 20) => ({
  clientX: x, clientY: y,
  preventDefault() {}, stopPropagation() {},
});
const overlaps = (a, b) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;

async function boardConcurrency(project, column) {
  const mk = title => provider.create({
    projectId: project.id, columnId: column.id, title, cmd: '', dir: '/tmp',
  });
  const [a, b] = await Promise.all([mk('txn-a'), mk('txn-b')]);
  const closedAB = await Promise.all([provider.close(a.id), provider.close(b.id)]);
  const [c, d] = await Promise.all([mk('txn-c'), mk('txn-d')]);
  const [closedC, renamedD] = await Promise.all([
    provider.close(c.id), provider.rename(d.id, 'txn-d-renamed'),
  ]);
  const other = await provider.createProject('txn-project');
  const otherCol = other.columns[0];
  const victim = await provider.create({
    projectId: other.id, columnId: otherCol.id, title: 'txn-victim', cmd: '', dir: '/tmp',
  });
  const [removedProject] = await Promise.all([
    provider.removeProject(other.id),
    provider.renameProject(project.id, 'main-smoke'),
    mk('txn-created'),
  ]);
  const disk = await inv('load_board');
  const parsed = JSON.parse(disk.data);
  const expected = boardData();
  const diskIds = parsed.cards.map(c0 => `${c0.id}:${c0.title}`).sort().join(',');
  const memoryIds = expected.cards.map(c0 => `${c0.id}:${c0.title}`).sort().join(',');
  let mask = 0;
  if (closedAB.every(Boolean)) mask |= 1;
  if (!provider.get(a.id) && !provider.get(b.id)) mask |= 2;
  if (closedC) mask |= 4;
  if (renamedD) mask |= 8;
  if (!provider.get(c.id) && provider.get(d.id)?.title === 'txn-d-renamed') mask |= 16;
  if (removedProject) mask |= 32;
  if (!provider.project(other.id) && !provider.get(victim.id)) mask |= 64;
  if (diskIds === memoryIds) mask |= 128;
  await report('board-concurrency', mask === 255, parsed.cards.length, mask);
}

async function boardFaultSmoke(project, column) {
  await inv('smoke_fault_set', { kind: 'board-save', count: 1 });
  let firstFailed = false;
  try {
    await provider.create({
      projectId: project.id, columnId: column.id, title: 'fault-first', cmd: '', dir: '/tmp',
    });
  } catch (e) { firstFailed = true; }
  const second = await provider.create({
    projectId: project.id, columnId: column.id, title: 'fault-second', cmd: '', dir: '/tmp',
  });
  const disk = JSON.parse((await inv('load_board')).data);
  const memory = boardData();
  const diskCards = disk.cards.map(card => `${card.id}:${card.title}`).sort().join(',');
  const memoryCards = memory.cards.map(card => `${card.id}:${card.title}`).sort().join(',');
  const same = diskCards === memoryCards && disk.projects.length === memory.projects.length;
  await report('board-fault', firstFailed && !store.cards.some(c => c.title === 'fault-first')
    && provider.get(second.id)?.title === 'fault-second' && same, store.cards.length, same ? 1 : 0);
}

async function themeSmoke(card) {
  await openSession(card.id);
  const pane = panes.get(card.session);
  let mask = 0;
  const cases = [
    ['deck-dark', 'teal', 'dark'],
    ['light', 'purple', 'light'],
    ['high-contrast', 'orange', 'dark'],
  ];
  for (let i = 0; i < cases.length; i++) {
    const [theme, accent, scheme] = cases[i];
    const resolved = activateTheme({ theme, accent });
    await pause(20);
    const cssBg = getComputedStyle(document.documentElement).getPropertyValue('--bg').trim();
    if (document.documentElement.dataset.effectiveTheme === theme
        && document.documentElement.dataset.accent === accent
        && document.documentElement.style.colorScheme === scheme
        && cssBg === resolved.css.bg
        && pane.term.options.theme.background === resolved.terminal.background
        && pane.term.options.theme.cursor === resolved.terminal.cursor) mask |= (1 << i);
  }
  await report('theme-switch', mask === 7, mask, panes.size);

  const originalTheme = settings.theme;
  const originalAccent = settings.accent;
  activateTheme(settings);
  $('set-theme').value = originalTheme === 'light' ? 'deck-dark' : 'light';
  $('set-accent').value = originalAccent === 'purple' ? 'teal' : 'purple';
  await inv('smoke_fault_set', { kind: 'settings-save', count: 1 });
  await persistThemeChoice();
  const rolledBack = settings.theme === originalTheme && settings.accent === originalAccent
    && $('set-theme').value === originalTheme && $('set-accent').value === originalAccent
    && document.documentElement.dataset.theme === originalTheme
    && pane.term.options.theme.background === activateTheme(settings).terminal.background;
  await report('theme-rollback', rolledBack, rolledBack ? 1 : 0, panes.size);

  $('settings-btn').click();
  const settingsOpened = await waitFor(() => $('settings-modal').style.display === 'flex', 3000);
  const settingsBox = $('settings-box');
  const settingsRect = settingsBox.getBoundingClientRect();
  const settingsStyle = getComputedStyle(settingsBox);
  const settingsBounded = settingsOpened
    && settingsRect.top >= 0 && settingsRect.left >= 0
    && settingsRect.bottom <= innerHeight && settingsRect.right <= innerWidth
    && settingsStyle.overflowY === 'auto'
    && settingsBox.clientHeight <= innerHeight - 40;
  await report('settings-viewport', settingsBounded,
    Math.round(settingsBox.clientHeight), Math.round(innerHeight));
  $('set-close').click();
}

async function renameSmoke(card) {
  await openSession(card.id);
  const host = document.querySelector(`#side-list .side-item[data-sid="${card.id}"] .name`);
  renameCardInline(card.id, host);
  const input = host.querySelector('input');
  input.value = 'renamed-enter-smoke';
  input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));
  const immediate = !host.querySelector('input') && host.textContent === 'renamed-enter-smoke';
  const committed = await waitFor(() => provider.get(card.id)?.title === 'renamed-enter-smoke');
  const sidebar = document.querySelector(`#side-list .side-item[data-sid="${card.id}"] .name`)?.textContent;
  const header = $('sess-name').textContent;
  const pane = panes.get(card.session)?.el.querySelector('.spane-head .name')?.textContent;
  backToBoard();
  const board = document.querySelector(`.card[data-sid="${card.id}"] .card-title`)?.textContent;
  const disk = JSON.parse((await inv('load_board')).data);
  const durable = disk.cards.find(c => c.id === card.id)?.title === 'renamed-enter-smoke';
  await report('rename', immediate && committed && durable
    && [sidebar, header, pane, board].every(v => v === 'renamed-enter-smoke'), 1, 1);
}

function pointer(type, id, x, y, button = 0) {
  return new PointerEvent(type, {
    pointerId: id, pointerType: 'mouse', isPrimary: true, button,
    buttons: type === 'pointerup' ? 0 : button === 2 ? 2 : 1,
    clientX: x, clientY: y, bubbles: true, cancelable: true,
  });
}

const fixtureClipboardLine = i => `R7C-${String(i).padStart(4, '0')}|中文|😀|é|👩‍💻|END`;
const fnv1a64 = text => {
  let hash = 0xcbf29ce484222325n;
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x100000001b3n);
  }
  return hash.toString(16).padStart(16, '0');
};

function visibleTerminalLine(pane, row) {
  const buffer = pane.term.buffer.active;
  return buffer.getLine(buffer.viewportY + row)?.translateToString(true) || '';
}

async function selectionSmoke(card) {
  let selectionStage = 0;
  try {
  selectionStage = 1;
  await openSession(card.id);
  const command = `python3 -c 'import time; f=lambda i: ((chr(96)*3+"rust") if i%101==0 else ("\\tTAB trailing   " if i%97==0 else ("" if i%89==0 else (f"R7-{i:04d}|中文|😀|é|👩‍💻️|" + ("x"*600 if i%113==0 else ""))))); [print(f(i)) for i in range(2500)]; print("R7-END",flush=True); [(time.sleep(.05),print(f"R7-LIVE-{j:03d}",flush=True)) for j in range(80)]'`;
  await inv('pty_write', { name: card.session, dataB64: strToB64(command + '\r') });
  await waitFor(async () => {
    const metrics = await inv('terminal_metrics', { name: card.session });
    return metrics.history_rows >= 2500;
  }, 15_000);
  selectionStage = 2;
  const pane = panes.get(card.session);
  const screen = pane.body.querySelector('.xterm-screen');
  let rect = screen.getBoundingClientRect();
  const neighbor = [...panes.values()].find(p => p !== pane);
  const neighborBefore = neighbor && await inv('terminal_metrics', { name: neighbor.session });

  selectionStage = 3;
  screen.dispatchEvent(pointer('pointerdown', 41, rect.left + 60, rect.bottom - 6));
  document.dispatchEvent(pointer('pointermove', 41, rect.left + 60, rect.top - 32));
  const started = await waitFor(() => pane.selection.hasSelection(), 3000);
  if (!started) {
    const metrics = await inv('terminal_metrics', { name: card.session });
    document.dispatchEvent(pointer('pointerup', 41, rect.left + 60, rect.top - 32));
    await report('selection-up', false, 0, metrics.in_copy_mode ? 1 : 0);
    return;
  }
  const stableDragPaint = await waitFor(() =>
    pane.selection.isDragging()
      && pane.body.querySelectorAll('.deck-selection-band').length > 0
      && !pane.body.querySelector('.xterm-cursor'), 3000);
  await report('selection-drag-overlay', stableDragPaint,
    pane.body.querySelectorAll('.deck-selection-band').length,
    pane.body.querySelector('.xterm-cursor') ? 1 : 0);
  const crossedUp = await waitFor(() => pane.selection.status()?.scroll_position > 120, 8000);
  const liveHistoryBefore = pane.selection.status()?.history_rows || 0;
  selectionStage = 4;
  let expanded = await copyTerminalSelection(pane);
  if (typeof expanded !== 'string') {
    const status = pane.selection.status();
    await report('selection-up', false, pane.selection.hasSelection() ? 1 : 0, status?.selection_present ? 1 : 0);
    return;
  }
  selectionStage = 5;
  let upMask = 0;
  if (crossedUp) upMask |= 1;
  if (expanded.split('\n').length > 100) upMask |= 2;
  if (expanded.includes('R7-')) upMask |= 4;
  if (expanded.includes('```rust')) upMask |= 8;
  // A terminal expands the source Tab before tmux owns it, and this broad
  // cross-screen half-open range need not include every padding cell. Assert
  // visible content here; exact trailing-space bytes have their own contract.
  if (expanded.includes('TAB trailing')) upMask |= 16;
  if (expanded.includes('\n\n')) upMask |= 32;
  if (expanded.includes('x'.repeat(200))) upMask |= 64;
  await report('selection-up', upMask === 127, expanded.split('\n').length, upMask);
  const markerIds = [...expanded.matchAll(/R7-(\d{4})/g)].map(match => Number(match[1]));
  const markerStart = markerIds[0] ?? -1;
  const markerEnd = markerIds.at(-1) ?? -1;
  await report('selection-markers', markerIds.length > 2 && markerStart < markerEnd,
    markerStart, markerEnd);
  const liveAdvanced = await waitFor(() =>
    (pane.selection.status()?.history_rows || liveHistoryBefore) > liveHistoryBefore, 3000);
  await report('selection-live', liveAdvanced && pane.selection.hasSelection(),
    (pane.selection.status()?.history_rows || liveHistoryBefore) - liveHistoryBefore,
    pane.selection.hasSelection() ? 1 : 0);

  document.dispatchEvent(pointer('pointermove', 41, rect.left + 60, rect.bottom + 32));
  const beforeReverse = pane.selection.status()?.scroll_position || 0;
  const reversed = await waitFor(() => (pane.selection.status()?.scroll_position || beforeReverse) < beforeReverse, 5000);
  const shrunk = await copyTerminalSelection(pane);
  selectionStage = 6;
  document.dispatchEvent(pointer('pointerup', 41, rect.left + 60, rect.bottom + 32));
  await report('selection-reverse', reversed && shrunk.length < expanded.length,
    expanded.length - shrunk.length, shrunk.length);

  /* Independent clipboard oracle: both endpoints are column zero on visible
     generated marker rows, so the expected half-open range is computed from
     the fixture generator. It never calls the production copy function. */
  await cancelTerminalSelection(pane);
  await inv('scroll_bottom', { name: card.session });
  const clipboardCommand = `python3 -c '[print(f"R7C-{i:04d}|中文|😀|é|👩‍💻|END") for i in range(400)]; print("R7C-DONE")'`;
  await inv('pty_write', { name: card.session, dataB64: strToB64(clipboardCommand + '\r') });
  const fixtureVisible = await waitFor(() => {
    for (let row = 0; row < pane.term.rows; row++) {
      if (visibleTerminalLine(pane, row).trim() === 'R7C-DONE') return true;
    }
    return false;
  }, 10_000);
  rect = screen.getBoundingClientRect();
  // After 400 newline-terminated rows and the DONE row, the returned shell
  // prompt occupies the last row; marker 399 is deterministically rows - 3.
  const anchorRow = pane.term.rows - 3;
  const cellX = rect.left + rect.width / pane.term.cols * 0.2;
  const rowY = row => rect.top + (row + 0.5) * rect.height / pane.term.rows;
  const downTrusted = anchorRow >= 0
    && screen.dispatchEvent(pointer('pointerdown', 61, cellX, rowY(anchorRow)));
  const compatDownReachedXterm = !screen.dispatchEvent(new MouseEvent('mousedown', {
    bubbles: true, cancelable: true, button: 0, buttons: 1,
    clientX: cellX, clientY: rowY(Math.max(0, anchorRow)),
  }));
  document.dispatchEvent(pointer('pointermove', 61, cellX, rect.top - 32));
  const exactStarted = await waitFor(() => pane.selection.hasSelection(), 3000);
  const exactCrossed = await waitFor(() => pane.selection.status()?.scroll_position > 80, 8000);
  const activeRow = Math.floor(pane.term.rows / 2);
  document.dispatchEvent(pointer('pointermove', 61, cellX, rowY(activeRow)));
  await pause(300);
  await pane.selection.idle();
  const exactStatus = pane.selection.status();
  const fixtureZero = exactStatus
    ? exactStatus.selection_start_row - 399 : Number.NaN;
  const activeId = exactStatus
    ? exactStatus.selection_end_row - fixtureZero : -1;
  const expected = activeId >= 0 && activeId < 399
    ? Array.from({ length: 399 - activeId }, (_, offset) => fixtureClipboardLine(activeId + offset)).join('\n') + '\n'
    : '';
  document.dispatchEvent(pointer('pointerup', 61, cellX, rowY(activeRow)));
  const lateMouseupTrusted = screen.dispatchEvent(new MouseEvent('mouseup', {
    bubbles: true, cancelable: true, button: 0, buttons: 0,
    clientX: cellX, clientY: rowY(activeRow),
  }));
  await pane.selection.idle();
  const ownership = pane.selection.ownership();
  const ownerMask = (downTrusted ? 1 : 0) | (compatDownReachedXterm ? 2 : 0)
    | (lateMouseupTrusted ? 4 : 0) | (ownership.promoted === 1 ? 8 : 0)
    | (!ownership.xtermSelection && ownership.owner === 'frozen-selection'
      && ownership.trustedClick === 0 && ownership.ended === 1 && ownership.frozen ? 16 : 0);
  await report('selection-owner', fixtureVisible && exactStarted && exactCrossed
    && ownerMask === 31,
  ownerMask, ownership.compatibilityBlocked);

  const expectedBytes = new TextEncoder().encode(expected).length;
  const expectedNewlines = (expected.match(/\n/g) || []).length;
  const expectedHash = fnv1a64(expected);
  pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'c', metaKey: true, bubbles: true, cancelable: true,
  }));
  const copiedByKey = await waitFor(async () => {
    const metrics = await inv('smoke_clipboard_metrics');
    return metrics.bytes === expectedBytes && metrics.newlines === expectedNewlines
      && metrics.hash === expectedHash;
  }, 3000);
  const clipboard = await inv('smoke_clipboard_metrics');
  selectionStage = 7;
  await report('selection-clipboard', expected.length > 0 && copiedByKey
    && clipboard.bytes === expectedBytes && clipboard.newlines === expectedNewlines
    && clipboard.hash === expectedHash, clipboard.bytes, clipboard.newlines);

  await cancelTerminalSelection(pane);
  pane.term.clearSelection();
  const clickRow = Math.max(2, pane.term.rows - 3);
  const clickX = rect.left + rect.width / pane.term.cols * 4.2;
  const clickY = rowY(clickRow);
  const physicalClick = (id, detail) => {
    screen.dispatchEvent(pointer('pointerdown', id, clickX, clickY));
    screen.dispatchEvent(new MouseEvent('mousedown', {
      bubbles: true, cancelable: true, button: 0, buttons: 1,
      clientX: clickX, clientY: clickY, detail,
    }));
    document.dispatchEvent(pointer('pointerup', id, clickX, clickY));
    screen.dispatchEvent(new MouseEvent('mouseup', {
      bubbles: true, cancelable: true, button: 0, buttons: 0,
      clientX: clickX, clientY: clickY, detail,
    }));
    screen.dispatchEvent(new MouseEvent('click', {
      bubbles: true, cancelable: true, button: 0,
      clientX: clickX, clientY: clickY, detail,
    }));
  };
  physicalClick(71, 1);
  const tapPlain = !pane.term.hasSelection();
  physicalClick(72, 2);
  const doubleWord = pane.term.hasSelection() && pane.term.getSelection().length > 0;
  const wordLength = pane.term.getSelection().length;
  physicalClick(73, 3);
  const tripleLine = pane.term.hasSelection()
    && pane.term.getSelection().length >= wordLength;
  const nativeExpected = pane.term.getSelection();
  const nativeExpectedBytes = new TextEncoder().encode(nativeExpected).length;
  const nativeExpectedNewlines = (nativeExpected.match(/\n/g) || []).length;
  const nativeExpectedHash = fnv1a64(nativeExpected);
  pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'C', metaKey: true, bubbles: true, cancelable: true,
  }));
  const nativeCopiedByKey = nativeExpected.length > 0 && await waitFor(async () => {
    const metrics = await inv('smoke_clipboard_metrics');
    return metrics.bytes === nativeExpectedBytes
      && metrics.newlines === nativeExpectedNewlines
      && metrics.hash === nativeExpectedHash;
  }, 3000);
  const rightUntouched = screen.dispatchEvent(pointer('pointerdown', 74, clickX, clickY, 2));
  pane.term.clearSelection();
  const singleAnchorRow = Math.max(2, pane.term.rows - 6);
  const cellWidth = rect.width / pane.term.cols;
  const singleStartCol = 7;
  const singleEndCol = 8;
  const singleStartX = rect.left + cellWidth * singleEndCol - 1;
  const singleEndX = rect.left + cellWidth * singleEndCol + 1;
  // Native xterm selection does not move the terminal cursor. Capture the
  // agent/shell input row before Deck promotes the drag and tmux replaces it
  // with a copy cursor at the selection endpoint.
  const liveCursorRowBeforeSingle = pane.term.buffer.active.cursorY;
  screen.dispatchEvent(pointer('pointerdown', 75, singleStartX, rowY(singleAnchorRow)));
  screen.dispatchEvent(new MouseEvent('mousedown', {
    bubbles: true, cancelable: true, button: 0, buttons: 1,
    clientX: singleStartX, clientY: rowY(singleAnchorRow),
  }));
  // Real WKWebView horizontal selection can deliver only compatibility
  // mousemove between pointerdown and pointerup. Two CSS pixels cross a cell;
  // Deck must promote this path before xterm retains a viewport-fixed range.
  const promotedMouseMoveBlocked = !screen.dispatchEvent(new MouseEvent('mousemove', {
    bubbles: true, cancelable: true, button: 0, buttons: 1,
    clientX: singleEndX, clientY: rowY(singleAnchorRow),
  }));
  const singleStarted = await waitFor(() => pane.selection.hasSelection(), 3000);
  document.dispatchEvent(pointer('pointerup', 75, singleEndX, rowY(singleAnchorRow)));
  screen.dispatchEvent(new MouseEvent('mouseup', {
    bubbles: true, cancelable: true, button: 0, buttons: 0,
    clientX: singleEndX, clientY: rowY(singleAnchorRow),
  }));
  await pane.selection.idle();
  const singleText = singleStarted ? await copyTerminalSelection(pane) : null;
  const singleExpected = visibleTerminalLine(pane, singleAnchorRow)
    .slice(singleStartCol, singleEndCol);
  const singleExpectedBytes = new TextEncoder().encode(singleExpected).length;
  pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'c', metaKey: true, bubbles: true, cancelable: true,
  }));
  const singleCopiedByKey = singleExpected.length > 0 && await waitFor(async () => {
    const metrics = await inv('smoke_clipboard_metrics');
    return metrics.bytes === singleExpectedBytes && metrics.newlines === 0
      && metrics.hash === fnv1a64(singleExpected);
  }, 3000);
  const singleOwned = singleStarted && promotedMouseMoveBlocked
    && pane.selection.ownership().promoted === 1
    && !pane.selection.ownership().xtermSelection
    && singleText === singleExpected && !singleText.includes('\n');
  const gestureMask = (tapPlain ? 1 : 0) | (doubleWord ? 2 : 0)
    | (tripleLine ? 4 : 0) | (rightUntouched && singleOwned ? 8 : 0)
    | (nativeCopiedByKey ? 16 : 0) | (singleCopiedByKey ? 32 : 0);
  await report('selection-gestures', gestureMask === 63, gestureMask, wordLength);

  /* The user-visible regression is specifically a physical single-row drag.
     Its immutable bytes and absolute row stay fixed while its one overlay
     band follows the tmux content frame, never the pointer's viewport row. */
  const stableBefore = pane.selection.status();
  const overlayBefore = new Map([...pane.body.querySelectorAll('.deck-selection-band')]
    .map(band => [band.dataset.absoluteRow, Number.parseFloat(band.style.top)]));
  const bytesBeforeScroll = await copyTerminalSelection(pane);
  await pane.selection.scroll(-3);
  const stableAfter = pane.selection.status();
  const bytesAfterScroll = await copyTerminalSelection(pane);
  let overlayAfter = new Map();
  const overlayMovedWithContent = await waitFor(() => {
    overlayAfter = new Map([...pane.body.querySelectorAll('.deck-selection-band')]
      .map(band => [band.dataset.absoluteRow, Number.parseFloat(band.style.top)]));
    const common = [...overlayBefore.keys()].filter(row => overlayAfter.has(row));
    return overlayBefore.size === 1 && overlayAfter.size === 1
      && common.some(row => overlayBefore.get(row) !== overlayAfter.get(row));
  }, 3000);
  const sameCoordinates = stableBefore.selection_start_row === stableAfter.selection_start_row
    && stableBefore.selection_start_col === stableAfter.selection_start_col
    && stableBefore.selection_end_row === stableAfter.selection_end_row
    && stableBefore.selection_end_col === stableAfter.selection_end_col;
  await report('selection-scroll-stable', sameCoordinates
    && bytesBeforeScroll === bytesAfterScroll, bytesAfterScroll?.length || 0,
  stableAfter.scroll_position - stableBefore.scroll_position);
  const liveCursorContentRow = liveCursorRowBeforeSingle + stableAfter.scroll_position;
  const expectedCursorRow = Math.min(pane.term.rows - 1, liveCursorContentRow);
  const expectedCursorVisible = liveCursorContentRow < pane.term.rows;
  const hiddenCursorStayedGone = expectedCursorVisible || await waitFor(() =>
    !pane.body.querySelector('.xterm-cursor'), 3000);
  const cursorFollowed = stableAfter.cursor_row === expectedCursorRow
    && stableAfter.cursor_visible === expectedCursorVisible
    && pane.scrollCursorVisible === expectedCursorVisible
    && hiddenCursorStayedGone;
  await report('selection-scroll-cursor', cursorFollowed,
    stableAfter.cursor_row, expectedCursorRow);
  await report('selection-overlay', overlayMovedWithContent
    && !pane.term.hasSelection(), overlayBefore.size, overlayAfter.size);

  /* Native word/line selections (double/triple click, or a focus click that
     WebKit folds into the next drag) must be adopted on the first wheel frame
     before tmux redraws the screen underneath xterm's fixed buffer rows. */
  await cancelTerminalSelection(pane);
  await inv('scroll_bottom', { name: card.session });
  const nativeRow = Math.max(2, pane.term.rows - 6);
  const nativeAbsoluteRow = pane.term.buffer.active.viewportY + nativeRow;
  pane.term.select(2, nativeAbsoluteRow, 4);
  const nativeScrollExpected = pane.term.getSelection();
  pane.body.dispatchEvent(new WheelEvent('wheel', {
    deltaY: -42, deltaMode: WheelEvent.DOM_DELTA_PIXEL,
    bubbles: true, cancelable: true,
  }));
  const nativeAdopted = await waitFor(() => pane.selection.isFrozen()
    && !pane.term.hasSelection()
    && pane.selection.status()?.scroll_position >= 3, 3000);
  await pane.selection.idle();
  const nativeCopied = nativeAdopted ? await copyTerminalSelection(pane) : null;
  const nativeBands = pane.body.querySelectorAll('.deck-selection-band').length;
  await report('selection-native-scroll', nativeAdopted
    && nativeScrollExpected.length === 4 && nativeCopied === nativeScrollExpected
    && nativeBands === 1, nativeCopied?.length || 0, nativeBands);

  /* Regression: start a second drag while tmux still owns the completed first
     selection. begin-selection is a toggle, so production must explicitly
     replace it rather than lose the new anchor. */
  screen.dispatchEvent(pointer('pointerdown', 76, cellX, rowY(singleAnchorRow + 2)));
  document.dispatchEvent(pointer('pointermove', 76,
    rect.left + rect.width / pane.term.cols * 10.2, rowY(singleAnchorRow)));
  const repeated = await waitFor(() => pane.selection.hasSelection(), 3000);
  document.dispatchEvent(pointer('pointerup', 76,
    rect.left + rect.width / pane.term.cols * 10.2, rowY(singleAnchorRow)));
  const repeatedText = repeated ? await copyTerminalSelection(pane) : null;
  await report('selection-repeat', repeated && typeof repeatedText === 'string'
    && repeatedText.length > 0, repeatedText?.length || 0, 0);

  await cancelTerminalSelection(pane);
  await inv('scroll_bottom', { name: card.session });

  /* Start a real pointer drag as soon as xterm observes a resize. This races
     the webview grid against the PTY confirmation and exercises the handshake
     that prevents coordinates from being applied to a stale tmux grid. The
     expected bytes come from visible fixture rows, independently of copy(). */
  rect = screen.getBoundingClientRect();
  const sidebarBeforeResize = document.body.classList.contains('side-collapsed');
  const resizeStartGrid = `${pane.term.cols}x${pane.term.rows}`;
  toggleSidebar();
  const xtermResized = await waitFor(() =>
    `${pane.term.cols}x${pane.term.rows}` !== resizeStartGrid, 5000);
  rect = screen.getBoundingClientRect();
  const resizeRows = pane.term.rows;
  const resizeAnchorRow = Math.max(2, resizeRows - 8);
  const resizeActiveRow = Math.max(resizeAnchorRow + 1, resizeRows - 5);
  const resizeLines = Array.from(
    { length: resizeActiveRow - resizeAnchorRow + 1 },
    (_, offset) => visibleTerminalLine(pane, resizeAnchorRow + offset),
  );
  const resizeFixtureReady = resizeLines.every(line => /^R7C-\d{4}\|/.test(line));
  const resizeExpected = resizeLines.slice(0, -1).join('\n') + '\n'
    + resizeLines.at(-1).slice(0, 8);
  const resizeStartX = rect.left + rect.width / pane.term.cols * 0.2;
  const resizeEndX = rect.left + rect.width / pane.term.cols * 8.2;
  screen.dispatchEvent(pointer('pointerdown', 77, resizeStartX, rowY(resizeAnchorRow)));
  document.dispatchEvent(pointer('pointermove', 77, resizeEndX, rowY(resizeActiveRow)));
  const resizeStarted = await waitFor(() => pane.selection.hasSelection(), 3000);
  await pane.selection.idle();
  const resizeSynced = await waitFor(async () => {
    const metrics = await inv('terminal_metrics', { name: card.session });
    return metrics.pane_cols === pane.term.cols && metrics.pane_rows === pane.term.rows;
  }, 5000);
  document.dispatchEvent(pointer('pointerup', 77, resizeEndX, rowY(resizeActiveRow)));
  await pane.selection.idle();
  const resizeStatus = pane.selection.status();
  const resizeText = resizeStarted ? await copyTerminalSelection(pane) : null;
  const resizeExact = xtermResized && resizeFixtureReady && resizeStarted && resizeSynced
    && resizeStatus?.selection_present && resizeText === resizeExpected;
  if (document.body.classList.contains('side-collapsed') !== sidebarBeforeResize) {
    const resizedGrid = `${pane.term.cols}x${pane.term.rows}`;
    toggleSidebar();
    await waitFor(async () => {
      const metrics = await inv('terminal_metrics', { name: card.session });
      return `${pane.term.cols}x${pane.term.rows}` !== resizedGrid
        && metrics.pane_cols === pane.term.cols && metrics.pane_rows === pane.term.rows;
    }, 5000);
  }
  await report('selection-resize', resizeExact,
    resizeText?.length || 0, resizeExpected.length);
  await cancelTerminalSelection(pane);
  await inv('scroll_bottom', { name: card.session });

  /* Real wheel routing: two sub-threshold pixel events must combine into one
     line; the retained rounding remainder must absorb the complementary tail;
     DOM_DELTA_LINE must remain line-scaled. */
  const wheel = (deltaY, deltaMode = 0) => screen.dispatchEvent(new WheelEvent('wheel', {
    deltaY, deltaMode, bubbles: true, cancelable: true,
  }));
  wheel(-4);
  await pause(100);
  const wheelSub = await inv('terminal_metrics', { name: card.session });
  wheel(-4);
  const wheelFirst = await waitFor(async () => {
    const metrics = await inv('terminal_metrics', { name: card.session });
    return metrics.scroll_position === 1 && metrics;
  }, 3000);
  wheel(-6);
  await pause(100);
  const wheelRemainder = await inv('terminal_metrics', { name: card.session });
  wheel(-2, 1);
  const wheelLines = await waitFor(async () => {
    const metrics = await inv('terminal_metrics', { name: card.session });
    return metrics.scroll_position === 3 && metrics;
  }, 3000);
  let wheelMask = 0;
  if (!wheelSub.in_copy_mode && wheelSub.scroll_position === 0) wheelMask |= 1;
  if (wheelFirst) wheelMask |= 2;
  if (wheelRemainder.scroll_position === 1) wheelMask |= 4;
  if (wheelLines) wheelMask |= 8;
  const cursorHidden = await waitFor(() => !pane.body.querySelector('.xterm-cursor'), 3000);
  wheel(60, 1);
  const returnedLive = await waitFor(async () => {
    const metrics = await inv('terminal_metrics', { name: card.session });
    return !metrics.in_copy_mode && !!pane.body.querySelector('.xterm-cursor');
  }, 3000);
  if (cursorHidden && returnedLive) wheelMask |= 16;
  await report('scroll-frame', wheelMask === 31, wheelMask,
    wheelRemainder.scroll_position);

  for (let i = 0; i < 12; i++) {
    await inv('scroll_session', { name: card.session, lines: -60 });
  }
  rect = screen.getBoundingClientRect();
  const downMetrics = await inv('terminal_metrics', { name: card.session });
  const downStart = downMetrics.history_rows;
  screen.dispatchEvent(pointer('pointerdown', 42, rect.left + 60, rect.top + 6));
  document.dispatchEvent(pointer('pointermove', 42, rect.left + 60, rect.bottom + 32));
  const startedDown = await waitFor(() => pane.selection.hasSelection(), 3000);
  if (!startedDown) {
    const metrics = await inv('terminal_metrics', { name: card.session });
    await report('selection-down', false, metrics.in_copy_mode ? 1 : 0, 0);
    await inv('scroll_bottom', { name: card.session });
  }
  const downInitialScroll = pane.selection.status()?.scroll_position || 0;
  const crossedDown = await waitFor(() =>
    (pane.selection.status()?.scroll_position ?? downInitialScroll)
      < downInitialScroll - pane.term.rows * 3, 8000);
  const downText = startedDown ? (await copyTerminalSelection(pane) || '') : '';
  selectionStage = 8;
  document.dispatchEvent(pointer('pointerup', 42, rect.left + 60, rect.bottom + 32));
  const crossedDownEvidence = crossedDown
    || downText.split('\n').length > pane.term.rows * 3;
  const downMask = (downMetrics.scroll_position > pane.term.rows * 10 ? 1 : 0)
    | (startedDown ? 2 : 0) | (crossedDownEvidence ? 4 : 0)
    | (downText.length > 1000 ? 8 : 0) | (downStart >= 2500 ? 16 : 0);
  await report('selection-down', downMask === 31,
    downText.split('\n').length, downMask);

  await cancelTerminalSelection(pane);
  screen.dispatchEvent(pointer('pointerdown', 43, rect.left + 60, rect.bottom - 6));
  document.dispatchEvent(pointer('pointermove', 43, rect.left + 60, rect.top - 32));
  const cancelStarted = await waitFor(() => pane.selection.hasSelection());
  toggleSidebar();
  await pause(300);
  rect = screen.getBoundingClientRect();
  pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'Escape', bubbles: true, cancelable: true,
  }));
  const cancelled = await waitFor(async () => !(await inv('terminal_metrics', { name: card.session })).in_copy_mode);
  let cancelMask = 0;
  if (cancelStarted) cancelMask |= 1;
  if (cancelled) cancelMask |= 2;
  if (!pane.selection.hasSelection()) cancelMask |= 4;
  const cancelMetrics = await inv('terminal_metrics', { name: card.session });
  await report('selection-cancel', cancelMask === 7, cancelMask,
    cancelMetrics.in_copy_mode ? 1 : 0);

  if (document.body.classList.contains('side-collapsed')) toggleSidebar();
  const neighborAfter = neighbor && await inv('terminal_metrics', { name: neighbor.session });
  await report('selection-split', !neighbor || (!neighborBefore.in_copy_mode
    && !neighborAfter.in_copy_mode && neighborAfter.pane_rows === neighbor.term.rows
    && attachedName === pane.session), neighborAfter?.pane_rows || 0, 0);

  const neighborSid = neighbor?.sid;
  rect = screen.getBoundingClientRect();
  screen.dispatchEvent(pointer('pointerdown', 44, rect.left + 60, rect.bottom - 6));
  document.dispatchEvent(pointer('pointermove', 44, rect.left + 60, rect.top - 32));
  const selectedBeforeDetach = await waitFor(() => pane.selection.hasSelection(), 3000);
  document.dispatchEvent(pointer('pointerup', 44, rect.left + 60, rect.top - 32));
  backToBoard();
  const detachedClean = await waitFor(async () =>
    !(await inv('terminal_metrics', { name: card.session })).in_copy_mode, 3000);
  await openSession(card.id);
  if (neighborSid) await addSplit(card.id, 'row', false, neighborSid);
  await report('selection-detach', selectedBeforeDetach && detachedClean
    && panes.has(card.session) && (!neighborSid || [...panes.values()].some(p => p.sid === neighborSid)),
  panes.size, detachedClean ? 1 : 0);
  } catch (error) {
    const message = String(error);
    const category = message.includes('dimensions') ? 1
      : message.includes('tmux') ? 2
      : message.includes('selection') ? 3
      : message.includes('session') ? 4 : 9;
    await report('selection-up', false, selectionStage, category);
    throw error;
  }
}

async function pathSmoke(card) {
  await openSession(card.id);
  const pane = panes.get(card.session);
  /* openSession can resolve before its first fit RAF. Generate width-sensitive
     wrap fixtures only after the xterm grid and PTY/tmux grid agree; otherwise
     the fixture itself is nondeterministic rather than testing redraw links. */
  pane.fit.fit();
  await waitFor(async () => {
    await pane.syncSize().catch(() => {});
    const metrics = await inv('terminal_metrics', { name: card.session });
    return metrics.pane_cols === pane.term.cols && metrics.pane_rows === pane.term.rows;
  }, 5000);
  const fixture = '"空 格😀/code.rs":12:3';
  const addressFixture = '192.168.31.120:6443';
  const missingFixture = 'memcache.go:265';
  const url = `https://node100.gitski.work:6443/api?timeout=32s&trace=${'a'.repeat(pane.term.cols + 12)}`;
  const urlLine = `E0829 memcache.go:265] err=\\"${url}\\": connect failed`;
  const hardUrl = 'https://hardwrap.gitski.work:6443/api?timeout=32s';
  const hardCut = 13; // `https://hardw`, matching the short first-row failure
  const hardPrefix = `${'H'.repeat(Math.max(0, pane.term.cols - hardCut - 1))} `;
  await inv('pty_write', {
    name: card.session,
    dataB64: strToB64(`mkdir -p '空 格😀' && : > '空 格😀/code.rs'; printf '%s\\n' '${fixture}' '${addressFixture}' '${missingFixture}' '${urlLine}'; printf '%s%s\\n%s\\n' '${hardPrefix}' '${hardUrl.slice(0, hardCut)}' '${hardUrl.slice(hardCut)}'\r`),
  });
  const fixtureReady = await waitFor(() => {
    for (let row = 0; row < pane.term.rows; row++) {
      if (visibleTerminalLine(pane, row).trim() === fixture) return true;
    }
    return false;
  }, 5000);
  const screen = pane.body.querySelector('.xterm-screen');
  const rect = screen.getBoundingClientRect();
  let fixtureRow = -1, fixtureCol = -1;
  for (let row = 0; row < pane.term.rows; row++) {
    const line = visibleTerminalLine(pane, row);
    const col = line.trim() === fixture ? line.indexOf(fixture) : -1;
    if (col >= 0) { fixtureRow = row; fixtureCol = col; break; }
  }
  const linkX = rect.left + (fixtureCol + 1.5) * rect.width / pane.term.cols;
  const linkY = rect.top + (fixtureRow + 0.5) * rect.height / pane.term.rows;
  screen.dispatchEvent(new MouseEvent('mousemove', {
    bubbles: true, clientX: linkX, clientY: linkY,
  }));
  await waitFor(() => pane.body.querySelector('.xterm')?.classList.contains('xterm-cursor-pointer'), 3000);
  screen.dispatchEvent(pointer('pointerdown', 31, linkX, linkY));
  screen.dispatchEvent(new MouseEvent('mousedown', {
    bubbles: true, cancelable: true, button: 0, buttons: 1,
    clientX: linkX, clientY: linkY, detail: 1,
  }));
  document.dispatchEvent(pointer('pointerup', 31, linkX, linkY));
  screen.dispatchEvent(new MouseEvent('mouseup', {
    bubbles: true, cancelable: true, button: 0, buttons: 0,
    clientX: linkX, clientY: linkY, detail: 1,
  }));
  screen.dispatchEvent(new MouseEvent('click', {
    bubbles: true, cancelable: true, button: 0,
    clientX: linkX, clientY: linkY, detail: 1,
  }));
  const serviceOpened = fixtureReady && $('ctx').style.display === 'block'
    && $('ctx').querySelector('.ctx-value')?.textContent === fixture;
  $('ctx').style.display = 'none';
  const linksAt = row => new Promise(resolve => {
    pane.linkProvider.provideLinks(pane.term.buffer.active.viewportY + row + 1,
      links => resolve(links || []));
  });
  const fixtureLinks = await linksAt(fixtureRow);
  const providerLink = fixtureLinks.find(link => link.text === fixture) || null;
  providerLink?.activate(eventAt(linkX, linkY), providerLink.text);
  const providerOpened = $('ctx').style.display === 'block'
    && $('ctx').querySelector('.ctx-value')?.textContent === fixture;
  screen.dispatchEvent(new MouseEvent('click', {
    bubbles: true, cancelable: true, button: 0,
    clientX: linkX, clientY: linkY, detail: 1,
  }));
  const survivedOpeningClick = $('ctx').style.display === 'block';
  $('ctx').querySelector('[data-a="copy"]')?.click();
  let addressRow = -1;
  for (let row = 0; row < pane.term.rows; row++) {
    if (visibleTerminalLine(pane, row).trim() === addressFixture) {
      addressRow = row;
      break;
    }
  }
  let addressLinks = [];
  if (addressRow >= 0) {
    addressLinks = await linksAt(addressRow);
  }
  const addressUnlinked = addressRow >= 0 && addressLinks.length === 0;
  const linkMask = (fixtureReady ? 1 : 0) | (providerLink ? 2 : 0)
    | (providerOpened ? 4 : 0) | (survivedOpeningClick ? 8 : 0)
    | (serviceOpened ? 16 : 0) | (addressUnlinked ? 32 : 0);
  await report('link-activate', linkMask === 63 && $('ctx').style.display === 'none',
    linkMask, fixture.length);

  const urlReady = await waitFor(() => {
    let visible = '';
    for (let row = 0; row < pane.term.rows; row++) visible += visibleTerminalLine(pane, row);
    return visible.includes(url) && visible.includes(hardUrl);
  }, 5000);
  const findLastVisibleRow = predicate => {
    /* The shell first echoes the long printf command and then prints its
       fixture output. The output is the later matching row; selecting the
       first occurrence accidentally tested URL text embedded in shell syntax. */
    for (let row = pane.term.rows - 1; row >= 0; row--) {
      if (predicate(visibleTerminalLine(pane, row))) return row;
    }
    return -1;
  };
  /* The shell prompt can arrive after urlReady and shift every visible row by
     one while async path validation is in flight. Take each provider result
     only when the row still identifies the same content afterwards; this keeps
     the smoke deterministic without weakening the production assertion. */
  const stableLinksAt = async (finder, accept = () => true) => {
    let snapshot = { row: -1, links: [], line: null };
    await waitFor(async () => {
      const row = finder();
      if (row < 0) return false;
      const absolute = pane.term.buffer.active.viewportY + row;
      const links = await linksAt(row);
      if (finder() !== row) return false;
      snapshot = { row, links, line: pane.term.buffer.active.getLine(absolute) };
      return accept(snapshot);
    }, 3000);
    return snapshot;
  };
  const missingSnapshot = await stableLinksAt(() =>
    findLastVisibleRow(line => line.includes(missingFixture) && !line.includes('E0829')));
  const urlSnapshot = await stableLinksAt(() =>
    findLastVisibleRow(line => line.includes('https://node100')),
  snapshot => snapshot.links.some(link => link.text === url));
  const hardSnapshot = await stableLinksAt(() =>
    findLastVisibleRow(line => line.includes('https://hardw')),
  snapshot => snapshot.links.some(link => link.text === hardUrl));
  const missingRow = missingSnapshot.row;
  const missingLinks = missingSnapshot.links;
  const urlLinks = urlSnapshot.links;
  const hardLinks = hardSnapshot.links;
  const wrappedUrl = urlLinks.find(link => link.text === url) || null;
  const hardWrappedUrl = hardLinks.find(link => link.text === hardUrl) || null;
  const hardFirstLine = hardSnapshot.line;
  const hardRedrawRecovered = !!hardWrappedUrl && hardFirstLine?.isWrapped === false;
  wrappedUrl?.activate(eventAt(linkX, linkY), wrappedUrl.text);
  const exactUrlMenu = $('ctx').style.display === 'block'
    && $('ctx').querySelector('.ctx-value')?.textContent === url;
  const tokenizerMask = (urlReady ? 1 : 0)
    | (missingRow >= 0 && missingLinks.length === 0 ? 2 : 0)
    | (wrappedUrl && urlLinks.length === 1 ? 4 : 0)
    | (exactUrlMenu ? 8 : 0)
    | (hardRedrawRecovered ? 16 : 0);
  $('ctx').style.display = 'none';
  const urlLogical = urlSnapshot.row >= 0
    ? terminalLogicalLine(pane.term, pane.term.buffer.active.viewportY + urlSnapshot.row + 1) : null;
  const hardLogical = hardSnapshot.row >= 0
    ? terminalLogicalLine(pane.term, pane.term.buffer.active.viewportY + hardSnapshot.row + 1) : null;
  const classifierDebug = (urlSnapshot.row >= 0 ? 1 : 0)
    | (urlLogical?.text.includes(url) ? 2 : 0)
    | (tokenizeTerminalLinks(urlLogical?.text || '').some(token => token.value === url) ? 4 : 0)
    | (hardSnapshot.row >= 0 ? 8 : 0)
    | (hardLogical?.text.includes(hardUrl) ? 16 : 0)
    | (tokenizeTerminalLinks(hardLogical?.text || '').some(token => token.value === hardUrl) ? 32 : 0)
    | (hardFirstLine?.isWrapped === false ? 64 : 0);
  await report('link-classify', tokenizerMask === 31, tokenizerMask, classifierDebug);

  const focus = document.createElement('button');
  focus.textContent = 'focus';
  document.body.appendChild(focus);
  focus.focus();
  showLinkCtx(eventAt(), 'url', 'https://example.com', card.dir, card.id);
  const urlActions = [...$('ctx').querySelectorAll('button')].map(b => b.dataset.a).join(',');
  $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  showLinkCtx(eventAt(), 'path', fixture, card.dir, card.id);
  const pathActions = [...$('ctx').querySelectorAll('button')].map(b => b.dataset.a).join(',');
  const first = $('ctx').querySelector('button');
  first.focus();
  $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
  const keyboard = document.activeElement?.dataset?.a === 'editor-parent';
  $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  await report('path-menu', urlActions === 'url,copy'
    && pathActions === 'editor,editor-parent,session-parent,reveal,copy'
    && keyboard && document.activeElement === focus, 2, 5);

  await inv('save_settings', { data: JSON.stringify({ editor: 'Cursor', debug: false }) });
  let editorOpened = false;
  try {
    await inv('open_target', { kind: 'editor-parent', value: fixture, cwd: card.dir });
    editorOpened = true;
  } catch (e) { /* report false */ }
  await report('path-editor', editorOpened, 1, 0);

  const beforeRelative = store.cards.length;
  showLinkCtx(eventAt(), 'path', fixture, card.dir, card.id);
  $('ctx').querySelector('[data-a="session-parent"]').click();
  const relativeMade = await waitFor(() => store.cards.length > beforeRelative, 10000);
  const relative = store.cards.at(-1);
  await report('path-session-relative', relativeMade && relative?.dir.endsWith('/空 格😀'), 1, 0);

  const beforeAbsolute = store.cards.length;
  const absoluteValue = `${card.dir}/空 格😀/code.rs`;
  showLinkCtx(eventAt(), 'path', absoluteValue, card.dir, card.id);
  $('ctx').querySelector('[data-a="session-parent"]').click();
  const absoluteMade = await waitFor(() => store.cards.length > beforeAbsolute, 10000);
  const absolute = store.cards.at(-1);
  await report('path-session-absolute', absoluteMade && absolute?.dir.endsWith('/空 格😀'), 1, 0);
  focus.remove();
}

async function imeRoutingSmoke(card) {
  await openSession(card.id);
  const pane = panes.get(card.session);
  const screen = pane.body.querySelector('.xterm-screen');
  await waitFor(() => {
    const ready = screen.getBoundingClientRect();
    return ready.width > 100 && ready.height > 100;
  }, 3000);
  const rect = screen.getBoundingClientRect();
  const x = rect.left + 40;
  const y0 = rect.top + rect.height * 0.35;
  const y1 = rect.top + rect.height * 0.55;
  screen.dispatchEvent(pointer('pointerdown', 35, x, y0));
  document.dispatchEvent(pointer('pointermove', 35, x + 20, y1));
  const selectionStarted = await waitFor(() => pane.selection.hasSelection(), 3000);
  document.dispatchEvent(pointer('pointerup', 35, x + 20, y1));
  await pane.selection.idle();
  const hadSelection = selectionStarted && pane.selection.hasSelection();
  const sidebarBefore = document.body.classList.contains('side-collapsed');
  const composingShortcut = new KeyboardEvent('keydown', {
    key: 'b', keyCode: 229, metaKey: true, isComposing: true,
    bubbles: true, cancelable: true,
  });
  pane.term.textarea.dispatchEvent(composingShortcut);
  const shortcutBypassed = document.body.classList.contains('side-collapsed') === sidebarBefore;
  pane.term.textarea.dispatchEvent(new CompositionEvent('compositionstart', {
    data: '', bubbles: true, cancelable: true,
  }));
  await (pane.liveQ || Promise.resolve());
  const cleaned = !pane.selection.hasSelection() && pane.term.options.disableStdin === false;
  for (const key of ['Process', 'Dead', 'Compose', '[', ']', '?', '(', ')']) {
    pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
      key, keyCode: 229, isComposing: true, bubbles: true, cancelable: true,
    }));
  }
  pane.term.textarea.dispatchEvent(new CompositionEvent('compositionend', {
    data: '', bubbles: true, cancelable: true,
  }));
  await pause(20);

  /* Reproduce both WKWebView orders reported for printable IME keyCode=229:
     input-before-keydown and keydown-before-input. The fixture characters are
     test-only; telemetry records only the closed mask below. */
  const imeData = [];
  const dataSub = pane.term.onData(data => imeData.push(data));
  const dispatchImePrintable = (key, code, keyUpCode, inputFirst) => {
    const input = () => {
      pane.term.textarea.value += key;
      pane.term.textarea.dispatchEvent(new InputEvent('input', {
        data: key, inputType: 'insertText', bubbles: true,
        cancelable: false, composed: true,
      }));
    };
    if (inputFirst) input();
    pane.term.textarea.dispatchEvent(new KeyboardEvent('keydown', {
      key, code, keyCode: 229, bubbles: true, cancelable: true,
    }));
    if (!inputFirst) input();
    pane.term.textarea.dispatchEvent(new KeyboardEvent('keyup', {
      key, code, keyCode: keyUpCode, bubbles: true, cancelable: true,
    }));
  };
  const dispatchShift = type => pane.term.textarea.dispatchEvent(new KeyboardEvent(type, {
    key: 'Shift', code: 'ShiftLeft', keyCode: 16,
    shiftKey: type === 'keydown', bubbles: true, cancelable: true,
  }));
  dispatchShift('keydown');
  dispatchImePrintable('?', 'Slash', 191, true);
  dispatchShift('keyup');
  dispatchShift('keydown');
  dispatchImePrintable(']', 'BracketRight', 221, false);
  dispatchShift('keyup');
  await pause(30);
  dataSub.dispose();
  pane.term.textarea.value = '';
  const nativeInputExact = imeData.join('') === '?]';
  await inv('pty_write', { name: card.session, dataB64: strToB64('\x03') });
  const imeMask = (hadSelection && cleaned ? 1 : 0)
    | (shortcutBypassed ? 2 : 0) | (nativeInputExact ? 4 : 0);
  await report('ime-routing', imeMask === 7, imeMask, 7);
}

async function completionSmoke(card, project, column) {
  await openSession(card.id);
  await pause(1100); // let fresh-pane clear_history finish before generating the fixture
  const other = await provider.create({
    projectId: project.id, columnId: column.id, title: 'completion-neighbor', cmd: '', dir: '/tmp',
  });
  await addSplit(card.id, 'row', false, other.id);
  await openSession(card.id);
  const pane = panes.get(card.session);
  await inv('pty_write', {
    name: card.session,
    dataB64: strToB64("python3 -c 'for i in range(120): print(i)'\r"),
  });
  let historyMetrics;
  await waitFor(async () => {
    historyMetrics = await inv('terminal_metrics', { name: card.session });
    return historyMetrics.history_rows > 20;
  }, 8000);
  await waitFor(() => pane.term.buffer.active.cursorY >= pane.term.rows - 2, 8000);
  const readPtyRows = async () => (await inv('terminal_metrics', { name: card.session })).pane_rows;
  const waitForPtyRows = async () => {
    let rows = 0;
    await waitFor(async () => {
      rows = await readPtyRows();
      return rows === pane.term.rows;
    }, 2500);
    return rows;
  };
  resetSuggest();
  pane.term.scrollToBottom();
  await pause(120);
  const rowsBefore = pane.term.rows;
  const historyPresent = historyMetrics.history_rows > 20;
  const promptAtBottom = pane.term.buffer.active.cursorY >= rowsBefore - 2;
  globalThis.histCache = ['echo one', 'echo two', 'echo three'];
  globalThis.lineBuf = 'ec';
  globalThis.freshShell = false;
  renderSuggest();
  await pause(450);
  const bar = $('quick-bar').getBoundingClientRect();
  const screen = pane.body.getBoundingClientRect(); // the clipped visible terminal viewport
  const neighbor = panes.get(other.session)?.el.getBoundingClientRect();
  const rowsAfter = pane.term.rows;
  const ptyShown = await waitForPtyRows();
  const bottomFollowed = pane.term.buffer.active.viewportY >= pane.term.buffer.active.baseY;
  await metric('completion-pixels', screen.height * 100, bar.height * 100);
  await metric('completion-gap', (bar.top - screen.bottom) * 100,
    neighbor ? (neighbor.left - bar.right) * 100 : 0);
  let bottomMask = 0;
  if (historyPresent) bottomMask |= 1;
  if (promptAtBottom) bottomMask |= 2;
  if (bottomFollowed) bottomMask |= 4;
  if (rowsAfter > 0 && rowsAfter < rowsBefore) bottomMask |= 8;
  if (ptyShown === rowsAfter) bottomMask |= 16;
  await metric('completion-bottom', bottomMask,
    rowsBefore * 1_000_000 + rowsAfter * 1000 + ptyShown);
  let mask = 0;
  if ($('quick-bar').parentElement === pane.el) mask |= 1;
  if (!overlaps(bar, screen)) mask |= 2;
  if (!neighbor || !overlaps(bar, neighbor)) mask |= 4;
  if (historyPresent && promptAtBottom && bottomFollowed
      && rowsAfter > 0 && rowsAfter < rowsBefore && ptyShown === rowsAfter) mask |= 8;

  resetSuggest();
  await pause(250);
  const scrollBefore = await inv('scroll_session', { name: card.session, lines: -12 });
  globalThis.lineBuf = 'ec';
  renderSuggest();
  await pause(350);
  const scrollAfter = await inv('scroll_session', { name: card.session, lines: 0 });
  await metric('completion-scroll', scrollBefore ? 1 : 0, scrollAfter ? 1 : 0);
  if (scrollBefore && scrollAfter) mask |= 16;
  await inv('scroll_bottom', { name: card.session });

  const wasCollapsed = document.body.classList.contains('side-collapsed');
  toggleSidebar();
  await pause(500);
  const resizedBar = $('quick-bar').getBoundingClientRect();
  const resizedScreen = pane.body.getBoundingClientRect();
  const ptyResized = await waitForPtyRows();
  await metric('completion-resize', pane.term.rows, ptyResized);
  if (!overlaps(resizedBar, resizedScreen) && ptyResized === pane.term.rows) mask |= 32;
  if (document.body.classList.contains('side-collapsed') !== wasCollapsed) toggleSidebar();
  await pause(350);

  const longPrefix = 'echo ' + '宽😀'.repeat(80);
  globalThis.histCache = [longPrefix + ' tail'];
  for (let i = 0; i < 4; i++) {
    globalThis.lineBuf = longPrefix;
    renderSuggest();
    resetSuggest();
  }
  globalThis.lineBuf = longPrefix;
  renderSuggest();
  await pause(350);
  const longBar = $('quick-bar').getBoundingClientRect();
  const longScreen = pane.body.getBoundingClientRect();
  const longPty = await waitForPtyRows();
  await metric('completion-long', $('quick-bar').style.display === 'flex' ? pane.term.rows : 0, longPty);
  if ($('quick-bar').style.display === 'flex' && !overlaps(longBar, longScreen)
      && longPty === pane.term.rows) mask |= 64;

  resetSuggest();
  await pause(350);
  const ptyHidden = await waitForPtyRows();
  await metric('completion-hidden', pane.term.rows, ptyHidden);
  if ($('quick-bar').style.display === 'none'
      && $('quick-bar').getBoundingClientRect().height === 0
      && pane.term.rows >= rowsAfter && ptyHidden === pane.term.rows) mask |= 128;
  await report('completion', mask === 255, mask, rowsBefore * 1000 + rowsAfter);
}

async function completionOwnerSmoke(card) {
  const paneA = panes.get(card.session);
  const paneB = [...panes.values()].find(p => p !== paneA);
  if (!paneA || !paneB) {
    await report('completion-owner', false, 0, 0);
    return;
  }
  const show = pane => {
    focusPane(pane.session);
    globalThis.histCache = ['echo owner transition'];
    globalThis.lineBuf = 'ec';
    globalThis.freshShell = false;
    renderSuggest();
  };
  show(paneA); show(paneB); show(paneA);
  await pause(700);
  const a1 = await inv('terminal_metrics', { name: paneA.session });
  const b1 = await inv('terminal_metrics', { name: paneB.session });
  let mask = 0;
  if (a1.pane_rows === paneA.term.rows && b1.pane_rows === paneB.term.rows) mask |= 1;
  show(paneB);
  await pause(350);
  await addSplit(paneA.sid, 'col', false, paneB.sid);
  await pause(500);
  const a2 = await inv('terminal_metrics', { name: paneA.session });
  const b2 = await inv('terminal_metrics', { name: paneB.session });
  if (a2.pane_rows === paneA.term.rows && b2.pane_rows === paneB.term.rows) mask |= 2;
  closePaneBySid(paneB.sid);
  await pause(600);
  const a3 = await inv('terminal_metrics', { name: paneA.session });
  if (!panes.has(paneB.session) && a3.pane_rows === paneA.term.rows
      && $('quick-bar').style.display === 'none') mask |= 4;
  await report('completion-owner', mask === 7, mask, a3.pane_rows * 1000 + paneA.term.rows);
}

async function naturalExitFaultSmoke(project, column) {
  const card = await provider.create({
    projectId: project.id, columnId: column.id, title: 'natural-fault', cmd: '', dir: '/tmp',
  });
  await openSession(card.id);
  await pollNow();
  await pause(1100); // wait out the fresh-shell history cleanup
  await inv('pty_write', {
    name: card.session,
    dataB64: strToB64("python3 -c '[print(f\"EXIT-{i:03d}\") for i in range(60)]'\r"),
  });
  await waitFor(async () =>
    (await inv('terminal_metrics', { name: card.session })).history_rows > 30);
  const pane = panes.get(card.session);
  const screen = pane.body.querySelector('.xterm-screen');
  const rect = screen.getBoundingClientRect();
  screen.dispatchEvent(pointer('pointerdown', 51, rect.left + 50, rect.bottom - 5));
  document.dispatchEvent(pointer('pointermove', 51, rect.left + 50, rect.top + 5));
  const selectedBeforeExit = await waitFor(() => pane.selection.hasSelection(), 3000);
  document.dispatchEvent(pointer('pointerup', 51, rect.left + 50, rect.top + 5));
  card.status = 'running';
  await inv('smoke_fault_set', { kind: 'queue-cancel', count: 8 });
  await inv('kill_session', { name: card.session });
  const selectionCleared = await waitFor(() => !pane.selection.hasSelection(), 3000);
  await pollNow();
  const keptAfterCancel = !!provider.get(card.id) && panes.has(card.session);
  await inv('smoke_fault_set', { kind: 'queue-cancel', count: 0 });
  await inv('smoke_fault_set', { kind: 'board-save', count: 8 });
  await pollNow();
  const keptAfterSave = !!provider.get(card.id) && panes.has(card.session);
  await inv('smoke_fault_set', { kind: 'board-save', count: 0 });
  await pollNow();
  const retired = await waitFor(() => !provider.get(card.id) && !panes.has(card.session), 6000);
  await pollNow();
  const stable = !provider.get(card.id) && !panes.has(card.session);
  const mask = (selectedBeforeExit ? 1 : 0) | (selectionCleared ? 2 : 0)
    | (keptAfterCancel ? 4 : 0) | (keptAfterSave ? 8 : 0)
    | (retired ? 16 : 0) | (stable ? 32 : 0);
  await report('natural-fault', mask === 63, mask, 63);
}

export async function run() {
  let stage = 0;
  try {
    stage = 1;
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    const column = project.columns.find(c => c.semantic === 'working') || project.columns[0];
    stage = 2;
    // Keep one harmless pane alive before exercising delete transactions.
    // A fresh lazy tmux server has no socket yet, so kill_session correctly
    // distinguishes that server-level failure from an already-missing pane.
    // Starting the main fixture first makes this smoke self-contained on a
    // machine with no pre-existing deck-smoke server or fixture directory.
    const main = await provider.create({
      projectId: project.id, columnId: column.id, title: 'wk-smoke', cmd: '', dir: '/tmp',
    });
    render();
    await openSession(main.id);
    stage = 3;
    await boardConcurrency(project, column);
    stage = 4;
    await boardFaultSmoke(project, column);
    stage = 5;
    await renameSmoke(main);
    stage = 6;
    await pathSmoke(main);
    stage = 7;
    await completionSmoke(main, project, column);
    stage = 8;
    await themeSmoke(main);
    stage = 9;
    await selectionSmoke(main);
    stage = 10;
    await imeRoutingSmoke(main);
    stage = 11;
    await completionOwnerSmoke(main);
    stage = 12;
    await naturalExitFaultSmoke(project, column);
    stage = 13;
    await inv('queue_add', { args: {
      session: main.session, cardId: main.id, dir: main.dir, cmd: main.cmd,
      text: 'deterministic smoke delivery', mode: 'at',
      at: Math.floor(Date.now() / 1000) + 3600,
      every: null, winFrom: null, winTo: null, untilN: null, untilAt: null,
    } });
    await openSession(main.id);
    toggleQueuePanel(true);
    await refreshQueue();
    const contextItem = (await inv('queue_list')).items?.find(item => item.card_id === main.id);
    const probe = contextItem && await inv('queue_probe_context', { id: contextItem.id });
    await refreshQueue();
    const hooklessReady = contextItem?.expected_process == null
      && contextItem?.attempts === 0 && probe?.status === 'ready';
    await inv('queue_send_now', { id: contextItem.id, acceptProcessMismatch: false });
    const compatibilitySent = !(await inv('queue_list')).items?.some(item => item.id === contextItem.id);
    const noPolicy = !$('q-policy') && !document.querySelector('#queue-list .q-policy');
    await inv('queue_add', { args: {
      session: main.session, cardId: main.id, dir: main.dir, cmd: 'codex',
      text: 'must remain queued during mismatch smoke', mode: 'at',
      at: Math.floor(Date.now() / 1000) + 3600,
      every: null, winFrom: null, winTo: null, untilN: null, untilAt: null,
    } });
    await refreshQueue();
    const mismatchItem = (await inv('queue_list')).items?.find(item => item.card_id === main.id);
    const mismatchProbe = mismatchItem && await inv('queue_probe_context', { id: mismatchItem.id });
    await refreshQueue();
    document.querySelector('#queue-list .q-now')?.click();
    const dangerShown = await waitFor(() => $('cfm').style.display === 'flex', 2000);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await pause(100);
    const enterRejected = $('cfm').style.display === 'flex';
    $('cfm-no')?.click();
    await pause(100);
    const mismatchWaits = mismatchProbe?.status === 'foreground-different'
      && (await inv('queue_list')).items?.some(item => item.id === mismatchItem?.id && item.attempts === 0);
    const mask = (hooklessReady ? 1 : 0) | (compatibilitySent ? 2 : 0)
      | (noPolicy ? 4 : 0) | (mismatchWaits ? 8 : 0)
      | (dangerShown ? 16 : 0) | (enterRejected ? 32 : 0);
    await report('scheduler-context', mask === 63, mask, 63);
    await inv('smoke_seed_ambiguous');
    await report('done', !smokeFailed, 1, 0);
  } catch (error) {
    const stack = String((error && error.stack) || '');
    const frame = /\/(app|board|dialogs|i18n|layout|scheduler|selection|state|terminal)\.js:(\d+):/.exec(stack);
    const files = ['app', 'board', 'dialogs', 'i18n', 'layout', 'scheduler', 'selection', 'state', 'terminal'];
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error',
      a: frame ? Number(frame[2]) : stage, b: frame ? files.indexOf(frame[1]) + 1 : 0 });
    await report('done', false, 0, stage);
  }
}

export async function verifyAmbiguousBoot() {
  try {
    await waitFor(() => provider.projects().length > 0);
    await refreshQueue();
    const ambiguous = await waitFor(async () => {
      const q = await inv('queue_list');
      return q.items?.some(item => item.state === 'ambiguous');
    }, 5000);
    const q = await inv('queue_list');
    const item = q.items?.find(entry => entry.state === 'ambiguous');
    const card = item && store.cards.find(entry => entry.session === item.session);
    if (card) await openSession(card.id);
    toggleQueuePanel(true);
    await pause(300);
    const actionsVisible = !!document.querySelector('#queue-list .q-ack')
      && !!document.querySelector('#queue-list .q-risk-retry');
    const failedRepair = await inv('smoke_queue_state');
    document.querySelector('#queue-list .q-ack')?.click();
    const resolved = await waitFor(async () => !(await inv('queue_list')).items?.length, 5000);
    const flushed = await inv('smoke_flush_queue');
    const recovered = await inv('smoke_queue_state');
    await report('ambiguous-boot', ambiguous && actionsVisible && failedRepair.dirty
      && !failedRepair.disk_matches && resolved && flushed && !recovered.dirty
      && recovered.disk_matches, actionsVisible ? 1 : 0, recovered.disk_matches ? 1 : 0);
    await report('done', !smokeFailed, 1, 0);
  } catch (error) {
    await report('done', false, 0, 12);
  }
}

export async function verifyRestart() {
  try {
    await waitFor(() => provider.projects().length > 0);
    const card = store.cards.find(c => c.title === 'renamed-enter-smoke');
    if (card) {
      state.projectId = card.projectId;
      state.view = 'board';
      render();
    }
    const boardTitle = card
      && document.querySelector(`.card[data-sid="${card.id}"] .card-title`)?.textContent;
    const disk = JSON.parse((await inv('load_board')).data);
    const durable = card && disk.cards.some(c => c.id === card.id && c.title === 'renamed-enter-smoke');
    await report('rename-restart', !!card && durable && boardTitle === 'renamed-enter-smoke', 1, 1);
    await report('done', !smokeFailed, 1, 0);
  } catch (error) {
    await report('done', false, 0, 8);
  }
}
