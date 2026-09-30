// A bounded own-window driver. The outer verifier performs actual system UI
// actions; this seam never calls a notification response handler.
export async function runNativeReminderSmoke() {
  const { $, ctx, inv, store, state } = await import('../js/state.js');
  const { provider, render, editReminder, pollNow, reconcileReminders } = await import('../js/board.js');
  const { openSession, leaveSessionView } = await import('../js/layout.js');
  const { localParts, reminderRequestId } = await import('../js/reminder-model.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 12000) => {
    const end = Date.now() + budget;
    while (!(await check())) { if (Date.now() >= end) throw new Error('native smoke deadline'); await pause(100); }
  };
  const report = (ok, stage) => inv('ui_event', { code: 'smoke-check', detail: 'reminder-native-stage', a: ok ? 1 : -1, b: stage });
  const click = async el => {
    if (!el) throw new Error('missing native smoke control');
    const rect = el.getBoundingClientRect();
    for (const kind of ['down', 'up']) await inv('smoke_native_input', { input: { kind, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, viewport: innerHeight } });
  };
  const editor = async (card, end = false, inAppOnly = false, ownDOM = false) => {
    const input = ownDOM ? async el => { if (!el) throw new Error("missing product control"); el.click(); } : click;
    const operation = editReminder(card.id); await until(() => $('reminder-date'));
    if (end) await input($('reminder-end'));
    else {
      // Legal minute-granularity UI input, with time to observe registration.
      $('reminder-date').value = localParts(Math.ceil((Date.now() + 30000) / 60000) * 60000, Intl.DateTimeFormat().resolvedOptions().timeZone);
      $('reminder-date').dispatchEvent(new Event('input', { bubbles: true }));
      if ($('reminder-in-app').checked !== inAppOnly) await input($('reminder-in-app'));
      if ($('reminder-save').disabled) throw new Error('system reminder save disabled');
      await input($('reminder-save'));
    }
    await operation;
  };
  let stage = 0; let last = ''; const expires = Date.now() + 600000;
  try {
    await until(() => store.projects.length);
    await report(true, 0);
    while (Date.now() < expires) {
      const request = await inv('smoke_native_scenario');
      const command = request.split(':')[0];
      if (request !== last && command !== 'native-idle') {
        last = request;
        const target = provider.list().find(c => c.title.startsWith('Reminder native '));
        if (command === 'native-activate') {
          stage = 10; await inv('reminder_show');
          await inv('smoke_native_input', { input: { kind: 'activate' } });
          const initial = await inv('smoke_native_input', { input: { kind: 'state' } });
          await inv('ui_event', { code: 'smoke-check', detail: 'reminder-native-observation', a: initial, b: 10 });
          await until(async () => (await inv('smoke_native_input', { input: { kind: 'state' } }) & 3) === 3);
          await report(true, stage);
        } else if (command === 'native-create' || command === 'native-create-inventory') {
          stage = 1; const p = provider.projects()[0];
          await provider.createStarted({ projectId: p.id, columnId: p.columns[0].id, title: 'Native reminder sentinel', dir: '/tmp', cmd: '' });
          const c = (await provider.createStarted({ projectId: p.id, columnId: p.columns[0].id, title: 'Reminder native pending', dir: '/tmp', cmd: '' })).card;
          await provider.rename(c.id, 'Reminder native ' + c.id.slice(-12));
          await editor(provider.get(c.id), false, false, command === 'native-create-inventory'); await pollNow();
          await openSession(c.id); await pollNow();
          await inv('pty_write', { name: provider.get(c.id).session, dataB64: btoa('exit\r') });
          leaveSessionView(); state.view = 'board'; render();
          await until(async () => { await pollNow(); return provider.get(c.id)?.reminderRetirements?.length > 0; });
          await report(provider.get(c.id).status === 'stopped', stage);
        } else if (['native-edit', 'native-in-app', 'native-edit-inventory', 'native-in-app-inventory'].includes(command)) {
          const inAppOnly = command.startsWith('native-in-app');
          stage = inAppOnly ? 7 : 2; await editor(target, false, inAppOnly, command.endsWith('-inventory')); await report(true, stage);
        } else if (command === 'native-end' || command === 'native-end-inventory') {
          stage = 3; await editor(target, true, false, command.endsWith('-inventory')); for (let i = 0; i < 4; i++) if (!(await pollNow())) throw new Error('poll failed');
          await report(!!provider.get(target.id) && !provider.get(target.id).reminder, stage);
        } else if (command === 'native-end-fault' || command === 'native-end-fault-inventory') {
          stage = 9; const prior = provider.get(target.id).reminder;
          await inv('smoke_fault_set', { kind: 'board-save', count: 1 });
          await editor(target, true, false, command.endsWith('-inventory'));
          await report(provider.get(target.id).reminder?.id === prior.id && provider.get(target.id).reminder?.revision === prior.revision, stage);
        } else if (command === 'native-observe') {
          stage = 4; await reconcileReminders(); await pollNow();
          const c = provider.list().find(c => c.title.startsWith('Reminder native '));
          await inv('smoke_native_snapshot', { name: 'reminder-native-observe' });
          // Numeric evidence of due, stopped, located and independent count.
          const mask = (c?.reminder?.due ? 1 : 0) | (c?.status === 'stopped' ? 2 : 0) | (ctx.attention.counts(provider.list()).pending >= 1 ? 4 : 0) | (document.activeElement?.dataset.sid === c?.id ? 8 : 0);
          await inv('ui_event', { code: 'smoke-check', detail: 'reminder-native-observation', a: mask, b: state.view === 'board' ? 1 : 0 });
          await report(true, stage);
        } else if (command === 'native-quit') {
          stage = 5; await report(true, stage);
          await inv('smoke_native_input', { input: { kind: 'key', keyCode: 12, text: 'q', modifiers: ['command'] } }); return;
        } else if (command === 'native-withdraw') {
          stage = 6; await inv('smoke_reminder_withdraw'); await report(true, stage);
        } else if (command === 'native-other') {
          stage = 8; const sentinel = provider.list().find(c => c.title === 'Native reminder sentinel');
          await inv('smoke_native_input', { input: { kind: 'activate' } });
          await openSession(sentinel.id); await report(state.sessionId === sentinel.id, stage);
        }
      }
      await inv('smoke_reminder_inventory');
      await pause(250);
    }
    throw new Error('native smoke bounded process lifetime expired');
  } catch (_) { await report(false, stage); await inv('smoke_native_snapshot', { name: 'reminder-native-failure' }).catch(() => {}); }
}
