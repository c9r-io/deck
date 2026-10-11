// Pi work status in the real window, with a real interactive Pi: the
// `pi-live` WKWebView smoke. Debug carrier only. The switch writes Pi's own
// settings file, and only a stable install may do that, so this mode needs
// the carrier at <home>/Applications/deck.app under a home of its own
// (`scripts/pi-live-verify.py` builds that root and launches it): never the
// home of a person.
//
// Real path end to end: the Settings switch and its confirmation (the real
// `agent_hooks_set` writing the relocated Pi settings), a clock automation
// that starts a real Pi in a real Deck pane, Pi loading Deck's observer from
// this carrier's bundle, the production status socket, `poll_sessions`, the
// card's status text and the Needs attention list, the scheduler's Pi hold on
// that run's first step, the real Send now, and the switch turned off while
// Pi runs. The model is the operator's local fixture provider; what is typed
// into Pi goes through `pty_write`, the path a key press takes.
//
// What the page cannot judge is the operator's: the bytes of Pi's settings
// file at each checkpoint, what reached the pane, and Pi's own event log.
export async function runPiLiveSmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, pollNow } = await import('../js/board.js');
  const { openSettings, persistInbound, selectSettingsSection } = await import('../js/settings.js');
  const { backToBoard, openSession } = await import('../js/layout.js');
  const { refreshQueue } = await import('../js/scheduler.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const HOLD = 2500; // the operator's time to copy Pi's settings file after a checkpoint
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    await pause(HOLD);
    return ok;
  };
  const waitFor = async (predicate, ms, step = 250) => {
    const until = Date.now() + ms;
    for (;;) {
      try { if (await predicate()) return true; } catch (_) { /* not yet */ }
      if (Date.now() >= until) return false;
      await pause(step);
    }
  };
  const status = () => inv('agent_hooks_status');
  const asked = () => $('cfm').style.display === 'flex';
  /* The box the way a click sets it, and the confirmation answered when one
     is asked: returns the question shown ('' when none was). */
  const setSwitch = async on => {
    const box = $('set-pi-hooks');
    box.checked = on;
    box.dispatchEvent(new Event('change'));
    let question = '';
    if (await waitFor(asked, on ? 3000 : 600, 100)) {
      question = $('cfm-msg').textContent;
      $('cfm-yes').click();
    }
    await waitFor(() => !box.disabled && !asked(), 15000, 100);
    await pause(300);
    return question;
  };
  const openAgents = async () => {
    await openSettings(); selectSettingsSection('agents');
    await waitFor(async () => $('set-pi-hooks').checked === ((await status()).pi === true), 8000);
  };
  const info = async session => (await inv('poll_sessions', { names: [session], tailFor: [], checkpointShells: false }))[0];
  const fixture = () => inv('smoke_pi_fixture');
  const type = (session, text) => inv('pty_write', { name: session, dataB64: strToB64(text) });
  let rule = null, project = null, keeper = null, card = null;
  try {
    // 1. the switch: off at first, on through its confirmation
    const fx = await fixture();
    await openAgents();
    const first = await status();
    await report('pi-clean', !!fx.command && fx.settled === 0 && first.pi === false && first.pi_installation === 'absent'
      && !$('set-pi-hooks').checked && !$('set-pi-hooks').disabled);
    const question = await setSwitch(true);
    const on = await status();
    await report('pi-enable', question === t('settings.piSignalEnableConfirm') && on.pi === true
      && on.pi_installation === 'current' && $('set-pi-hooks').checked
      && $('set-pi-note').textContent === t('settings.pi.note.current'));
    $('set-close').click();

    // 2. a clock automation whose run starts a real Pi; its first step is the row under test
    state.view === 'board' || backToBoard();
    project = provider.projects()[0];
    const column = project.columns[0];
    keeper = (await provider.createStarted({ projectId: project.id, columnId: column.id, title: 'keeper', cmd: '', dir: '/tmp' })).card;
    await provider.saveTemplate(project.id, 'pi-smoke', [fx.marker]);
    const clock = new Date();
    const minute = Math.max(0, clock.getHours() * 60 + clock.getMinutes() - 1);
    const id = 'p' + Date.now().toString(36);
    rule = { id, source: 'clock', badge: id, projectId: project.id, columnId: column.id,
      cmd: fx.command, template: 'pi-smoke', dir: fx.dir,
      name: 'Pi smoke', enabled: true, schedule: { unit: 'day', days: [], minute }, finish: 'keep', since: 0 };
    await persistInbound({ ...ctx.settings.inbound, rules: [...ctx.settings.inbound.rules, rule] });
    const saved = ctx.settings.inbound.rules.some(r => r.id === id && r.cmd === rule.cmd);
    const mine = () => store.cards.filter(c => c.origin && c.origin.source === 'clock' && c.origin.badge === id);
    await inv('inbound_check_now');
    const created = saved && await waitFor(() => mine().length === 1, 45_000);
    card = mine()[0];
    if (!await report('pi-run-created', saved && created && !!card)) throw new Error('no run card');

    // 3. Pi loaded the entry: connected, limited, and nothing said about a turn
    const connected = await waitFor(async () => (await info(card.session)).pi_signal === 'connected', 60_000, 500);
    await pollNow();
    const idle = await info(card.session);
    // connected is limited coverage, and coverage is no attention: no card asks for input or has an unread ending
    const calm = ctx.attention.counts(store.cards);
    await report('pi-connected', connected && idle.agent == null && idle.episode == null
      && ctx.attention.get(card)?.agent == null && ctx.attention.get(card)?.piSignal === 'connected'
      && calm.input === 0 && calm.done === 0);

    // 4. the run's first step is Deck's automatic row: held, never typed
    const row = async () => {
      await refreshQueue();
      const item = (ctx.queueCache.items || []).find(i => i.session === card.session);
      const plan = item && (ctx.queueCache.plans || []).find(p => p.item === item.id);
      return { item, plan };
    };
    const held = await waitFor(async () => (await row()).plan?.stage === 'pi-input-unverified', 60_000, 1000);
    // several scheduler passes later it is still there, and Pi was asked nothing
    await pause(12_000);
    const later = await row();
    const quiet = await fixture();
    await report('pi-hold', held && later.plan?.stage === 'pi-input-unverified' && !!later.item
      && (later.item.attempts || 0) === 0 && (later.item.fired || 0) === 0
      && quiet.settled === 0 && quiet.requests === 0);

    // 5. typed by a person: Pi works, the card says so, then that the turn ended
    await openSession(card.id);
    await waitFor(() => state.view === 'session', 8000);
    await pause(1500);
    await type(card.session, 'hello fixture\r');
    const ended = await waitFor(async () => { await pollNow(); return (await info(card.session)).agent === 'turn-done'; }, 30_000, 300);
    const afterTurn = await fixture();
    const done = await info(card.session);
    await report('pi-turn-done', ended && afterTurn.settled === 1 && done.pi_signal === 'connected' && done.episode != null);

    // 6. a turn that ends while the Board is in front is unread attention, counted once
    await type(card.session, '/fx-slow 5000\r');
    const working = await waitFor(async () => (await info(card.session)).agent === 'working', 10_000, 200);
    backToBoard();
    const again = await waitFor(async () => (await fixture()).settled >= 2, 30_000, 300);
    const shown = await waitFor(async () => { await pollNow(); return ctx.attention.get(card)?.agent === 'turn-done'; }, 10_000, 300);
    const counts = ctx.attention.counts(store.cards);
    await report('pi-attention', working && again && shown && counts.done === 1 && counts.pending === 1);

    // 7. the held row, sent by the user's own Send now: Pi receives exactly that text once
    const before = await fixture();
    const target = (await row()).item;
    const probe = await inv('queue_probe_context', { id: target.id });
    if (probe.status !== 'ready') throw new Error('probe');
    await inv('queue_send_now', { id: target.id });
    const sent = await waitFor(async () => (await fixture()).settled >= before.settled + 1, 30_000, 300);
    const afterSend = await row();
    await report('pi-send-now', !!target && sent && (!afterSend.item || (afterSend.item.fired || 0) >= 1));

    // 8. the switch off while Pi runs: the card loses its word at once and gets none back
    await openAgents();
    const quietOff = await setSwitch(false);
    const off = await status();
    $('set-close').click();
    const withdrawn = await waitFor(async () => {
      const now = await info(card.session);
      return now.agent == null && now.pi_signal === 'unavailable';
    }, 5000, 200);
    const settledBefore = (await fixture()).settled;
    await openSession(card.id);
    await pause(1000);
    await type(card.session, 'hello fixture\r');
    const ran = await waitFor(async () => (await fixture()).settled >= settledBefore + 1, 30_000, 300);
    await pause(3000);
    await pollNow();
    const silent = await info(card.session);
    await report('pi-disabled', quietOff === '' && off.pi === false && off.pi_installation === 'absent' && withdrawn && ran
      && silent.agent == null && silent.pi_signal === 'unavailable');

    // 9. on again; the running Pi follows only after its own reload
    await openAgents();
    await setSwitch(true);
    $('set-close').click();
    await pause(3000);
    const stillOut = (await info(card.session)).pi_signal === 'unavailable';
    await type(card.session, '/reload\r');
    const back = await waitFor(async () => (await info(card.session)).pi_signal === 'connected', 60_000, 500);
    const fresh = await info(card.session);
    await report('pi-reload', stillOut && back && fresh.agent == null);
    backToBoard();
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 20, b: 0 });
    failed = true;
  } finally {
    // the run's Pi leaves by its own quit; the rule and the template go with the run
    if (card) await type(card.session, '/quit\r').catch(() => {});
    await pause(1500);
    if (rule) await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) }).catch(() => {});
    if (project) await provider.deleteTemplate(project.id, 'pi-smoke').catch(() => {});
  }
  await report('done', !failed);
}
