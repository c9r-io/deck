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
// that run's first step, the real Send now, the switch turned off while Pi
// runs, and the second switch (session observation): on, read in the real
// Settings dialog, on the card and in its "Why?" dialog, and in the real
// shell-service restart review (opened and read, never confirmed); off again
// while Pi runs, after which the SAME Pi's next run is still reported; and a
// Pi whose session id Deck does not keep, which the card says in so many
// words. The model is the operator's local fixture provider; what is typed
// into Pi or into the pane's shell goes through `pty_write`, the path a key
// press takes. Snapshots of the window land in <data dir>/evidence/.
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
  const { getLocale, setLocale, t } = await import('../js/i18n.js');
  let failed = false;
  /* This page opens the real restart review to read it. It never confirms it, and for the rest
     of this page's life the one command that would replace the shell service is refused here. */
  let restartsAsked = 0;
  const realTauri = window.__TAURI__;
  const realInvoke = realTauri.core.invoke;
  window.__TAURI__ = { ...realTauri, core: { ...realTauri.core, invoke: (command, args) => {
    if (command !== 'restart_tmux_server') return realInvoke(command, args);
    restartsAsked += 1;
    return Promise.reject(new Error('refused by the Pi smoke'));
  } } };
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
  const setSwitch = async (on, id = 'set-pi-hooks') => {
    const box = $(id);
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

    // 9. on again; the running Pi follows only after its own reload, and Settings says so until then
    await openAgents();
    await setSwitch(true);
    const again9 = await status();
    const waits = $('set-pi-note').textContent === t('settings.pi.note.needsReload');
    $('set-close').click();
    await pause(3000);
    const stillOut = (await info(card.session)).pi_signal === 'unavailable';
    await type(card.session, '/reload\r');
    const back = await waitFor(async () => (await info(card.session)).pi_signal === 'connected', 60_000, 500);
    const fresh = await info(card.session);
    const settled9 = await status();
    await report('pi-reload', again9.pi_installation === 'needs-reload' && waits && stillOut && back && fresh.agent == null
      && settled9.pi_installation === 'current');

    // 10. session observation: a second switch on the same entry. The running Pi keeps
    //     its earlier setting until its own reload; then the card may say that a session
    //     is observed, as one closed word and nothing of the session itself
    await openAgents();
    const asked2 = await setSwitch(true, 'set-pi-lifecycle');
    const both = await status();
    const waiting = $('set-pi-note').textContent === t('settings.pi.note.needsReload');
    $('set-close').click();
    const notYet = (await info(card.session)).pi_session == null;
    await type(card.session, '/reload\r');
    const observed = await waitFor(async () => (await info(card.session)).pi_session === 'observed', 60_000, 500);
    await pollNow();
    const seen = await info(card.session);
    const sessionShown = ctx.attention.get(card)?.piSession === 'observed';
    // the payload holds the closed word and no value that could be an id of the session
    // (the session's own name, its directory, the two process names and the pane's opaque identity are Deck's, not Pi's)
    const strings = Object.entries(seen).filter(([key, value]) => typeof value === 'string' && !['name', 'cwd', 'fg', 'finish_fg', 'lifecycle'].includes(key));
    const closed = strings.every(([, value]) => ['observed', 'connected', 'working', 'turn-done'].includes(value));
    await report('pi-session', asked2 === t('settings.piLifecycleEnableConfirm') && both.pi === true && both.pi_lifecycle === true
      && both.pi_installation === 'needs-reload' && waiting && notYet && observed && sessionShown && seen.pi_signal === 'connected' && closed
      && (await status()).pi_installation === 'current');

    // 11. what the user SEES of all that, in the real window with the real backend's answers:
    //     the two switches and their note, the card's status and its explanation
    const shots = { asked: 0, written: 0 };
    const snapshot = async name => {
      shots.asked += 1;
      await pause(150);
      if (await inv('smoke_native_snapshot', { name }).catch(() => -9) === 0) shots.written += 1;
    };
    const rect = node => node.getBoundingClientRect();
    const visibleNode = node => !!node && node.getClientRects().length > 0 && rect(node).height > 0;
    const view = () => ({ left: 0, top: 0, right: innerWidth, bottom: innerHeight });
    const inside = (inner, outer) => inner.left >= outer.left - 1 && inner.right <= outer.right + 1
      && inner.top >= outer.top - 1 && inner.bottom <= outer.bottom + 1;
    const reachable = node => {
      const r = rect(node);
      const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return !!top && (top === node || node.contains(top) || (top.tagName === 'LABEL' && top.contains(node)));
    };
    const languages = async body => {
      const parts = [];
      for (const language of ['en', 'zh-Hans']) {
        // a language that is set already is not set again: that would repaint the static texts only
        if (getLocale() !== language) setLocale(language);
        await pause(200);
        parts.push(await body(language === 'en' ? 'en' : 'zh'));
      }
      if (getLocale() !== 'en') setLocale('en');
      await pause(200);
      return parts.every(Boolean);
    };
    await openAgents();
    const settingsShown = await languages(async language => {
      $('set-pi-hooks').scrollIntoView({ block: 'center' });
      await pause(80);
      const ok = [$('set-pi-hooks'), $('set-pi-lifecycle')].every(box => box.checked && visibleNode(box) && reachable(box))
        && !$('set-pi-note').hidden && $('set-pi-note').textContent === t('settings.pi.note.current')
        && $('set-content').scrollWidth <= $('set-content').clientWidth + 1 && inside(rect($('settings-box')), view());
      await snapshot(`pilive-settings-${language}`);
      return ok;
    });
    $('set-close').click();
    backToBoard();
    await pollNow();
    const cardNode = () => document.querySelector(`.card[data-sid="${card.id}"]`);
    const explained = async (keys, name) => languages(async language => {
      await pollNow();
      const help = cardNode().querySelector('.card-signal-help');
      const visible = visibleNode(help) && help.textContent === t('signal.details') && reachable(help);
      await snapshot(`pilive-board-${name}-${language}`);
      help.click();
      const opened = await waitFor(() => $('chd').style.display === 'flex', 3000, 50);
      await pause(100);
      const box = rect($('chd-box'));
      const buttons = [...$('chd-actions').querySelectorAll('button')];
      const ok = visible && opened && $('chd-msg').textContent === keys.map(key => t(key)).join('\n\n')
        && inside(box, view()) && $('chd-msg').scrollWidth <= $('chd-msg').clientWidth + 1
        && buttons.length === 2 && buttons.every(button => inside(rect(button), box) && reachable(button));
      await snapshot(`pilive-why-${name}-${language}`);
      buttons[0]?.click();
      await waitFor(() => $('chd').style.display !== 'flex', 3000, 50);
      return ok;
    });
    const whyObserved = await explained(['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.session'], 'observed');
    await report('pi-shown', settingsShown && whyObserved && (await info(card.session)).pi_signal === 'connected', 1, shots.written);

    // 12. the restart review, asked of the real backend while this Pi is connected and its
    //     session observed: the card is named as Pi, told Pi's own reason, and cannot be chosen.
    //     The real dialog is opened and read; nothing in it is confirmed.
    const { reentryPlan, planView, reentryRequest, withAllSelected } = await import('../js/reentry-model.js');
    const preview = await inv('agent_reentry_preview');
    const listed = (preview.entries || []).find(entry => entry.cardId === card.id);
    const all = withAllSelected(reentryPlan(preview), true);
    const reviewRow = planView(all).rows.find(each => each.cardId === card.id);
    const modelSays = !!listed && listed.kind === 'unsupported' && listed.reason === 'client-identity-unverifiable'
      && listed.source === 'pi' && listed.launchProfile == null && !!reviewRow && reviewRow.selectable === false
      && reviewRow.checked === false && reentryRequest(all) === null
      && t(reviewRow.note.key) === t('reentry.reason.client-identity-unverifiable');
    const section = $('tmux-reentry');
    // opened once per language: the review's opening paragraph is written when it opens
    const reviewShown = await languages(async language => {
      $('set-tmux-restart').click();
      if (!await waitFor(() => $('tmux-lifecycle-modal').style.display === 'flex' && !section.hidden, 15_000, 100)) return false;
      await pause(150);
      const rows = [...section.querySelectorAll('.reentry-list > *')];
      const mine = rows.find(row => row.querySelector('.reentry-choice-text')?.textContent === card.title);
      const other = rows.find(row => row.querySelector('.reentry-choice-text')?.textContent === keeper.title);
      const frame = rect($('tmux-lifecycle-box'));
      mine?.scrollIntoView({ block: 'center' });
      await pause(80);
      const ok = !!mine && mine.classList.contains('reentry-off') && !mine.querySelector('input, button')
        && mine.querySelector('.reentry-note').textContent === t('reentry.reason.client-identity-unverifiable')
        && !!other && other.querySelector('.reentry-note').textContent === t('reentry.kind.shell')
        // no candidate exists here: nothing can be selected, and the one action is the plain restart
        && section.querySelectorAll('input').length === 0 && $('tmux-restart').textContent === t('tmux.restart')
        && inside(frame, view()) && section.scrollWidth <= section.clientWidth + 1
        && rows.every(row => rect(row).left >= rect(section).left - 1 && rect(row).right <= rect(section).right + 1)
        && [$('tmux-later'), $('tmux-restart')].every(button => visibleNode(button) && inside(rect(button), frame) && reachable(button));
      await snapshot(`pilive-review-${language}`);
      $('tmux-done').click();
      await waitFor(() => $('tmux-lifecycle-modal').style.display !== 'flex', 3000, 50);
      return ok;
    });
    await report('pi-review', modelSays && reviewShown && restartsAsked === 0
      && (await info(card.session)).pi_signal === 'connected', 1, restartsAsked);

    // 13. session observation off again while this Pi runs: its stream loses that and nothing
    //     else. Without any reload, the next run is reported like the ones before it.
    await openAgents();
    const quiet2 = await setSwitch(false, 'set-pi-lifecycle');
    const after = await status();
    $('set-close').click();
    const withdrawn2 = await waitFor(async () => (await info(card.session)).pi_session == null, 5000, 200);
    const stays = (await info(card.session)).pi_signal === 'connected';
    const settledNarrow = (await fixture()).settled;
    await openSession(card.id);
    await pause(1000);
    await type(card.session, '/fx-slow 3000\r');
    const worksOn = await waitFor(async () => (await info(card.session)).agent === 'working', 10_000, 200);
    const endsOn = await waitFor(async () => { await pollNow(); return (await info(card.session)).agent === 'turn-done'; }, 30_000, 300);
    const narrowed = await info(card.session);
    backToBoard();
    await report('pi-narrowed', quiet2 === '' && after.pi === true && after.pi_lifecycle === false
      && after.pi_installation === 'current' && withdrawn2 && stays && worksOn && endsOn
      && (await fixture()).settled === settledNarrow + 1 && narrowed.pi_signal === 'connected' && narrowed.pi_session == null
      && narrowed.episode != null);

    // 14. a session whose id Deck does not keep: said on the card, never passed over, and the
    //     same Pi's runs are reported as ever. (A new Pi, started by a person in the same pane.)
    await openAgents();
    await setSwitch(true, 'set-pi-lifecycle');
    $('set-close').click();
    await openSession(card.id);
    await pause(800);
    await type(card.session, '/quit\r');
    const shellBack = await waitFor(async () => { const now = await info(card.session); return now.fg === 'zsh' && now.pi_signal == null; }, 30_000, 300);
    const settledLong = (await fixture()).settled;
    await type(card.session, `${fx.command} --no-session --session-id ${'k'.repeat(300)}\r`);
    const said = await waitFor(async () => (await info(card.session)).pi_session === 'unsupported', 60_000, 500);
    const long = await info(card.session);
    await type(card.session, 'hello fixture\r');
    const ranLong = await waitFor(async () => { await pollNow(); return (await info(card.session)).agent === 'turn-done'; }, 30_000, 300);
    const afterLong = await info(card.session);
    backToBoard();
    await pollNow();
    const whyUnsupported = await explained(['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.sessionUnsupported'], 'unsupported');
    const noId = !JSON.stringify(afterLong).includes('kkkkkkkk') && !document.body.textContent.includes('kkkkkkkk');
    await report('pi-unsupported', shellBack && said && long.pi_signal === 'connected' && ranLong
      && afterLong.pi_session === 'unsupported' && afterLong.pi_signal === 'connected'
      && (await fixture()).settled === settledLong + 1 && whyUnsupported && noId
      && shots.written === shots.asked && shots.asked >= 12, 1, shots.written);
    await openAgents();
    await setSwitch(false, 'set-pi-lifecycle');
    $('set-close').click();
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
