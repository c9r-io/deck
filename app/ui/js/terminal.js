// terminal.js — context menus & link opening, ghost completion, chrome wiring
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, duev, inv, state, uev } from './state.js';
import { copyExact, copyShortcutAction, createSelectionAutoScroller, latestRequestGuard, linkMenuItems } from './pure.js';
import { confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { closeSession, panes, provider, renameTab, render, switchProject, activeProject } from './board.js';
import { backToBoard, openSession, strToB64 } from './layout.js';

/* ---------- context menus ---------- */
export function placeCtx(e) {
  const ctx = $('ctx');
  ctx.style.display = 'block';
  ctx.style.left = Math.min(e.clientX, innerWidth - ctx.offsetWidth - 8) + 'px';
  ctx.style.top = Math.min(e.clientY, innerHeight - ctx.offsetHeight - 8) + 'px';
}

export function renameCardInline(sid, preferredHost = null) {
  const s = provider.get(sid);
  if (!s) return;
  const host = (preferredHost && preferredHost.isConnected ? preferredHost : null)
    || document.querySelector(`#side-list .side-item[data-sid="${sid}"] .name`)
    || document.querySelector(`.card[data-sid="${sid}"] .card-title`)
    || (state.sessionId === sid ? document.querySelector('.sess-head .sess-title .name') : null);
  if (!host) return;
  inlineRename(host, s.title, async v => {
    if (v && v !== s.title) await provider.rename(sid, v);
    else render();
  });
}

export function editDescInline(sid) {
  const s = provider.get(sid);
  const card = document.querySelector(`.card[data-sid="${sid}"]`);
  if (!card) {
    // window.prompt is a silent no-op in WKWebView — use the in-app dialog
    promptDialog('Description', s.desc || '').then(async v => {
      if (v !== null) await provider.setDesc(sid, v.trim());
    });
    return;
  }
  let d = card.querySelector('.card-desc');
  if (!d) {
    d = document.createElement('div');
    d.className = 'card-desc';
    card.querySelector('.card-meta').after(d);
  }
  inlineRename(d, s.desc || '', async v => {
    if (v !== null) await provider.setDesc(sid, v);
    render();
  }, true);
}

export function showSessionCtx(e, sid) {
  e.preventDefault();
  e.stopPropagation();
  const s = provider.get(sid);
  const renameHost = e.currentTarget && e.currentTarget.querySelector
    ? e.currentTarget.querySelector('.name, .card-title') : null;
  const ctx = $('ctx');
  ctx.innerHTML = `
    <button data-a="rename">Rename card</button>
    <button data-a="desc">${s.desc ? 'Edit' : 'Add'} description</button>
    <button data-a="here">New session in this directory</button>
    <button data-a="copy">Copy output…</button>
    <hr>
    <button data-a="close" class="danger">Close session</button>`;
  ctx.onclick = ev => {
    const a = ev.target.dataset && ev.target.dataset.a;
    ctx.style.display = 'none';
    if (a === 'rename') renameCardInline(sid, renameHost);
    if (a === 'desc') editDescInline(sid);
    if (a === 'here') newSession(s.dir);
    if (a === 'copy') openCopyPanel(s.session, s.title);
    if (a === 'close') closeSession(sid);
  };
  placeCtx(e);
}

export function showProjectCtx(e, pid) {
  e.preventDefault();
  e.stopPropagation();
  const p = provider.project(pid);
  const ctx = $('ctx');
  ctx.innerHTML = `
    <button data-a="rename">Rename project</button>
    <button data-a="remove" class="danger">Delete project</button>`;
  ctx.onclick = async ev => {
    const a = ev.target.dataset && ev.target.dataset.a;
    ctx.style.display = 'none';
    if (a === 'rename') {
      switchProject(pid);
      const tab = document.querySelector('#tabs .tab.active');
      if (tab) renameTab(tab, p);
    }
    if (a === 'remove') {
      if (provider.projects().length <= 1) { toast('at least one project is required'); return; }
      const n = provider.list(pid).length;
      if (!(await confirmDialog(`Delete project "${p.name}"?${n ? ` Its ${n} session(s) will be closed and their scheduled prompts cancelled.` : ''}`))) return;
      /* nothing is removed unless every card's schedule was cancelled and
         persisted first (the toast explains a refusal) */
      if (!(await provider.removeProject(pid))) return;
      if (state.projectId === pid) {
        state.projectId = provider.projects()[0].id;
        state.view = 'board';
        state.sessionId = null;
      }
      toast(`deleted project: ${p.name}`);
      render();
    }
  };
  placeCtx(e);
}

let linkActionGeneration = 0;

export function showLinkCtx(e, kind, value, cwd, sid = null) {
  const ctx = $('ctx');
  linkActionGeneration++; // invalidate any older path resolution
  const restoreFocus = document.activeElement;
  ctx.innerHTML = '<span class="ctx-value"></span>' + linkMenuItems(kind)
    .map(item => `<button data-a="${item.action}">${item.label}</button>`).join('');
  ctx.querySelector('.ctx-value').textContent = value;
  ctx.setAttribute('role', 'menu');
  ctx.querySelectorAll('button').forEach(b => b.setAttribute('role', 'menuitem'));
  const close = () => {
    linkActionGeneration++;
    ctx.style.display = 'none';
    ctx.onkeydown = null;
    if (restoreFocus && restoreFocus.isConnected && restoreFocus.focus) restoreFocus.focus();
  };
  ctx.onclick = async ev => {
    ev.stopPropagation();
    const a = ev.target.dataset && ev.target.dataset.a;
    if (!a) return;
    const request = ++linkActionGeneration;
    ctx.style.display = 'none';
    if (restoreFocus && restoreFocus.isConnected && restoreFocus.focus) restoreFocus.focus();
    if (a === 'copy') {
      writeClipboard(value).then(() => toast('copied'), () => toast('copy failed'));
    } else if (a === 'session-parent') {
      try {
        const origin = sid && provider.get(sid);
        if (!origin) throw new Error('the source session is no longer available');
        const resolved = await inv('resolve_parent_dir', { value, cwd: cwd || HOME });
        if (request !== linkActionGeneration || !provider.get(sid)) return;
        await newSession(resolved.directory, { projectId: origin.projectId, requireStart: true });
        toast('opened a new session in the parent folder');
      } catch (err) {
        if (request === linkActionGeneration) toast('could not create a session from this path');
      }
    } else {
      inv('open_target', { kind: a, value, cwd: cwd || HOME })
        .then(() => toast(a === 'url' ? 'opened in browser' : a.startsWith('editor') ? 'opened in editor' : 'revealed in Finder'))
        .catch(err => toast('open failed: ' + err));
    }
  };
  ctx.onkeydown = ev => {
    const buttons = [...ctx.querySelectorAll('button')];
    const i = buttons.indexOf(document.activeElement);
    if (ev.key === 'Escape') { ev.preventDefault(); ev.stopPropagation(); close(); return; }
    if (ev.key === 'ArrowDown' || ev.key === 'ArrowUp' || ev.key === 'Home' || ev.key === 'End') {
      ev.preventDefault(); ev.stopPropagation();
      const next = ev.key === 'Home' ? 0 : ev.key === 'End' ? buttons.length - 1
        : ev.key === 'ArrowDown' ? (i + 1 + buttons.length) % buttons.length
        : (i - 1 + buttons.length) % buttons.length;
      buttons[next].focus();
    }
  };
  placeCtx(e);
  const first = ctx.querySelector('button');
  if (first) first.focus();
}

document.addEventListener('click', () => {
  linkActionGeneration++;
  $('ctx').style.display = 'none';
});

/* ---------- new session: no modal — create a shell and enter it.
   Title is renamed on the board later; command is typed in the shell
   (with quick-command chips as a shortcut); dir defaults to $HOME. ---------- */
export function nextShellTitle(p) {
  const used = new Set(provider.list(p.id).map(c => c.title));
  let n = 1;
  while (used.has('shell ' + n)) n++;
  return 'shell ' + n;
}

export async function newSession(dir, opts = {}) {
  if (creatingSession) {
    if (opts.requireStart) throw new Error('a session is already being created');
    return;
  }
  creatingSession = true;
  try {
    const p = opts.projectId ? provider.project(opts.projectId) : activeProject();
    if (!p) throw new Error('project no longer exists');
    const selected = p.columns.find(c => c.id === p.selected);
    const columnId = (selected
      || p.columns.find(c => c.name === 'Working')
      || p.columns[1] || p.columns[0]).id;
    let started = false;
    const card = await provider.create({
      projectId: p.id, columnId,
      title: nextShellTitle(p),
      cmd: '',
      dir: dir || HOME,
    }, opts.requireStart ? {
      beforePersist: async card => {
        started = await inv('start_session', { name: card.session, dir: card.dir, cmd: card.cmd });
        return started;
      },
      rollback: async card => {
        if (started) await inv('kill_session', { name: card.session });
      },
    } : {});
    await openSession(card.id);
    return card;
  } catch (error) {
    if (opts.requireStart) throw error;
    toast('new session could not be created safely');
  } finally {
    creatingSession = false;
  }
}

/* ---------- command suggestions (Warp-style, driven by an input mirror) ----------
   We mirror what the user types (via onData) into `lineBuf` and prefix-match it
   against command history. Escape sequences / Tab desync the mirror → suggestions
   pause until the next Enter/^C resets the line. Accepting a suggestion sends
   Ctrl+U + the full command, which is immune to mirror desync. */

/* returns the completed command line when the chunk contained an Enter */
export function feedMirror(d) {
  let completed = null;
  for (const ch of d) {
    if (ch === '\r' || ch === '\n') {
      if (lineBuf && lineBuf.trim().length >= 2) completed = lineBuf.trim();
      lineBuf = '';
      freshShell = false;
    } else if (ch === '\x03' || ch === '\x15') {
      lineBuf = '';
      freshShell = false;
    } else if (ch === '\x7f') {
      if (lineBuf !== null) lineBuf = lineBuf.slice(0, -1);
    } else if (ch === '\x1b' || ch === '\t') {
      lineBuf = null;
    } else if (lineBuf !== null && ch >= ' ') {
      lineBuf += ch;
    }
  }
  return completed;
}

/* record commands typed into deck shells — the user's ~/.zsh_history only
   fills when a shell exits, and deck shells live for days. Agent prompts
   are excluded via the pane's foreground process. */
export const SHELL_FG = /^-?(zsh|bash|fish|sh|dash)$/;
/* foreground-process CATEGORY for diagnostics — the process name itself
   never reaches the log (backend closed allowlist would redact it) */
const fgClass = fg => {
  if (!fg) return 'no-fg';
  const f = fg.replace(/^-/, '');
  if (/^(claude|codex|gemini|aider|goose)$/.test(f)) return 'agent';
  if (/^(vim|nvim|nano|emacs|hx|micro)$/.test(f)) return 'editor';
  if (/^(node|python\d*|ruby|irb|deno|bun)$/.test(f)) return 'repl';
  return 'other';
};
export function maybeRecordCommand(cmd) {
  const c = provider.get(state.sessionId);
  if (!c) { duev('record-skip', 'no-card'); return; }
  if (!SHELL_FG.test(c.fg || '')) { duev('record-skip', fgClass(c.fg)); return; }
  duev('record', null, cmd.length);
  inv('record_command', { cmd }).catch(() => uev('record-fail'));
  histCache = [cmd, ...histCache.filter(x => x !== cmd)];
}

export function suggestions() {
  if (lineBuf === null) return [];
  if (lineBuf.length >= 2) {
    return histCache.filter(c => c.startsWith(lineBuf) && c !== lineBuf).slice(0, 6);
  }
  if (freshShell && lineBuf === '') return histCache.slice(0, 8);
  return [];
}

export function acceptSuggestion(cmd) {
  if (!attachedName) return;
  inv('pty_write', { name: attachedName, dataB64: strToB64('\x15' + cmd) }).catch(() => {});
  inv('record_command', { cmd }).catch(() => {});
  lineBuf = cmd;
  freshShell = false;
  renderSuggest();
  if (term) term.focus();
}

/* ---------- inline ghost suggestion (Warp-style) ---------- */
/* cell size from public layout only: .xterm-screen is sized to exactly
   cols×rows cells, so division measures a cell without any xterm private
   API (deck code never touches _core). */
export function ghostCellDims(host) {
  const screen = host.querySelector('.xterm-screen');
  return { w: screen.clientWidth / term.cols, h: screen.clientHeight / term.rows };
}

export function updateGhost() {
  if (!term || !ghostEl) return;
  const items = suggestions();
  if (!(lineBuf && lineBuf.length >= 2 && items.length)) {
    ghostRemainder = '';
    ghostEl.style.display = 'none';
    return;
  }
  ghostRemainder = items[0].slice(lineBuf.length);
  const fp = attachedName && panes.get(attachedName);
  if (!fp) { ghostEl.style.display = 'none'; return; }
  const host = fp.body;
  const screen = host.querySelector('.xterm-screen');
  if (!screen) return;
  const buf = term.buffer.active;
  const { w, h } = ghostCellDims(host);
  const hostRect = host.getBoundingClientRect();
  const sRect = screen.getBoundingClientRect();
  ghostEl.textContent = ghostRemainder;
  ghostEl.style.left = (sRect.left - hostRect.left + buf.cursorX * w) + 'px';
  ghostEl.style.top = (sRect.top - hostRect.top + buf.cursorY * h) + 'px';
  ghostEl.style.lineHeight = h + 'px';
  ghostEl.style.display = 'block';
}

/* Debounced ghost: echoes come back asynchronously, so painting on every
   keystroke uses a momentarily stale cursor and the ghost jitters during
   fast edits (backspace bursts). Hide instantly, settle after 150ms. */
export function scheduleGhost() {
  ghostRemainder = '';
  if (ghostEl) ghostEl.style.display = 'none';
  clearTimeout(ghostTimer);
  ghostTimer = setTimeout(updateGhost, 150);
}

/* the mirror is append-only while synced, so completing = typing the rest */
export function acceptGhost() {
  if (!ghostRemainder || !attachedName) return;
  const full = lineBuf + ghostRemainder;
  inv('pty_write', { name: attachedName, dataB64: strToB64(ghostRemainder) }).catch(() => {});
  inv('record_command', { cmd: full }).catch(() => {});
  histCache = [full, ...histCache.filter(x => x !== full)];
  lineBuf = full;
  renderSuggest();
}

let quickBarFitFrame = null;
let quickBarHeight = 0;

function refitQuickBarPane() {
  if (quickBarFitFrame != null) window.cancelAnimationFrame(quickBarFitFrame);
  quickBarFitFrame = requestAnimationFrame(() => {
    quickBarFitFrame = null;
    const pane = attachedName && panes.get(attachedName);
    if (!pane) return;
    const buf = pane.term.buffer.active;
    const followedBottom = buf.viewportY >= buf.baseY;
    try {
      pane.fit.fit();
      inv('pty_resize', { name: pane.session, cols: pane.term.cols, rows: pane.term.rows }).catch(() => {});
      if (followedBottom) pane.term.scrollToBottom();
    } catch (e) { /* pane was destroyed during the frame */ }
  });
}

export function mountQuickBar(pane) {
  const bar = $('quick-bar');
  if (pane && bar.parentElement !== pane.el) pane.el.appendChild(bar);
}

function showQuickBar(show) {
  const bar = $('quick-bar');
  const changed = (bar.style.display === 'flex') !== show;
  bar.style.display = show ? 'flex' : 'none';
  if (!show) quickBarHeight = 0;
  if (changed) refitQuickBarPane();
  if (show) requestAnimationFrame(() => {
    const height = bar.offsetHeight;
    if (height !== quickBarHeight) {
      quickBarHeight = height;
      if (!changed) refitQuickBarPane();
    }
  });
}

export function renderSuggest() {
  scheduleGhost();
  const bar = $('quick-bar');
  const items = suggestions();
  if (!items.length) { showQuickBar(false); return; }
  const pane = attachedName && panes.get(attachedName);
  if (pane) mountQuickBar(pane);
  const completing = lineBuf && lineBuf.length >= 2;
  bar.innerHTML = `<span class="qb-label">${completing ? 'tab ⇥' : 'recent'}</span>`;
  items.forEach((c, i) => {
    const b = document.createElement('button');
    b.className = 'qb-chip' + (completing && i === 0 ? ' first' : '');
    b.title = c;
    if (completing) {
      const bold = document.createElement('b');
      bold.textContent = lineBuf;
      b.appendChild(bold);
      b.appendChild(document.createTextNode(c.slice(lineBuf.length)));
    } else {
      b.textContent = c;
    }
    b.onclick = () => acceptSuggestion(c);
    bar.appendChild(b);
  });
  showQuickBar(true);
}

export function resetSuggest() {
  lineBuf = '';
  freshShell = false;
  ghostRemainder = '';
  clearTimeout(ghostTimer);
  if (ghostEl) ghostEl.style.display = 'none';
  showQuickBar(false);
}

/* ---------- copy panel ----------
   A selection inside the terminal can only ever cover ONE screen: tmux owns
   the scrollback and repaints the same rows in place, so a drag cannot be
   carried past the top of the pane and a long answer could not be copied in
   one piece. The panel shows the pane's scrollback as plain text, where the
   selection scrolls and ⌘C behaves like it does in any document. */
const COPY_LINES = 20000;
const copyRequests = latestRequestGuard();
let copyAnchor = null;
let copyPointerId = null;

function copyCaretPoint(body, x, y) {
  const rect = body.getBoundingClientRect();
  const cx = Math.max(rect.left + 1, Math.min(rect.right - 1, x));
  const cy = Math.max(rect.top + 1, Math.min(rect.bottom - 1, y));
  if (document.caretPositionFromPoint) {
    const p = document.caretPositionFromPoint(cx, cy);
    if (p) return { node: p.offsetNode, offset: p.offset };
  }
  if (document.caretRangeFromPoint) {
    const r = document.caretRangeFromPoint(cx, cy);
    if (r) return { node: r.startContainer, offset: r.startOffset };
  }
  return null;
}

function extendCopySelection(point, rect) {
  const body = $('cb-body');
  if (!copyAnchor || !body.isConnected) return;
  const focus = copyCaretPoint(
    body,
    point.x,
    Math.max(rect.top + 1, Math.min(rect.bottom - 1, point.y)),
  );
  if (!focus || !body.contains(focus.node)) return;
  const selection = window.getSelection();
  if (selection && selection.setBaseAndExtent) {
    selection.setBaseAndExtent(copyAnchor.node, copyAnchor.offset, focus.node, focus.offset);
  }
}

const copyAutoScroller = createSelectionAutoScroller({
  frame: cb => requestAnimationFrame(cb),
  cancelFrame: id => window.cancelAnimationFrame(id),
  measure: () => $('cb-body').getBoundingClientRect(),
  scrollBy: dy => {
    const body = $('cb-body');
    const before = body.scrollTop;
    body.scrollTop = Math.max(0, Math.min(body.scrollHeight - body.clientHeight, before + dy));
    return body.scrollTop !== before;
  },
  extend: extendCopySelection,
});

function stopCopySelection() {
  copyAutoScroller.stop();
  const body = $('cb-body');
  if (copyPointerId != null && body.releasePointerCapture) {
    try { body.releasePointerCapture(copyPointerId); } catch (e) { /* already released */ }
  }
  copyPointerId = null;
  copyAnchor = null;
}

export async function writeClipboard(text) {
  try {
    return await copyExact(text, value => inv('write_clipboard', { text: value }));
  } catch (nativeError) {
    if (!navigator.clipboard || !navigator.clipboard.writeText) throw nativeError;
    return copyExact(text, value => navigator.clipboard.writeText(value));
  }
}

export async function openCopyPanel(session, title) {
  const request = copyRequests.begin();
  const box = $('copybox');
  const body = $('cb-body');
  $('cb-title').textContent = title ? `Output — ${title}` : 'Output';
  body.textContent = 'reading the scrollback…';
  $('cb-all').textContent = 'Copy all';
  $('cb-all').disabled = true;
  $('cb-all').onclick = null;
  box.style.display = 'flex';
  let capture;
  try {
    capture = await inv('capture_scrollback', { name: session, lines: COPY_LINES });
  } catch (e) {
    if (!copyRequests.isCurrent(request)) return;
    body.textContent = 'could not read this session’s output: ' + e;
    $('cb-all').textContent = 'Retry';
    $('cb-all').disabled = false;
    $('cb-all').onclick = () => openCopyPanel(session, title);
    return;
  }
  if (!copyRequests.isCurrent(request)) return;
  const text = capture.text;
  body.textContent = text;
  const n = capture.captured_rows;
  $('cb-title').textContent = (title ? `Output — ${title}` : 'Output') +
    `  ·  ${n} terminal row${n === 1 ? '' : 's'}` +
    (capture.truncated ? ` · newest ${capture.line_limit.toLocaleString()} terminal rows only` : '');
  body.scrollTop = body.scrollHeight;   // the newest output is what you came for
  $('cb-all').textContent = 'Copy all';
  $('cb-all').disabled = false;
  $('cb-all').onclick = async () => {
    try {
      await writeClipboard(text);
      toast(capture.truncated
        ? `copied newest ${capture.line_limit.toLocaleString()} terminal rows only`
        : `copied ${n} terminal row${n === 1 ? '' : 's'}`);
    } catch (e) {
      toast('copy failed — clipboard was not changed');
    }
  };
}

export function closeCopyPanel() {
  stopCopySelection();
  copyRequests.cancel();
  $('copybox').style.display = 'none';
}
export const copyPanelOpen = () => $('copybox').style.display === 'flex';

$('cb-close').onclick = closeCopyPanel;
$('cb-body').addEventListener('pointerdown', e => {
  if (e.button !== 0) return;
  const body = $('cb-body');
  const anchor = copyCaretPoint(body, e.clientX, e.clientY);
  if (!anchor || !body.contains(anchor.node)) return;
  e.preventDefault();
  e.stopPropagation();
  stopCopySelection();
  copyAnchor = anchor;
  copyPointerId = e.pointerId;
  if (body.setPointerCapture) {
    try { body.setPointerCapture(e.pointerId); } catch (err) { /* document listeners still work */ }
  }
  const selection = window.getSelection();
  if (selection && selection.setBaseAndExtent) {
    selection.setBaseAndExtent(anchor.node, anchor.offset, anchor.node, anchor.offset);
  }
  copyAutoScroller.start({ x: e.clientX, y: e.clientY });
});
document.addEventListener('pointermove', e => {
  if (!copyAutoScroller.active() || (copyPointerId != null && e.pointerId !== copyPointerId)) return;
  e.preventDefault();
  copyAutoScroller.move({ x: e.clientX, y: e.clientY });
}, true);
document.addEventListener('pointerup', e => {
  if (copyPointerId == null || e.pointerId === copyPointerId) stopCopySelection();
}, true);
document.addEventListener('pointercancel', stopCopySelection, true);
window.addEventListener('blur', stopCopySelection);
$('copybox').addEventListener('mousedown', e => {
  if (e.target === $('copybox')) closeCopyPanel();   // click the backdrop
});

/* the focused pane's output, from anywhere in the session view */
export function copyFocusedPaneOutput() {
  if (!attachedName) return;
  const p = panes.get(attachedName);
  const c = p && provider.get(p.sid);
  openCopyPanel(attachedName, c ? c.title : '');
}

/* ---------- chrome wiring ---------- */
export function toggleSidebar() {
  document.body.classList.toggle('side-collapsed');
  const collapsed = document.body.classList.contains('side-collapsed');
  $('collapse-btn').title = (collapsed ? 'Expand' : 'Collapse') + ' sidebar (⌘B)';
  $('collapse-btn').firstElementChild.style.transform = collapsed ? 'scaleX(-1)' : '';
}
$('collapse-btn').onclick = toggleSidebar;
$('home-btn').onclick = backToBoard;
$('back-btn').onclick = backToBoard;
$('board-new').onclick = () => newSession(HOME);
$('side-new').onclick = () => newSession(HOME);
$('sess-close').onclick = () => closeSession(state.sessionId, true);
$('sess-col').addEventListener('change', async e => {
  await provider.move(state.sessionId, e.target.value);
  const c = activeProject().columns.find(c => c.id === e.target.value);
  toast(`moved to ${c.name}`);
});
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'b') { e.preventDefault(); toggleSidebar(); return; }
  const selection = window.getSelection && window.getSelection();
  const rangeNode = selection && selection.rangeCount ? selection.getRangeAt(0).commonAncestorContainer : null;
  const selectionHost = rangeNode && (rangeNode.nodeType === 3 ? rangeNode.parentElement : rangeNode);
  const selected = selectionHost && $('cb-body').contains(selectionHost) ? selection.toString() : '';
  const copyAction = copyShortcutAction({
    metaKey: e.metaKey, shiftKey: e.shiftKey, key: e.key,
    panelOpen: copyPanelOpen(), sessionView: state.view === 'session', hasSelection: !!selected,
  });
  if (copyAction === 'copy-panel-selection') {
    e.preventDefault();
    e.stopPropagation();
    writeClipboard(selected).then(
      () => toast('copied selection'),
      () => toast('copy failed — clipboard was not changed'),
    );
    return;
  }
  if (copyAction === 'none') { e.preventDefault(); return; }
  if (copyAction === 'open-copy-panel') {
    e.preventDefault();
    copyFocusedPaneOutput();
    return;
  }
  if (e.key === 'Escape') {
    if (copyPanelOpen()) { closeCopyPanel(); return; }
    if ($('ctx').style.display === 'block') { $('ctx').style.display = 'none'; return; }
    /* Esc inside the terminal belongs to the terminal (agents use it) */
    if (state.view === 'session' && !(document.activeElement && document.activeElement.closest('#terminal'))) {
      backToBoard();
    }
  }
});
