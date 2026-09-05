// terminal.js — context menus & link opening, ghost completion, chrome wiring
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// The completion bar is a real flex row inside one pane, never an overlay.
// Its generation-based transition is: detach/hide old owner, refit old owner,
// mount new owner, refit new owner. Stale RAF work must not resize a newer
// owner, and each pane preserves its own bottom-follow/scrollback position.
import { $, duev, inv, state, uev } from './state.js';
import { copyExact, isComposingKeyEvent, linkMenuItems } from './pure.js';
import { confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { closeSession, panes, provider, renameTab, render, switchProject, activeProject } from './board.js';
import { backToBoard, openSession, strToB64 } from './layout.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';
import { formatShortcut, registerShortcutAction } from './shortcuts.js';

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
    promptDialog(t('terminal.description'), s.desc || '').then(async v => {
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
  ctx.innerHTML = '<button data-a="rename"></button><button data-a="desc"></button><button data-a="here"></button><hr><button data-a="close" class="danger"></button>';
  ctx.querySelector('[data-a="rename"]').textContent = t('menu.renameCard');
  ctx.querySelector('[data-a="desc"]').textContent = t(s.desc ? 'menu.editDescription' : 'menu.addDescription');
  ctx.querySelector('[data-a="here"]').textContent = t('menu.newSessionHere');
  ctx.querySelector('[data-a="close"]').textContent = t('menu.closeSession');
  ctx.onclick = ev => {
    const a = ev.target.dataset && ev.target.dataset.a;
    ctx.style.display = 'none';
    if (a === 'rename') renameCardInline(sid, renameHost);
    if (a === 'desc') editDescInline(sid);
    if (a === 'here') newSession(s.dir);
    if (a === 'close') closeSession(sid);
  };
  placeCtx(e);
}

export function showProjectCtx(e, pid) {
  e.preventDefault();
  e.stopPropagation();
  const p = provider.project(pid);
  const ctx = $('ctx');
  ctx.innerHTML = '<button data-a="rename"></button><button data-a="remove" class="danger"></button>';
  ctx.querySelector('[data-a="rename"]').textContent = t('menu.renameProject');
  ctx.querySelector('[data-a="remove"]').textContent = t('menu.deleteProject');
  ctx.onclick = async ev => {
    const a = ev.target.dataset && ev.target.dataset.a;
    ctx.style.display = 'none';
    if (a === 'rename') {
      switchProject(pid);
      const tab = document.querySelector('#tabs .tab.active');
      if (tab) renameTab(tab, p);
    }
    if (a === 'remove') {
      if (provider.projects().length <= 1) { toast(t('project.atLeastOne')); return; }
      const n = provider.list(pid).length;
      if (!(await confirmDialog(t('project.delete', { name: p.name, sessions: n ? t('project.deleteSessions', { count: formatNumber(n) }) : '' })))) return;
      /* nothing is removed unless every card's schedule was cancelled and
         persisted first (the toast explains a refusal) */
      if (!(await provider.removeProject(pid))) return;
      if (state.projectId === pid) {
        state.projectId = provider.projects()[0].id;
        state.view = 'board';
        state.sessionId = null;
      }
      toast(t('project.deleted', { name: p.name }));
      render();
    }
  };
  placeCtx(e);
}

let linkActionGeneration = 0;
let ignoreLinkOpeningClickUntil = 0;

export function showLinkCtx(e, kind, value, cwd, sid = null) {
  const ctx = $('ctx');
  linkActionGeneration++; // invalidate any older path resolution
  // xterm activates providers on mouseup. The browser's compatibility click
  // follows immediately; it opened this menu and must not also close it.
  ignoreLinkOpeningClickUntil = Date.now() + 120;
  const restoreFocus = document.activeElement;
  ctx.innerHTML = '<span class="ctx-value"></span>' + linkMenuItems(kind)
    .map(item => `<button data-a="${item.action}"></button>`).join('');
  const valueLabel = ctx.querySelector('.ctx-value');
  valueLabel.textContent = value;
  valueLabel.title = value;
  const labelKeys = { url: 'link.url', copy: kind === 'url' ? 'link.copyUrl' : 'link.copyPath', editor: 'link.editor', 'editor-parent': 'link.editor-parent', 'session-parent': 'link.session-parent', reveal: 'link.reveal' };
  ctx.querySelectorAll('button').forEach(button => { button.textContent = t(labelKeys[button.dataset.a]); });
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
      writeClipboard(value).then(() => toast(t('terminal.copied')), () => toast(t('terminal.copyFailed')));
    } else if (a === 'session-parent') {
      try {
        const origin = sid && provider.get(sid);
        if (!origin) throw new Error('the source session is no longer available');
        const resolved = await inv('resolve_parent_dir', { value, cwd: cwd || HOME });
        if (request !== linkActionGeneration || !provider.get(sid)) return;
        await newSession(resolved.directory, { projectId: origin.projectId, requireStart: true });
        toast(t('terminal.openedParent'));
      } catch (err) {
        if (request === linkActionGeneration) toast(t('terminal.createPathFailed'));
      }
    } else {
      inv('open_target', { kind: a, value, cwd: cwd || HOME })
        .then(() => toast(t(a === 'url' ? 'terminal.openBrowser' : a.startsWith('editor') ? 'terminal.openEditor' : 'terminal.revealFinder')))
        .catch(() => toast(t('terminal.openFailed')));
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

/* ---------- new session: no modal — create a shell and enter it.
   Title is renamed on the board later; command is typed in the shell
   (with quick-command chips as a shortcut); dir defaults to $HOME. ---------- */
export function nextShellTitle(p) {
  const used = new Set(provider.list(p.id).map(c => c.title));
  let n = 1;
  while (used.has(t('session.shellName', { number: n }))) n++;
  return t('session.shellName', { number: n });
}

export async function newSession(dir, opts = {}) {
  if (tmuxRestarting || (tmuxServerStatus && tmuxServerStatus.pendingRestart)) {
    const error = new Error('tmux server restart required');
    if (opts.requireStart) throw error;
    toast(t(tmuxRestarting ? 'tmux.restarting' : 'tmux.createBlocked'));
    return;
  }
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
      || p.columns.find(c => c.semantic === 'working')
      || p.columns[1] || p.columns[0]).id;
    let started = false;
    const card = await provider.create({
      projectId: p.id, columnId,
      title: nextShellTitle(p),
      cmd: '',
      dir: dir || HOME,
    }, opts.requireStart ? {
      beforePersist: async card => {
        const result = await inv('start_session', {
          name: card.session, dir: card.dir, cmd: card.cmd,
          restoreShell: !!settings.sessionRestore,
        });
        started = !!result.created;
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
    toast(t('terminal.createFailed'));
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

let quickBarFitFrames = [];
let quickBarGeneration = 0;
let quickBarHeight = 0;

function cancelQuickBarFrames() {
  quickBarFitFrames.forEach(id => window.cancelAnimationFrame(id));
  quickBarFitFrames = [];
}

function paneHoldingQuickBar() {
  const bar = $('quick-bar');
  const owner = bar.closest('.spane');
  return owner ? [...panes.values()].find(p => p.el === owner) || null : null;
}

function refitQuickBarPane(pane, followedBottom, generation) {
  if (!pane || generation !== quickBarGeneration || !pane.el.isConnected) return;
  try {
    pane.fit.fit();
    pane.syncSize?.().catch(() => {});
    if (followedBottom) pane.term.scrollToBottom();
    if (pane.selection) pane.selection.resize();
  } catch (e) { /* pane was destroyed during the frame */ }
}

function transitionQuickBar(pane, show) {
  const bar = $('quick-bar');
  const old = paneHoldingQuickBar();
  const visible = bar.style.display === 'flex';
  const ownerChanged = old !== pane;
  const visibilityChanged = visible !== show;
  if (!ownerChanged && !visibilityChanged) return quickBarGeneration;
  cancelQuickBarFrames();
  const generation = ++quickBarGeneration;
  const oldFollowed = old && old.term.buffer.active.viewportY >= old.term.buffer.active.baseY;
  const newFollowed = pane && pane.term.buffer.active.viewportY >= pane.term.buffer.active.baseY;

  /* Explicit layout transaction:
     old owner -> hidden/detached -> old refit -> new mount -> new refit. */
  bar.style.display = 'none';
  quickBarHeight = 0;
  if (old) $('session-view').appendChild(bar);
  const first = requestAnimationFrame(() => {
    refitQuickBarPane(old, oldFollowed, generation);
    if (generation !== quickBarGeneration) return;
    if (pane && pane.el.isConnected) pane.el.appendChild(bar);
    bar.style.display = show ? 'flex' : 'none';
    const second = requestAnimationFrame(() => {
      refitQuickBarPane(pane, newFollowed, generation);
      if (!show || generation !== quickBarGeneration) return;
      const height = bar.offsetHeight;
      if (height !== quickBarHeight) {
        quickBarHeight = height;
        const third = requestAnimationFrame(() => refitQuickBarPane(pane, newFollowed, generation));
        quickBarFitFrames.push(third);
      }
    });
    quickBarFitFrames.push(second);
  });
  quickBarFitFrames.push(first);
  return generation;
}

export function mountQuickBar(pane) {
  if (paneHoldingQuickBar() !== pane) transitionQuickBar(pane, false);
}

function showQuickBar(show, pane = attachedName && panes.get(attachedName)) {
  const generation = transitionQuickBar(pane || null, show);
  if (show) {
    const frame = requestAnimationFrame(() => {
      if (generation !== quickBarGeneration) return;
      const bar = $('quick-bar');
      const height = bar.offsetHeight;
      if (height !== quickBarHeight) {
        quickBarHeight = height;
        const followed = pane && pane.term.buffer.active.viewportY >= pane.term.buffer.active.baseY;
        refitQuickBarPane(pane, followed, generation);
      }
    });
    quickBarFitFrames.push(frame);
  }
}

export function renderSuggest() {
  scheduleGhost();
  const bar = $('quick-bar');
  const items = suggestions();
  if (!items.length) { showQuickBar(false); return; }
  const pane = attachedName && panes.get(attachedName);
  const completing = lineBuf && lineBuf.length >= 2;
  bar.innerHTML = '<span class="qb-label"></span>';
  bar.querySelector('.qb-label').textContent = t(completing ? 'terminal.quickTab' : 'terminal.quickRecent');
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
  showQuickBar(true, pane);
}

export function resetSuggest(nextPane = null) {
  lineBuf = '';
  freshShell = false;
  ghostRemainder = '';
  clearTimeout(ghostTimer);
  if (ghostEl) ghostEl.style.display = 'none';
  showQuickBar(false, nextPane);
}

export async function writeClipboard(text) {
  try {
    const result = await copyExact(text, value => inv('write_clipboard', { text: value }));
    uev('clipboard-write', 'pbcopy-success', text.length);
    return result;
  } catch (nativeError) {
    uev('clipboard-write', 'pbcopy-failed', text.length);
    if (!navigator.clipboard || !navigator.clipboard.writeText) {
      uev('clipboard-write', 'web-unavailable', text.length);
      throw nativeError;
    }
    try {
      const result = await copyExact(text, value => navigator.clipboard.writeText(value));
      uev('clipboard-write', 'web-success', text.length);
      return result;
    } catch (webError) {
      uev('clipboard-write', 'web-failed', text.length);
      throw webError;
    }
  }
}

/* ---------- chrome wiring ---------- */
export function toggleSidebar() {
  document.body.classList.toggle('side-collapsed');
  const collapsed = document.body.classList.contains('side-collapsed');
  refreshShortcutChrome();
  $('collapse-btn').firstElementChild.style.transform = collapsed ? 'scaleX(-1)' : '';
}
export function refreshShortcutChrome() {
  const collapsed = document.body.classList.contains('side-collapsed');
  $('collapse-btn').title = t(collapsed ? 'app.expandSidebar' : 'app.collapseSidebar', {
    shortcut: formatShortcut(settings.shortcuts.toggleSidebar),
  });
  $('split-right').title = t('session.splitRight', { shortcut: formatShortcut(settings.shortcuts.splitRight) });
  $('split-down').title = t('session.splitDown', { shortcut: formatShortcut(settings.shortcuts.splitDown) });
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initTerminalChrome() {
  document.addEventListener('click', event => {
    if (Date.now() < ignoreLinkOpeningClickUntil) return;
    if ($('ctx').contains(event.target)) return;
    linkActionGeneration++;
    $('ctx').style.display = 'none';
  });

  $('collapse-btn').onclick = toggleSidebar;

  $('home-btn').onclick = backToBoard;

  $('back-btn').onclick = backToBoard;

  $('board-new').onclick = () => newSession(HOME);

  registerShortcutAction('newSession', () => newSession(HOME));

  registerShortcutAction('toggleSidebar', toggleSidebar);

  $('sess-close').onclick = () => closeSession(state.sessionId, true);

  document.addEventListener('keydown', e => {
    if (isComposingKeyEvent(e)) return;
    if (e.key === 'Escape') {
      if ($('ctx').style.display === 'block') { $('ctx').style.display = 'none'; return; }
      /* Esc inside the terminal belongs to the terminal (agents use it) */
      if (state.view === 'session' && !(document.activeElement && document.activeElement.closest('#terminal'))) {
        backToBoard();
      }
    }
  });

  window.addEventListener('deck-shortcuts-changed', refreshShortcutChrome);

  onLocaleChange(refreshShortcutChrome);

  refreshShortcutChrome();
}
