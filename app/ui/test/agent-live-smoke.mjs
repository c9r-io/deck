// The Agent handover trigger end to end in the real window, the `agent-live`
// WKWebView smoke. Debug carrier only, on an isolated data dir and a
// deck-smoke socket, under a home of its own with the carrier at
// <home>/Applications/deck.app (the status hooks are installed only from a
// stable location; app/SMOKE.md "Agent handover: delegated MCP client").
//
// The page does what the person does, and nothing a person could not:
// Remote access on and ONE client authorized with the automation delegation
// (the two IPCs the Settings dialog calls after its confirmations), the
// Claude status hooks on (the Settings switch's command, into this home's
// own settings file), a three-step template through the provider (step 2
// carries the item: `{{msg.text}}`), and an "Agent handover" rule through
// the automation editor: the approval tick, the external-content consent,
// and under the `live` scenario the first step without readiness. Then it
// reports `agent-live-armed` and waits for an EXTERNAL client: the operator
// runs scripts/mcp-agent-e2e.py against this instance's debug adapter
// (`deck_automation_list` → `deck_automation_trigger` over the production
// control socket), which is the only way a card appears here. The page
// watches that card and its three external rows, the deliveries, withdraws
// the delegation through the client row's own button, checks that the sweep
// took the approval and the override off every unsent row and that nothing
// more is delivered, closes the card and delegates the client again through
// the same button (its confirmation), so the operator can replay the request
// against a closed card under a standing delegation. No hook word and no
// event is synthesized: the Signal comes from the real client in the pane.
//
// Scenario (`<data>/translation-fixture/scenario`, read through the
// debug-only `smoke_native_scenario`; the `b` of a scenario-dependent
// checkpoint is 1 for `live`, 2 for `rehearsal`):
//   live      — a Claude Code signed in under this home (trust and first-run
//               screens answered beforehand, in a separate session): the
//               first step goes under the escape hatch, the second after the
//               real turn boundary and the chain quiet, the third is withheld
//               after the withdrawal.
//   rehearsal — Claude Code without a sign-in: the first-send policy stays
//               off, so the head row is held at `first-send` and nothing is
//               ever typed into the login screen; the delegation, the
//               trigger, the card, the hold, the withdrawal and the close are
//               exercised, the deliveries are not (reported with a=2).
export async function runAgentLiveSmoke() {
  const { $, ctx, inv, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { openAutomations, closeAutomations } = await import('../js/automation.js');
  const { openSettings, renderMcpSettings } = await import('../js/settings.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
    return ok;
  };
  /* a bounded wait whose every poll is a real read of the state judged;
     the carrier window is in front, so its timers are not napped */
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
  const AUTOMATION = 'patrol-mention';
  const TEMPLATE = 'agent-live';
  const STEPS = [
    'Reply with exactly: AGENT-LIVE-STEP-1-OK. Do not use tools.',
    'A handed-over item from {{msg.from}} in {{msg.where}}: {{msg.text}}\nReply with exactly AGENT-LIVE-STEP-2-OK followed by the first word of the item. Do not use tools.',
    'Reply with exactly: AGENT-LIVE-STEP-3-OK. Do not use tools.',
  ];
  const queue = async session => {
    const q = await inv('queue_list');
    const mine = (q.items || []).filter(item => item.session === session)
      .sort((x, y) => ((x.tpl_idx ?? 0) - (y.tpl_idx ?? 0)) || ((x.seq ?? 0) - (y.seq ?? 0)) || (x.added - y.added));
    const plans = (q.plans || []).filter(plan => mine.some(item => item.id === plan.item));
    const sent = id => (q.deliveries || []).some(delivery => delivery.item === id);
    return { q, mine, plan: item => plans.find(plan => plan.item === item.id), sent };
  };
  let card = null;
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    const scenario = await inv('smoke_native_scenario').catch(() => '');
    if (!['live', 'rehearsal'].includes(scenario)) throw new Error('scenario');
    const live = scenario === 'live';
    const tag = live ? 1 : 2;
    const home = await inv('default_dir');
    const work = home.slice(0, home.lastIndexOf('/')) + '/work';
    const project = provider.projects()[0];
    const column = project.columns[0];

    /* 1. Remote access on, one client with the automation delegation */
    await inv('mcp_enable');
    const client = await inv('mcp_client_add', { name: 'patrol', projects: [{ projectId: project.id, roots: [work] }],
      allowCreate: false, allowAutomations: true });
    const listed = await until(async () => {
      const status = await inv('mcp_status');
      return status.enabled && status.socketReady
        && status.clients.some(value => value.id === client.id && value.allowAutomations === true && !value.revoked) ? status : null;
    }, 15_000, 250);
    await report('agent-live-client', !!listed && client.allowAutomations === true && client.allowCreate === false);

    /* 2. the Claude status hooks, into this home's own settings file (the
       adapter's source word is `claude-code`; `claude` is an executable) */
    await inv('agent_hooks_set', { agent: 'claude-code', enable: true });
    const hooks = await inv('agent_hooks_status');
    await report('agent-live-hooks', hooks.claude === true);

    /* 3. the template through the provider, the rule through the editor's
       Agent trigger: fields first (an edit withdraws a tick), then the
       approval, the external-content consent, the first-send policy */
    await provider.saveTemplate(project.id, TEMPLATE, STEPS);
    await openAutomations({ trigger: 'agent' });
    $('auto-badge').value = AUTOMATION;
    $('auto-column').value = column.id;
    $('auto-template').value = TEMPLATE; $('auto-template').dispatchEvent(new Event('change'));
    $('auto-dir').value = work; $('auto-cmd').value = 'claude';
    document.querySelector('#auto-finish [data-v="keep"]').click();
    $('auto-send').click();
    if (!$('auto-send').checked || $('auto-send-external-row').hidden || $('auto-send-external').disabled) throw new Error('approval row');
    $('auto-send-external').click();
    if (live) {
      $('auto-first-send').click();
      if ($('auto-first-send').checked || $('cfm').style.display !== 'flex') throw new Error('missing first-send confirmation');
      $('cfm-yes').click();
      if (!(await until(() => $('auto-first-send').checked, 3000, 50))) throw new Error('first-send not accepted');
    }
    $('auto-save').click();
    const rule = await until(() => ctx.settings.inbound.rules.find(value => value.source === 'agent' && value.badge === AUTOMATION) || null, 10_000, 100);
    closeAutomations();
    const ruleOk = !!rule && rule.cmd === 'claude' && rule.template === TEMPLATE && rule.dir === work && rule.finish === 'keep'
      && rule.columnId === column.id && rule.enabled === true && !!rule.autoSend?.digest && rule.autoSend.external === true
      && (rule.firstSendWithoutReadiness === true) === live;
    if (!(await report('agent-live-rule', ruleOk, 1, tag))) throw new Error('rule');
    await report('agent-live-armed', true, 1, tag);

    /* 4. the external client's trigger: the one way a card appears here */
    card = await until(() => store.cards.find(value => value.origin?.source === 'agent' && value.origin.badge === AUTOMATION) || null, 600_000);
    if (!card) throw new Error('no card');
    const seen = await until(async () => { const state = await queue(card.session); return state.mine.length === 3 ? state : null; }, 30_000);
    const cardOk = !!seen && card.cmd === 'claude' && card.dir === work && card.columnId === column.id
      && seen.mine.every(item => item.external === true && ['fixed', 'bounded'].includes(item.authority?.class))
      && seen.mine[1].authority?.class === 'bounded'
      && seen.mine.every(item => seen.plan(item)?.authorized === true)
      && (seen.plan(seen.mine[0])?.first_send_override === true) === live;
    if (!(await report('agent-live-card', cardOk, seen ? seen.mine.length : 0, tag))) throw new Error('card');
    const [first, second, third] = seen.mine;

    /* 5. the first step: under the escape hatch (live), or held at the
       first-interaction gate with nothing typed (rehearsal) */
    if (live) {
      const sent = await until(async () => (await queue(card.session)).sent(first.id), 150_000, 2000);
      await report('agent-live-first', !!sent, 1, tag);
    } else {
      await pause(45_000);
      const state = await queue(card.session);
      await report('agent-live-first', !state.sent(first.id) && state.mine.some(item => item.id === first.id)
        && state.plan(first)?.stage === 'first-send', 1, tag);
    }

    /* 6. the chain: the second step after the real turn boundary and the
       chain quiet (live); behind an unsent head it stays unsent (rehearsal) */
    if (live) {
      const sent = await until(async () => (await queue(card.session)).sent(second.id), 480_000, 2000);
      await report('agent-live-chain', !!sent, 1, tag);
    } else {
      const state = await queue(card.session);
      await report('agent-live-chain', !state.sent(second.id) && state.mine.some(item => item.id === second.id), 1, tag);
    }

    /* 7. the withdrawal through the client row's own button */
    await openSettings({ section: 'remote' });
    await renderMcpSettings();
    const row = [...document.querySelectorAll('.mcp-client-row')].find(value => value.textContent.includes('patrol'));
    const button = row?.querySelector('button.mcp-automations');
    if (!button || button.textContent !== t('mcp.automationsWithdraw')) throw new Error('withdraw button');
    button.click();
    const withdrawn = await until(async () => {
      const status = await inv('mcp_status');
      return status.clients.some(value => value.id === client.id && value.allowAutomations === false) ? status : null;
    }, 10_000, 250);
    $('set-close').click();
    /* the scheduler's sweep takes the approval and the override off every
       unsent row within its tick (thread.rs) */
    const swept = await until(async () => {
      const state = await queue(card.session);
      const unsent = state.mine.filter(item => !state.sent(item.id));
      return unsent.length && unsent.every(item => state.plan(item)?.authorized === false && state.plan(item)?.first_send_override === false)
        ? state : null;
    }, 90_000, 2000);
    await report('agent-live-withdrawn', !!withdrawn && !!swept, swept ? swept.mine.filter(item => !swept.sent(item.id)).length : 0, tag);

    /* 8. what was not sent stays unsent: longer than the chain quiet and
       the send gap after the second turn (live) */
    await pause(live ? 300_000 : 30_000);
    const after = await queue(card.session);
    const unsent = live ? [third] : [first, second, third];
    const withheld = unsent.every(item => after.mine.some(value => value.id === item.id) && !after.sent(item.id));
    await report('agent-live-withheld', withheld, unsent.length, tag);

    /* 9. the card closed through the ordinary close: a replay of the
       request afterwards must not revive it */
    const closed = await provider.close(card.id);
    await report('agent-live-closed', closed === true && !store.cards.some(value => value.id === card.id), 1, tag);

    /* 10. delegated again through the same button (its own confirmation),
       so the operator's replay meets a standing delegation and a closed card */
    await openSettings({ section: 'remote' });
    await renderMcpSettings();
    const again = [...document.querySelectorAll('.mcp-client-row')].find(value => value.textContent.includes('patrol'))
      ?.querySelector('button.mcp-automations');
    if (!again || again.textContent !== t('mcp.automationsAllow')) throw new Error('delegate button');
    again.click();
    if (!(await until(() => $('cfm').style.display === 'flex', 3000, 50))) throw new Error('missing delegation confirmation');
    $('cfm-yes').click();
    const redelegated = await until(async () => {
      const status = await inv('mcp_status');
      return status.clients.some(value => value.id === client.id && value.allowAutomations === true) ? status : null;
    }, 10_000, 250);
    $('set-close').click();
    await report('agent-live-redelegated', !!redelegated, 1, tag);
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 21, b: 0 });
    await report('agent-live-exception', false);
    await report('done', false, 0, 21);
  }
}
