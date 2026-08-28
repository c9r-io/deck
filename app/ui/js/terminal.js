// terminal.js — context menus & link opening, ghost completion, chrome wiring
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, duev, inv, state, uev } from './state.js';
import { quickBarBottom } from './pure.js';
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

export function renameCardInline(sid) {
  const s = provider.get(sid);
  const host = document.querySelector(`.card[data-sid="${sid}"] .card-title`)
    || document.querySelector('#side-list .side-item.active .name');
  if (!host) return;
  inlineRename(host, s.title, v => {
    if (v) provider.rename(sid, v);
    render();
  });
}

export function editDescInline(sid) {
  const s = provider.get(sid);
  const card = document.querySelector(`.card[data-sid="${sid}"]`);
  if (!card) {
    // window.prompt is a silent no-op in WKWebView — use the in-app dialog
    promptDialog('Description', s.desc || '').then(v => {
      if (v !== null) provider.setDesc(sid, v.trim());
    });
    return;
  }
  let d = card.querySelector('.card-desc');
  if (!d) {
    d = document.createElement('div');
    d.className = 'card-desc';
    card.querySelector('.card-meta').after(d);
  }
  inlineRename(d, s.desc || '', v => {
    if (v !== null) provider.setDesc(sid, v);
    render();
  }, true);
}

export function showSessionCtx(e, sid) {
  e.preventDefault();
  e.stopPropagation();
  const s = provider.get(sid);
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
    if (a === 'rename') renameCardInline(sid);
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

export function showLinkCtx(e, kind, value, cwd) {
  const ctx = $('ctx');
  ctx.innerHTML = `<span class="ctx-value"></span>` + (kind === 'url'
    ? `<button data-a="url">Open in browser</button>
       <button data-a="copy">Copy URL</button>`
    : `<button data-a="editor">Open in editor</button>
       <button data-a="reveal">Reveal in Finder</button>
       <button data-a="copy">Copy path</button>`);
  ctx.querySelector('.ctx-value').textContent = value;
  ctx.onclick = ev => {
    const a = ev.target.dataset && ev.target.dataset.a;
    ctx.style.display = 'none';
    if (a === 'copy') {
      navigator.clipboard.writeText(value).then(() => toast('copied'), () => toast('copy failed'));
    } else if (a) {
      inv('open_target', { kind: a, value, cwd: cwd || HOME })
        .then(() => toast(a === 'url' ? 'opened in browser' : a === 'editor' ? 'opened in editor' : 'revealed in Finder'))
        .catch(err => toast('open failed: ' + err));
    }
  };
  placeCtx(e);
}

document.addEventListener('click', () => { $('ctx').style.display = 'none'; });

/* ---------- new session: no modal — create a shell and enter it.
   Title is renamed on the board later; command is typed in the shell
   (with quick-command chips as a shortcut); dir defaults to $HOME. ---------- */
export function nextShellTitle(p) {
  const used = new Set(provider.list(p.id).map(c => c.title));
  let n = 1;
  while (used.has('shell ' + n)) n++;
  return 'shell ' + n;
}

export async function newSession(dir) {
  if (creatingSession) return;
  creatingSession = true;
  try {
    const p = activeProject();
    const selected = p.columns.find(c => c.id === p.selected);
    const columnId = (selected
      || p.columns.find(c => c.name === 'Working')
      || p.columns[1] || p.columns[0]).id;
    const card = provider.create({
      projectId: p.id, columnId,
      title: nextShellTitle(p),
      cmd: '',
      dir: dir || HOME,
    });
    await openSession(card.id);
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

/* Keep the bar off the line being typed: a shell prompt at the bottom of a
   pane sits exactly where a bottom-anchored overlay would be. */
export function placeQuickBar(bar) {
  const view = bar.offsetParent;
  const fp = attachedName && panes.get(attachedName);
  const screen = fp && fp.body.querySelector('.xterm-screen');
  if (!view || !screen || !term) { bar.style.bottom = '0px'; return; }
  const { h } = ghostCellDims(fp.body);
  const vRect = view.getBoundingClientRect();
  const sRect = screen.getBoundingClientRect();
  const cursorTop = sRect.top - vRect.top + term.buffer.active.cursorY * h;
  bar.style.bottom = quickBarBottom({
    viewH: vRect.height, cursorTop, cellH: h, barH: bar.offsetHeight,
  }) + 'px';
}

export function renderSuggest() {
  scheduleGhost();
  const bar = $('quick-bar');
  const items = suggestions();
  if (!items.length) { bar.style.display = 'none'; return; }
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
  bar.style.display = 'flex';
  placeQuickBar(bar);   // after display+content: offsetHeight must be real
}

export function resetSuggest() {
  lineBuf = '';
  freshShell = false;
  ghostRemainder = '';
  clearTimeout(ghostTimer);
  if (ghostEl) ghostEl.style.display = 'none';
  $('quick-bar').style.display = 'none';
}

/* ---------- copy panel ----------
   A selection inside the terminal can only ever cover ONE screen: tmux owns
   the scrollback and repaints the same rows in place, so a drag cannot be
   carried past the top of the pane and a long answer could not be copied in
   one piece. The panel shows the pane's scrollback as plain text, where the
   selection scrolls and ⌘C behaves like it does in any document. */
const COPY_LINES = 20000;

export async function openCopyPanel(session, title) {
  const box = $('copybox');
  const body = $('cb-body');
  $('cb-title').textContent = title ? `Output — ${title}` : 'Output';
  body.textContent = 'reading the scrollback…';
  box.style.display = 'flex';
  let text = '';
  try {
    text = await inv('capture_scrollback', { name: session, lines: COPY_LINES });
  } catch (e) {
    body.textContent = 'could not read this session’s output: ' + e;
    return;
  }
  body.textContent = text;
  const n = text ? text.split('\n').length : 0;
  $('cb-title').textContent = (title ? `Output — ${title}` : 'Output') +
    `  ·  ${n} line${n === 1 ? '' : 's'}` + (n >= COPY_LINES ? ' (newest)' : '');
  body.scrollTop = body.scrollHeight;   // the newest output is what you came for
  $('cb-all').onclick = () => {
    navigator.clipboard.writeText(text)
      .then(() => toast(`copied ${n} line${n === 1 ? '' : 's'}`), () => toast('copy failed'));
  };
}

export function closeCopyPanel() {
  $('copybox').style.display = 'none';
}
export const copyPanelOpen = () => $('copybox').style.display === 'flex';

$('cb-close').onclick = closeCopyPanel;
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
$('sess-col').addEventListener('change', e => {
  provider.move(state.sessionId, e.target.value);
  const c = activeProject().columns.find(c => c.id === e.target.value);
  toast(`moved to ${c.name}`);
});
document.addEventListener('keydown', e => {
  if ((e.metaKey || e.ctrlKey) && e.key === 'b') { e.preventDefault(); toggleSidebar(); return; }
  /* ⌘⇧C: copy panel for the focused pane (⌘C stays "copy the selection") */
  if (e.metaKey && e.shiftKey && (e.key === 'c' || e.key === 'C')
      && state.view === 'session' && !copyPanelOpen()) {
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
