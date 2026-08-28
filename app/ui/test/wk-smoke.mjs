// Debug-only real-WKWebView smoke. main.rs imports this module only when the
// app was launched with --smoke-wkwebview and an isolated --smoke-data-dir.
// The release gate also runs `node --test app/ui/test/*.mjs`; defer production
// DOM imports so Node can load this carrier without fabricating a browser.
let $, inv, state, store, panes, provider, render, boardData;
let closeCopyPanel, openCopyPanel, renameCardInline, renderSuggest, resetSuggest;
let showLinkCtx, toggleSidebar, addSplit, backToBoard, openSession, strToB64;
if (typeof window !== 'undefined') {
  ({ $, inv, state, store } = await import('../js/state.js'));
  ({ panes, provider, render } = await import('../js/board.js'));
  ({ boardData } = await import('../js/persistence.js'));
  ({
    closeCopyPanel, openCopyPanel, renameCardInline, renderSuggest, resetSuggest,
    showLinkCtx, toggleSidebar,
  } = await import('../js/terminal.js'));
  ({ addSplit, backToBoard, openSession, strToB64 } = await import('../js/layout.js'));
}

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(test, timeout = 8000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    if (await test()) return true;
    await pause(50);
  }
  return false;
}
const report = (name, ok, a = 0, b = 0) => inv('ui_event', {
  code: 'smoke-check', detail: name,
  a: ok ? Math.trunc(a || 1) : -Math.abs(Math.trunc(a || 1)), b: Math.trunc(b || 0),
});
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
    pointerId: id, pointerType: 'mouse', button, buttons: type === 'pointerup' ? 0 : 1,
    clientX: x, clientY: y, bubbles: true, cancelable: true,
  });
}

async function copySmoke(card) {
  await openSession(card.id);
  const command = `python3 -c 'for i in range(2200): print(f"{i:04d} 中文 😀 é" + ("x"*600 if i==100 else ""))'`;
  await inv('pty_write', { name: card.session, dataB64: strToB64(command + '\r') });
  await waitFor(async () => {
    const capture = await inv('capture_scrollback', { name: card.session, lines: 20_000 });
    return capture.text.includes('2199');
  }, 12_000);
  await openCopyPanel(card.session, card.title);
  const body = $('cb-body');
  const loaded = await waitFor(() => !$('cb-all').disabled && body.textContent.includes('2199'));
  const lines = body.textContent.split('\n').length;
  const chars = body.textContent.length;
  const rect = body.getBoundingClientRect();

  body.scrollTop = 0;
  body.dispatchEvent(pointer('pointerdown', 41, rect.left + 40, rect.top + 8));
  document.dispatchEvent(pointer('pointermove', 41, rect.left + 40, rect.bottom + 48));
  await pause(850);
  const downScroll = body.scrollTop;
  const downSelection = window.getSelection().toString().length;
  document.dispatchEvent(pointer('pointerup', 41, rect.left + 40, rect.bottom + 48));
  await report('copy-down', loaded && downScroll > 0 && downSelection > 0, downScroll, downSelection);

  body.scrollTop = body.scrollHeight;
  const fromBottom = body.scrollTop;
  body.dispatchEvent(pointer('pointerdown', 42, rect.left + 40, rect.bottom - 8));
  document.dispatchEvent(pointer('pointermove', 42, rect.left + 40, rect.top - 48));
  await pause(850);
  const upScroll = body.scrollTop;
  const selection = window.getSelection().toString();
  document.dispatchEvent(pointer('pointerup', 42, rect.left + 40, rect.top - 48));
  await report('copy-up', upScroll < fromBottom && selection.length > 0, fromBottom - upScroll, selection.length);

  document.dispatchEvent(new KeyboardEvent('keydown', {
    key: 'c', metaKey: true, bubbles: true, cancelable: true,
  }));
  await pause(350);
  await report('copy-selection', selection.length > 0, selection.length, chars);
  await pause(3000); // external smoke runner pastes/measures the native selection here
  $('cb-all').click();
  await pause(500);
  await report('copy-all', loaded && lines >= 2200 && chars > 25_000, lines, chars);
  closeCopyPanel();
}

async function pathSmoke(card) {
  const focus = document.createElement('button');
  focus.textContent = 'focus';
  document.body.appendChild(focus);
  focus.focus();
  showLinkCtx(eventAt(), 'url', 'https://example.com', card.dir, card.id);
  const urlActions = [...$('ctx').querySelectorAll('button')].map(b => b.dataset.a).join(',');
  $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  showLinkCtx(eventAt(), 'path', '"空 格😀/code.rs":12:3', card.dir, card.id);
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
    await inv('open_target', { kind: 'editor-parent', value: '"空 格😀/code.rs":12:3', cwd: card.dir });
    editorOpened = true;
  } catch (e) { /* report false */ }
  await report('path-editor', editorOpened, 1, 0);

  const beforeRelative = store.cards.length;
  showLinkCtx(eventAt(), 'path', '"空 格😀/code.rs":12:3', card.dir, card.id);
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

async function completionSmoke(card, project, column) {
  await openSession(card.id);
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
  let historyCapture;
  await waitFor(async () => {
    historyCapture = await inv('capture_scrollback', { name: card.session, lines: 500 });
    return historyCapture.captured_rows > historyCapture.pane_rows + 20;
  }, 8000);
  await waitFor(() => pane.term.buffer.active.cursorY >= pane.term.rows - 2, 8000);
  const readPtyRows = async () => (await inv('capture_scrollback', {
    name: card.session, lines: 1,
  })).pane_rows;
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
  const historyPresent = historyCapture.captured_rows > historyCapture.pane_rows + 20;
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

export async function run() {
  let stage = 0;
  try {
    stage = 1;
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    const column = project.columns.find(c => c.name === 'Working') || project.columns[0];
    stage = 2;
    await boardConcurrency(project, column);
    stage = 3;
    const main = await provider.create({
      projectId: project.id, columnId: column.id, title: 'wk-smoke', cmd: '', dir: '/tmp/deck-r6-path',
    });
    render();
    stage = 4;
    await renameSmoke(main);
    stage = 5;
    await copySmoke(main);
    stage = 6;
    await pathSmoke(main);
    stage = 7;
    await completionSmoke(main, project, column);
    await report('done', true, 1, 0);
  } catch (error) {
    await report('done', false, 0, stage);
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
    await report('done', true, 1, 0);
  } catch (error) {
    await report('done', false, 0, 8);
  }
}
