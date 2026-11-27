// ipc-stall-smoke.mjs — the `ipc-stall` WKWebView smoke (2026-11-27 freeze).
// The debug-only stall faults (smoke_faults.rs `stall-*`) hold the attach
// worker's spawn, a session start, a session kill or a clipboard write until
// the carrier releases them, standing in for a process creation the kernel
// holds; `smoke_tmux_server_signal` stops the smoke's own tmux server. The
// window must keep answering throughout, each wait must end within its
// budget and say so in the pane or a toast, a release must leave nothing
// behind (no extra pane client, no orphan session, no worker or reaper
// still waiting), and no byte typed while waiting may reach any pane.
export async function runIpcStallSmoke() {
  const { inv, state, store } = await import('../js/state.js');
  const { panes, provider, pollNow, activeProject } = await import('../js/board.js');
  const { newSession } = await import('../js/terminal.js');
  const { addSplit, closePaneBySid, backToBoard } = await import('../js/layout.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { TERMINAL_WAITS } = await import('../js/pane-wait-model.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  let failed = false;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name,
      a: ok ? Math.max(1, Math.trunc(a)) : -Math.max(1, Math.abs(Math.trunc(a))), b: Math.trunc(b) });
  };
  const waitFor = async (test, timeout = 8000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) { if (await test()) return true; await pause(50); }
    return false;
  };
  const lines = pane => {
    const buffer = pane.term.buffer.active;
    return Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i).translateToString(true));
  };
  const shows = (pane, text) => !!pane && lines(pane).some(line => line.includes(text));
  const type = (pane, text) => inv('pty_write', { name: pane.session, gen: pane.inputGen, dataB64: strToB64(text) });
  const fault = (kind, count) => inv('smoke_fault_set', { kind, count });
  const attachState = session => inv('smoke_attach_state', { session });
  /* the window answers while something is stalled: the slowest of ten
     round trips of a command that touches nothing */
  const responsive = async () => {
    let worst = 0;
    for (let i = 0; i < 10; i++) {
      const t0 = performance.now();
      await inv('debug_logging_enabled');
      worst = Math.max(worst, performance.now() - t0);
      await pause(30);
    }
    return Math.round(worst);
  };
  const noticeKind = pane => pane?.el.querySelector('.spane-notice')?.dataset.kind || null;
  const project = activeProject();
  const cardsBefore = () => store.cards.length;

  try {
    // A live card B whose pane must keep working while A's attach stalls.
    const b = await newSession('/tmp', { projectId: project.id, rethrow: true });
    const paneB = await waitFor(() => panes.get(b.session)?.attached) ? panes.get(b.session) : null;
    // B's shell answers before anything is stalled (a shell still starting
    // may drop what is typed into it)
    let ready = false;
    for (let i = 0; paneB && !ready && i < 8; i++) {
      await type(paneB, "printf '%s%s\\n' 'B-' 'READY'\r");
      ready = await waitFor(() => shows(paneB, 'B-READY'), 2000);
    }
    await report('stall-setup', ready, 1);
    if (!ready) throw new Error('setup');
    // A: a second live card, opened next to B while its attach is held
    const a = await provider.createStarted({ projectId: project.id, columnId: b.columnId, title: 'stall-a', cmd: '', dir: '/tmp' });
    await fault('stall-attach-spawn', 1);
    const t0 = performance.now();
    const split = addSplit(b.id, 'row', false, a.card.id);
    const paneA = await waitFor(() => panes.get(a.card.session)) ? panes.get(a.card.session) : null;
    const connecting = await waitFor(() => noticeKind(paneA) === 'connecting', TERMINAL_WAITS.connectingNoticeMs + 1500);
    // keys typed into A while it connects: never delivered anywhere
    paneA?.term.input("printf '%s%s\\n' 'LEAK' 'ED-WHILE-CONNECTING'\r");
    // the timeout notice is awaited beside B's check, not after it
    const timing = waitFor(() => noticeKind(paneA) === 'timeout', TERMINAL_WAITS.attachBudgetMs + 2500)
      .then(ok => ({ ok, waited: Math.round(performance.now() - t0) }));
    // B keeps echoing while A's spawn is held: after the split's resize has
    // settled (a line typed into zsh mid-redraw may be lost), at most three
    // tries, each its own marker
    await pause(600);
    let bAlive = false, tries = 0;
    while (!bAlive && tries < 3) {
      tries++;
      await type(paneB, `printf '%s%s\\n' 'B-' 'ALIVE-${tries}'\r`);
      bAlive = await waitFor(() => shows(panes.get(b.session), `B-ALIVE-${tries}`), 1000);
    }
    const worst = await responsive();
    const { ok: timedOut, waited } = await timing;
    await split;
    await report('stall-attach-notice', connecting && timedOut && waited <= TERMINAL_WAITS.attachBudgetMs + 2000,
      waited, (connecting ? 1 : 0) | (timedOut ? 2 : 0));
    // b: the tries B needed (still inside A's held spawn: the budget is 10 s)
    await report('stall-ipc-responsive', bAlive && worst < 500, worst || 1, tries);
    // release the held spawn: that late attach is retired, never shown
    await fault('stall-attach-spawn', 0);
    const settled = await waitFor(async () => {
      const s = await attachState(a.card.session);
      return s.attachWorkers === 0 && s.reaping === 0 && !s.pending && s.paneClients === 0 && !s.attached;
    }, 8000);
    await report('stall-attach-retired', settled && noticeKind(paneA) === 'timeout', 1, settled ? 1 : 0);
    // the pane's retry attaches; A then shows content and takes input
    await pollNow();
    paneA.el.querySelector('.spane-notice button')?.click();
    const retried = await waitFor(() => panes.get(a.card.session)?.attached, 6000);
    if (retried) await type(paneA, "printf '%s%s\\n' 'A-' 'AFTER-RETRY'\r");
    const shown = retried && await waitFor(() => shows(paneA, 'A-AFTER-RETRY'), 4000);
    const one = await attachState(a.card.session);
    await report('stall-attach-retry', shown && one.paneClients === 1, one.paneClients + 1, shown ? 1 : 0);
    await pause(500);
    const leaked = [paneA, paneB].some(pane => shows(pane, 'LEAKED-WHILE-CONNECTING'));
    await report('stall-input-dropped', !leaked, 1, leaked ? 1 : 0);

    // A start that does not answer: no card, a toast, the window answers,
    // and once released the late session is cleaned up
    const sessionsBefore = (await attachState(b.session)).sessions;
    const before = cardsBefore();
    await fault('stall-start', 1);
    const s0 = performance.now();
    const creating = newSession('/tmp', { projectId: project.id, rethrow: true }).then(() => true, () => false);
    await pause(1500);
    const startWorst = await responsive();
    const created = await creating;
    const startWaited = Math.round(performance.now() - s0);
    await report('stall-start-timeout', !created && cardsBefore() === before
      && startWaited <= TERMINAL_WAITS.startBudgetMs + 2000 && startWorst < 500, startWaited, startWorst);
    await fault('stall-start', 0);
    const noOrphan = await waitFor(async () => (await attachState(b.session)).sessions === sessionsBefore
      && (await attachState(b.session)).sessionWorkers === 0, 8000);
    await report('stall-start-reconciled', noOrphan && cardsBefore() === before, 1, (await attachState(b.session)).sessions);

    // A kill that does not answer: the card is kept and may be closed again
    closePaneBySid(a.card.id);
    await fault('stall-kill', 1);
    const k0 = performance.now();
    const firstClose = await provider.close(a.card.id, { quiet: true });
    const killWaited = Math.round(performance.now() - k0);
    const kept = !!provider.get(a.card.id);
    await fault('stall-kill', 0);
    await waitFor(async () => (await attachState(b.session)).sessionWorkers === 0, 8000);
    await pollNow();
    const secondClose = await provider.close(a.card.id, { quiet: true });
    await report('stall-kill-kept', !firstClose && kept && killWaited <= 5000 + 2000 && secondClose
      && !provider.get(a.card.id), killWaited, (kept ? 1 : 0) | (secondClose ? 2 : 0));

    // A clipboard write that does not answer is a reported failure
    await fault('stall-clipboard', 1);
    const c0 = performance.now();
    const copied = await inv('write_clipboard', { text: 'ipc-stall' }).then(() => true, e => String(e));
    const copyWaited = Math.round(performance.now() - c0);
    await fault('stall-clipboard', 0);
    await report('stall-clipboard', copied === 'clipboard-write-failed' && copyWaited <= 3000 + 1500, copyWaited);

    // The smoke's own tmux server stopped: a new card and a close each end
    // within their budgets, the window answers, and once it continues no
    // session without a card is left and no stopped card stays behind
    const sessionsLive = (await attachState(b.session)).sessions;
    const cardsLive = cardsBefore();
    await inv('smoke_tmux_server_signal', { stop: true });
    const n0 = performance.now();
    const made = newSession('/tmp', { projectId: project.id, rethrow: true }).then(() => true, () => false);
    await pause(1500);
    const stoppedWorst = await responsive();
    const madeOk = await made;
    const madeWaited = Math.round(performance.now() - n0);
    const closeStopped = await provider.close(b.id, { quiet: true });
    await inv('smoke_tmux_server_signal', { stop: false });
    await report('stall-server-stopped', !madeOk && madeWaited <= TERMINAL_WAITS.startBudgetMs + 2000
      && stoppedWorst < 500 && !closeStopped && !!provider.get(b.id), madeWaited, stoppedWorst);
    // after the continue: a session the cut start made is killed by the
    // recheck, and the card that could not close closes now
    await pause(1000);
    await pollNow();
    const secondB = await provider.close(b.id, { quiet: true });
    const clean = await waitFor(async () => {
      const s = await attachState('deck-none');
      return s.sessions === sessionsLive - 1 && s.sessionWorkers === 0 && s.reaping === 0;
    }, 20000);
    await report('stall-server-continued', secondB && clean && cardsBefore() === cardsLive - 1,
      1, (await attachState('deck-none')).sessions);
  } catch (error) {
    await report('stall-exception', false, 1);
  } finally {
    for (const kind of ['stall-attach-spawn', 'stall-start', 'stall-kill', 'stall-clipboard']) await fault(kind, 0).catch(() => {});
    await inv('smoke_tmux_server_signal', { stop: false }).catch(() => {});
    if (state.view === 'session') backToBoard({ home: true });
  }
  await report('done', !failed, 1);
}
