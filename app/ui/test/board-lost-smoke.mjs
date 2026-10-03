// Isolated packaged WKWebView carrier for the lost Board's way out
// (documents.rs `lost_exit_at`, dialogs.js `createBoardExit`). The operator
// seeds the root BEFORE launch so that nothing is loadable (SMOKE.md); the
// carrier then follows the one exit the backend offers for that root:
// Cancel first, a refused change offers the exit once more, the exit is
// taken, and the chosen Board must be on disk and usable. `a` on each check
// names the exit: 1 restore, 2 new.
// A restore root whose settings also carry a clock rule that is due at launch
// (SMOKE.md) checks, under the same checkpoints, that inbound triggers wait
// for the user's Board (inbound.js `startInbound`): while the exit is on
// offer the rule's slot stays pending in the backend — not acknowledged, no
// run recorded, no notice — and taking the exit starts its run on the
// restored Board. Its rules are judged against the user's Board only
// (automation.js `startOrphanPruning`): a `projects` event on the placeholder
// drops none of them, and after the exit the same event drops the one whose
// project is not on the restored Board, if the root carries such a rule.
export async function runBoardLostSmoke() {
  const { ctx, inv, state, store } = await import('../js/state.js');
  const { mutateBoard } = await import('../js/persistence.js');
  const { openProjectDefaults } = await import('../js/board.js');
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
  const toasts = () => [...document.getElementById('toasts').children].map(el => el.textContent);
  const ruleIds = () => (ctx.settings.inbound?.rules || []).map(value => value.id).join(',');
  const seeded = ruleIds();
  /* the project defaults dialog confirmed as it was: a Board operation that
     changes nothing and still announces `projects` */
  const confirmDefaults = async () => {
    const dialog = document.getElementById('pdf');
    openProjectDefaults(store.projects[0].id);
    await until(() => dialog.style.display === 'flex');
    document.getElementById('pdf-yes').click();
    await until(() => dialog.style.display !== 'flex', 2000);
    await pause(400);
  };
  let stage = 0;
  try {
    stage = 1;
    const recovery = await inv('board_recovery_state');
    const restorable = !!recovery.kept;
    const exit = restorable ? 1 : 2;
    const cards = restorable ? recovery.kept.cards : 0;
    /* the seeded clock rule, if this restore root carries one, and its run */
    const rule = restorable
      ? (ctx.settings.inbound?.rules || []).find(value => value.source === 'clock' && value.enabled !== false) || null
      : null;
    const run = () => store.cards.find(card => card.origin?.source === 'clock' && card.origin.badge === rule.id);
    const label = t(restorable ? 'board.restoreKept' : 'board.startNew');
    const message = restorable
      ? t('board.lostRestorable', {
        date: formatDateTime(recovery.kept.savedAt, { dateStyle: 'medium', timeStyle: 'short' }), n: cards,
      })
      : t('board.lostNew');
    // boot: one dialog, only the exit the backend names, Cancel focused
    await until(shown);
    let held = true;
    if (rule) {
      // the backend's first poll comes a few seconds after launch; a webview
      // that listened for it would have pulled the slot well within the pause
      await until(async () => (await inv('inbound_pending')).length + (await inv('inbound_runs')).length > 0, 15000);
      await pause(1500);
      const pending = await inv('inbound_pending');
      held = pending.length === 1 && pending[0].rule.id === rule.id && (await inv('inbound_runs')).length === 0
        && !toasts().some(text => text.includes(rule.name || rule.id));
    }
    await report('board-lost-offer', held && recovery.state === 'lost'
      && buttons().map(b => b.textContent).join('|') === [t('common.cancel'), label].join('|')
      && document.getElementById('chd-msg').textContent === message
      && document.activeElement === buttons()[0]
      && store.cards.length === 0, exit, buttons().length + (rule ? 10 : 0));

    stage = 2;
    // Cancel keeps the placeholder; a refused change offers the exit once more
    buttons()[0].click();
    await until(() => !shown(), 2000);
    const placeholder = store.projects[0].name;
    let kept = true;
    if (rule) { await confirmDefaults(); kept = ruleIds() === seeded; }
    let refused = false;
    await mutateBoard(draft => { draft.projects[0].name = 'refused'; }).catch(() => { refused = true; });
    await until(shown);
    await report('board-lost-reoffer', kept && refused && store.projects[0].name === placeholder
      && (await inv('board_recovery_state')).state === 'lost'
      && buttons().length === 2 && buttons()[1].textContent === label, exit);

    stage = 3;
    // the exit: the webview holds the Board the backend committed, and saves it
    buttons()[1].click();
    await until(async () => (await inv('board_recovery_state')).state === 'other');
    await until(() => !shown(), 2000);
    // with a seeded rule, the slot that waited becomes its run on that Board:
    // the card created and saved, the template queued, the slot acknowledged
    const expected = cards + (rule ? 1 : 0);
    if (rule) await until(async () => !!run() && (await inv('inbound_pending')).length === 0, 15000);
    let disk = null;
    await until(async () => {
      try { disk = await inv('load_board'); } catch (_) { return false; }
      return disk.source === 'main' && (!rule || JSON.parse(disk.data).cards.length === expected);
    });
    const saved = JSON.parse(disk.data);
    const started = !rule || (await inv('inbound_runs')).some(value => value.rule === rule.id
      && value.outcome === 'running' && value.card === run().id);
    await report('board-lost-exit', started && store.cards.length === expected && saved.cards.length === expected
      && store.projects.length >= 1 && saved.projects.length === store.projects.length
      && state.projectId === store.projects[0].id
      && document.querySelectorAll('.card').length === store.cards.filter(c => c.projectId === state.projectId).length,
    exit, saved.cards.length);

    stage = 4;
    // an ordinary Board again: a change saves, and nothing is offered
    await mutateBoard(draft => { draft.projects[0].name = 'after exit'; });
    const after = JSON.parse((await inv('load_board')).data);
    await pause(600);
    // the same event on the user's Board leaves only the rules whose project is on it
    let live = true;
    if (rule) {
      await confirmDefaults();
      await until(() => ruleIds() === rule.id, 4000).catch(() => {});
      live = ruleIds() === rule.id;
    }
    await report('board-lost-usable', live && after.projects[0].name === 'after exit' && !shown(), exit);
    await report('done', true, 1, 0);
  } catch (error) {
    await report('board-lost-exception', false, 1, stage);
    await report('done', false, 1, stage);
  }
}
