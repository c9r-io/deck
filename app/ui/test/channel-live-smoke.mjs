// Real-service acceptance for the first-party Slack integration inside the
// real WKWebView: a sandbox Slack app, its real Socket Mode connection, real
// messages. Debug carrier only, on an isolated data root and tmux socket.
//
// Nothing here seeds a credential behind the product's back. The two tokens
// go in through the Settings fields, the way a user pastes them: the page
// sets the field and fires its `change`, and the production handler stores
// them (`inbound_set_secret`, verified with Slack, process-local in a smoke
// launch). The rule is saved by the real Automations editor. What the page
// cannot know by itself — the tokens, the test channel, the sender — comes
// from one private file of the operator's, read through a card's own pane:
// `<root>/params`, hex in lines of at most 64 characters, where `<root>` is
// `/private/tmp/deck-live-<12 hex>` and the launch's scenario word
// (`translation-fixture/scenario`) is `live-<12 hex>`. The card is closed
// as soon as the file has been read. `<root>/work` is the rule's directory.
//
// The Slack side is the operator's (a person or a driver following app.log):
// each `live-want a=<n>` asks for ONE top-level message `DECKLIVE-<marker>
// <n>` in the test channel from the allowed sender, within the b seconds it
// names. Messages 1, 2, 4 and 5 must arrive; 3 is asked for while the
// connection is down and may or may not be delivered later by Slack's own
// retry (`live-late` records which; Deck backfills nothing either way).
//
// A matching message starts the rule's command in the new card. The rule's
// command is the real `claude`: no prompt is sent to it (the head step waits
// at the first-interaction gate and nothing here grants a first send).
//
// The faults are this process's own debug hooks (`smoke_fault_set`): they end
// or fail only this app's connection and write nothing to Slack.

const MODE_TEMPLATE = 'channel live';
const STEPS = ['Summarize {{msg.text}}', 'Report what changed'];

async function harness() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { panes, provider, render } = await import('../js/board.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitFor = async (test, timeout = 8000) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { if (await test()) return true; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return false;
      await pause(250);
    }
  };
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(a, 1), b });
    return ok;
  };
  const channelCards = () => store.cards.filter(card => card.origin?.source === 'channel');
  const entries = card => (card?.buffer?.entries || []).filter(entry => entry.kind === 'external');
  /* every fact a fault, a recovery or a replay must leave alone: no first-send
     choice or grant on the rule, no authority on any queue row */
  const authority = async () => {
    const queue = await inv('queue_list');
    const rules = ctx.settings.inbound.channelRules || [];
    return {
      grants: rules.filter(rule => rule.firstSend || rule.firstSendGrant).length,
      rows: (queue.items || []).length,
      external: (queue.items || []).filter(item => item.external).length,
      authorized: (queue.items || []).filter(item => item.authority || item.channel_first_send?.authorized
        || item.readiness_override).length,
      deliveries: (queue.deliveries || []).length,
    };
  };
  return { $, ctx, inv, state, store, panes, provider, render, pause, waitFor, report, channelCards, entries,
    authority, failed: () => failed };
}

export async function runChannelLiveSmoke() {
  const h = await harness();
  const { $, ctx, inv, state, store, panes, provider, render, pause, waitFor, report, channelCards, entries,
    authority } = h;
  const { newSession } = await import('../js/terminal.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { openSettings, renderInboundSettings, selectSettingsSection } = await import('../js/settings.js');
  const { openAutomations } = await import('../js/automation.js');
  const { startInbound } = await import('../js/inbound.js');
  const { t } = await import('../js/i18n.js');
  let armed = '';
  const settingsText = async () => {
    await renderInboundSettings();
    return { transport: $('set-slack-transport').textContent, channel: $('set-channel-status').textContent };
  };
  const want = async (n, seconds) => {
    await inv('ui_event', { code: 'smoke-check', detail: 'live-want', a: n, b: seconds });
  };
  try {
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    state.projectId = project.id; state.view = 'board'; render();
    const word = await inv('smoke_native_scenario');
    const root = /^live-[0-9a-f]{12}$/.test(word) ? `/private/tmp/deck-${word}` : '';
    const idle = await inv('slack_connection_status');
    const quiet = await inv('channel_status');
    if (!await report('live-clean', !!root && store.cards.length === 0 && !idle.botPresent && !idle.appPresent
      && !idle.userPresent && !quiet.connected && !(ctx.settings.inbound.channelRules || []).length)) throw new Error('root');

    // the operator's parameters, through a pane of this isolated server
    const reader = await newSession(root, { projectId: project.id,
      cmd: "cat params; printf '%s%s\\n' 'DECKLIVE-' 'END'", rethrow: true });
    const attached = await waitFor(() => panes.get(reader.session)?.attached, 15000);
    const screen = () => {
      const buffer = panes.get(reader.session).term.buffer.active;
      return Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i).translateToString(true));
    };
    const printed = attached && await waitFor(() => screen().includes('DECKLIVE-END'), 15000);
    let params = {};
    if (printed) {
      const lines = screen();
      const hex = lines.slice(0, lines.lastIndexOf('DECKLIVE-END')).filter(line => /^[0-9a-f]{2,64}$/.test(line)).join('');
      params = JSON.parse(new TextDecoder().decode(Uint8Array.from(hex.match(/../g) || [], pair => parseInt(pair, 16))));
    }
    await inv('pty_write', { name: reader.session, gen: ctx.ptyGens.get(reader.session), dataB64: strToB64("clear; printf '\\033[3J'\r") }).catch(() => {});
    await pause(400);
    await provider.close(reader.id);
    await waitFor(() => store.cards.length === 0, 5000);
    const shaped = /^xoxb-/.test(params.bot || '') && /^xapp-/.test(params.app || '')
      && /^[CG][A-Z0-9]{8,}$/.test(params.channel || '') && /^[UW][A-Z0-9]{8,}$/.test(params.sender || '')
      && /^[a-z0-9]{6,16}$/.test(params.marker || '');
    if (!await report('live-params', shaped && store.cards.length === 0, Object.keys(params).length)) throw new Error('params');

    // the two tokens, through the Settings fields and their own handler
    await openSettings(); selectSettingsSection('integrations'); await renderInboundSettings();
    const enter = async (id, value) => {
      const box = $(id);
      box.value = value;
      box.dispatchEvent(new Event('change'));
      return waitFor(() => !box.disabled && box.value === ''
        && box.placeholder === t('settings.inboundTokenSaved'), 45000);
    };
    const appStored = await enter('set-inbound-slack-app', params.app);
    $('set-channel-upgrade').click();
    const botShown = !$('set-channel-upgrade-steps').hidden;
    const botStored = botShown && await enter('set-channel-bot', params.bot);
    params.app = ''; params.bot = '';
    const verified = await inv('slack_connection_status');
    if (!await report('live-credentials', appStored && botStored && verified.botValid && verified.appPresent
      && verified.workspaceMatch && !verified.userPresent, 1, 1)) throw new Error('credentials');

    // the rule, through the real editor; then the channel switch
    $('set-close').click();
    await provider.saveTemplate(project.id, MODE_TEMPLATE, STEPS);
    await openAutomations({ trigger: 'channel' });
    $('auto-channel-ids').value = params.channel;
    $('auto-sender-users').value = params.sender;
    $('auto-match-kind').value = 'contains'; $('auto-match-kind').dispatchEvent(new Event('change'));
    $('auto-match-value').value = `DECKLIVE-${params.marker}`;
    $('auto-dir').value = `${root}/work`; $('auto-cmd').value = 'claude';
    $('auto-idle').value = '30'; $('auto-template').value = MODE_TEMPLATE;
    $('auto-save').click();
    const saved = await waitFor(() => ctx.settings.inbound.channelRules?.length === 1, 15000);
    const rule = ctx.settings.inbound.channelRules?.[0];
    await openSettings(); selectSettingsSection('integrations'); await renderInboundSettings();
    $('set-channel-enabled').checked = true;
    $('set-channel-enabled').dispatchEvent(new Event('change'));
    const enabled = await waitFor(() => ctx.settings.inbound.channelConnection?.enabled === true, 15000);
    if (!await report('live-rule', saved && enabled && rule?.channelIds?.[0] === params.channel
      && rule?.senderUserIds?.[0] === params.sender && rule?.cmd === 'claude'
      && !rule?.firstSend && !rule?.firstSendGrant)) throw new Error('rule');

    await startInbound();
    const connected = await waitFor(async () => (await inv('channel_status')).connected, 90000);
    const first = await inv('channel_status');
    const up = await settingsText();
    if (!await report('live-connected', connected && !first.gapUnresolved && !first.lastError
      && up.transport === t('settings.slackSocketConnected')
      && !up.channel.includes(t('settings.channelStatus.disconnected')))) throw new Error('connect');
    const before = await authority();
    // How often message n is on the card. Messages are counted one by one,
    // never as a total: Slack may deliver message 3 by its own retry at any
    // later moment, and that must not look like a duplicate of another.
    const times = n => entries(channelCards()[0])
      .filter(entry => (entry.text || '').startsWith(`DECKLIVE-${params.marker} ${n} `)).length;
    const once = (...numbers) => numbers.every(n => times(n) === 1);
    const drained = async () => (await inv('channel_pending')).length === 0;

    // 1: one message, one card, nothing pending, the plan queued as external rows
    await want(1, 180);
    const created = await waitFor(() => channelCards().length === 1 && channelCards()[0].channelRun?.initialQueued, 180000);
    await pause(1500);
    const card = channelCards()[0];
    const afterFirst = await authority();
    await report('live-card', created && await drained() && channelCards().length === 1 && card.origin.badge === rule.id
      && once(1) && entries(card).length === 1 && afterFirst.rows === STEPS.length
      && afterFirst.external === STEPS.length && afterFirst.authorized === 0 && afterFirst.deliveries === 0,
    channelCards().length, afterFirst.rows);

    // 2: a second message of the same run joins the card; no second card
    await want(2, 180);
    const joined = await waitFor(() => times(2) === 1, 180000);
    await pause(1500);
    await report('live-append', joined && channelCards().length === 1 && once(1, 2) && entries(channelCards()[0]).length === 2
      && await drained() && (await authority()).rows === STEPS.length,
    channelCards().length, entries(channelCards()[0]).length);

    // a fault after the connection was up: said as it is, where the user looks
    armed = 'channel-network';
    await inv('smoke_fault_set', { kind: armed, count: 8 });
    const down = await waitFor(async () => {
      const status = await inv('channel_status');
      return !status.connected && status.lastError === 'network' && status.gapUnresolved;
    }, 20000);
    const broken = await settingsText();
    await report('live-fault', down && broken.transport === t('settings.slackSocketDisconnected')
      && broken.channel.includes(t('settings.channelStatus.disconnected'))
      && broken.channel.includes(t('settings.channelGap'))
      && broken.channel.includes(t('settings.inboundStatus.error', { code: 'network' })));
    // 3: sent while nothing is connected; the fault is kept armed for the whole minute
    await want(3, 60);
    for (let slice = 0; slice < 6; slice++) {
      await inv('smoke_fault_set', { kind: armed, count: 8 });
      await pause(10000);
    }
    const missed = times(3) === 0 && entries(channelCards()[0]).length === 2 && await drained();
    await inv('smoke_fault_set', { kind: armed, count: 0 });
    armed = '';
    const back = await waitFor(async () => (await inv('channel_status')).connected, 60000);
    const status = await inv('channel_status');
    const mended = await settingsText();
    await report('live-recovered', back && missed && status.gapUnresolved && !status.lastError
      && mended.transport === t('settings.slackSocketConnected')
      && mended.channel.includes(t('settings.channelGap'))
      && !mended.channel.includes(t('settings.channelStatus.disconnected')),
    1, status.pendingCount || 0);

    // 4: a message after the recovery is handled like any other
    await want(4, 180);
    const resumed = await waitFor(() => times(4) === 1, 180000);
    // whether Slack's own retry delivers message 3 later is Slack's; Deck asks for no history
    await pause(75000);
    await report('live-late', times(3) <= 1, times(3) === 1 ? 2 : 1, entries(channelCards()[0]).length);
    await report('live-resume', resumed && once(1, 2, 4) && channelCards().length === 1 && await drained()
      && (await inv('channel_status')).gapUnresolved, channelCards().length, entries(channelCards()[0]).length);

    // 5: an event that could not be made durable is not acknowledged; Slack
    //    sends it again and it is taken exactly once
    const linked = (await inv('channel_status')).lastConnected || 0;
    armed = 'channel-inbox-save';
    await inv('smoke_fault_set', { kind: armed, count: 1 });
    await want(5, 240);
    // the refused stage ends the connection with `capacity` (seen as that
    // error, or as the reconnect a second later that clears it)
    const refused = await waitFor(async () => {
      const now = await inv('channel_status');
      return now.lastError === 'capacity' || (now.lastConnected || 0) > linked;
    }, 240000);
    const again = await waitFor(() => times(5) === 1, 420000);
    await inv('smoke_fault_set', { kind: armed, count: 0 });
    armed = '';
    await pause(5000);
    await report('live-redelivery', refused && again && once(1, 2, 4, 5) && times(3) <= 1
      && channelCards().length === 1 && await drained(), 1, entries(channelCards()[0]).length);

    // faults, recovery and redelivery granted nothing
    const after = await authority();
    const final = ctx.settings.inbound.channelRules?.[0];
    await report('live-authority', before.grants === 0 && after.grants === 0 && after.authorized === 0
      && after.deliveries === 0 && after.rows === STEPS.length && after.external === STEPS.length
      && !final?.firstSend && !final?.firstSendGrant, 1, after.rows);
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    if (armed) await inv('smoke_fault_set', { kind: armed, count: 0 }).catch(() => {});
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 19, b: 0 });
    await report('done', false, 0, 19);
  }
}

// The same root, launched again with no credentials: what was applied stays
// applied. No connection can exist (a smoke launch forgets its tokens), so
// this is the durable state alone: one card, the same messages on it, the
// same external rows, the unresolved gap still said, nothing granted.
export async function runChannelLiveReplaySmoke() {
  const h = await harness();
  const { $, ctx, inv, pause, waitFor, report, channelCards, entries, authority } = h;
  const { openSettings, renderInboundSettings, selectSettingsSection } = await import('../js/settings.js');
  const { startInbound } = await import('../js/inbound.js');
  const { t } = await import('../js/i18n.js');
  try {
    const loaded = await waitFor(() => channelCards().length === 1, 20000);
    const count = entries(channelCards()[0]).length;
    await startInbound();
    await pause(8000); // the launch's own drains and one re-announcement
    const idle = await inv('slack_connection_status');
    const status = await inv('channel_status');
    const facts = await authority();
    await openSettings(); selectSettingsSection('integrations'); await renderInboundSettings();
    const text = $('set-channel-status').textContent;
    const rule = ctx.settings.inbound.channelRules?.[0];
    await report('live-replay', loaded && channelCards().length === 1 && entries(channelCards()[0]).length === count
      && count >= 4 && !idle.botPresent && !idle.appPresent && !status.connected && status.gapUnresolved
      && (await inv('channel_pending')).length === 0 && text.includes(t('settings.channelGap'))
      && facts.rows === STEPS.length && facts.external === STEPS.length && facts.authorized === 0
      && facts.deliveries === 0 && facts.grants === 0 && !rule?.firstSendGrant, channelCards().length, count);
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 19, b: 0 });
    await report('done', false, 0, 19);
  }
}
