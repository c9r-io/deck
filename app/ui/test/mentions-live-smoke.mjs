// Real-service acceptance for the Slack mentions view inside the real
// WKWebView: a sandbox workspace, its real search index, real messages that
// mention the user. Debug carrier only, on an isolated data root, tmux socket
// and home.
//
// Nothing here seeds a credential or a mention behind the product's back.
// The user token goes in through its Settings field, the way a user pastes
// it (`inbound_set_secret`, verified with Slack, process-local in a smoke
// launch). The badge rule is saved by the real Automations editor; the
// mention is routed and dismissed by the row's own controls. What the page
// cannot know by itself — the token, the user's own id, the marker — comes
// from one private file of the operator's, read through a card's own pane:
// `<root>/params`, hex in lines of at most 64 characters, where `<root>` is
// `/private/tmp/deck-mentions-<12 hex>` and the launch's scenario word is
// `mentions-<12 hex>`. The card is closed as soon as the file is read.
// `<root>/work` is the rule's directory.
//
// The Slack side is the operator's (a driver following app.log). Each
// `mentions-want a=<n> b=<seconds>` asks for ONE message holding
// `DECKLIVE-<marker> <n>` that explicitly mentions the user:
//   1, 2 — written by someone else (a Slackbot reminder is enough);
//   3    — written by the user themself (it must never be listed).
//
// The rule's command is `claude`; the carrier's PATH and home are the
// operator's, so the operator decides what that is (a harmless stand-in).
// No prompt is sent to it: the head step waits at the first-interaction gate
// and nothing here grants a first send.

const MODE_TEMPLATE = 'mentions live';
const STEPS = ['Summarize {{msg.text}}', 'Report what changed'];
const BADGE = 'decklive';

export async function runMentionsLiveSmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { panes, provider, render } = await import('../js/board.js');
  const { newSession } = await import('../js/terminal.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { openSettings, renderInboundSettings, selectSettingsSection } = await import('../js/settings.js');
  const { openAutomations, closeAutomations } = await import('../js/automation.js');
  const { showMentions, refreshMentions } = await import('../js/slack-mentions.js');
  const { t } = await import('../js/i18n.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitFor = async (test, timeout = 8000, every = 250) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { if (await test()) return true; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return false;
      await pause(every);
    }
  };
  let failed = false;
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(a, 1), b });
    return ok;
  };
  const want = (n, seconds) => inv('ui_event', { code: 'smoke-check', detail: 'mentions-want', a: n, b: seconds });
  const screenOf = session => {
    const buffer = panes.get(session).term.buffer.active;
    return Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i).translateToString(true));
  };
  const queue = async () => {
    const q = await inv('queue_list');
    const items = q.items || [];
    return { rows: items.length, external: items.filter(item => item.external).length,
      authorized: items.filter(item => item.authority || item.readiness_override).length,
      deliveries: (q.deliveries || []).length };
  };
  const slackCards = () => store.cards.filter(card => card.origin?.source === 'slack');
  try {
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    state.projectId = project.id; state.view = 'board'; render();
    const word = await inv('smoke_native_scenario');
    const root = /^mentions-[0-9a-f]{12}$/.test(word) ? `/private/tmp/deck-${word}` : '';
    const idle = await inv('slack_mentions');
    if (!await report('mentions-clean', !!root && store.cards.length === 0 && !idle.enabled
      && $('mentions-btn').hidden && !(ctx.settings.inbound.rules || []).length)) throw new Error('root');

    // the operator's parameters, through a pane of this isolated server
    const reader = await newSession(root, { projectId: project.id,
      cmd: "cat params; printf '%s%s\\n' 'DECKLIVE-' 'END'", rethrow: true });
    const attached = await waitFor(() => panes.get(reader.session)?.attached, 15000);
    const printed = attached && await waitFor(() => screenOf(reader.session).includes('DECKLIVE-END'), 15000);
    let params = {};
    if (printed) {
      const lines = screenOf(reader.session);
      const hex = lines.slice(0, lines.lastIndexOf('DECKLIVE-END')).filter(line => /^[0-9a-f]{2,64}$/.test(line)).join('');
      params = JSON.parse(new TextDecoder().decode(Uint8Array.from(hex.match(/../g) || [], pair => parseInt(pair, 16))));
    }
    await inv('pty_write', { name: reader.session, gen: ctx.ptyGens.get(reader.session), dataB64: strToB64("clear; printf '\\033[3J'\r") }).catch(() => {});
    await pause(400);
    await provider.close(reader.id);
    await waitFor(() => store.cards.length === 0, 5000);
    const shaped = /^xoxp-/.test(params.user || '') && /^[UW][A-Z0-9]{8,}$/.test(params.me || '')
      && /^[a-z0-9]{6,16}$/.test(params.marker || '');
    if (!await report('mentions-params', shaped && store.cards.length === 0, Object.keys(params).length)) throw new Error('params');
    const marker = `DECKLIVE-${params.marker}`;

    // the user token through its Settings field, then the Slack switch
    await openSettings(); selectSettingsSection('integrations'); await renderInboundSettings();
    const box = $('set-inbound-slack-user');
    box.value = params.user;
    box.dispatchEvent(new Event('change'));
    params.user = '';
    const stored = await waitFor(() => !box.disabled && box.value === ''
      && box.placeholder === t('settings.inboundTokenSaved'), 45000);
    if (!ctx.settings.inbound.sources.slack.enabled) {
      $('set-inbound-slack').checked = true;
      $('set-inbound-slack').dispatchEvent(new Event('change'));
      // turning Slack on asks once, as it does for a person
      if (await waitFor(() => $('cfm').style.display === 'flex', 5000)) $('cfm-yes').click();
    }
    const on = await waitFor(() => ctx.settings.inbound.sources.slack.enabled === true, 15000);
    $('set-close').click();
    if (!await report('mentions-credentials', stored && on)) throw new Error('credentials');

    // one Slack badge rule, through the real editor
    await provider.saveTemplate(project.id, MODE_TEMPLATE, STEPS);
    await openAutomations({ trigger: 'slack' });
    $('auto-badge').value = BADGE;
    $('auto-dir').value = `${root}/work`; $('auto-cmd').value = 'claude';
    $('auto-template').value = MODE_TEMPLATE;
    $('auto-save').click();
    const saved = await waitFor(() => (ctx.settings.inbound.rules || []).some(rule => rule.source === 'slack' && rule.badge === BADGE), 15000);
    const rule = (ctx.settings.inbound.rules || []).find(r => r.source === 'slack' && r.badge === BADGE);
    // a second rule on a standard emoji, to see the menu show the emoji itself
    await openAutomations({ trigger: 'slack' });
    $('auto-badge').value = 'eyes';
    $('auto-dir').value = `${root}/work`; $('auto-cmd').value = 'claude';
    $('auto-template').value = MODE_TEMPLATE;
    $('auto-save').click();
    const second = await waitFor(() => (ctx.settings.inbound.rules || []).some(r => r.source === 'slack' && r.badge === 'eyes'), 15000);
    closeAutomations();
    if (!await report('mentions-rule', saved && second && rule?.cmd === 'claude' && !rule?.autoSend
      && rule?.firstSendWithoutReadiness !== true)) throw new Error('rule');

    // the entry appears once the first search ran
    const entry = await waitFor(async () => (await inv('slack_mentions')).lastPoll && !$('mentions-btn').hidden, 120000, 1000);
    const first = await inv('slack_mentions');
    if (!await report('mentions-entry', entry && first.enabled && !first.lastError, 1, first.items.length)) throw new Error('entry');
    showMentions();

    const listed = n => (lastView?.items || []).find(item => item.text.includes(`${marker} ${n}`));
    let lastView = null;
    const pull = async () => { await inv('slack_mentions_refresh'); await pause(11000); lastView = await inv('slack_mentions'); await refreshMentions(); return lastView; };
    const rowOf = key => [...$('mentions-list').children].find(node => node.dataset?.key === key);

    // 3 first (the user's own), then 1 (someone else's): when 1 is listed,
    // 3 has had at least as long to reach the index
    await want(3, 120);
    await pause(20000);
    await want(1, 300);
    const one = await waitFor(async () => { await pull(); return !!listed(1); }, 360000, 1000);
    const mention = listed(1);
    const row = mention && rowOf(mention.key);
    const counted = $('mentions-count').textContent === String(lastView.items.length);
    if (!await report('mentions-listed', one && !!row && counted && !listed(3), 1, lastView.items.length)) throw new Error('listed');

    // the reader pane: its context is read, names stand where Slack wrote ids
    row.click();
    const read = await waitFor(async () => { lastView = await inv('slack_mentions'); return !!listed(1)?.context; }, 90000, 2000);
    await refreshMentions();
    row.click();
    const context = listed(1)?.context;
    const pane = $('mentions-detail');
    const shown = !!pane.querySelector('.mention-message') && pane.textContent.includes(`${marker} 1`)
      && pane.textContent.includes(t('mentions.theMention'));
    await report('mentions-context', read && shown && context.before.length >= 1 && !context.text.includes(params.me)
      && !/^U[A-Z0-9]{6,}$/.test(listed(1).from), context?.before.length || 0, context?.thread ? 1 : 0);

    // route it with the row's own menu: the badge path makes the card
    const before = await queue();
    // the actions are in the reader pane of the selected mention; dropdown.js
    // wraps every <select> in its own control: find, never walk
    rowOf(mention.key).click();
    const menu = $('mentions-detail').querySelector('select.mention-route');
    const labels = [...(menu?.options || [])].map(option => option.textContent);
    await report('mentions-emoji', labels.some(label => label.startsWith('👀 → '))
      && labels.some(label => label.startsWith(`:${BADGE}: → `)), labels.length);
    menu.value = rule.id;
    menu.dispatchEvent(new Event('change'));
    const created = await waitFor(() => slackCards().length === 1 && slackCards()[0].inboundPlan?.initialQueued, 30000);
    const card = slackCards()[0];
    await pause(1500);
    const after = await queue();
    const gone = !rowOf(mention.key) && !(await inv('slack_mentions')).items.some(item => item.key === mention.key);
    await report('mentions-routed', created && gone && card.origin.key === mention.key && card.origin.badge === BADGE
      && after.rows - before.rows === STEPS.length && after.external - before.external === STEPS.length
      && after.authorized === 0 && after.deliveries === 0, slackCards().length, after.rows - before.rows);

    // once: the same mention is refused, and no second card appears
    let refused = '';
    try { await inv('slack_mention_route', { key: mention.key, rule: rule.id }); } catch (error) { refused = String(error); }
    await pause(3000);
    await report('mentions-once', refused === 'settled' && slackCards().length === 1);

    // 2: dismissed from its row; it stays gone after a fresh search
    await want(2, 300);
    const two = await waitFor(async () => { await pull(); return !!listed(2); }, 360000, 1000);
    const dismissed = listed(2);
    const dismissRow = dismissed && rowOf(dismissed.key);
    dismissRow?.click();
    $('mentions-detail').querySelector('.mention-dismiss')?.click();
    const hidden = two && await waitFor(() => !rowOf(dismissed.key), 5000);
    await pull();
    await report('mentions-dismissed', hidden && !listed(2) && slackCards().length === 1, 1, lastView.items.length);

    // the user's own message never showed up, through the whole run
    await pull();
    await report('mentions-self', !listed(3), 1, lastView.items.length);

    for (const value of [...store.cards]) await provider.close(value.id).catch(() => {});
    await waitFor(() => store.cards.length === 0, 10000);
    await report('done', !failed && store.cards.length === 0);
  } catch (error) {
    await inv('ui_event', { code: 'smoke-check', detail: 'mentions-failed', a: -1, b: 0 });
    await report('done', false);
  }
}
