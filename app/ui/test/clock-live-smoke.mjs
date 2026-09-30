// Real-clock acceptance setup, debug isolated WKWebView only. This driver
// uses the production template provider and automation editor/confirmation.
// It never creates cards, queue rows, inbound events or Agent evidence.
// After arming it returns: all later observation is read-only, out of process.
export async function runClockLiveSmoke() {
  const { $, ctx, inv, store, genId } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { openAutomations, closeAutomations } = await import('../js/automation.js');
  // Setup-only metadata IPC keeps this bounded wait independent of App Nap.
  // No target is armed yet; this neither focuses nor inputs to a terminal.
  const pause = async ms => {
    const end = Date.now() + ms;
    do { await inv('smoke_native_input', { input: { kind: 'state' } }); } while (Date.now() < end);
  };
  const until = async (predicate, budget = 10_000) => {
    const deadline = Date.now() + budget;
    while (!(await predicate())) { if (Date.now() >= deadline) throw new Error('setup timeout'); await pause(50); }
  };
  const report = async (name, ok, count = 1) => inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? count : -1, b: 0 });
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    const snapshot = name => inv('smoke_native_snapshot', { name });
    const home = await inv('default_dir');
    const work = home.slice(0, home.lastIndexOf('/')) + '/work';
    const project = provider.projects()[0];
    const codex = (await inv('smoke_native_scenario').catch(() => '')) === 'codex';
    const command = codex ? 'codex --no-daemon' : 'claude --permission-mode default';
    // This separate setup generation validates REAL native hook attribution.
    // It is closed before any clock rule is configured; it is never a test target.
    const { card: probe } = await provider.createStarted({ id: genId('S'), projectId: project.id,
      columnId: project.columns[0].id, title: 'clock setup hook probe', cmd: command, dir: work, desc: '' });
    await snapshot('clock-setup-created');
    await inv('queue_add', { args: { session: probe.session, cardId: probe.id, dir: work, cmd: command,
      text: codex ? 'Reply with exactly SETUP-DECK-CODEX-HOOK-OK. Do not use tools.' : 'Reply with exactly SETUP-DECK-CLAUDE-HOOK-OK. Do not use tools.',
      mode: 'at', at: Math.floor(Date.now() / 1000) + 3600 } });
    await snapshot('clock-setup-queued');
    await pause(10_000); // Setup-only manual input risk acceptance, never readiness evidence.
    await snapshot('clock-setup-waited');
    const probeQueue = await inv('queue_list');
    const probeRow = probeQueue.items.find(row => row.session === probe.session);
    const context = await inv('queue_probe_context', { id: probeRow.id });
    await snapshot('clock-setup-context');
    if (context.status !== 'ready') throw new Error('setup context unavailable');
    const { confirmDialog } = await import('../js/dialogs.js');
    const accepted = confirmDialog('Isolated setup only: accept first-input risk for the harmless hook probe.');
    $('cfm-yes').click(); if (!(await accepted)) throw new Error('setup declined');
    await inv('queue_send_now', { id: probeRow.id });
    await snapshot('clock-setup-sent');
    await until(async () => {
      const infos = await inv('poll_sessions', { names: [probe.session], tailFor: [], checkpointShells: false });
      const info = infos.find(value => value.name === probe.session);
      return info?.agent === 'turn-done' && (!codex || info.codex_signal === 'trusted');
    }, 180_000);
    if (!(await provider.close(probe.id))) throw new Error('setup generation cleanup failed');
    await report('clock-live-preflight', true);
    const firstDue = new Date(Math.ceil((Date.now() + 150_000) / 60_000) * 60_000);
    const trials = codex ? [{ name: 'E', on: true, count: 1 }] : [
      { name: 'A', on: false, count: 1 }, { name: 'B', on: true, count: 1 },
      { name: 'C1', on: true, count: 3 }, { name: 'C2', on: true, count: 3 },
      { name: 'C3', on: true, count: 3 }, { name: 'D', on: true, count: 3, review: true },
    ];
    for (const [index, trial] of trials.entries()) {
      const due = new Date(firstDue.getTime() + index * 60_000);
      const time = `${String(due.getHours()).padStart(2, '0')}:${String(due.getMinutes()).padStart(2, '0')}`;
      const name = `clock-live-${trial.name}-${due.getTime()}`;
      // Expected markers are not present verbatim in input: assistant must join.
      const steps = Array.from({ length: trial.count }, (_, index) =>
        `Do not use tools. Reply with only the result of joining these three pieces with hyphens: CLOCK${due.getTime()}, ${trial.name}, REPLY${index + 1}.`);
      await provider.saveTemplate(project.id, name, steps);
      await openAutomations({ trigger: 'clock' });
      $('auto-name').value = name; $('auto-template').value = name;
      $('auto-dir').value = work; $('auto-cmd').value = command;
      $('auto-unit').value = 'day'; $('auto-unit').dispatchEvent(new Event('change'));
      $('auto-time').value = time;
      document.querySelector('#auto-finish [data-v="keep"]').click();
      if (trial.review) $('auto-review').click();
      if ($('auto-first-send').closest('label').hidden || $('auto-first-send').checked) throw new Error('clock default/visibility');
      if (trial.on) {
        // Exercise decline, enable, disable and explicit re-enable wiring.
        $('auto-first-send').click();
        if ($('auto-first-send').checked || $('cfm').style.display !== 'flex') throw new Error('missing confirmation');
        $('cfm-no').click(); await pause(50);
        if ($('auto-first-send').checked) throw new Error('declined consent');
        $('auto-first-send').click(); $('cfm-yes').click();
        await until(() => $('auto-first-send').checked);
        $('auto-first-send').click();
        if ($('auto-first-send').checked) throw new Error('disable failed');
        $('auto-first-send').click(); $('cfm-yes').click();
        await until(() => $('auto-first-send').checked);
      }
      $('auto-save').click();
      await until(() => ctx.settings.inbound.rules.some(rule => rule.name === name));
      const saved = ctx.settings.inbound.rules.find(rule => rule.name === name);
      if ((saved.firstSendWithoutReadiness === true) !== trial.on || !!saved.reviewEach !== !!trial.review || saved.autoSend) throw new Error('saved policy');
    }
    closeAutomations();
    if (Date.now() >= firstDue.getTime() - 30_000) throw new Error('insufficient arming margin');
    await report('clock-live-configured', true, trials.length);
    await report('clock-live-armed', true, trials.length);
    await report('done', true);
    // No input, state changes, focus/redraw or rescue beyond this point.
  } catch (_) {
    await report('clock-live-exception', false);
    await report('done', false);
  }
}
