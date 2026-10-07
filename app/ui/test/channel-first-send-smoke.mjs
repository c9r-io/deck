// Real background acceptance for a channel monitor's first-send choice.
// This runs only in the isolated debug WKWebView carrier. It saves the rule
// through the native settings authority, starts the production inbound drain,
// hides Deck, then makes one IPC call whose backend waits at least 245 seconds
// before parsing/matching/staging a synthetic Slack envelope. No frontend
// timer, poll, snapshot, activation or input runs during that hidden wait.
//
// The target is the harmless no-network `claude` fixture installed by
// app/run.sh in an isolated PATH. It emits no Agent Signal, so the granted
// head step may be sent after compatibility stabilization while step two
// remains held by the production scheduler.
export async function runChannelFirstSendSmoke() {
  const { ctx, inv, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { persistInbound } = await import('../js/settings.js');
  const { startInbound } = await import('../js/inbound.js');
  let failed = false;
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(a, 1), b });
  };
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    const fixtureBefore = await inv('smoke_channel_fixture');
    await report('channel-bg-preflight', fixtureBefore.available && fixtureBefore.environmentPrivate
      && fixtureBefore.keychainBlocked && fixtureBefore.credentialsCleared
      && !fixtureBefore.ready && fixtureBefore.receipts === 0);

    const project = provider.projects()[0];
    const column = project.columns[0];
    const template = 'channel first send';
    await provider.saveTemplate(project.id, template,
      ['CHANNEL-FIRST-SEND-STEP-1', 'CHANNEL-FIRST-SEND-STEP-2']);
    const identity = await inv('channel_smoke_identity');
    if (identity.workspaceId !== 'TSMOKE' || identity.ownUserId !== 'USMOKE') throw new Error('smoke identity');
    const rule = {
      id: 'channel-first-send', enabled: true, connectionId: 'default',
      channelIds: ['CSMOKE'], senderUserIds: ['UEXTERNAL'], senderBotIds: [],
      match: { kind: 'contains', value: 'CHANNEL-FIRST-SEND', caseSensitive: true },
      includeThreads: true, firstSend: true,
      projectId: project.id, columnId: column.id, dir: ctx.HOME, cmd: 'claude', template, idleMinutes: 0,
    };
    const saved = await persistInbound({ ...ctx.settings.inbound,
      channelConnection: { enabled: true, connectionId: 'default' }, channelRules: [rule] },
    [{ ruleId: rule.id, external: true }]);
    const granted = ctx.settings.inbound.channelRules.find(value => value.id === rule.id);
    const diskSettings = JSON.parse((await inv('load_settings')).data);
    const diskRule = diskSettings.inbound.channelRules.find(value => value.id === rule.id);
    await report('channel-bg-config', saved && granted?.firstSend === true
      && /^[a-f0-9]{64}$/.test(granted?.firstSendGrant?.digest || '')
      && diskRule?.firstSendGrant?.digest === granted.firstSendGrant.digest);

    // Explicitly start the production listeners/drains after the durable rule.
    await startInbound();
    // Let app.js's one four-second startup update check finish (the native
    // smoke gate returns before network) before the measured hidden period.
    await new Promise(resolve => setTimeout(resolve, 5000));
    await inv('smoke_native_input', { input: { kind: 'hide' } });
    // AppKit hide completes on a later main-loop turn. This single state
    // sample precedes the measured native-only hidden interval.
    await new Promise(resolve => setTimeout(resolve, 1000));
    const hiddenState = await inv('smoke_native_input', { input: { kind: 'state' } });
    await report('channel-bg-hidden', (hiddenState & 4) === 4);
    if ((hiddenState & 4) !== 4) throw new Error('Deck did not hide');

    const envelope = JSON.stringify({ envelope_id: 'ENV_CHANNEL_FIRST_SEND', type: 'events_api', payload: {
      team_id: 'TSMOKE', api_app_id: 'ASMOKE', event_id: 'EV_CHANNEL_FIRST_SEND',
      event: { type: 'message', channel: 'CSMOKE', user: 'UEXTERNAL',
        text: 'CHANNEL-FIRST-SEND synthetic fixture event' },
    } });
    // NOTHING may be added between this await and its completion: the native
    // command owns the monotonic hidden wait, stamps/injects the envelope only
    // after it expires, then keeps this promise pending until its native-only
    // fixture/queue oracle has observed delivery and the later-row hold.
    const staged = await inv('channel_smoke_envelope', { envelope, delaySecs: 245 });
    await report('channel-bg-envelope', staged.waitedSecs >= 245 && staged.disposition === 'ack', staged.waitedSecs);

    // The awaited native oracle has already seen the durable delivery and a
    // 75-second later-row hold. These are single post-oracle reads, not polls.
    const card = store.cards.find(value => value.origin?.source === 'channel');
    if (!card) throw new Error('channel card unavailable after native oracle');
    const queue = await inv('queue_list');
    const fixture = await inv('smoke_channel_fixture');
    const ownItems = (queue.items || []).filter(value => value.session === card.session);
    const ownDeliveries = (queue.deliveries || []).filter(value => value.session === card.session);
    const ownIds = new Set(ownItems.map(value => value.id));
    const ownPlans = (queue.plans || []).filter(value => ownIds.has(value.item));
    await report('channel-bg-delivered', staged.delivered === true && ownDeliveries.length === 1
      && ownDeliveries[0].automatic === true && ownDeliveries[0].readiness_overridden === true
      && !!ownDeliveries[0].channel_first_send);
    await report('channel-bg-fixture', staged.receipts === 1 && fixture.ready && fixture.alive
      && fixture.receipts === 1 && fixture.firstMatches && !fixture.secondSeen);
    await report('channel-bg-held', staged.held === true && ownItems.length === 1
      && ownItems[0].mode === 'chain' && ownDeliveries.length === 1
      && fixture.receipts === 1 && !fixture.secondSeen && ownPlans.length === 1
      && ['external', 'first-send'].includes(ownPlans[0].stage));
    await report('channel-bg-persisted', staged.persistedItems === 1 && staged.persistedDeliveries === 1
      && staged.persistedOverrides === 1 && fixture.persistedItems === 1
      && fixture.persistedDeliveries === 1 && fixture.persistedOverrides === 1);
  } catch (_) {
    await report('channel-bg-exception', false);
  }
  await report('done', !failed);
}
