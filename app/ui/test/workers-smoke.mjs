// Worker cards end to end in the real window, the `workers` WKWebView
// smoke. Debug carrier only, on an isolated data dir and a deck-smoke
// socket, under the user's own home (so the Claude Code in the cards is the
// one already signed in; nothing here signs in, and no file of that home is
// written by Deck: the status hooks come from `claude --settings`, a file the
// operator put in `<data>/translation-fixture/worker-hooks.json`, never from
// Deck's own hook installer). app/SMOKE.md "Worker cards".
//
// The page does what the person does: Remote access on and ONE client
// authorized with the automation delegation AND the worker choice (the
// Settings dialog's IPCs), a one-step template `Task: {{msg.text}}` (a step
// may not open with the message) and an "Agent
// handover" rule through the automation editor (approval, external-content
// consent, first step without readiness). Then `workers-armed`, and two
// external clients in turn:
//   A. scripts/mcp-worker-e2e.py, a scripted client: it starts one worker,
//      reads its reply, proves EPISODE_MOVED and STALE_REQUEST, sends a line
//      and reads that reply. The page then takes the card over through the
//      pane's own input path (one keystroke and its erasure), sees the
//      client's next line held at `human` and not delivered, hands the card
//      back through the card's own button, sees it delivered, withdraws the
//      worker choice through the client row's button (the client then gets
//      PERMISSION_DENIED) and closes the card.
//   B. after the choice is granted again (the row's button and its
//      confirmation): a real orchestrating agent (the operator runs Codex or
//      Claude Code with this instance's adapter) starts two workers and
//      sends each at least one line. The page counts them.
// No hook word and no event is synthesized: the Signal comes from the real
// Claude Code in each card.
export async function runWorkersSmoke({ drafting = false } = {}) {
  const { $, ctx, inv, store } = await import('../js/state.js');
  const { provider, panes } = await import('../js/board.js');
  const { openSession, backToBoard } = await import('../js/layout.js');
  const { openAutomations, closeAutomations } = await import('../js/automation.js');
  const { openSettings, renderMcpSettings } = await import('../js/settings.js');
  const { refreshWorkers, workerOf } = await import('../js/workers.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    return ok;
  };
  const until = async (test, ms, step = 1000) => {
    const deadline = Date.now() + ms;
    for (;;) {
      let value = null;
      try { value = await test(); } catch (_) { value = null; }
      if (value) return value;
      if (Date.now() >= deadline) return null;
      await pause(step);
    }
  };
  const AUTOMATION = 'worker';
  const TEMPLATE = 'worker';
  const queue = async session => {
    const q = await inv('queue_list');
    const mine = (q.items || []).filter(item => item.session === session);
    const plans = (q.plans || []).filter(plan => mine.some(item => item.id === plan.item));
    const delivered = (q.deliveries || []).filter(d => d.session === session);
    /* the client's later lines: a worker type's first line is delegated
       text too, but it is the run's head (`at`), not a line written after a
       turn boundary */
    const delegated = delivered.filter(d => d.authority?.class === 'delegated' && d.mode === 'chain');
    return { q, mine, plan: item => plans.find(plan => plan.item === item.id), delivered, delegated };
  };

  /* a worker card: one an Agent trigger made (`live`, `codex`) or one
     `deck_worker_start` made from a worker type (`types`) */
  const workerCards = () => store.cards.filter(value => (value.origin?.source === 'agent' && value.origin.badge === AUTOMATION)
    || value.origin?.source === 'worker');
  /* one choice changed through the client row's Permissions… dialog and
     its Save, as the person does it (extension-mcp.js) */
  const setPermission = async (clientName, checkbox, on) => {
    await openSettings({ section: 'remote' });
    await renderMcpSettings();
    const row = [...document.querySelectorAll('.mcp-client-row')].find(value => value.textContent.includes(clientName));
    const open = row?.querySelector('button.mcp-permissions');
    if (!open || open.textContent !== t('mcp.permissions')) throw new Error('permissions button');
    open.click();
    if (!(await until(() => $('mcp-auth').style.display === 'flex', 3000, 50))) throw new Error('permissions dialog');
    if ($(checkbox).checked !== on) $(checkbox).click();
    $('mcp-auth-yes').click();
    await until(() => $('mcp-auth').style.display === 'none', 3000, 50);
  };
  /* `drafts`: a client that may draft (and trigger automations); the
     scripted client (scripts/mcp-drafts-e2e.py) drafts four times and the
     page answers each through the Needs attention row and the review
     dialog, as the person does: approve, save and edit, reject, and the
     fourth is withdrawn with the choice. Then the choice is granted again
     for a real agent's own draft, which the page approves (`done` b=4). */
  const drafts = async ({ project, column, work }) => {
    const { showAttention } = await import('../js/attention.js');
    const { pendingNow, refreshProposals, drafter } = await import('../js/proposals.js');
    const { grantState } = await import('../js/automation-model.js');
    await inv('mcp_enable');
    const client = await inv('mcp_client_add', { name: 'orchestrator', projects: [{ projectId: project.id, roots: [work] }],
      permissions: { allowCreate: false, allowAutomations: true, allowWorkers: false, allowReplies: false,
        workerTypes: [], workerFirstSend: false, allowDrafts: true } });
    const listed = await until(async () => {
      const status = await inv('mcp_status');
      return status.enabled && status.socketReady && status.clients.some(value => value.id === client.id && value.allowDrafts) ? status : null;
    }, 15_000, 250);
    await report('workers-client', !!listed && client.allowDrafts === true);
    await report('workers-armed', true);
    /* the next pending draft, opened from its Needs attention row */
    const seen = new Set();
    const review = async ms => {
      const p = await until(async () => { await refreshProposals(); return pendingNow().find(value => !seen.has(value.id)) || null; }, ms, 1000);
      if (!p) return null;
      seen.add(p.id);
      showAttention();
      const row = await until(() => document.querySelector(`#attention-list [data-key="proposal-${p.id}"]`), 10_000, 200);
      if (!row) throw new Error('draft row');
      row.querySelector('.attention-open').click();
      if (!(await until(() => $('prop').style.display === 'flex', 3000, 50))) throw new Error('draft dialog');
      return p;
    };
    const rules = () => ctx.settings.inbound.rules.filter(value => value.source === 'agent' && value.badge === 'code-review');
    const template = name => provider.project(project.id)?.templates?.find(value => value.name === name) || null;

    /* 1. approve: the template and the rule, its approval signed by the click */
    const p1 = await review(600_000);
    if (!p1) throw new Error('no first draft');
    const shownSteps = [...document.querySelectorAll('#prop-body .prop-step-text')].map(el => el.textContent);
    $('prop-approve').click();
    const saved = await until(() => rules()[0] || null, 15_000, 200);
    const tpl = template('Code review');
    const grant = saved && tpl ? await grantState(saved, tpl) : 'none';
    await refreshProposals();
    await openAutomations();
    const label = [...document.querySelectorAll('.auto-rule')].some(el => el.textContent.includes(t('proposals.draftedBy'))
      && el.textContent.includes('orchestrator'));
    closeAutomations();
    await report('drafts-approved', shownSteps.length === 3 && !!tpl && tpl.steps.length === 3 && tpl.followUps?.length === 1
      && saved?.cmd === 'claude' && saved.dir === work && saved.columnId === column.id && grant === 'valid'
      && saved.autoSend?.external === true && saved.firstSendWithoutReadiness === true
      && drafter(saved.id) === 'orchestrator' && label, shownSteps.length, grant === 'valid' ? 1 : 0);

    /* 2. a change: what changes is shown; save and edit opens the editor */
    const p2 = await review(600_000);
    if (!p2) throw new Error('no second draft');
    const diff = [...document.querySelectorAll('#prop-body .prop-diff li')].map(el => el.textContent);
    $('prop-edit').click();
    const editor = await until(() => (!$('auto-editor').hidden && $('auto-badge').value === 'code-review') || null, 15_000, 200);
    const edited = rules()[0];
    closeAutomations();
    await report('drafts-edited', diff.length >= 1 && !!editor && rules().length === 1
      && edited?.cmd === 'claude --permission-mode acceptEdits' && !edited.autoSend && !edited.firstSendWithoutReadiness, diff.length);

    /* 3. rejected: nothing saved */
    const p3 = await review(600_000);
    if (!p3) throw new Error('no third draft');
    $('prop-reject').click();
    await pause(2000);
    await report('drafts-rejected', !template('Scratch') && !pendingNow().some(value => value.id === p3.id));

    /* 4. the fourth waits; withdrawing the choice withdraws it */
    const p4 = await until(async () => { await refreshProposals(); return pendingNow().find(value => !seen.has(value.id)) || null; }, 600_000, 1000);
    await setPermission('orchestrator', 'mcp-perm-drafts', false);
    $('set-close').click();
    const gone = await until(async () => { await refreshProposals(); return p4 && !pendingNow().some(value => value.id === p4.id) ? true : null; }, 15_000, 500);
    await report('drafts-withdrawn', !!gone && !template('Later'));
    /* the choice stays off long enough for the client to see its
       PERMISSION_DENIED (it polls every second) before it is granted again */
    await pause(15_000);

    /* 5. granted again for a real agent: its own draft, approved as shown */
    await setPermission('orchestrator', 'mcp-perm-drafts', true);
    $('set-close').click();
    await report('drafts-regranted', true);
    const p5 = await review(1_500_000);
    let agentOk = false;
    if (p5) {
      $('prop-approve').click();
      await pause(3000);
      await refreshProposals();
      agentOk = !pendingNow().some(value => value.id === p5.id) && !!p5.template;
    }
    await report('drafts-agent', agentOk, p5?.template?.steps?.length || 0, p5?.rule ? 1 : 0);
    await report('done', !failed, 1, 4);
  };
  let first = null;
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    /* `live`: Claude Code workers (parts A and B); `codex`: one Codex
       `--no-daemon` worker through part A only (no reply is forwarded for
       Codex, and part B is not run: `done` then reports b=2) */
    const scenario = drafting ? 'drafts' : await inv('smoke_native_scenario').catch(() => '');
    if (!['live', 'codex', 'types', 'drafts'].includes(scenario)) throw new Error('scenario');
    const codex = scenario === 'codex';
    /* `types`: no template and no rule; the client carries one Claude worker
       type and the first-send choice, and the scripted client starts the
       worker with deck_worker_start (part A only, `done` b=3) */
    const types = scenario === 'types';
    const marker = await inv('smoke_native_fixture');
    const fixture = marker.slice(0, marker.lastIndexOf('/'));
    const work = fixture + '/work';
    /* only the fixture's hooks: the user's own settings may carry another
       Deck's hooks, whose second turn ending would be dropped as a duplicate
       and take the forwarded reply with it */
    const cmd = codex ? 'codex --no-daemon'
      : `claude --setting-sources project --permission-mode acceptEdits --settings ${fixture}/worker-hooks.json`;
    const project = provider.projects()[0];
    const column = project.columns[0];
    if (scenario === 'drafts') { await drafts({ project, column, work }); return; }

    /* 1. Remote access on; one client that may start and drive workers */
    await inv('mcp_enable');
    const client = await inv('mcp_client_add', { name: 'orchestrator', projects: [{ projectId: project.id, roots: [work] }],
      permissions: { allowCreate: false, allowAutomations: !types, allowWorkers: true, allowReplies: true,
        workerTypes: types ? [{ name: 'claude', cmd, columnId: column.id, description: 'Write code' }] : [], workerFirstSend: types } });
    const listed = await until(async () => {
      const status = await inv('mcp_status');
      return status.enabled && status.socketReady
        && status.clients.some(value => value.id === client.id && value.allowWorkers && value.allowReplies) ? status : null;
    }, 15_000, 250);
    await report('workers-client', !!listed && client.allowWorkers === true && client.allowReplies === true);

    /* 2. the one-step template and the rule, through the editor (a worker
       type needs neither) */
    if (!types) {
    await provider.saveTemplate(project.id, TEMPLATE, ['Task: {{msg.text}}']);
    await openAutomations({ trigger: 'agent' });
    $('auto-badge').value = AUTOMATION;
    $('auto-column').value = column.id;
    $('auto-template').value = TEMPLATE; $('auto-template').dispatchEvent(new Event('change'));
    $('auto-dir').value = work; $('auto-cmd').value = cmd;
    document.querySelector('#auto-finish [data-v="keep"]').click();
    $('auto-send').click();
    if (!$('auto-send').checked || $('auto-send-external-row').hidden || $('auto-send-external').disabled) throw new Error('approval row');
    $('auto-send-external').click();
    $('auto-first-send').click();
    if ($('cfm').style.display !== 'flex') throw new Error('missing first-send confirmation');
    $('cfm-yes').click();
    if (!(await until(() => $('auto-first-send').checked, 3000, 50))) throw new Error('first-send not accepted');
    $('auto-save').click();
    const rule = await until(() => ctx.settings.inbound.rules.find(value => value.source === 'agent' && value.badge === AUTOMATION) || null, 10_000, 100);
    closeAutomations();
    const ruleOk = !!rule && rule.cmd === cmd && rule.dir === work && rule.autoSend?.external === true
      && rule.firstSendWithoutReadiness === true;
    if (!(await report('workers-rule', ruleOk))) throw new Error('rule');
    } else {
      await report('workers-rule', client.workerTypes?.[0]?.cmd === cmd && client.workerFirstSend === true, 1, 3);
    }
    await report('workers-armed', true);

    /* A1. the scripted client's worker: a quiet card, labelled as driven */
    first = await until(() => workerCards()[0] || null, 600_000);
    if (!first) throw new Error('no worker card');
    await refreshWorkers();
    const labelled = await until(async () => { await refreshWorkers(); return workerOf(first.id); }, 10_000, 500);
    await report('workers-card', first.quietDone === true && labelled?.clientName === 'orchestrator' && labelled.takenOver === false
      && first.cmd === cmd && first.dir === work);

    /* A2. its first line goes under the first-send policy; the client's
       next line goes as delegated text after the real turn boundary */
    const head = await until(async () => { const s = await queue(first.session); return s.delivered.length >= 1 ? s : null; }, 240_000, 2000);
    await report('workers-first', !!head);
    const sent = await until(async () => { const s = await queue(first.session); return s.delegated.length >= 1 ? s : null; }, 480_000, 2000);
    await report('workers-send', !!sent, sent ? sent.delegated.length : 0);
    // the client reads that turn's reply first
    await pause(45_000);

    /* A3. the user's own keystroke in the pane takes the card over */
    await openSession(first.id);
    const pane = await until(() => panes.get(first.session) || null, 10_000, 200);
    if (!pane) throw new Error('no pane');
    await pause(1500);
    pane.term.input(' ', true);
    await pause(300);
    pane.term.input('\x7f', true);
    const taken = await until(async () => { await refreshWorkers(); return workerOf(first.id)?.takenOver ? true : null; }, 10_000, 250);
    await report('workers-taken', !!taken);
    backToBoard();

    /* A4. the client's next line waits at `human`, and is not delivered */
    const held = await until(async () => {
      const s = await queue(first.session);
      const row = s.mine.find(item => item.authority?.class === 'delegated' && ['pending', 'failed'].includes(item.state));
      return row && s.plan(row)?.stage === 'human' ? { s, row } : null;
    }, 300_000, 2000);
    await pause(30_000);
    const still = await queue(first.session);
    await report('workers-held', !!held && still.delegated.length === sent.delegated.length,
      still.delegated.length, held ? 1 : 0);

    /* A5. handed back through the card's own button: it goes out */
    const chip = await until(() => document.querySelector(`.card[data-sid="${first.id}"] .card-worker.taken`), 10_000, 250);
    if (!chip || chip.textContent !== t('workers.takenOver', { name: 'orchestrator' })) throw new Error('hand-back button');
    chip.click();
    const back = await until(async () => {
      const s = await queue(first.session);
      return s.delegated.length > sent.delegated.length ? s : null;
    }, 300_000, 2000);
    await report('workers-handed-back', !!back && !workerOf(first.id)?.takenOver);

    /* A6. the worker choice withdrawn through the client row's dialog */
    await pause(60_000);
    await setPermission('orchestrator', 'mcp-perm-workers', false);
    const withdrawn = await until(async () => {
      const status = await inv('mcp_status');
      return status.clients.some(value => value.id === client.id && !value.allowWorkers && !value.allowReplies) ? status : null;
    }, 10_000, 250);
    $('set-close').click();
    await report('workers-withdrawn', !!withdrawn);
    await pause(60_000);
    const closed = await provider.close(first.id);
    await report('workers-closed', closed === true);
    if (codex) { await report('done', !failed, 1, 2); return; }
    if (types) { await report('done', !failed, 1, 3); return; }

    /* B. granted again through the same dialog (turning it on brings back
       one Claude type and shared replies), for a real orchestrating agent:
       two workers, at least one line each */
    await setPermission('orchestrator', 'mcp-perm-workers', true);
    const again = await until(async () => {
      const status = await inv('mcp_status');
      return status.clients.some(value => value.id === client.id && value.allowWorkers && value.allowReplies) ? status : null;
    }, 10_000, 250);
    $('set-close').click();
    await report('workers-redelegated', !!again);
    const orchestrated = await until(async () => {
      const cards = workerCards().filter(value => value.id !== first.id);
      if (cards.length < 2) return null;
      const states = await Promise.all(cards.map(value => queue(value.session)));
      return states.filter(s => s.delegated.length >= 1).length >= 2 ? cards : null;
    }, 1_500_000, 5000);
    await report('workers-orchestrated', !!orchestrated, orchestrated ? orchestrated.length : 0);
    for (const value of workerCards()) await provider.close(value.id);
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 22, b: 0 });
    await report('workers-exception', false);
    await report('done', false, 0, 22);
  }
}
