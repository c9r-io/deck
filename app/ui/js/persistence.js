// persistence.js — debounced board persistence
// Part of deck's no-build frontend: native ES modules, no bundler.
import { inv, store } from './state.js';
import { toast } from './dialogs.js';

/* ---------- persistence ---------- */
export function saveBoard() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const data = {
      projects: store.projects,
      cards: store.cards.map(c => ({
        id: c.id, projectId: c.projectId, columnId: c.columnId,
        title: c.title, desc: c.desc || '', cmd: c.cmd, dir: c.dir, session: c.session,
      })),
    };
    inv('save_board', { data: JSON.stringify(data, null, 2) })
      .catch(e => toast('save failed: ' + e));
  }, 300);
}
