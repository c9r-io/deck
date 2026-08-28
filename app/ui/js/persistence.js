// persistence.js — debounced board persistence
// Part of deck's no-build frontend: native ES modules, no bundler.
import { inv, store } from './state.js';
import { toast } from './dialogs.js';

/* ---------- persistence ---------- */
let boardSaveChain = Promise.resolve();

export function boardData(projects = store.projects, cards = store.cards) {
  return {
    projects,
    cards: cards.map(c => ({
      id: c.id, projectId: c.projectId, columnId: c.columnId,
      title: c.title, desc: c.desc || '', cmd: c.cmd, dir: c.dir, session: c.session,
    })),
  };
}

function enqueueBoardSave(data) {
  const write = () => inv('save_board', { data: JSON.stringify(data, null, 2) });
  const pending = boardSaveChain.catch(() => {}).then(write);
  boardSaveChain = pending;
  return pending;
}

/** Immediate, awaitable durability barrier used before destructive UI commits. */
export function saveBoardNow(projects = store.projects, cards = store.cards) {
  clearTimeout(saveTimer);
  return enqueueBoardSave(boardData(projects, cards));
}

export function saveBoard() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    enqueueBoardSave(boardData()).catch(e => toast('save failed: ' + e));
  }, 300);
}
