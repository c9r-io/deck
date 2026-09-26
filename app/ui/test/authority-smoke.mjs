// Automation delivery authority, live-agent acceptance setup (the
// `authority-live` WKWebView smoke; debug bundles only, isolated data dir and
// deck-smoke socket). It builds what the Slack badge dispatcher builds —
// an approved badge rule saved through the ONE settings writer, a card
// created through the ordinary Board transaction with a frozen
// `inboundPlan` carrying the approval, and the plan queued through the real
// `provider.queueInboundPlan` → `channel_queue_add` → native claim check —
// for Claude, Codex and `codex --no-daemon`, in the already-trusted /tmp. The only
// synthetic part is the Slack event itself (no Slack credentials), so every
// step is a FIXED owner step (a bounded step needs the backend's pending
// event and would be admitted manual). It reports only that setup; the live
// agent behaviour is judged by the person running app/SMOKE.md's
// "Automation authority: live agents" sequence from app.log and the panes.
// No hook word is synthesized: real CLIs report through the installed
// status helper to this instance (DECK_STATUS_SOCK).
export async function runAuthorityLiveSmoke() {
  const { ctx, genId, inv, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { persistInbound } = await import('../js/settings.js');
  const { refreshQueue } = await import('../js/scheduler.js');
  const { approveRule } = await import('../js/automation-model.js');
  const { channelDigestId } = await import('../js/channel-model.js');
  let failed = false;
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  try {
    const project = provider.projects()[0];
    const column = project.columns[0];
    const template = 'authority-live';
    const steps = [
      'Reply with exactly: AUTHZ-STEP-1',
      'Use your shell tool to run exactly this command, then reply DONE: mkdir /tmp/deck-authz-live-probe',
      'Reply with exactly: AUTHZ-STEP-3',
    ];
    await provider.saveTemplate(project.id, template, steps);
    const tpl = provider.project(project.id).templates.find(value => value.name === template);
    const rules = [];
    /* `codex --no-daemon` keeps Codex's hooks in its own pane process; plain
       `codex` may share a managed daemon whose hooks carry the starter's
       pane (Codex Signal Unavailable/Unknown: approved rows must hold) */
    for (const [k, agent] of ['claude', 'codex', 'codex --no-daemon'].entries()) {
      rules.push(await approveRule({ id: genId('R'), source: 'slack', badge: `authz-${k}`, projectId: project.id,
        columnId: column.id, cmd: agent, template, dir: '/tmp', name: '', enabled: true, finish: 'keep', since: 0 }, tpl));
    }
    await persistInbound({ ...ctx.settings.inbound, rules: [...ctx.settings.inbound.rules, ...rules] });
    const now = Math.floor(Date.now() / 1000);
    for (const rule of rules) {
      const cardId = genId('S');
      const key = `live/${rule.badge}`;
      const initialSteps = await Promise.all(steps.map(async (text, index) => ({
        operationId: await channelDigestId('B', `${cardId}/step/${index}`), text,
        mode: index ? 'chain' : 'at', at: index ? null : now, ...(index ? { quietSecs: 20 } : {}),
        tpl: template, tplIdx: index + 1, tplTotal: steps.length,
      })));
      const card = await provider.create({ id: cardId, projectId: project.id, columnId: column.id,
        title: `authz live · ${rule.cmd}`, cmd: rule.cmd, dir: '/tmp', desc: template,
        origin: { source: 'slack', key, badge: rule.badge },
        inboundPlan: { operationId: await channelDigestId('B', `${cardId}/list`), reviewEach: false, initialSteps,
          initialQueued: false, authority: { rule: rule.id, grant: rule.autoSend.digest, trigger: 'slack-badge',
            classes: [...rule.autoSend.classes], event: key, skeletons: steps.map(() => null) } } });
      await provider.queueInboundPlan(card.id, key);
    }
    await refreshQueue();
    const rows = (ctx.queueCache.items || []).filter(item => store.cards.some(card => card.session === item.session
      && card.title.startsWith('authz live')));
    await report('authority-live-setup', rows.length === 9
      && rows.every(row => row.external === true && row.authority && row.authority.class === 'fixed'), rows.length, 9);
  } catch (_) {
    await report('authority-live-exception', false);
  }
  await report('done', !failed);
}

// The zero-session automation deadlock (B.2), the `empty-start` WKWebView
// smoke: a fresh isolated root whose tmux socket has NO session — no helper,
// no other card. A clock-style automation card (shell only, one owner `:`
// no-op step) is created and queued through the real dispatcher path; the
// scheduler must recognize the positively empty server, start the card's
// session and deliver the no-op. Before B.2 a failed `list-panes -a` on an
// empty server selected nothing, forever. No agent, no prompt, no network.
export async function runEmptyStartSmoke() {
  const { genId, inv, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { channelDigestId } = await import('../js/channel-model.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  const alive = async session => {
    try {
      const infos = await inv('poll_sessions', { names: [session], tailFor: [], checkpointShells: false });
      return (infos || []).some(info => info.name === session && info.alive);
    } catch (_) { return false; }
  };
  try {
    const project = provider.projects()[0];
    const column = project.columns[0];
    await report('empty-start-empty', store.cards.length === 0, 1, store.cards.length);
    const cardId = genId('S');
    const key = String(Math.floor(Date.now() / 1000));
    const card = await provider.create({ id: cardId, projectId: project.id, columnId: column.id,
      title: 'empty start', cmd: '', dir: '/tmp', desc: '',
      origin: { source: 'clock', key, badge: 'empty-start' },
      inboundPlan: { operationId: await channelDigestId('B', `${cardId}/list`), reviewEach: false, initialQueued: false,
        initialSteps: [{ operationId: await channelDigestId('B', `${cardId}/step/0`), text: ':', mode: 'at',
          at: Math.floor(Date.now() / 1000), tpl: 'empty-start', tplIdx: 1, tplTotal: 1 }] } });
    await provider.queueInboundPlan(card.id, key);
    await report('empty-start-queued', !(await alive(card.session)));
    let started = false, delivered = false;
    const until = Date.now() + 90_000;
    while (Date.now() < until && !(started && delivered)) {
      started ||= await alive(card.session);
      const queue = await inv('queue_list');
      delivered = (queue.deliveries || []).some(d => d.session === card.session);
      if (!(started && delivered)) await pause(1000);
    }
    await report('empty-start-started', started);
    await report('empty-start-delivered', delivered);
  } catch (_) {
    await report('empty-start-exception', false);
  }
  await report('done', !failed);
}
