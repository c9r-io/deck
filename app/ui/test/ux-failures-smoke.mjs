// ux-failures-smoke.mjs — the `ux-failures` WKWebView smoke: each place a
// refused credential or a failed check now says why (518a36fa, 70b0798b),
// shown in the real webview and kept as evidence/uxf-*.png. A failure of an
// outside service cannot be produced in an isolated run (no Slack workspace,
// no provider key, no Tunnel helper, no update feed), so those backend
// replies are put in front of the page's ONE invoke for the named commands
// only: what is judged is the real Settings and dialog code laying out the
// replies the backend tests pin. The MCP credential reissue is the real
// backend (an isolated run keeps the bearer under its own data root).

export async function runUxFailuresSmoke() {
  const { $, ctx, inv, store } = await import('../js/state.js');
  const { openSettings, renderInboundSettings, renderConnectorSettings, renderMcpSettings } = await import('../js/settings.js');
  const { extensionController } = await import('../js/settings-extensions.js');
  const { cfmDone } = await import('../js/dialogs.js');
  const { openProjectDefaults } = await import('../js/board.js');
  const { manualUpdateCheck } = await import('../js/app.js');
  const { t } = await import('../js/i18n.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  let failed = false;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -a, b });
  };
  const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
  const shown = el => !!el && el.getClientRects().length > 0 && el.getBoundingClientRect().height > 0;
  const textOf = id => $(id)?.textContent || '';
  const show = async (id, setting) => {
    await openSettings({ setting });
    await pause(400);
    $(id)?.scrollIntoView({ block: 'center' });
    await pause(300);
  };

  // Tauri may freeze its `core`; then the whole global is swapped for one
  // whose invoke answers the named commands and hands the rest on.
  const original = window.__TAURI__;
  const real = original.core.invoke.bind(original.core);
  const replies = new Map();
  const invoke = (cmd, args) => (replies.has(cmd) ? replies.get(cmd)(args) : real(cmd, args));
  const how = [];
  try { original.core.invoke = invoke; } catch (_) {}
  if (window.__TAURI__.core.invoke !== invoke) {
    how.push(1);
    const swapped = { ...original, core: { ...original.core, invoke } };
    try { window.__TAURI__ = swapped; } catch (_) {}
    if (window.__TAURI__ !== swapped) {
      how.push(2);
      try { Object.defineProperty(window, '__TAURI__', { value: swapped, configurable: true, writable: true }); } catch (_) { how.push(4); }
    }
  }
  const restore = () => {
    try { original.core.invoke = real; } catch (_) {}
    if (window.__TAURI__ !== original) {
      try { Object.defineProperty(window, '__TAURI__', { value: original, configurable: true, writable: true }); } catch (_) {}
    }
  };
  replies.set('uxf_probe', async () => 'ok');
  const intercepted = (await inv('uxf_probe').catch(() => '')) === 'ok';
  await report('uxf-intercept', intercepted, 1, how.reduce((a, b) => a + b, 0));
  if (!intercepted) { await report('done', false); return; }

  try {
    // Slack: a refused user token (poll + reaction line) and a refused App token (socket line)
    // the `scope` case of fixtures/channel-fault-status.json (a fetch of a fixture is not served in the app)
    const vector = { scope: {"connection": {"userPresent": false, "botPresent": true, "appPresent": true, "legacyPresent": false, "userValid": false, "botValid": true, "appValid": false, "userError": null, "botError": null, "appError": "scope", "workspaceMatch": true, "workspace": null, "connected": false, "reactionEnabled": false, "channelEnabled": true, "channelRules": 1, "appId": "A1", "workspaceId": "T1"}, "channel": {"enabled": true, "connected": false, "tokenReady": true, "pendingCount": 1, "lastConnected": 1, "gapSince": 1, "gapUnresolved": true, "rejectedCount": 0, "lastError": "scope"}} };
    const now = Math.floor(Date.now() / 1000);
    replies.set('inbound_status', async () => ({ sources: [{ id: 'slack', live: false, lastPoll: now - 120, lastError: 'auth',
      secrets: [{ slot: 'slack-user-token', present: true }, { slot: 'slack-app-token', present: true }] }] }));
    replies.set('slack_connection_status', async () => ({ ...vector.scope.connection, userPresent: true, userValid: false,
      userError: 'auth', reactionEnabled: true }));
    replies.set('channel_status', async () => vector.scope.channel);
    // both halves switched on in memory only (no settings write), as a user with Slack set up has them
    const savedInbound = ctx.settings.inbound;
    ctx.settings.inbound = { ...savedInbound, sources: { ...savedInbound.sources, slack: { enabled: true } },
      channelConnection: { ...(savedInbound.channelConnection || {}), enabled: true, connectionId: 'default' } };
    await show('set-inbound-status', 'slack-reactions');
    await renderInboundSettings();
    await pause(300);
    $('set-inbound-status')?.scrollIntoView({ block: 'center' });
    await pause(200);
    const auth = t('settings.slackFailure.auth'); const scope = t('settings.slackFailure.scope');
    const slackOk = textOf('set-slack-transport').includes(scope) && textOf('set-slack-reaction-ready').includes(auth);
    await snapshot('uxf-slack');
    ctx.settings.inbound = savedInbound;
    await report('uxf-slack', slackOk, 1, (textOf('set-slack-transport').includes(scope) ? 0 : 1)
      + (textOf('set-slack-reaction-ready').includes(auth) ? 0 : 2) + (textOf('set-inbound-status').includes(auth) ? 0 : 4));

    // Model APIs: a key a turn summary found refused
    replies.set('model_api_status', async () => ({ openai: { saved: false, check: null },
      anthropic: { saved: true, check: { ok: false, code: 'auth', at: now - 300 } } }));
    await show('set-model-anthropic', 'model-api');
    await extensionController('model-api').refresh();
    await pause(300);
    const modelOk = textOf('set-model-anthropic-state').includes(t('error.modelApiAuth'));
    await snapshot('uxf-model-api');
    await report('uxf-model-api', modelOk);

    // Phone Connector: the Keychain refused the identity
    replies.set('connector_status', async () => ({ ...(await real('connector_status').catch(() => ({}))),
      enabled: true, running: false, refusal: 'keychain' }));
    await show('set-connector-status', 'connector');
    await renderConnectorSettings();
    await pause(300);
    $('set-connector-status')?.scrollIntoView({ block: 'center' });
    await pause(200);
    await snapshot('uxf-connector');
    await report('uxf-connector', textOf('set-connector-status') === t('connector.refused.keychain') && shown($('set-connector-status')));

    // Updates: a feed this build cannot read
    replies.set('check_for_update', async () => { throw 'update check failed: feed-invalid'; });
    await show('set-upd-status', 'updates');
    await manualUpdateCheck();
    await pause(300);
    await snapshot('uxf-update');
    await report('uxf-update', textOf('set-upd-status') === t('update.checkFailedInvalid'));

    // MCP: a real client, its Tunnel row after a start that timed out, and a real reissue
    if (!(await real('mcp_status')).enabled) await real('mcp_enable');
    const client = await real('mcp_client_add', { name: 'Tunnel client', projects: [],
      permissions: { allowCreate: false, allowAutomations: false, allowWorkers: false, allowReplies: false, workerFirstSend: false,
        allowDrafts: false, allowMentions: false, ruleColumn: false, workerTypes: [], allProjects: true } });
    replies.set('tunnel_helper_status', async () => ({ helperState: 'installed', tunnelState: 'starting', developmentHelper: false,
      tunnelId: null, failedAction: 'start', failureCode: 'tunnel_client_readiness_timeout' }));
    await show('set-mcp-clients', 'mcp');
    await renderMcpSettings();
    await pause(500);
    const row = () => [...document.querySelectorAll('#set-mcp-clients .mcp-client-row')]
      .find(el => el.textContent.includes('Tunnel client'));
    row()?.scrollIntoView({ block: 'center' });
    await pause(200);
    const failureLine = row()?.querySelector('.mcp-tunnel-failure');
    const buttons = [...(row()?.querySelectorAll('button') || [])].map(b => b.textContent);
    await snapshot('uxf-mcp-row');
    await report('uxf-tunnel', shown(failureLine) && buttons.includes(t('mcp.tunnelResetup')), 1,
      (shown(failureLine) ? 0 : 1) + (buttons.includes(t('mcp.tunnelResetup')) ? 0 : 2));
    const reissue = [...(row()?.querySelectorAll('button') || [])].find(b => b.textContent === t('mcp.reissue'));
    reissue?.click();
    await pause(400);
    await snapshot('uxf-mcp-reissue-confirm');
    const asked = $('cfm').style.display === 'flex';
    cfmDone(true);
    await pause(800);
    const toastShown = [...document.querySelectorAll('#toasts .toast')]
      .some(el => el.textContent.includes(t('mcp.reissued', { name: 'Tunnel client' })));
    await snapshot('uxf-mcp-reissued');
    const after = (await real('mcp_status')).clients.find(c => c.id === client.id);
    await report('uxf-mcp-reissue', !!reissue && asked && !!after && !after.revoked, 1,
      (reissue ? 0 : 1) + (asked ? 0 : 2) + (after && !after.revoked ? 0 : 4) + (toastShown ? 0 : 8));
    replies.delete('tunnel_helper_status');
    await real('mcp_client_revoke', { clientId: client.id });
    await real('mcp_client_delete', { clientId: client.id });
    $('set-close').click();
    await pause(300);

    // Project environment: a stored environment that cannot be read offers the two-click clear
    replies.set('project_env_names', async () => { throw 'project environment is invalid'; });
    const project = store.projects[0];
    const done = openProjectDefaults(project.id);
    await pause(700);
    const clear = $('pdf-env-clear');
    clear?.scrollIntoView({ block: 'center' });
    await pause(200);
    await snapshot('uxf-project-env');
    await report('uxf-project-env', shown(clear) && clear.textContent === t('projectEnv.clear'));
    $('pdf-no').click();
    await done.catch(() => {});
  } catch (error) {
    failed = true;
    await snapshot('uxf-exception');
    await report('uxf-exception', false, 1, String(error?.message || error).length);
    await inv('ui_event', { code: 'smoke-check', detail: 'uxf-exception', a: -2, b: Number(String(error?.stack || '').match(/ux-failures-smoke\.mjs:(\d+)/)?.[1] || 0) });
  } finally {
    restore();
  }
  await report('done', !failed);
}
