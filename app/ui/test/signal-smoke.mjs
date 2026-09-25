// Signal Integrity FR-SI-01 regression in the real app, the `signal-finish`
// WKWebView smoke. Real path end to end: a clock automation with
// "close the card" launches examples/signal_fixture.rs in a real Deck tmux
// pane; the fixture reports through the bundle's real deck-status-helper,
// agent_status.rs binds each event to the pane, poll_sessions carries the
// word, and the Board poll runs the finish rule and the run lifecycle.
//
// The P0 assertion: working → background STARTED → turn-done while the
// background command has not COMPLETED must NOT retire the run (the defect
// closed it ~7 s after the first turn-done and killed the work). The run
// is retired only after the work completed, the agent resumed and ended
// its turn, and the fixture program exited. The carrier forces polls
// (`pollNow`), so a regression fails within about a second — deterministic
// regression timing, not production timing; real-agent timing belongs to
// the candidate step in SMOKE.md. No model, no network; the
// fixture refuses to run outside this isolated data dir and deck-smoke
// socket.
export async function runSignalFinishSmoke() {
  const { ctx, inv, state, store } = await import('../js/state.js');
  const { provider, pollNow } = await import('../js/board.js');
  const { persistInbound } = await import('../js/settings.js');
  const { backToBoard } = await import('../js/layout.js');
  const nativeInvoke = window.__TAURI__.core.invoke;
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  const waitFor = async (predicate, ms, step = 250) => {
    const until = Date.now() + ms;
    while (Date.now() < until) {
      if (await predicate()) return true;
      await pause(step);
    }
    return !!(await predicate());
  };
  const fixture = () => inv('smoke_signal_fixture');
  let rule = null, project = null, keeper = null;
  try {
    const fx = await fixture();
    await report('signal-fixture', !!fx.fixture && !!fx.helper && !fx.started && !fx.completed && !fx.fixture_alive);
    state.view === 'board' || backToBoard();
    project = provider.projects()[0];
    const column = project.columns[0];
    /* an ordinary empty shell keeps the tmux listing non-empty, so the
       final poll can prove the run's session is gone */
    keeper = (await provider.createStarted({ projectId: project.id, columnId: column.id, title: 'keeper', cmd: '', dir: '/tmp' })).card;
    await provider.saveTemplate(project.id, 'signal-smoke', ['signal-smoke-step']);
    const clock = new Date();
    const minute = Math.max(0, clock.getHours() * 60 + clock.getMinutes() - 1);
    const id = 's' + Date.now().toString(36);
    rule = { id, source: 'clock', badge: id, projectId: project.id, columnId: column.id,
      cmd: `'${fx.fixture}'`, template: 'signal-smoke', dir: '',
      name: 'Signal smoke', enabled: true, schedule: { unit: 'day', days: [], minute }, finish: 'close', since: 0 };
    await persistInbound({ ...ctx.settings.inbound, rules: [...ctx.settings.inbound.rules, rule] });
    const saved = ctx.settings.inbound.rules.some(r => r.id === id && r.finish === 'close' && r.cmd === rule.cmd);
    const mine = () => store.cards.filter(c => c.origin && c.origin.source === 'clock' && c.origin.badge === id);
    await inv('inbound_check_now');
    const created = await waitFor(() => mine().length === 1, 45_000);
    const card = mine()[0];
    await report('signal-run-created', saved && created && !!card);
    if (!card) throw new Error('no run card');

    /* the prompt is delivered to the fixture, which reports working,
       starts its background command and ends its turn */
    const started = await waitFor(async () => (await fixture()).started, 90_000, 500);
    const ended = await waitFor(async () => { await pollNow(); return ctx.attention.get(card)?.agent === 'turn-done'; }, 10_000);
    const early = await fixture();
    await report('signal-turn-done', started && ended && !early.completed && early.fixture_alive && early.child_alive);

    /* the P0 window: many more polls than the finish rule's three-poll
       confirmation, every one reading turn-done with the queue drained */
    let polls = 0, retained = true, doneRead = 0, stillRunning = true;
    for (let i = 0; i < 10; i++) {
      await pollNow();
      polls++;
      if (ctx.attention.get(card)?.agent === 'turn-done') doneRead++;
      retained = retained && !!provider.get(card.id);
      const now = await fixture();
      stillRunning = stillRunning && !now.completed && now.child_alive;
      await pause(400);
    }
    const drained = !(ctx.queueCache.items || []).some(i => i.session === card.session);
    await report('signal-held', retained && stillRunning && drained && doneRead >= 4, doneRead, polls);

    /* the background command completes with the card still there */
    const completed = await waitFor(async () => (await fixture()).completed, 20_000, 300);
    await report('signal-completed', completed && !!provider.get(card.id));

    /* the fixture resumes, ends its turn and exits: only now, with the
       program gone from the foreground, does the existing close path run */
    const closed = await waitFor(async () => { await pollNow(); return !provider.get(card.id); }, 30_000, 500);
    const runClosed = await waitFor(async () => (await inv('inbound_runs')).some(r => r.rule === id && r.card === card.id && r.outcome === 'closed'), 5000);
    await report('signal-closed', closed && runClosed);

    /* nothing of the fixture survives: both processes and the session */
    const gone = await waitFor(async () => { const now = await fixture(); return !now.fixture_alive && !now.child_alive; }, 5000);
    const infos = await inv('poll_sessions', { names: [card.session], tailFor: [], checkpointShells: false });
    await report('signal-cleanup', gone && infos.length === 1 && infos[0].alive === false);
  } catch (_) {
    await report('signal-exception', false);
  } finally {
    if (rule) await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) }).catch(() => {});
    if (project) await provider.deleteTemplate(project.id, 'signal-smoke').catch(() => {});
    if (keeper) await provider.close(keeper.id, { quiet: true }).catch(() => {});
  }
  await report('done', !failed);
}
