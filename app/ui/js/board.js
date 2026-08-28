// board.js — board CRUD provider, polling loop, sidebar/tabs/board rendering
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, COL_HINTS, DOT_TITLES, POLL_MS, QUIET_SECS, emit, genId, inv, listeners, sessionName, setMemChip, state, store, uev } from './state.js';
import { saveBoard } from './persistence.js';
import { confirmDialog, inlineRename, toast } from './dialogs.js';
import { clearSeparators, closePaneBySid, leaveSessionView, openSession, renderSessionView, updatePaneChrome } from './layout.js';
import { SHELL_FG, showProjectCtx, showSessionCtx } from './terminal.js';
import { renderQueueUI, setQueueChip, updateQuietHints } from './scheduler.js';

/* ---------- provider (board CRUD is sync-local, persistence async) ---------- */
export const activeProject = () => provider.project(state.projectId);

export const provider = {
  projects: () => store.projects,
  project: pid => store.projects.find(p => p.id === pid),
  list: pid => store.cards.filter(c => !pid || c.projectId === pid),
  get: sid => store.cards.find(c => c.id === sid),
  subscribe: fn => { listeners.add(fn); return () => listeners.delete(fn); },

  createProject(name) {
    const p = {
      id: genId('P'),
      name,
      columns: ['Attention', 'Working', 'Queued', 'Parked'].map(n => ({ id: genId('C'), name: n })),
    };
    store.projects.push(p);
    saveBoard();
    emit('projects', p);
    return p;
  },
  renameProject(pid, name) {
    this.project(pid).name = name;
    saveBoard();
    emit('projects');
  },
  removeProject(pid) {
    for (const c of store.cards.filter(c => c.projectId === pid)) {
      if (c.status !== 'stopped') inv('kill_session', { name: c.session }).catch(() => {});
    }
    store.cards = store.cards.filter(c => c.projectId !== pid);
    store.projects = store.projects.filter(p => p.id !== pid);
    saveBoard();
    emit('projects');
  },

  addColumn(pid, name) {
    const col = { id: genId('C'), name };
    this.project(pid).columns.push(col);
    saveBoard();
    emit('projects');
    return col;
  },
  renameColumn(pid, cid, name) {
    const col = this.project(pid).columns.find(c => c.id === cid);
    if (col) col.name = name;
    saveBoard();
    emit('projects');
  },
  removeColumn(pid, cid) {
    const p = this.project(pid);
    if (p.columns.length <= 1) return false;
    if (p.selected === cid) p.selected = null;
    p.columns = p.columns.filter(c => c.id !== cid);
    const fallback = p.columns[0];
    let moved = 0;
    for (const c of store.cards) {
      if (c.projectId === pid && c.columnId === cid) { c.columnId = fallback.id; moved++; }
    }
    saveBoard();
    emit('projects');
    return { fallback: fallback.name, moved };
  },

  create({ projectId, columnId, title, cmd, dir, desc = '' }) {
    const id = genId('S');
    const card = {
      id, projectId, columnId, title, cmd, dir, desc,
      session: sessionName(title, id),
      status: 'stopped', mem: null, tail: [],
    };
    store.cards.push(card);
    saveBoard();
    emit('list', card);
    return card;
  },

  /* stop and delete are one operation */
  async close(sid) {
    const c = this.get(sid);
    if (!c) return;
    if (c.status !== 'stopped') {
      try { await inv('kill_session', { name: c.session }); } catch (e) { /* already gone */ }
    }
    inv('queue_clear_session', { session: c.session }).catch(() => {});
    store.cards = store.cards.filter(x => x.id !== sid);
    saveBoard();
    emit('list', c);
  },

  move(sid, columnId) {
    const c = this.get(sid);
    if (!c || c.columnId === columnId) return;
    c.columnId = columnId;
    saveBoard();
    emit('list', c);
  },
  rename(sid, title) {
    const c = this.get(sid);
    if (c) { c.title = title; saveBoard(); emit('list', c); }
  },
  setDesc(sid, desc) {
    const c = this.get(sid);
    if (c) { c.desc = desc; saveBoard(); emit('list', c); }
  },
};

/* ---------- polling ---------- */
export async function pollNow() {
  if (!store.cards.length) return;
  const names = store.cards.map(c => c.session);
  const tailFor = state.view === 'board'
    ? store.cards.filter(c => c.projectId === state.projectId).map(c => c.session)
    : [];
  let infos;
  try {
    infos = await inv('poll_sessions', { names, tailFor });
  } catch (e) {
    /* a silently dead poll leaves every card gray — log once per distinct
       error so app.log shows WHY the board went stale */
    if (String(e) !== state.lastPollError) {
      state.lastPollError = String(e);
      uev('poll-fail');
    }
    return;
  }
  if (state.lastPollError) { state.lastPollError = null; uev('poll-recovered'); }
  const byName = new Map(infos.map(i => [i.name, i]));
  const exited = [];
  for (const c of store.cards) {
    const info = byName.get(c.session);
    if (!info) continue;
    /* the shell exited (Ctrl+D etc.) → the card has nothing left to hold;
       close it without ceremony. Only live→dead transitions count, so cards
       that were already stopped (e.g. after an app restart) stay. */
    if (!info.alive && (c.status === 'running' || c.status === 'waiting')) {
      exited.push(c);
      continue;
    }
    const status = !info.alive ? 'stopped'
      : (info.idle_secs != null && info.idle_secs >= QUIET_SECS) ? 'waiting' : 'running';
    const mem = info.alive && info.mem_mb != null ? info.mem_mb : null;
    const tail = info.tail || [];
    const prevFg = c.fg;
    c.fg = info.fg || null;
    /* shell → agent transition: shell-era separator lines would overlap the
       TUI's in-place repaints — drop them for this pane */
    if (prevFg !== c.fg && !SHELL_FG.test(c.fg || '')) {
      const pn = panes.get(c.session);
      if (pn && pn.seps.length) clearSeparators(pn);
    }
    c.idle = info.alive ? (info.idle_secs != null ? info.idle_secs : null) : null;
    /* copy-mode = the visible frame is FROZEN scrollback; surface it (the
       pane header chip) instead of letting an agent TUI look hung */
    const scrolled = !!(info.alive && info.scrolled);
    if (scrolled !== !!c.scrolled) { c.scrolled = scrolled; updatePaneChrome(c); }
    if (status !== c.status) { c.status = status; emit('status', c); }
    if ((mem == null) !== (c.mem == null) || (mem != null && Math.abs(mem - c.mem) > 1)) {
      c.mem = mem;
      emit('mem', c);
    }
    if (tail.join('\n') !== (c.tail || []).join('\n')) { c.tail = tail; emit('output', c); }
  }
  updateQuietHints();
  for (const c of exited) {
    inv('queue_clear_session', { session: c.session }).catch(() => {});
    closePaneBySid(c.id);
    store.cards = store.cards.filter(x => x.id !== c.id);
    toast(`"${c.title}" closed — shell exited`);
    emit('list', c);
  }
  if (exited.length) saveBoard();
}
export function startPolling() {
  clearInterval(pollTimer);
  pollTimer = setInterval(pollNow, POLL_MS);
  pollNow();
}

/* ---------- sidebar ---------- */
export function renderSidebar() {
  const list = $('side-list');
  const st = list.scrollTop;
  list.innerHTML = '';
  const all = provider.list(state.projectId);
  const order = { waiting: 0, running: 1, stopped: 2 };
  all.sort((a, b) => order[a.status] - order[b.status]);
  $('side-count').textContent = all.length;
  const proj = activeProject();
  for (const s of all) {
    const el = document.createElement('div');
    el.className = 'side-item' + (state.view === 'session' && state.sessionId === s.id ? ' active' : '');
    el.title = s.title;
    el.innerHTML = `<span class="dot ${s.status}"></span><span class="name"></span><span class="col-tag"></span>`;
    el.querySelector('.name').textContent = s.title;
    const c = proj && proj.columns.find(c => c.id === s.columnId);
    el.querySelector('.col-tag').textContent = c ? c.name : '';
    el.onclick = () => openSession(s.id);
    el.oncontextmenu = e => showSessionCtx(e, s.id);
    /* sidebar items are drag sources for split (方案 A) */
    el.draggable = true;
    el.addEventListener('dragstart', ev => {
      ev.dataTransfer.setData('text/deck-session', s.id);
    });
    el.addEventListener('dragend', () => { $('dropzone').style.display = 'none'; });
    list.appendChild(el);
  }
  list.scrollTop = st;
  $('home-btn').classList.toggle('active', state.view === 'board');
}

/* ---------- project tabs ---------- */
export function renderTabs() {
  const bar = $('tabs');
  bar.innerHTML = '';
  for (const p of provider.projects()) {
    const el = document.createElement('div');
    el.className = 'tab' + (p.id === state.projectId ? ' active' : '');
    el.innerHTML = `<span class="name"></span>`;
    el.querySelector('.name').textContent = p.name;
    if (p.id !== state.projectId &&
        provider.list(p.id).some(s => s.status === 'waiting')) {
      el.insertAdjacentHTML('beforeend', '<span class="wait-dot" title="a session has been quiet — may be waiting for input"></span>');
    }
    el.onclick = () => switchProject(p.id);
    el.ondblclick = () => renameTab(el, p);
    el.oncontextmenu = e => showProjectCtx(e, p.id);
    bar.appendChild(el);
  }
  const add = document.createElement('button');
  add.className = 'tab-add';
  add.title = 'New project';
  add.textContent = '＋';
  add.onclick = () => {
    const p = provider.createProject('new project');
    switchProject(p.id);
    const tab = document.querySelector('#tabs .tab.active');
    if (tab) renameTab(tab, p);
  };
  bar.appendChild(add);
}

export function renameTab(el, p) {
  inlineRename(el, p.name, v => {
    if (v) provider.renameProject(p.id, v);
    render();
  });
}

export function switchProject(pid) {
  if (state.projectId === pid && state.view === 'board') return;
  if (state.view === 'session') leaveSessionView();
  state.projectId = pid;
  state.view = 'board';
  state.sessionId = null;
  render();
  pollNow();
}

/* ---------- board ---------- */
export function renderBoard() {
  const p = activeProject();
  if (!p) return;
  $('board-title').textContent = p.name;
  const wrap = $('columns');
  const hScroll = wrap.scrollLeft;
  const colScroll = {};
  wrap.querySelectorAll('.column[data-cid]').forEach(el => {
    colScroll[el.dataset.cid] = el.querySelector('.col-cards').scrollTop;
  });
  wrap.innerHTML = '';
  const all = provider.list(p.id);

  for (const c of p.columns) {
    const colEl = document.createElement('div');
    colEl.className = 'column';
    colEl.dataset.cid = c.id;
    const cards = all.filter(s => s.columnId === c.id);
    if (p.selected === c.id) colEl.classList.add('selected');
    colEl.innerHTML = `
      <div class="col-head">
        <span class="col-name"></span>
        <span class="count">${cards.length}</span>
        <button class="col-del" title="Delete board">✕</button>
      </div>
      <div class="col-cards"></div>`;
    /* click anywhere on the board (header or empty space) to make it the
       target for new sessions; cards stop propagation. Toggle classes in
       place — a re-render here would replace the title element between the
       two clicks of a double-click and break rename. */
    colEl.addEventListener('click', e => {
      if (e.target.closest('.card, .col-del') || e.target.tagName === 'INPUT') return;
      p.selected = p.selected === c.id ? null : c.id;
      saveBoard();
      document.querySelectorAll('#columns .column[data-cid]').forEach(el2 => {
        el2.classList.toggle('selected', el2.dataset.cid === p.selected);
      });
    });
    const nameEl = colEl.querySelector('.col-name');
    nameEl.textContent = c.name;
    const hint = COL_HINTS[c.name];
    nameEl.title = (hint ? hint + ' · ' : '') +
      'click to target new sessions here · double-click to rename';
    nameEl.ondblclick = () => {
      inlineRename(nameEl, c.name, v => {
        if (v) provider.renameColumn(p.id, c.id, v);
        render();
      });
    };
    colEl.querySelector('.col-del').onclick = async e => {
      e.stopPropagation();
      if (p.columns.length <= 1) { toast('a project needs at least one board'); return; }
      if (cards.length &&
          !(await confirmDialog(`Delete board "${c.name}"? Its ${cards.length} card(s) move to "${p.columns.find(x => x.id !== c.id).name}".`))) return;
      const r = provider.removeColumn(p.id, c.id);
      if (r && r.moved) toast(`moved ${r.moved} card(s) to ${r.fallback}`);
    };

    let dragDepth = 0;
    colEl.addEventListener('dragover', e => e.preventDefault());
    colEl.addEventListener('dragenter', e => {
      e.preventDefault();
      dragDepth++;
      colEl.classList.add('drop-target');
    });
    colEl.addEventListener('dragleave', () => {
      if (--dragDepth <= 0) { dragDepth = 0; colEl.classList.remove('drop-target'); }
    });
    colEl.addEventListener('drop', e => {
      e.preventDefault();
      dragDepth = 0;
      colEl.classList.remove('drop-target');
      const sid = e.dataTransfer.getData('text/deck-session');
      if (sid) { provider.move(sid, c.id); toast(`moved to ${c.name}`); }
    });

    const body = colEl.querySelector('.col-cards');
    for (const s of cards) body.appendChild(cardEl(s));
    wrap.appendChild(colEl);
  }

  const ghost = document.createElement('div');
  ghost.className = 'col-ghost';
  ghost.innerHTML = '<button>＋ New board</button>';
  ghost.querySelector('button').onclick = () => {
    const c = provider.addColumn(p.id, 'new board');
    render();
    wrap.scrollLeft = wrap.scrollWidth;
    const heads = document.querySelectorAll('#columns .column .col-name');
    const nameEl = heads[heads.length - 1];
    if (nameEl) inlineRename(nameEl, c.name, v => {
      if (v) provider.renameColumn(p.id, c.id, v);
      render();
    });
  };
  wrap.appendChild(ghost);

  wrap.scrollLeft = hScroll;
  wrap.querySelectorAll('.column[data-cid]').forEach(el => {
    const st = colScroll[el.dataset.cid];
    if (st) el.querySelector('.col-cards').scrollTop = st;
  });
}

export function cardEl(s) {
  const el = document.createElement('div');
  el.className = 'card' + (s.status === 'waiting' ? ' waiting' : '');
  el.dataset.sid = s.id;
  el.draggable = true;

  const tail = (s.tail || []).slice(-2);
  /* fixed shape: the tail box is always present (2 lines) and there are no
     hover-only rows — cards never change size under the pointer */
  el.innerHTML = `
    <div class="card-top"><span class="dot ${s.status}" title="${DOT_TITLES[s.status] || ''}"></span><span class="card-title"></span><button class="card-x" title="Close session">✕</button></div>
    <div class="card-meta"><span class="cmd"></span><span class="dir"></span><span class="q-chip"></span><span class="mem-chip" title="Memory of the whole session process tree: shell + agent + spawned children"></span></div>
    ${s.desc ? '<div class="card-desc"></div>' : ''}
    <div class="card-tail"><div></div><div></div></div>`;
  el.querySelector('.card-title').textContent = s.title;
  setMemChip(el.querySelector('.mem-chip'), s);
  setQueueChip(el.querySelector('.q-chip'), s);   // self-fill: survives card rebuilds
  el.querySelector('.cmd').textContent = s.cmd ? '$ ' + s.cmd : '';
  el.querySelector('.dir').textContent = s.dir;
  if (s.desc) {
    const d = el.querySelector('.card-desc');
    d.textContent = s.desc;
    d.title = s.desc;
  }
  const tailDivs = el.querySelectorAll('.card-tail div');
  tail.forEach((l, i) => { tailDivs[i].textContent = l; });

  el.addEventListener('dragstart', e => {
    e.dataTransfer.setData('text/deck-session', s.id);
    el.classList.add('dragging');
  });
  el.addEventListener('dragend', () => el.classList.remove('dragging'));
  el.querySelector('.card-x').onclick = e => {
    e.stopPropagation();
    closeSession(s.id);
  };
  el.addEventListener('click', e => {
    e.stopPropagation();   // don't toggle the board selection underneath
    if (e.target.closest('.card-x')) return;
    openSession(s.id);
  });
  el.oncontextmenu = e => showSessionCtx(e, s.id);
  return el;
}

export function updateCardInPlace(s) {
  const el = document.querySelector(`.card[data-sid="${s.id}"]`);
  if (!el) return;
  const tail = (s.tail || []).slice(-2);
  const tailDivs = el.querySelectorAll('.card-tail div');
  tailDivs.forEach((d, i) => { d.textContent = tail[i] || ''; });
  const dot = el.querySelector('.dot');
  if (dot) { dot.className = 'dot ' + s.status; dot.title = DOT_TITLES[s.status] || ''; }
  el.classList.toggle('waiting', s.status === 'waiting');
}

export async function closeSession(sid, needConfirm = false) {
  const s = provider.get(sid);
  if (!s) return;
  const live = s.status !== 'stopped';
  if (needConfirm &&
      !(await confirmDialog(`Close "${s.title}"?${live ? ' The shell will be terminated.' : ''}`))) return;
  closePaneBySid(sid, { detach: false });   // pane out first (may fall back to board)
  await provider.close(sid);
  toast(`closed: ${s.title}`);
  render();
}

/* ---------- terminal (xterm + PTY bridge, split panes) ----------
   The session view is a split tree of panes, one live session each.
   `term` and `attachedName` always alias the FOCUSED pane, so the
   completion / ghost / clipboard code keeps its single-terminal world. */
export const panes = new Map();      // session name -> {sid, session, el, body, term, fit}

export const TERM_THEME = {
  background: '#101318',
  foreground: '#dce3ec',
  cursor: '#4fd6be',
  selectionBackground: 'rgba(79,214,190,0.25)',
  black: '#171b22', brightBlack: '#566072',
  red: '#e06c75', brightRed: '#e06c75',
  green: '#41d392', brightGreen: '#41d392',
  yellow: '#e8b45a', brightYellow: '#e8b45a',
  blue: '#6ca8ff', brightBlue: '#6ca8ff',
  magenta: '#c678dd', brightMagenta: '#c678dd',
  cyan: '#4fd6be', brightCyan: '#4fd6be',
  white: '#dce3ec', brightWhite: '#ffffff',
};

/* ---------- render root ---------- */
export function render() {
  $('board-view').style.display = state.view === 'board' ? 'flex' : 'none';
  $('session-view').style.display = state.view === 'session' ? 'flex' : 'none';
  renderTabs();
  renderSidebar();
  if (state.view === 'board') renderBoard();
  else renderSessionView();
  panes.forEach(p => updatePaneChrome(provider.get(p.sid)));
  renderQueueUI();
}

provider.subscribe((ev, s) => {
  if (document.activeElement && document.activeElement.tagName === 'INPUT'
      && document.activeElement.closest('#tabs, .col-head, .card, .side-item, .sess-head')) return;

  if (ev === 'mem') {
    if (state.view === 'session' && s.id === state.sessionId) setMemChip($('sess-mem'), s);
    else setMemChip(document.querySelector(`.card[data-sid="${s.id}"] .mem-chip`), s);
    return;
  }
  if (ev === 'output') {
    if (state.view === 'board' && s.projectId === state.projectId) updateCardInPlace(s);
    return;
  }
  if (ev === 'status') {
    renderTabs();
    renderSidebar();
    updatePaneChrome(s);
    if (state.view === 'session' && s.id === state.sessionId) renderSessionView();
    else if (state.view === 'board' && s.projectId === state.projectId) {
      /* start/stop changes the chip row → rebuild the one card */
      const old = document.querySelector(`.card[data-sid="${s.id}"]`);
      if (old) old.replaceWith(cardEl(s));
    }
    return;
  }
  render();
});
