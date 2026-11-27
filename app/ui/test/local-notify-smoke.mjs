// Real-app acceptance for local notifications inside the real WKWebView:
// the real Automations editor saves two local notification rules, the real
// Notifications page turns the entry on, the OPERATOR (a driver following
// app.log, outside this page) sends notifications with the bundled
// `deck-notify` of this very carrier and the reference adapter, and real
// Claude Code cards receive what the rules sent. Debug carrier only, on an
// isolated data root and tmux socket.
//
// The operator's side, one step per `local-want a=<n>`:
//   1 — publish an unmatched notification twice (accepted, duplicate), the
//       same id with other content (conflict), refused inputs, `status`,
//       two more unmatched ones (`hello-2`, `hello-3`), and the adapter's
//       `fail 1842` twice (the auto rule's producer);
//   2 — the adapter's `fail 1842 --attempt 2` (update in place), a late
//       event with a lower revision (stale), `pass … --attempt 3` (resolve)
//       and `fail … --attempt 4` (the next occurrence);
//   3 — `crash-ack-1` for the auto rule while its ack is refused (fault);
//   4 — `crash-claim-1` for the auto rule while its claim is not offered
//       (fault);
//   5 — publish with the entry off (must exit 5, no socket).
// Before step 1 the receipt of the next stored publish is dropped (fault):
// the operator's first publish (`lost-1`) must exit 6 and its retry answer
// `duplicate`. After this launch the runner kills the carrier and starts it
// again on the same root (`local-notify-relaunch`, below).
// Nothing here writes the inbox, settings or the Board behind the product's
// back: rules through the editor, the switch through its box, routes
// through the row's menu (one race through the route command itself).
//
// The rules' command is the fixture's Claude Code (its own hooks file, the
// work directory the runner trusted). The auto rule carries the approval
// and the first-step option, so a notification it takes is answered with
// no click; the manual rule carries neither, so its head waits at the
// first-interaction gate and nothing is typed into its card.

const TEMPLATE = 'local notify';
const STEPS = [
  'A local notification from {{msg.from}} ({{msg.where}}): {{msg.text}}\nReply with exactly LOCAL-NOTIFY-STEP-1-OK followed by the attempt number in it. Do not use tools.',
  'Reply with exactly: LOCAL-NOTIFY-STEP-2-OK. Do not use tools.',
];

export async function runLocalNotifySmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, render } = await import('../js/board.js');
  const { openAutomations, closeAutomations } = await import('../js/automation.js');
  const { showNotices, refreshNotices } = await import('../js/notices.js');
  const { persistInbound } = await import('../js/settings.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (test, timeout = 8000, every = 250) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { const value = await test(); if (value) return value; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return null;
      await pause(every);
    }
  };
  let failed = false;
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    return ok;
  };
  const want = n => inv('ui_event', { code: 'smoke-check', detail: 'local-want', a: n, b: 0 });
  const local = () => inv('local_notifications');
  const notice = async (producer, key) => (await local()).items.find(item => item.producer === producer && item.key === key) || null;
  const queue = async session => {
    const q = await inv('queue_list');
    const mine = (q.items || []).filter(item => item.session === session)
      .sort((x, y) => ((x.tpl_idx ?? 0) - (y.tpl_idx ?? 0)) || (x.added - y.added));
    const plans = q.plans || [];
    const sent = id => (q.deliveries || []).some(delivery => delivery.item === id);
    return { mine, plan: item => plans.find(plan => plan.item === item.id), sent };
  };
  const cardsOf = rule => store.cards.filter(card => card.origin?.source === 'local' && card.origin.badge === rule);
  /* the numbered step running now: an exception says where it happened */
  let stage = 0;
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    const marker = await inv('smoke_native_fixture');
    const fixture = marker.slice(0, marker.lastIndexOf('/'));
    const work = `${fixture}/work`;
    const cmd = `claude --setting-sources project --permission-mode acceptEdits --settings ${fixture}/worker-hooks.json`;
    const project = provider.projects()[0];
    const column = project.columns[0];
    state.projectId = project.id; state.view = 'board'; render();
    const idle = await local();
    await report('local-clean', idle.enabled === false && idle.listening === false && idle.items.length === 0
      && !(ctx.settings.inbound.rules || []).length && !$('notices-btn').hidden);

    stage = 1;
    /* 1. the template, then two rules through the editor's Local trigger */
    await provider.saveTemplate(project.id, TEMPLATE, STEPS);
    const editRule = async ({ name, producer, types, auto, approve }) => {
      await openAutomations({ trigger: 'local' });
      $('auto-name').value = name;
      $('auto-local-producer').value = producer;
      $('auto-local-types').value = types;
      $('auto-local-labels').value = '';
      if ($('auto-local-auto').checked !== auto) $('auto-local-auto').click();
      $('auto-column').value = column.id;
      $('auto-template').value = TEMPLATE; $('auto-template').dispatchEvent(new Event('change'));
      $('auto-dir').value = work; $('auto-cmd').value = cmd;
      document.querySelector('#auto-finish [data-v="keep"]').click();
      if (approve) {
        $('auto-send').click();
        if (!$('auto-send').checked || $('auto-send-external-row').hidden) throw new Error('approval row');
        $('auto-send-external').click();
        $('auto-first-send').click();
        if ($('cfm').style.display !== 'flex') throw new Error('first-send confirmation');
        $('cfm-yes').click();
        if (!(await until(() => $('auto-first-send').checked, 3000, 50))) throw new Error('first-send');
      }
      const before = (ctx.settings.inbound.rules || []).length;
      $('auto-save').click();
      const saved = await until(() => (ctx.settings.inbound.rules || []).length === before + 1
        && ctx.settings.inbound.rules.find(rule => rule.source === 'local' && rule.name === name), 10_000, 100);
      closeAutomations();
      return saved;
    };
    const auto = await editRule({ name: 'CI failures', producer: 'ci-example', types: 'build.failed', auto: true, approve: true });
    const manual = await editRule({ name: 'Manual triage', producer: '*', types: '', auto: false, approve: false });
    const rulesOk = !!auto && !!manual && auto.localMatch.auto === true && auto.localMatch.producer === 'ci-example'
      && auto.localMatch.types[0] === 'build.failed' && !!auto.autoSend?.digest && auto.autoSend.external === true
      && auto.firstSendWithoutReadiness === true && auto.badge === auto.id && auto.since > 0
      && manual.localMatch.auto === false && !manual.autoSend && !manual.firstSendWithoutReadiness
      && ctx.settings.inbound.rules.findIndex(rule => rule.id === auto.id) < ctx.settings.inbound.rules.findIndex(rule => rule.id === manual.id);
    if (!(await report('local-rules', rulesOk, ctx.settings.inbound.rules.length))) throw new Error('rules');

    stage = 2;
    /* 2. the entry, turned on by its box in the Notifications page */
    showNotices();
    await until(() => !$('notices-view').hidden, 3000, 50);
    $('notices-accept').click();
    const on = await until(async () => { const v = await local(); return v.enabled && v.listening ? v : null; }, 20_000, 500);
    await refreshNotices();
    await report('local-accept', !!on && /\/deck-notify$/.test(on.cliPath || '') && $('notices-cli-path').textContent === on.cliPath
      && ctx.settings.inbound.sources.local?.enabled === true);
    await report('local-armed', true);
    await inv('smoke_fault_set', { kind: 'local-no-receipt', count: 1 });
    await want(1);

    stage = 3;
    /* 3. an unmatched notification is listed, in the page */
    const misc = await until(() => notice('misc-tool', 'hello-1'), 300_000, 1000);
    const others = await until(async () => (await notice('misc-tool', 'hello-2')) && (await notice('misc-tool', 'hello-3')), 60_000, 500);
    await refreshNotices();
    const row = await until(() => [...$('notices-list').children].find(node => node.dataset?.id === misc?.id), 5000, 100);
    await report('local-unmatched', !!misc && !!others && misc.attention === 'open' && !misc.routing && misc.title === 'Hello from a script'
      && misc.body.includes('日本語') && !!row, (await local()).items.length);

    stage = 4;
    /* 4. the auto rule took the adapter's build once: one card, two rows,
       the head under the first-step option, both approved */
    const first = await until(() => cardsOf(auto.id)[0] || null, 120_000, 500);
    const rows = first && await until(async () => { const s = await queue(first.session); return s.mine.length === 2 ? s : null; }, 30_000, 500);
    const ci = await notice('ci-example', 'build-1842');
    await report('local-auto-card', !!first && !!rows && cardsOf(auto.id).length === 1 && first.cmd === cmd && first.dir === work
      && rows.mine.every(item => item.external === true && item.authority?.trigger === 'local')
      && rows.plan(rows.mine[0])?.first_send_override === true && rows.mine[1].authority?.class === 'fixed'
      && rows.mine[0].authority?.class === 'bounded' && ci?.routing?.by === 'auto' && ci?.attention === 'routed',
    rows ? rows.mine.length : 0);
    const [head, chain] = rows.mine;
    const firstSent = await until(async () => (await queue(first.session)).sent(head.id), 180_000, 2000);
    await report('local-auto-first', !!firstSent && head.text.includes('<external_message') && head.text.includes('attempt 1'));
    const chainSent = await until(async () => (await queue(first.session)).sent(chain.id), 480_000, 2000);
    await report('local-auto-chain', !!chainSent);

    stage = 5;
    /* 5. a route by hand through the row's menu */
    await refreshNotices();
    [...$('notices-list').children].find(node => node.dataset?.id === misc.id)?.click();
    const menu = await until(() => document.querySelector('#notices-detail .mention-route'), 3000, 50);
    menu.value = manual.id; menu.dispatchEvent(new Event('change'));
    const manualCard = await until(() => cardsOf(manual.id)[0] || null, 30_000, 250);
    const manualRows = manualCard && await until(async () => { const s = await queue(manualCard.session); return s.mine.length === 2 ? s : null; }, 30_000, 500);
    await report('local-manual', !!manualCard && !!manualRows && manualRows.mine.every(item => item.external === true && !item.authority)
      && manualRows.plan(manualRows.mine[0])?.first_send_override === false
      && (await notice('misc-tool', 'hello-1'))?.routing?.by === 'manual');

    stage = 6;
    /* 6. the exclusive claim: a route of a taken notification is refused,
       and two routes of one fresh notification in the same instant make
       one card */
    const takenWord = await inv('local_notification_route', { id: ci.id, occurrence: ci.occurrence, revision: ci.revision, rule: manual.id })
      .then(() => 'ok', error => String(error));
    const h2 = await notice('misc-tool', 'hello-2');
    const words = await Promise.all([0, 1].map(() => inv('local_notification_route',
      { id: h2.id, occurrence: h2.occurrence, revision: h2.revision, rule: manual.id }).then(() => 'ok', error => String(error))));
    await until(() => cardsOf(manual.id).length >= 2, 20_000, 250);
    await pause(3000);
    await report('local-taken', takenWord === 'taken' && words.filter(w => w === 'ok').length === 1
      && words.filter(w => w === 'taken').length === 1 && cardsOf(manual.id).length === 2, cardsOf(manual.id).length);
    await want(2);

    stage = 7;
    /* 7. updates: the row changes in place, a late one changes nothing, a
       resolve settles it without touching the card, and the next
       occurrence is taken again — never the frozen run rewritten */
    const second = await until(() => cardsOf(auto.id).find(card => card.id !== first.id) || null, 300_000, 1000);
    /* the approval and the first-step option withdrawn at once, as the
       editor's save would: the second run's unsent rows lose both */
    const current = ctx.settings.inbound.rules.find(rule => rule.id === auto.id);
    const withdrawn = { ...current };
    delete withdrawn.autoSend; delete withdrawn.firstSendWithoutReadiness;
    const saved = await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.map(rule => (rule.id === auto.id ? withdrawn : rule)) });
    const now = await notice('ci-example', 'build-1842');
    /* the first run keeps what was claimed then: its card (titled from the
       claimed text when it was made) is still there after the update, the
       resolve and the next occurrence, and its delivered head carried
       attempt 1 (checked above); the inbox's frozen copy is the operator's
       evidence (its rows were sent and the plan emptied, as for any run) */
    const kept = store.cards.find(card => card.id === first.id);
    await report('local-frozen', !!now && now.occurrence === 2 && now.external === 'active' && now.revision === 41
      && !!kept && kept.title.includes('attempt 1') && !kept.title.includes('again'),
    now ? now.occurrence : 0, now ? now.revision : 0);
    const secondRows = second && await until(async () => { const s = await queue(second.session); return s.mine.length === 2 ? s : null; }, 30_000, 500);
    await report('local-reoccur', !!second && !!secondRows && secondRows.mine[0].text.includes('attempt 4') && cardsOf(auto.id).length === 2);
    const stripped = secondRows && await until(async () => {
      const s = await queue(second.session);
      return s.mine.every(item => !item.authority && !item.readiness_override) ? s : null;
    }, 90_000, 2000);
    await pause(45_000);
    const after = second && await queue(second.session);
    await report('local-revoked', saved !== false && !!stripped && !!after && after.mine.every(item => !after.sent(item.id)),
      after ? after.mine.filter(item => !after.sent(item.id)).length : 0);

    stage = 8;
    /* 8. a card closed does not bring its notification back */
    const closed = await provider.close(manualCard.id);
    await pause(40_000);
    const still = await notice('misc-tool', 'hello-1');
    await report('local-deleted', closed === true && cardsOf(manual.id).length === 1 && still?.attention === 'routed'
      && still?.routing?.state === 'done');

    stage = 9;
    /* 9. a card made but its ack lost: the claim stays pending, no second
       card however often it is offered again */
    await inv('smoke_fault_set', { kind: 'local-ack-refused', count: 8 });
    await want(3);
    const ackCard = await until(() => store.cards.find(card => card.origin?.source === 'local' && card.title.includes('crash ack')) || null, 300_000, 500);
    await pause(45_000);
    const unacked = await notice('ci-example', 'crash-ack-1');
    await report('local-unacked', !!ackCard && unacked?.routing?.state === 'pending'
      && store.cards.filter(card => card.title.includes('crash ack')).length === 1);

    stage = 10;
    /* 10. a claim stored but never offered: no card while it is held */
    await inv('smoke_fault_set', { kind: 'local-hold-offer', count: 8 });
    await want(4);
    const held = await until(async () => { const n = await notice('ci-example', 'crash-claim-1'); return n?.routing?.state === 'pending' ? n : null; }, 300_000, 500);
    await pause(40_000);
    await report('local-held', !!held && !store.cards.some(card => card.title.includes('crash claim')));

    stage = 11;
    /* 11. the entry off: the socket goes, the operator's publish is refused */
    await refreshNotices();
    $('notices-accept').click();
    const off = await until(async () => { const v = await local(); return !v.enabled && !v.listening ? v : null; }, 20_000, 500);
    await report('local-disabled', !!off && !ctx.settings.inbound.sources.local);
    /* both faults armed again so they still hold when the runner kills the
       carrier (each hit uses one, and a hit count is at most 8) */
    await inv('smoke_fault_set', { kind: 'local-ack-refused', count: 8 });
    await inv('smoke_fault_set', { kind: 'local-hold-offer', count: 8 });
    await want(5);
    await pause(30_000);
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 31, b: stage });
    await report('local-exception', false, Math.max(1, stage), typeof error === 'string' ? error.length : -1);
    await report('done', false, 0, 31);
  }
}

// The launch after the kill, on the same root: what was stored is still
// there, the claim whose ack was lost becomes `done` on the card it already
// has, the claim that was never offered gets its one card, nothing settled
// is routed again, and the entry stays off as it was saved.
export async function runLocalNotifyRelaunchSmoke() {
  const { inv, ctx, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (test, timeout = 8000, every = 250) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { const value = await test(); if (value) return value; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return null;
      await pause(every);
    }
  };
  let failed = false;
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    return ok;
  };
  const notice = async (producer, key) => (await inv('local_notifications')).items.find(item => item.producer === producer && item.key === key) || null;
  const titled = word => store.cards.filter(card => card.origin?.source === 'local' && card.title.includes(word));
  try {
    await until(() => provider.projects().length > 0, 20_000);
    const before = store.cards.filter(card => card.origin?.source === 'local').length;
    const view = await inv('local_notifications');
    await report('relaunch-kept', view.enabled === false && view.listening === false && !ctx.settings.inbound.sources.local
      && view.items.length >= 7, view.items.length, before);
    const acked = await until(async () => ((await notice('ci-example', 'crash-ack-1'))?.routing?.state === 'done' ? true : null), 120_000, 1000);
    await report('relaunch-acked', !!acked && titled('crash ack').length === 1);
    const claimed = await until(async () => ((await notice('ci-example', 'crash-claim-1'))?.routing?.state === 'done' ? true : null), 120_000, 1000);
    await report('relaunch-claimed', !!claimed && titled('crash claim').length === 1);
    await pause(40_000);
    const hello3 = await notice('misc-tool', 'hello-3');
    const hello1 = await notice('misc-tool', 'hello-1');
    const ci = await notice('ci-example', 'build-1842');
    const after = store.cards.filter(card => card.origin?.source === 'local').length;
    await report('relaunch-settled', hello3?.attention === 'open' && !hello3.routing && hello1?.routing?.state === 'done'
      && ci?.occurrence === 2 && titled('crash ack').length === 1 && titled('crash claim').length === 1
      && titled('Build 1842').length === 2 && titled('Second from a script').length === 1
      && titled('Hello from a script').length === 0 && titled('Third from a script').length === 0,
    after, before);
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 32, b: 0 });
    await report('relaunch-exception', false);
    await report('done', false, 0, 32);
  }
}
