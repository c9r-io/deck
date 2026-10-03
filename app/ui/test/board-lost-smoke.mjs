// Isolated packaged WKWebView carrier for the lost Board's way out
// (documents.rs `lost_exit_at`, dialogs.js `createBoardExit`). The operator
// seeds the root BEFORE launch so that nothing is loadable (SMOKE.md); the
// carrier then follows the one exit the backend offers for that root:
// Cancel first, a refused change offers the exit once more, the exit is
// taken, and the chosen Board must be on disk and usable. `a` on each check
// names the exit: 1 restore, 2 new.
export async function runBoardLostSmoke() {
  const { inv, state, store } = await import('../js/state.js');
  const { mutateBoard } = await import('../js/persistence.js');
  const { formatDateTime, t } = await import('../js/i18n.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) throw new Error('bounded board-lost condition timeout');
      await pause(50);
    }
  };
  const report = (name, ok, a = 1, b = 0) => inv('ui_event', {
    code: 'smoke-check', detail: name, a: ok ? a : -a, b,
  });
  const shown = () => document.getElementById('chd').style.display === 'flex';
  const buttons = () => [...document.getElementById('chd-actions').children];
  let stage = 0;
  try {
    stage = 1;
    const recovery = await inv('board_recovery_state');
    const restorable = !!recovery.kept;
    const exit = restorable ? 1 : 2;
    const cards = restorable ? recovery.kept.cards : 0;
    const label = t(restorable ? 'board.restoreKept' : 'board.startNew');
    const message = restorable
      ? t('board.lostRestorable', {
        date: formatDateTime(recovery.kept.savedAt, { dateStyle: 'medium', timeStyle: 'short' }), n: cards,
      })
      : t('board.lostNew');
    // boot: one dialog, only the exit the backend names, Cancel focused
    await until(shown);
    await report('board-lost-offer', recovery.state === 'lost'
      && buttons().map(b => b.textContent).join('|') === [t('common.cancel'), label].join('|')
      && document.getElementById('chd-msg').textContent === message
      && document.activeElement === buttons()[0]
      && store.cards.length === 0, exit, buttons().length);

    stage = 2;
    // Cancel keeps the placeholder; a refused change offers the exit once more
    buttons()[0].click();
    await until(() => !shown(), 2000);
    const placeholder = store.projects[0].name;
    let refused = false;
    await mutateBoard(draft => { draft.projects[0].name = 'refused'; }).catch(() => { refused = true; });
    await until(shown);
    await report('board-lost-reoffer', refused && store.projects[0].name === placeholder
      && (await inv('board_recovery_state')).state === 'lost'
      && buttons().length === 2 && buttons()[1].textContent === label, exit);

    stage = 3;
    // the exit: the webview holds the Board the backend committed, and saves it
    buttons()[1].click();
    await until(async () => (await inv('board_recovery_state')).state === 'other');
    await until(() => !shown(), 2000);
    let disk = null;
    await until(async () => {
      try { disk = await inv('load_board'); } catch (_) { return false; }
      return disk.source === 'main';
    });
    const saved = JSON.parse(disk.data);
    await report('board-lost-exit', store.cards.length === cards && saved.cards.length === cards
      && store.projects.length >= 1 && saved.projects.length === store.projects.length
      && state.projectId === store.projects[0].id
      && document.querySelectorAll('.card').length === store.cards.filter(c => c.projectId === state.projectId).length,
    exit, saved.cards.length);

    stage = 4;
    // an ordinary Board again: a change saves, and nothing is offered
    await mutateBoard(draft => { draft.projects[0].name = 'after exit'; });
    const after = JSON.parse((await inv('load_board')).data);
    await pause(600);
    await report('board-lost-usable', after.projects[0].name === 'after exit' && !shown(), exit);
    await report('done', true, 1, 0);
  } catch (error) {
    await report('board-lost-exception', false, 1, stage);
    await report('done', false, 1, stage);
  }
}
