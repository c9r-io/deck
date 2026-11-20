// pi-worker-smoke.mjs — a Pi worker card end to end, the `pi-worker`
// WKWebView smoke (approved 2026-11-20, `agent_pi/handover.rs`). Debug
// carrier only, run by `scripts/pi-live-verify.py --mode pi-worker` under a
// home of its own with a real interactive Pi on the local fixture model.
//
// The page does what the person does: Pi's work status on through its
// Settings switch, Remote access on and ONE client authorized to drive
// worker cards of one type, the operator's Pi command (`piw-armed`). The
// operator is then the orchestrator: through this carrier's own adapter it
// starts the worker, waits for its turns and sends two more lines — one
// while an extension dialog is open in Pi, one while the user's draft sits
// in Pi's editor — and judges Pi's own event log and the pane. The page
// sees the card appear (`piw-started`) and its three lines go out through
// the observer, never typed (`piw-delivered`), with no uncertain delivery
// (`piw-certain`). Snapshots of the window land in <data dir>/evidence/.
export async function runPiWorkerSmoke() {
  const { $, inv, state, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { backToBoard, openSession } = await import('../js/layout.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    return ok;
  };
  const waitFor = async (predicate, ms, step = 500) => {
    const until = Date.now() + ms;
    for (;;) {
      try { if (await predicate()) return true; } catch (_) { /* not yet */ }
      if (Date.now() >= until) return false;
      await pause(step);
    }
  };
  const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
  const status = () => inv('agent_hooks_status');
  try {
    // 1. Pi's work status, through the switch and its confirmation
    const fx = await inv('smoke_pi_fixture');
    await openSettings(); selectSettingsSection('agents');
    // the section renders the switch from the backend first; a click before that is painted over
    await waitFor(async () => $('set-pi-hooks').checked === ((await status()).pi === true) && !$('set-pi-hooks').disabled, 8000, 100);
    await pause(300);
    const box = $('set-pi-hooks');
    box.checked = true;
    box.dispatchEvent(new Event('change'));
    if (await waitFor(() => $('cfm').style.display === 'flex', 3000, 100)) $('cfm-yes').click();
    await waitFor(async () => (await status()).pi === true && !box.disabled && $('cfm').style.display !== 'flex', 15000, 200);
    await pause(300);
    await report('piw-enable', (await status()).pi === true && $('set-pi-note').textContent === t('settings.pi.note.current'));
    $('set-close').click();

    // 2. one client that drives worker cards of one type: the operator's Pi command
    state.view === 'board' || backToBoard();
    const project = provider.projects()[0];
    await inv('mcp_enable');
    const client = await inv('mcp_client_add', {
      name: 'orchestrator', projects: [{ projectId: project.id, roots: [fx.dir] }],
      permissions: { allowCreate: false, allowAutomations: false, allowWorkers: true, allowReplies: true,
        workerFirstSend: false, allowDrafts: false, allowMentions: false, ruleColumn: false, allProjects: false,
        workerTypes: [{ name: 'pi', cmd: fx.command, description: 'Pi on the local fixture model' }] },
    });
    await report('piw-armed', !!client?.id && client.workerTypes?.[0]?.cmd === fx.command);

    // 3. the operator starts the worker: a card of that type appears
    const worker = () => store.cards.find(card => card.origin?.source === 'worker' && card.origin.badge === 'pi');
    const started = await waitFor(() => !!worker(), 180_000);
    if (started) {
      openSession(worker().id);
      await pause(1500);
      await snapshot('piw-started');
    }
    await report('piw-started', started);

    // 4. its three lines go out through the observer, one per turn; none uncertain
    const lines = async () => {
      const q = await inv('queue_list');
      const session = worker()?.session;
      const mine = (q.deliveries || []).filter(d => d.session === session && d.authority?.class === 'delegated');
      const open = (q.items || []).filter(item => item.session === session);
      return { delivered: mine.length, uncertain: open.filter(item => item.state === 'ambiguous').length,
        stages: open.map(item => (q.plans || []).find(plan => plan.item === item.id)?.stage).filter(Boolean) };
    };
    const seenStages = new Set();
    const delivered = await waitFor(async () => {
      const now = await lines();
      now.stages.forEach(stage => seenStages.add(stage));
      return now.delivered >= 3 || now.uncertain > 0;
    }, 420_000, 1000);
    const end = await lines();
    await snapshot('piw-delivered');
    await report('piw-delivered', delivered && end.delivered === 3, Math.max(1, end.delivered), seenStages.has('pi-handover') ? 1 : 0);
    await report('piw-certain', end.uncertain === 0 && !seenStages.has('pi-input-unverified'), 1, end.uncertain);
  } catch (error) {
    failed = true;
    await snapshot('piw-exception');
    await report('piw-exception', false, 1, String(error?.message || error).length);
  }
  await report('done', !failed);
}
