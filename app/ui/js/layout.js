// layout.js — split-tree layout, pane lifecycle, terminal creation, session view
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, DOT_TITLES, duev, inv, listen, setMemChip, state, store, uev } from './state.js';
import { inlineRename, toast } from './dialogs.js';
import { TERM_THEME, panes, pollNow, provider, render, renderSidebar, activeProject } from './board.js';
import { SHELL_FG, acceptGhost, feedMirror, maybeRecordCommand, nextShellTitle, renderSuggest, resetSuggest, showLinkCtx, updateGhost } from './terminal.js';
import { toggleQueuePanel } from './scheduler.js';

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

export function b64ToU8(b64) {
  const bin = atob(b64);
  const u8 = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u8[i] = bin.charCodeAt(i);
  return u8;
}
export function strToB64(str) {
  const u8 = new TextEncoder().encode(str);
  let bin = '';
  for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]);
  return btoa(bin);
}

export const LINK_RE = /(https?:\/\/[^\s"'`)\]]+)|((?:~\/|\.{0,2}\/)?[\w.\-]+(?:\/[\w.\-]+)+(?::\d+(?::\d+)?)?|~\/[\w.\-]+)/g;
export function looksLikePath(v) {
  if (/^(~\/|\.{1,2}\/|\/)/.test(v)) return true;
  if (v.split('/').length > 2) return true;
  return /\.[A-Za-z0-9]{1,8}(?::\d+(?::\d+)?)?$/.test(v.split('/').pop());
}

export function createPane(card) {
  const el = document.createElement('div');
  el.className = 'spane';
  el.innerHTML = `
    <div class="spane-head"><span class="dot ${card.status}"></span><span class="name"></span><button class="px" title="Close pane (session keeps running)">✕</button></div>
    <div class="spane-body"></div>`;
  el.querySelector('.name').textContent = card.title;
  const body = el.querySelector('.spane-body');
  const session = card.session;

  const t = new Terminal({
    fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace',
    fontSize: 12.5,
    lineHeight: 1.7,
    cursorBlink: true,
    macOptionIsMeta: true,
    scrollback: 5000,
    allowProposedApi: true,   // registerDecoration (input separators)
    theme: TERM_THEME,
  });
  const fit = new FitAddon.FitAddon();
  t.loadAddon(fit);
  try {
    /* OSC52: tmux mouse selections land in the system clipboard */
    t.loadAddon(new ClipboardAddon.ClipboardAddon());
  } catch (e) { uev('clipboard-addon-fail'); }
  t.open(body);

  if (!ghostEl) {
    ghostEl = document.createElement('div');
    ghostEl.id = 'ghost';
  }
  /* echo arrives asynchronously — reposition after each parsed write */
  const pane = { sid: card.id, session, el, body, term: t, fit, seps: [] };

  t.onWriteParsed(() => {
    if (ghostRemainder && attachedName === session) updateGhost();
    positionSeparators(pane);
  });
  t.onScroll(() => positionSeparators(pane));
  panes.set(session, pane);

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
  body.addEventListener('mousedown', () => { if (attachedName !== session) focusPane(session); });

  /* drag a card (from sidebar or board) onto a pane edge to split (方案 A) */
  el.addEventListener('dragover', e => {
    e.preventDefault();
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
  el.addEventListener('dragleave', () => { $('dropzone').style.display = 'none'; });
  el.addEventListener('drop', e => {
    e.preventDefault();
    const dz = $('dropzone');
    dz.style.display = 'none';
    const droppedSid = e.dataTransfer.getData('text/deck-session');
    if (!droppedSid || dz.dataset.target !== pane.sid) return;
    addSplit(pane.sid, dz.dataset.dir, dz.dataset.before === 'true', droppedSid);
  });

  wireTerminalInput(pane, t, body);
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
    if (!marker) { if (sepLogged < 5) { sepLogged++; uev('separator', 'no-marker'); } return; }
    const el = document.createElement('div');
    el.style.cssText = 'position:absolute; left:0; right:0; height:1px;' +
      'background:rgba(126,138,153,0.25); pointer-events:none; z-index:4; display:none;';
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
    if (sepLogged < 5) { sepLogged++; uev('separator', 'at', marker.line); }
    positionSeparators(pane);
  } catch (e) {
    if (sepLogged < 5) { sepLogged++; uev('separator', 'fail'); }
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

  /* xterm auto-answers terminal queries (DA `ESC[?..c`, DSR `ESC[..R`,
     OSC `ESC]..BEL`) through onData. Those are not user input — without
     this filter they desync the input mirror on every attach, and the
     first command typed after attaching is never captured. */
  /* Auto-replies, not user input:
     - CSI with a ?/> prefix (DA1/DA2, DECRPM `$y`, mode reports …) — user
       keys never carry those prefixes
     - DSR cursor reports `ESC[..R`, focus events `ESC[I`/`ESC[O`
     - OSC / DCS responses */
  const AUTO_REPLY = /^(?:\x1b\[[?>][0-9;$]*[a-zA-Z]|\x1b\[[0-9;]*R|\x1b\[[IO]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1bP[^\x1b]*\x1b\\)+$/;
  let odLogged = 0, escLogged = 0;
  term.onData(d => {
    const isAutoReply = AUTO_REPLY.test(d);
    /* the input mirror / completion only tracks the focused pane */
    if (!isAutoReply && attachedName === session) {
      if (d.includes('\x1b') && escLogged < 5) {
        escLogged++;
        /* control replies (ESC-prefixed) are loggable; anything else could be
           typed/pasted user text — length only */
        duev('mirror-desync', d.startsWith('\x1b') ? 'esc' : 'plain', d.length);
      }
      const preDesynced = lineBuf === null;
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
        duev('ondata', lineBuf === null ? 'desync' : 'ok', d.length, lineBuf === null ? -1 : lineBuf.length);
      }
    }
    inv('pty_write', { name: session, dataB64: strToB64(d) })
      .catch(() => uev('pty-write-fail'));
  });
  /* app shortcuts pass through; ⌘C/⌘V are handled here because a menu-less
     macOS app gets no standard edit actions in the webview */
  term.attachCustomKeyEventHandler(e => {
    if (e.metaKey && e.key === 'b') return false;
    /* split shortcuts (方案 B): ⌘D right, ⌘⇧D down */
    if (e.type === 'keydown' && e.metaKey && (e.key === 'd' || e.key === 'D')) {
      e.preventDefault();
      showSplitPicker(e.shiftKey ? 'col' : 'row');
      return false;
    }
    /* ⌘V: returning false skips xterm's key handling; the browser then fires
       a native paste event, which xterm's textarea handler feeds into the
       PTY. (navigator.clipboard.readText is permission-blocked in WKWebView —
       the native paste event is the reliable path.) */
    if (e.type === 'keydown' && e.metaKey && e.key === 'v') return false;
    if (e.type === 'keydown' && e.metaKey && e.key === 'c' && term.hasSelection()) {
      navigator.clipboard.writeText(term.getSelection()).catch(() => {});
      return false;
    }
    /* ghost suggestion: Tab or → applies it in place; Esc dismisses */
    if (e.type === 'keydown' && ghostRemainder) {
      if (e.key === 'Tab' || e.key === 'ArrowRight') {
        e.preventDefault();
        acceptGhost();
        return false;
      }
      if (e.key === 'Escape') {
        lineBuf = null;
        freshShell = false;
        renderSuggest();
        return false;
      }
    }
    if (e.type === 'keydown' && e.key === 'Escape'
        && $('quick-bar').style.display === 'flex') {
      lineBuf = null;
      freshShell = false;
      renderSuggest();
      return false;
    }
    return true;
  });

  /* clickable paths and URLs in terminal output */
  term.registerLinkProvider({
    provideLinks(lineNo, cb) {
      const line = term.buffer.active.getLine(lineNo - 1);
      if (!line) return cb(undefined);
      const text = line.translateToString(true);
      const links = [];
      LINK_RE.lastIndex = 0;
      let m;
      while ((m = LINK_RE.exec(text)) !== null) {
        let value = m[0];
        while (/[.,;:]$/.test(value)) value = value.slice(0, -1);
        const kind = m[1] ? 'url' : 'path';
        if (kind === 'path' && !looksLikePath(value)) continue;
        links.push({
          range: { start: { x: m.index + 1, y: lineNo }, end: { x: m.index + value.length, y: lineNo } },
          text: value,
          activate: (e, txt) => {
            e.stopPropagation();
            /* the menu swallows the mouseup, leaving xterm's selection
               tracker in drag mode — end the gesture synthetically */
            const scr = host.querySelector('.xterm-screen');
            if (scr) {
              scr.dispatchEvent(new MouseEvent('mouseup', {
                bubbles: true, clientX: e.clientX, clientY: e.clientY,
              }));
            }
            try { term.clearSelection(); } catch (e2) { /* fine */ }
            const c = card();
            showLinkCtx(e, kind, txt, c ? c.dir : HOME);
          },
        });
      }
      cb(links);
    },
  });

  /* Wheel handling, deck-driven: tmux mouse mode stays OFF so xterm keeps
     its native local selection (drag + ⌘C). Wheel deltas are batched and
     translated into tmux copy-mode scrolling on the backend — long agent
     output is reachable, and an empty shell is a true no-op. If an app
     requests mouse reporting itself, the wheel passes through to it. */
  let wheelAcc = 0, wheelTimer = null;
  host.addEventListener('wheel', e => {
    const mode = term.modes && term.modes.mouseTrackingMode;
    if (mode && mode !== 'none') return;   // app owns the mouse
    e.preventDefault();
    e.stopPropagation();
    wheelAcc += e.deltaY;
    if (wheelTimer) return;
    wheelTimer = setTimeout(() => {
      const lines = Math.round(wheelAcc / 14);
      wheelAcc = 0;
      wheelTimer = null;
      if (lines) inv('scroll_session', { name: session, lines }).catch(() => {});
    }, 50);
  }, { passive: false, capture: true });

}

/* ----- layout rendering & pane lifecycle ----- */
export function fitAll() {
  requestAnimationFrame(() => {
    panes.forEach(p => {
      try {
        p.fit.fit();
        inv('pty_resize', { name: p.session, cols: p.term.cols, rows: p.term.rows }).catch(() => {});
      } catch (e) { /* pane mid-teardown */ }
    });
    if (ghostRemainder) updateGhost();
    panes.forEach(positionSeparators);
  });
}
new ResizeObserver(() => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(fitAll, 80);
}).observe(document.getElementById('terminal'));

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
  if (layout) buildNode(layout, host, 1);
  fitAll();
}

/* keep pane mini-headers in sync with polling / renames */
export function updatePaneChrome(card) {
  const p = card && panes.get(card.session);
  if (!p) return;
  const dot = p.el.querySelector('.spane-head .dot');
  if (dot) { dot.className = 'dot ' + card.status; dot.title = DOT_TITLES[card.status] || ''; }
  const name = p.el.querySelector('.spane-head .name');
  if (name && name.textContent !== card.title) name.textContent = card.title;
}

export function focusPane(session) {
  const p = panes.get(session);
  if (!p) return;
  const changed = attachedName !== session;
  attachedName = session;
  term = p.term;
  panes.forEach(q => q.el.classList.toggle('focus', q === p));
  state.sessionId = p.sid;
  if (changed) resetSuggest();
  if (ghostEl && ghostEl.parentElement !== p.body) p.body.appendChild(ghostEl);
  renderSessionView();
  renderSidebar();
  p.term.focus();
}

/* Returns true only when the session was actually CREATED here (backend is
   idempotent: an already-live session is success, not an error). Callers use
   that to decide fresh-shell behaviors — clearing history on a session that
   merely LOOKED stopped would eat a live agent's scrollback. */
export async function ensureAttached(pane) {
  const card = provider.get(pane.sid);
  if (!card) return false;
  let created = false;
  try {
    if (card.status === 'stopped') {
      created = await inv('start_session', { name: card.session, dir: card.dir, cmd: card.cmd });
    }
    await inv('attach_session', { name: card.session, cols: pane.term.cols, rows: pane.term.rows });
  } catch (e) {
    toast('attach failed: ' + e);
  }
  return created;
}

export async function addSplit(targetSid, dir, before, newSid) {
  const card = provider.get(newSid);
  if (!card || state.view !== 'session' || !layout) return;
  if (newSid === targetSid) return;
  /* already open in a pane → this is a MOVE: pluck the leaf and re-insert
     at the drop position; the terminal instance is reused untouched */
  if (panes.has(card.session)) {
    if (!collectLeaves(layout).includes(newSid)) { focusPane(card.session); return; }
    layout = removeFromLayout(layout, newSid);
    layout = splitAt(layout, targetSid, dir, newSid, before);
    renderLayout();
    focusPane(card.session);
    return;
  }
  const pane = createPane(card);
  layout = splitAt(layout, targetSid, dir, newSid, before);
  renderLayout();
  const created = await ensureAttached(pane);
  if (created) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  freshShell = created && !card.cmd.trim();
  focusPane(card.session);
  pollNow();
}

/* close one pane; the session keeps running unless the card itself closes */
export function closePaneBySid(sid, opts = {}) {
  const entry = [...panes.values()].find(p => p.sid === sid);
  if (!entry) return;
  if (opts.detach !== false) inv('detach_session', { name: entry.session }).catch(() => {});
  try { entry.term.dispose(); } catch (e) { /* already gone */ }
  entry.el.remove();
  panes.delete(entry.session);
  layout = removeFromLayout(layout, sid);
  if (!layout || !collectLeaves(layout).length) {
    backToBoard();
    return;
  }
  renderLayout();
  if (attachedName === entry.session) {
    const nextSid = collectLeaves(layout)[0];
    const c = provider.get(nextSid);
    if (c) focusPane(c.session);
  }
}

/* 方案 B: split button / ⌘D — pick a session for the new pane */
export function showSplitPicker(dir) {
  if (state.view !== 'session' || !state.sessionId) return;
  const targetSid = state.sessionId;
  const openSids = new Set(collectLeaves(layout));
  const candidates = store.cards.filter(c => !openSids.has(c.id));
  const order = { running: 0, waiting: 0, stopped: 1 };
  candidates.sort((a, b) => order[a.status] - order[b.status]);
  const ctx = $('ctx');
  ctx.innerHTML = `<div class="ctx-label">Split ${dir === 'col' ? 'down' : 'right'} — choose session</div>` +
    candidates.slice(0, 12).map(c =>
      `<button data-sid="${c.id}"><span style="color:${c.status === 'stopped' ? 'var(--faint)' : c.status === 'waiting' ? 'var(--wait)' : 'var(--run)'}">●</span> ${c.title.replace(/</g, '&lt;')}</button>`).join('') +
    `<hr><button data-new="1">＋ new shell here</button>`;
  ctx.onclick = async ev => {
    const sid = ev.target.closest('button') && ev.target.closest('button').dataset.sid;
    const isNew = ev.target.closest('button') && ev.target.closest('button').dataset.new;
    ctx.style.display = 'none';
    if (sid) addSplit(targetSid, dir, false, sid);
    if (isNew) {
      const p = activeProject();
      const focused = provider.get(targetSid);
      const c = provider.create({
        projectId: p.id,
        columnId: (p.columns.find(x => x.name === 'Working') || p.columns[0]).id,
        title: nextShellTitle(p),
        cmd: '',
        dir: focused ? focused.dir : HOME,
      });
      addSplit(targetSid, dir, false, c.id);
    }
  };
  const btn = $(dir === 'col' ? 'split-down' : 'split-right');
  const r = btn.getBoundingClientRect();
  ctx.style.display = 'block';
  ctx.style.left = Math.min(r.left, innerWidth - ctx.offsetWidth - 8) + 'px';
  ctx.style.top = (r.bottom + 6) + 'px';
}
$('split-right').onclick = e => { e.stopPropagation(); showSplitPicker('row'); };
$('split-down').onclick = e => { e.stopPropagation(); showSplitPicker('col'); };

/* NOTE: listen() requires the core:event permission in
   src-tauri/capabilities/default.json — without it registration is refused
   with a silent promise rejection and the terminal never receives output. */
listen('pty-data', ev => {
  const { name, data } = ev.payload;
  const p = panes.get(name);
  if (p) {
    const u8 = b64ToU8(data);
    rxBytes += u8.length;
    if (rxLogged < 3 || rxLogged % 200 === 0) uev('pty-rx', null, u8.length, rxBytes);
    rxLogged++;
    p.term.write(u8);
  }
}).catch(() => uev('listen-fail', 'pty-data'));
listen('pty-exit', ev => {
  if (panes.has(ev.payload.name)) {
    toast('session ended');
    pollNow();
  }
}).catch(() => uev('listen-fail', 'pty-exit'));

/* ---------- session view ---------- */
export async function openSession(sid) {
  const card = provider.get(sid);
  if (!card) return;
  /* already open in a pane → just focus it */
  if (state.view === 'session' && panes.has(card.session)) {
    focusPane(card.session);
    return;
  }
  leaveSessionView();
  state.projectId = card.projectId;
  state.view = 'session';
  state.sessionId = sid;
  toggleQueuePanel(false);
  render();
  const pane = createPane(card);
  layout = leafOf(sid);
  renderLayout();
  const created = await ensureAttached(pane);
  if (created) setTimeout(() => inv('clear_history', { name: card.session }).catch(() => {}), 900);
  freshShell = created && !card.cmd.trim();
  focusPane(card.session);
  /* history feeds both the fresh-shell chips and typed-prefix completion */
  inv('recent_commands', { limit: 50 })
    .then(c => { histCache = c; renderSuggest(); })
    .catch(() => { histCache = []; });
  pollNow();
}

export function leaveSessionView() {
  resetSuggest();
  toggleQueuePanel(false);
  panes.forEach(p => {
    inv('detach_session', { name: p.session }).catch(() => {});
    try { p.term.dispose(); } catch (e) { /* fine */ }
    p.el.remove();
  });
  panes.clear();
  layout = null;
  attachedName = null;
  term = null;
}

export function backToBoard() {
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
  $('sess-dot').title = DOT_TITLES[s.status] || '';
  /* back button names the board this card lives on */
  const proj0 = activeProject();
  const col0 = proj0 && proj0.columns.find(c => c.id === s.columnId);
  $('back-label').textContent = col0 ? col0.name : 'Board';
  const nameEl = $('sess-name');
  nameEl.textContent = s.title;
  nameEl.title = 'double-click to rename';
  nameEl.ondblclick = () => {
    inlineRename(nameEl, s.title, v => {
      if (v) provider.rename(s.id, v);
      renderSessionView();
      renderSidebar();
    });
  };
  setMemChip($('sess-mem'), s);
  $('sess-path').textContent = (s.cmd ? '$ ' + s.cmd + '  ·  ' : '') + s.dir;

  const proj = activeProject();
  const sel = $('sess-col');
  sel.innerHTML = proj.columns.map(c =>
    `<option value="${c.id}" ${c.id === s.columnId ? 'selected' : ''}></option>`).join('');
  [...sel.options].forEach((o, i) => { o.textContent = proj.columns[i].name; });
}
