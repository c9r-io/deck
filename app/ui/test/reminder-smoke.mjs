// Isolated packaged WKWebView carrier. Ordinary production UI mutations and
// actual UN inventory only; never fabricates OS responses or notification PASS.
export async function runReminderSmoke() {
  const { $, ctx, inv, store, state } = await import('../js/state.js');
  const { panes, provider, render, editReminder, pollNow, reconcileReminders } = await import('../js/board.js');
  const { mutateBoard } = await import('../js/persistence.js');
  const { localParts, reminderClaim, reminderRequestId } = await import('../js/reminder-model.js');
  const { showSessionCtx } = await import('../js/terminal.js');
  const { openSession, leaveSessionView } = await import('../js/layout.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 12000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) { if (Date.now() >= deadline) throw new Error('bounded reminder condition timeout'); await pause(100); }
  };
  const report = (name, ok, stage = 0) => inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? 1 : -1, b: stage });
  // The webview's own reminder requests, counted where every invoke leaves
  // it: a fetch to the ipc:// protocol (`__TAURI__.core` is frozen). The
  // count only proves silence once it has seen such a request.
  let reminderIpc = 0;
  const systemFetch = window.fetch;
  try {
    window.fetch = function (url, init) {
      const command = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]+)/.exec(String(url?.url || url));
      if (command && /^reminder_(status|actions)$/.test(decodeURIComponent(command[1]))) reminderIpc += 1;
      return systemFetch.call(window, url, init);
    };
  } catch (_) { /* nothing counted: the quiet check below fails */ }
  const click = async el => {
    if (!el) throw new Error('missing UI control');
    const rect = el.getBoundingClientRect();
    for (const kind of ['down', 'up']) await inv('smoke_native_input', { input: { kind, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, viewport: innerHeight } });
  };
  const scenario = await inv("smoke_native_scenario");
  if (scenario.startsWith('native-')) return (await import('./reminder-native-smoke.mjs')).runNativeReminderSmoke();
  let stage = 0;
  let protectedCard;
  try {
    stage = 1;
    await until(() => store.projects.length);
    const project = provider.projects()[0];
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    if (scenario === 'reminder-resume' || scenario === 'reminder-retired') {
      protectedCard = provider.list().find(c => c.title === 'Renamed reminder shell');
      if (!protectedCard) throw new Error('missing durable card after restart');
      if (scenario === 'reminder-retired') {
        for (let i = 0; i < 4; i++) if (!(await pollNow())) throw new Error('failed poll cannot prove retirement suppression');
        await report('reminder-restart-held', !!provider.get(protectedCard.id) && !provider.get(protectedCard.id).reminder && provider.get(protectedCard.id).reminderRetirements.length > 0);
        await openSession(protectedCard.id); await pollNow();
        await inv('pty_write', { name: provider.get(protectedCard.id).session, dataB64: btoa('exit\r') });
        await until(async () => { await pollNow(); return !provider.get(protectedCard.id); });
        await report('reminder-new-lifecycle', !provider.get(protectedCard.id));
        const sentinel = provider.list().find(c => c.title === 'Reminder lifecycle sentinel');
        if (sentinel) await provider.close(sentinel.id, { quiet: true });
        await inv('smoke_reminder_withdraw');
        await report('reminder-cleanup', (await inv('smoke_reminder_inventory')).pending.length === 0);
        await report('done', true); return;
      }
      await report('reminder-restart-saved', !!protectedCard.reminder && protectedCard.reminder.inAppOnly);
    } else {
    await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id, title: 'Reminder lifecycle sentinel', dir: '/tmp', cmd: '' });
    protectedCard = (await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id, title: 'Reminder protected shell', dir: '/tmp', cmd: '' })).card;
    await pollNow(); render();
    const ctxEvent = { preventDefault() {}, stopPropagation() {}, currentTarget: null, clientX: 350, clientY: 200 };
    showSessionCtx(ctxEvent, protectedCard.id);
    await click($('ctx').querySelector('[data-a="reminder"]'));
    await until(() => $('reminder-date'));
    const fill = value => { $('reminder-date').value = value; $('reminder-date').dispatchEvent(new Event('input', { bubbles: true })); };
    fill('2020-01-01T09:00');
    await report('reminder-date-reject', $('reminder-save').disabled);
    fill(localParts(Math.ceil((Date.now() + 20000) / 60000) * 60000, Intl.DateTimeFormat().resolvedOptions().timeZone));
    $('reminder-note').value = 'x'.repeat(281); $('reminder-note').dispatchEvent(new Event('input'));
    await report('reminder-note-reject', $('reminder-save').disabled);
    $('reminder-note').value = 'Harmless private reminder note'; $('reminder-note').dispatchEvent(new Event('input'));
    await click($('reminder-in-app'));
    await report('reminder-preview', $('reminder-preview').textContent.includes(Intl.DateTimeFormat().resolvedOptions().timeZone));
    await click($('reminder-save'));
    await until(() => provider.get(protectedCard.id)?.reminder);
    // a real click on the sidebar's date label edits the reminder and nothing
    // else: the row under it does not open (or start) the session
    const label = document.querySelector('#side-list .side-item .card-reminder');
    let labelOnly = false;
    if (label) {
      await click(label);
      await until(() => $('reminder-date'));
      await pause(300);
      labelOnly = state.view === 'board' && !panes.has(protectedCard.session);
      await click(document.querySelector('.reminder-editor .cfm-actions button'));
      await until(() => !$('reminder-date'), 3000);
    }
    await report('reminder-ui-save', !!label && labelOnly);
    const saved = JSON.parse((await inv('load_board')).data).cards.find(c => c.id === protectedCard.id);
    await report('reminder-durable', saved.reminder.id === provider.get(protectedCard.id).reminder.id && saved.reminder.inAppOnly);
    await provider.rename(protectedCard.id, 'Renamed reminder shell');
    // the rest of the row opens the session, as before: a real click on its name
    await click(document.querySelector(`#side-list .side-item[data-sid="${protectedCard.id}"] .name`));
    await until(() => state.view === 'session' && panes.get(protectedCard.session)?.attached);
    render();
    await click($('reminder-btn'));
    await until(() => $('reminder-date'));
    await report('reminder-header', !!$('reminder-end'));
    await click(document.querySelector('.reminder-editor .cfm-actions button'));
    leaveSessionView(); state.view = 'board'; render();
    await report('reminder-phase-ready', true);
    await pause(100);
    await inv('smoke_native_input', { input: { kind: 'key', keyCode: 12, text: 'q', modifiers: ['command'] } });
    return;
    }
    stage = 2;
    const rejected = await provider.close(protectedCard.id, { detail: true, quiet: true });
    await report('reminder-close-guard', !rejected.ok && rejected.stage === 'reminder-protected' && !!provider.get(protectedCard.id));
    stage = 3;
    const ordinary = (await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id, title: 'Unprotected exit control', dir: '/tmp', cmd: '' })).card;
    await pollNow();
    // A real owning shell exit; no fake poll evidence or retirement flag.
    await openSession(ordinary.id);
    await inv('pty_write', { name: provider.get(ordinary.id).session, dataB64: btoa('exit\r') });
    await until(async () => { await pollNow(); return !provider.get(ordinary.id); });
    stage = 4;
    await openSession(protectedCard.id);
    await pollNow();
    await inv('pty_write', { name: provider.get(protectedCard.id).session, dataB64: btoa('exit\r') });
    leaveSessionView(); state.view = 'board'; render();
    await until(async () => { await pollNow(); return !provider.get(ordinary.id) && provider.get(protectedCard.id)?.reminderRetirements?.length; });
    await report('reminder-real-exit', provider.get(protectedCard.id).status === 'stopped');
    stage = 5;
    await until(async () => { await reconcileReminders(); return provider.get(protectedCard.id)?.reminder?.due === true; }, 90000);
    const dueInventory = await inv('smoke_reminder_inventory');
    await report('reminder-real-due', ctx.attention.matches(provider.get(protectedCard.id), 'reminder') && ctx.attention.counts(provider.list()).pending >= 1 && dueInventory.dockBadge === '1');
    // Arrival observation is complete. Configure the End control only after
    // restoring this owned window; inactive AppKit first clicks may be ignored.
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    await until(async () => (await inv('smoke_native_input', { input: { kind: 'state' } }) & 3) === 3);
    const end = editReminder(protectedCard.id); await until(() => $('reminder-end')); await click($('reminder-end')); await end;
    for (let i = 0; i < 4; i++) if (!(await pollNow())) throw new Error('failed poll cannot prove retirement suppression');
    // No reminder is left on the Board: reconciliation has no work, and the
    // webview stops asking the backend (two periods of its reconcile tick).
    const counted = reminderIpc; await pause(5000);
    await report('reminder-no-backlog', !!provider.get(protectedCard.id) && !provider.get(protectedCard.id).reminder && counted > 0 && reminderIpc === counted);
    stage = 6;
    const inventory = await inv('smoke_reminder_inventory');
    await report('reminder-native-empty', inventory.pending.length === 0 && inventory.delivered.length === 0);
    await inv('smoke_native_snapshot', { name: 'reminder-completed' });
    await reconcileReminders();
    await inv('smoke_reminder_withdraw');
    await report('reminder-phase-ready', true);
    await pause(100);
    await inv('smoke_native_input', { input: { kind: 'key', keyCode: 12, text: 'q', modifiers: ['command'] } });
  } catch (_) {
    await inv('smoke_native_snapshot', { name: 'reminder-failure' }).catch(() => {});
    await report('reminder-exception', false, stage);
    await inv('smoke_reminder_withdraw').catch(() => {});
    await report('done', false);
  }
}
