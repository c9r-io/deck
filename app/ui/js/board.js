// board.js — board CRUD provider, polling loop, sidebar/tabs/board rendering
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, columnHint, dotTitle, POLL_MS, QUIET_SECS, emit, genId, inv, listeners, sessionName, setMemChip, state, store, uev } from './state.js';
import { mutateBoard, mutateBoardDebounced } from './persistence.js';
import { createExitRetirementTracker, sidebarGroups } from './pure.js';
import { confirmDialog, inlineRename, toast } from './dialogs.js';
import { clearSeparators, closePaneBySid, leaveSessionView, openSession, renderSessionView, updatePaneChrome } from './layout.js';
import { SHELL_FG, showProjectCtx, showSessionCtx } from './terminal.js';
import { renderQueueUI, setQueueChip, updateQuietHints } from './scheduler.js';
import { t } from './i18n.js';
import { createDefaultColumns, migrateColumnSemantics } from './board-defaults.js';
export { migrateColumnSemantics } from './board-defaults.js';

export const defaultColumns = () => createDefaultColumns(genId, t);

/* ---------- provider (every persistent mutation is one queued transaction) ---------- */
export const activeProject = () => provider.project(state.projectId);
const exitRetirement = createExitRetirementTracker();
const closeOperations = new Map();

export const provider = {
  projects: () => store.projects,
  project: pid => store.projects.find(p => p.id === pid),
  list: pid => store.cards.filter(c => !pid || c.projectId === pid),
  get: sid => store.cards.find(c => c.id === sid),
  subscribe: fn => { listeners.add(fn); return () => listeners.delete(fn); },

  async createProject(name) {
    const p = {
      id: genId('P'),
      name,
      columns: defaultColumns(),
    };
    await mutateBoard(draft => { draft.projects.push(p); });
    emit('projects', p);
    return p;
  },
  async renameProject(pid, name) {
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      if (!p || p.name === name) return { noop: true };
      p.name = name;
    });
    emit('projects');
  },
  /* Deleting a card or a project means "cancel every future scheduling of
     its session(s), permanently". That cancellation must be PERSISTED
     before anything leaves the board — a card that disappears while its
     recurring rule survives would have the scheduler restart a tmux session
     nobody can see or manage any more. A failure therefore deletes nothing
     and says so. */
  async cancelSchedule(cards, opts = {}) {
    const sessions = (Array.isArray(cards) ? cards : [cards]).map(c => c.session);
    if (!sessions.length) return true;
    try {
      await inv('queue_clear_sessions', { sessions });
      return true;
    } catch (e) {
      if (!opts.quiet) toast(t('error.scheduleCancel'));
      return false;
    }
  },

  async removeProject(pid, opts = {}) {
    try {
      await mutateBoard(async draft => {
        if (!draft.projects.some(p => p.id === pid)) return { noop: true };
        const cards = draft.cards.filter(c => c.projectId === pid);
        if (!(await this.cancelSchedule(cards, { quiet: opts.quiet }))) {
          const error = new Error('schedule cancellation failed');
          error.stage = 'cancel';
          throw error;
        }
        const failed = [];
        for (const card of cards) {
          try {
            await inv('kill_session', { name: card.session });
            const live = this.get(card.id);
            if (live) live.status = 'stopped';
            card.status = 'stopped';
          } catch (error) { failed.push({ card, error }); }
        }
        if (failed.length) {
          const error = new Error('one or more sessions could not be closed');
          error.stage = 'kill';
          error.failed = failed;
          throw error;
        }
        draft.cards = draft.cards.filter(c => c.projectId !== pid);
        draft.projects = draft.projects.filter(p => p.id !== pid);
      });
    } catch (error) {
      if (!opts.quiet && error.stage === 'kill') {
        toast(t('error.projectClose'));
      } else if (!opts.quiet && error.stage !== 'cancel') {
        toast(t('error.projectSave'));
      }
      emit('projects');
      return false;
    }
    emit('projects');
    return true;
  },

  async addColumn(pid, name) {
    const col = { id: genId('C'), name };
    await mutateBoard(draft => { draft.projects.find(p => p.id === pid).columns.push(col); });
    emit('projects');
    return col;
  },
  async renameColumn(pid, cid, name) {
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      const col = p && p.columns.find(c => c.id === cid);
      if (!col || col.name === name) return { noop: true };
      col.name = name;
    });
    emit('projects');
  },
  async removeColumn(pid, cid) {
    let result = false;
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      if (!p || p.columns.length <= 1) return { noop: true };
      if (p.selected === cid) p.selected = null;
      p.columns = p.columns.filter(c => c.id !== cid);
      const fallback = p.columns[0];
      let moved = 0;
      for (const c of draft.cards) {
        if (c.projectId === pid && c.columnId === cid) { c.columnId = fallback.id; moved++; }
      }
      result = { fallback: fallback.name, moved };
    });
    emit('projects');
    return result;
  },

  async create({ projectId, columnId, title, cmd, dir, desc = '' }, opts = {}) {
    const id = genId('S');
    const card = {
      id, projectId, columnId, title, cmd, dir, desc,
      session: sessionName(title, id),
      status: 'stopped', mem: null, tail: [],
    };
    let sideEffect;
    try {
      await mutateBoard(async draft => {
        if (!draft.projects.some(p => p.id === projectId)) throw new Error('project no longer exists');
        if (opts.beforePersist) sideEffect = await opts.beforePersist(card);
        draft.cards.push(card);
      });
    } catch (error) {
      if (opts.rollback) await opts.rollback(card, sideEffect).catch(() => {});
      throw error;
    }
    emit('list', card);
    return card;
  },

  /* stop and delete are one operation; returns false when the card was
     KEPT (its schedule could not be cancelled reliably) */
  async close(sid, opts = {}) {
    const existing = closeOperations.get(sid);
    if (existing) {
      const result = await existing;
      return opts.detail ? { ok: result.ok, applied: false } : result.ok;
    }
    const operation = (async () => {
      let closedCard = null;
      try {
        await mutateBoard(async draft => {
          const card = draft.cards.find(c => c.id === sid);
          if (!card) return { noop: true };
          closedCard = card;
          if (!opts.cancelled && !(await this.cancelSchedule(card, { quiet: opts.quiet }))) {
            const error = new Error('schedule cancellation failed');
            error.stage = 'cancel';
            throw error;
          }
          try {
            await inv('kill_session', { name: card.session });
          } catch (cause) {
            const error = new Error('session could not be closed');
            error.stage = 'kill'; error.cause = cause;
            throw error;
          }
          const live = this.get(sid);
          if (live) live.status = 'stopped';
          card.status = 'stopped';
          draft.cards = draft.cards.filter(c => c.id !== sid);
        });
      } catch (error) {
        const c = this.get(sid);
        if (!opts.quiet && error.stage === 'kill') toast(t('error.sessionClose'));
        if (!opts.quiet && error.stage !== 'cancel' && error.stage !== 'kill') toast(t('error.sessionSave'));
        if (c) emit('status', c);
        return { ok: false, applied: false };
      }
      if (closedCard) {
        emit('list', closedCard);
        return { ok: true, applied: true };
      }
      // Another committed transaction already removed it. This is an
      // idempotent success, but this caller owns no UI success effect.
      return { ok: true, applied: false };
    })();
    closeOperations.set(sid, operation);
    try {
      const result = await operation;
      return opts.detail ? result : result.ok;
    } finally {
      if (closeOperations.get(sid) === operation) closeOperations.delete(sid);
    }
  },

  async move(sid, columnId) {
    await mutateBoard(draft => {
      const c = draft.cards.find(x => x.id === sid);
      if (!c || c.columnId === columnId) return { noop: true };
      c.columnId = columnId;
    });
    const c = this.get(sid);
    if (c) emit('list', c);
  },
  async rename(sid, title) {
    const c = this.get(sid);
    if (!c || !title || title === c.title) return true;
    try {
      await mutateBoard(draft => {
        const card = draft.cards.find(x => x.id === sid);
        if (!card) return { noop: true };
        card.title = title;
      });
      const saved = this.get(sid);
      if (saved) emit('list', saved);
      return true;
    } catch (e) {
      emit('list', c);
      toast(t('error.renameSave'));
      return false;
    }
  },
  async setDesc(sid, desc) {
    await mutateBoard(draft => {
      const c = draft.cards.find(x => x.id === sid);
      if (!c || c.desc === desc) return { noop: true };
      c.desc = desc;
    });
    const c = this.get(sid);
    if (c) emit('list', c);
  },
  observeDir(sid, dir) {
    const live = this.get(sid);
    if (!live || !dir || live.dir === dir) return;
    const previous = live.dir;
    // Update link resolution and chrome immediately. The debounced mutation
    // still enters the same global transaction queue as every board write.
    live.dir = dir;
    emit('path', live);
    mutateBoardDebounced(draft => {
      const card = draft.cards.find(c => c.id === sid);
      if (!card || card.dir === dir) return { noop: true };
      card.dir = dir;
    }, { delay: 500, onCommit: () => {
      const saved = this.get(sid);
      if (saved) emit('path', saved);
    }, onError: () => {
      const current = this.get(sid);
      // A newer cwd observation wins. Otherwise roll back so the next poll
      // sees a mismatch and retries instead of silently claiming durability.
      if (current && current.dir === dir) {
        current.dir = previous;
        emit('path', current);
      }
    } });
  },
  selectColumn(pid, selected) {
    mutateBoardDebounced(draft => {
      const p = draft.projects.find(x => x.id === pid);
      if (!p) return { noop: true };
      p.selected = selected;
    }, {
      onCommit: () => emit('projects'),
      onError: () => toast(t('error.targetSave')),
    });
  },
  async saveTemplate(pid, name, steps) {
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      if (!p) throw new Error('project no longer exists');
      if (!Array.isArray(p.templates)) p.templates = [];
      const existing = p.templates.find(t => t.name === name);
      if (existing) existing.steps = steps; else p.templates.push({ name, steps });
    });
    emit('projects');
  },
  async renameTemplate(pid, oldName, name) {
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      const t = p && (p.templates || []).find(x => x.name === oldName);
      if (!t) return { noop: true };
      t.name = name;
    });
    emit('projects');
  },
  async deleteTemplate(pid, name) {
    await mutateBoard(draft => {
      const p = draft.projects.find(x => x.id === pid);
      if (!p) return { noop: true };
      p.templates = (p.templates || []).filter(t => t.name !== name);
    });
    emit('projects');
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
    infos = await inv('poll_sessions', {
      names, tailFor, checkpointShells: !!settings.sessionRestore,
    });
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
  for (const c of store.cards) {
    const info = byName.get(c.session);
    if (!info) continue;
    /* the shell exited (Ctrl+D etc.) → the card has nothing left to hold;
       close it without ceremony. Only live→dead transitions count, so cards
       that were already stopped (e.g. after an app restart) stay. */
    if (!info.alive && (c.status === 'running' || c.status === 'waiting')) {
      exitRetirement.observe(c.id);
      continue;
    }
    const status = !info.alive ? 'stopped'
      : (info.idle_secs != null && info.idle_secs >= QUIET_SECS) ? 'waiting' : 'running';
    const mem = info.alive && info.mem_mb != null ? info.mem_mb : null;
    const tail = info.tail || [];
    const prevFg = c.fg;
    c.fg = info.fg || null;
    if (info.alive && info.cwd && info.cwd !== c.dir) provider.observeDir(c.id, info.cwd);
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
  /* a shell that exited on its own retires its card through the SAME
     reliable path as an explicit close: cancel the schedule first, and keep
     the card if that cannot be persisted */
  await exitRetirement.drain({
    get: sid => provider.get(sid),
    markStopped: c => { c.status = 'stopped'; emit('status', c); },
    close: c => provider.close(c.id, { quiet: true, detail: true }),
    failed: () => toast(t('error.retire')),
    succeeded: c => {
      closePaneBySid(c.id, { detach: false });
      toast(t('session.closedExited', { name: c.title }));
    },
  });
}
export function startPolling() {
  clearInterval(pollTimer);
  pollTimer = setInterval(pollNow, POLL_MS);
  pollNow();
}
export function stopPolling() {
  clearInterval(pollTimer);
  pollTimer = null;
  exitRetirement.clear();
}

/* ---------- sidebar ---------- */
export function renderSidebar() {
  const list = $('side-list');
  const st = list.scrollTop;
  list.innerHTML = '';
  const all = provider.list(state.projectId);
  $('side-count').textContent = all.length;
  const proj = activeProject();
  if (!all.length) {
    const empty = document.createElement('div');
    empty.className = 'side-empty';
    empty.textContent = t('board.empty');
    list.appendChild(empty);
  }
  for (const group of sidebarGroups(proj, all)) {
    const heading = document.createElement('div');
    heading.className = 'side-board-group';
    heading.innerHTML = '<span class="side-board-name"></span><span class="side-board-count"></span>';
    heading.querySelector('.side-board-name').textContent = group.column.name;
    heading.querySelector('.side-board-count').textContent = group.count;
    heading.title = group.column.name;
    list.appendChild(heading);
    for (const s of group.sessions) {
    const el = document.createElement('div');
    el.className = 'side-item' + (state.view === 'session' && state.sessionId === s.id ? ' active' : '');
    el.dataset.sid = s.id;
    el.title = s.title;
    el.innerHTML = `<span class="dot ${s.status}"></span><span class="name"></span>`;
    el.querySelector('.name').textContent = s.title;
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
      const wait = document.createElement('span');
      wait.className = 'wait-dot';
      wait.title = t('session.quietTab');
      el.appendChild(wait);
    }
    el.onclick = () => switchProject(p.id);
    el.ondblclick = () => renameTab(el, p);
    el.oncontextmenu = e => showProjectCtx(e, p.id);
    bar.appendChild(el);
  }
  const add = document.createElement('button');
  add.className = 'tab-add';
  add.title = t('app.newProject');
  add.textContent = '＋';
  add.onclick = async () => {
    const p = await provider.createProject(t('project.newName'));
    switchProject(p.id);
    const tab = document.querySelector('#tabs .tab.active');
    if (tab) renameTab(tab, p);
  };
  bar.appendChild(add);
}

export function renameTab(el, p) {
  inlineRename(el, p.name, async v => {
    if (v) await provider.renameProject(p.id, v);
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
        <button class="col-del">✕</button>
      </div>
      <div class="col-cards"></div>`;
    /* click anywhere on the board (header or empty space) to make it the
       target for new sessions; cards stop propagation. Toggle classes in
       place — a re-render here would replace the title element between the
       two clicks of a double-click and break rename. */
    colEl.addEventListener('click', e => {
      if (e.target.closest('.card, .col-del') || e.target.tagName === 'INPUT') return;
      const next = p.selected === c.id ? null : c.id;
      provider.selectColumn(p.id, next);
      document.querySelectorAll('#columns .column[data-cid]').forEach(el2 => {
        el2.classList.toggle('selected', el2.dataset.cid === next);
      });
    });
    const nameEl = colEl.querySelector('.col-name');
    nameEl.textContent = c.name;
    const hint = columnHint(c);
    nameEl.title = t('board.targetTitle', { hint: hint ? hint + ' · ' : '' });
    nameEl.ondblclick = () => {
      inlineRename(nameEl, c.name, async v => {
        if (v) await provider.renameColumn(p.id, c.id, v);
        render();
      });
    };
    colEl.querySelector('.col-del').onclick = async e => {
      e.stopPropagation();
      if (p.columns.length <= 1) { toast(t('board.atLeastOne')); return; }
      if (cards.length &&
          !(await confirmDialog(t('board.delete', { name: c.name, count: cards.length, fallback: p.columns.find(x => x.id !== c.id).name })))) return;
      const r = await provider.removeColumn(p.id, c.id);
      if (r && r.moved) toast(t('board.movedCards', { count: r.moved, name: r.fallback }));
    };
    colEl.querySelector('.col-del').title = t('board.deleteTitle');

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
    colEl.addEventListener('drop', async e => {
      e.preventDefault();
      dragDepth = 0;
      colEl.classList.remove('drop-target');
      const sid = e.dataTransfer.getData('text/deck-session');
      if (sid) { await provider.move(sid, c.id); toast(t('board.movedTo', { name: c.name })); }
    });

    const body = colEl.querySelector('.col-cards');
    for (const s of cards) body.appendChild(cardEl(s));
    wrap.appendChild(colEl);
  }

  const ghost = document.createElement('div');
  ghost.className = 'col-ghost';
  ghost.innerHTML = '<button></button>';
  ghost.querySelector('button').textContent = '＋ ' + t('app.newBoard');
  ghost.querySelector('button').onclick = async () => {
    const c = await provider.addColumn(p.id, t('board.newName'));
    render();
    wrap.scrollLeft = wrap.scrollWidth;
    const heads = document.querySelectorAll('#columns .column .col-name');
    const nameEl = heads[heads.length - 1];
    if (nameEl) inlineRename(nameEl, c.name, async v => {
      if (v) await provider.renameColumn(p.id, c.id, v);
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
    <div class="card-top"><span class="dot ${s.status}"></span><span class="card-title"></span><button class="card-x">✕</button></div>
    <div class="card-meta"><span class="cmd"></span><span class="dir"></span><span class="q-chip"></span><span class="mem-chip"></span></div>
    ${s.desc ? '<div class="card-desc"></div>' : ''}
    <div class="card-tail"><div></div><div></div></div>`;
  el.querySelector('.card-title').textContent = s.title;
  el.querySelector('.dot').title = dotTitle(s.status);
  el.querySelector('.card-x').title = t('session.closeTitle');
  el.querySelector('.mem-chip').title = t('session.memory');
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
  if (dot) { dot.className = 'dot ' + s.status; dot.title = dotTitle(s.status); }
  el.classList.toggle('waiting', s.status === 'waiting');
}

export async function closeSession(sid, needConfirm = false) {
  if (!state.destructiveCards) state.destructiveCards = new Set();
  if (state.destructiveCards.has(sid)) return;
  const s = provider.get(sid);
  if (!s) return;
  const live = s.status !== 'stopped';
  if (needConfirm &&
      !(await confirmDialog(t('session.closeConfirm', { name: s.title, live: live ? t('session.closeLive') : '' })))) return;
  state.destructiveCards.add(sid);
  try {
    const result = await provider.close(sid, { detail: true });
    if (!result.ok || !result.applied) return;
    closePaneBySid(sid, { detach: false });
    toast(t('session.closed', { name: s.title }));
    render();
  } finally {
    state.destructiveCards.delete(sid);
  }
}

/* ---------- terminal (xterm + PTY bridge, split panes) ----------
   The session view is a split tree of panes, one live session each.
   `term` and `attachedName` always alias the FOCUSED pane, so the
   completion / ghost / clipboard code keeps its single-terminal world. */
export const panes = new Map();      // session name -> {sid, session, el, body, term, fit}

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
  if (ev === 'path') {
    if (state.view === 'session' && s.id === state.sessionId) renderSessionView();
    else if (state.view === 'board' && s.projectId === state.projectId) {
      const dir = document.querySelector(`.card[data-sid="${s.id}"] .card-meta .dir`);
      if (dir) dir.textContent = s.dir;
    }
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
