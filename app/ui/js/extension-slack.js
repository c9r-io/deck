// Slack's first-party custom Settings contribution. Account connection,
// credentials and Channel Monitoring remain one panel and one mount lifecycle.
// Controller owns feature state and actions; the host retains the one settings writer.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { slackConnectionView } from './slack-connection-model.js';
import { createSlackChannelSetup, mergeSlackChannelManifest, slackAppSettingsUrl } from './slack-setup-model.js';
import { removeLegacySlackCredentials } from './slack-legacy-cleanup.js';

const PANEL = `<div class="set-group" id="set-item-slack-reactions" data-setting-id="slack-reactions" tabindex="-1" role="group" aria-labelledby="set-item-slack-reactions-title">
<div class="set-group-head"><h3 class="set-group-title" id="set-item-slack-reactions-title" data-i18n="settings.slack">Slack</h3><span class="set-group-status" id="set-inbound-status"></span><button class="btn" id="set-inbound-check" data-i18n="settings.inboundCheckNow">Check now</button></div>
<div class="set-hint" data-i18n="settings.slackHint">A badge you put on a Slack message opens a card through the project’s automations (↻ on the Board). deck never writes to Slack.</div>
<div class="set-row"><label for="set-inbound-slack" data-i18n="settings.slackConnect">Connect</label><input type="checkbox" id="set-inbound-slack"><button class="btn" id="set-inbound-setup" style="margin-left:auto" data-i18n="settings.inboundSetup">Create the Slack app…</button></div>
<details class="set-details"><summary data-i18n="settings.details">Learn more</summary><div class="set-hint" data-i18n="settings.inboundSetupHint">New users create one Deck Slack app with the unified manifest. Existing Reaction users update their existing app. Install or reinstall it, then copy only the credentials needed for your capabilities.</div></details>
<div class="set-row"><label for="set-inbound-slack-user" data-i18n="settings.inboundUserToken">User token</label><input type="password" id="set-inbound-slack-user" autocomplete="off" spellcheck="false" placeholder="xoxp-…"><button class="btn" id="set-inbound-slack-user-clear" data-i18n="common.clear">Clear</button></div>
<div class="set-row"><label for="set-inbound-slack-app" data-i18n="settings.inboundAppToken">App token</label><input type="password" id="set-inbound-slack-app" autocomplete="off" spellcheck="false" placeholder="xapp-…"><button class="btn" id="set-inbound-slack-app-clear" data-i18n="common.clear">Clear</button></div>
<div class="set-hint" id="set-slack-workspace"></div><div class="set-hint" id="set-slack-transport"></div><div class="set-hint" id="set-slack-reaction-ready"></div><div class="set-hint" id="set-channel-status"></div><div class="set-hint" id="set-slack-legacy" hidden></div>
<button class="btn" id="set-slack-legacy-clear" data-i18n="settings.channelLegacyClear" hidden>Remove legacy Slack credentials</button>
<div class="set-row"><label for="set-channel-enabled" data-i18n="settings.channelMonitor">Channel monitoring</label><input type="checkbox" id="set-channel-enabled"><button class="btn" id="set-channel-upgrade" data-i18n="settings.channelEnable">Enable Channel Monitoring</button></div>
<div id="set-channel-upgrade-steps" class="slack-setup" hidden><div class="set-hint" id="set-channel-upgrade-hint" data-i18n="settings.channelUpgradeSteps"></div><p id="set-channel-setup-feedback" role="status" aria-live="polite" tabindex="-1"></p>
<h4 data-i18n="settings.channelStepApp">1. Update the existing Slack app</h4><div class="set-hint" id="set-channel-app-location"></div>
<div class="set-row"><label for="set-channel-app-id" data-i18n="settings.channelAppId">App ID (optional)</label><input id="set-channel-app-id" autocomplete="off" spellcheck="false" placeholder="A…"><button class="btn" id="set-channel-open-app" data-i18n="settings.channelOpenApp">Open Slack app settings</button></div>
<div class="set-hint" data-i18n="settings.channelManifestHelp"></div><label for="set-channel-existing-manifest" data-i18n="settings.channelExistingManifest">Current App Manifest (JSON)</label><textarea id="set-channel-existing-manifest" spellcheck="false" maxlength="131072" rows="4"></textarea>
<button class="btn" id="set-channel-manifest" data-i18n="settings.channelManifest">Prepare, copy and open Slack</button><textarea id="set-channel-manifest-text" aria-label="Updated App Manifest" readonly spellcheck="false" rows="4" hidden></textarea>
<h4 data-i18n="settings.channelStepToken">2. Reinstall and copy the Bot token</h4><div class="set-hint" data-i18n="settings.channelReinstallHelp"></div><button class="btn" id="set-channel-open-oauth" data-i18n="settings.channelOpenOAuth">Open OAuth &amp; Permissions</button>
<h4 data-i18n="settings.channelStepEnable">3. Paste the token and enable</h4><div class="set-row"><label for="set-channel-bot" data-i18n="settings.channelBotToken">Bot token</label><input type="password" id="set-channel-bot" autocomplete="off" spellcheck="false" placeholder="xoxb-…"><button class="btn" id="set-channel-bot-clear" data-i18n="common.clear">Clear</button></div>
<div class="set-hint" id="set-channel-ids"></div><div class="set-hint" data-i18n="settings.channelMembershipHelp"></div><div class="set-row"><button class="btn" id="set-channel-finish" data-i18n="settings.channelFinish">Verify and enable channel monitoring</button><button class="btn" id="set-channel-cancel" data-i18n="common.cancel">Cancel</button></div></div>
<details class="set-details"><summary data-i18n="settings.details">Learn more</summary><div class="set-hint" data-i18n="settings.inboundSlackHint">Tokens remain in macOS Keychain. User OAuth powers badge search; Bot OAuth powers channel monitoring; one App token powers live Socket Mode. Badge search continues as catch-up. Add the Deck bot to each monitored channel.</div></details></div>`;

export const CONTRIBUTION = Object.freeze({
  id: 'slack',
  slots: Object.freeze([Object.freeze({ slotId: 'set-extension-integrations', html: PANEL })]),
  settings: Object.freeze([Object.freeze({
    id: 'slack-reactions', section: 'integrations', order: 0,
    labelKeys: ['settings.slack', 'settings.channelMonitor', 'settings.inboundUserToken',
      'settings.channelBotToken', 'settings.inboundAppToken'],
    keywords: ['slack', 'reaction', 'emoji', 'badge', 'channel', 'monitor', 'bot', 'token', 'keychain',
      '标记', '表情', '频道', '监控', '令牌'],
  })]),
  create: createController,
});

function createController(host) {
  const { $, ctx, inv, t, toast, confirmDialog, confirmDangerDialog, uev, formatNumber, commitSettings, persistInbound } = host;
  let channelUpgradeOpen = false;
  let slackConnection = null;
  let slackRenderGeneration = 0;
  let slackSetupRefreshGeneration = 0;
  const slackSecretWrites = new Map();
  const SLACK_TOKEN_FIELDS = [['slack-user-token', 'set-inbound-slack-user'], ['slack-app-token', 'set-inbound-slack-app'], ['slack-bot-token', 'set-channel-bot']];
  function revealSlackChannelSetup() {
    channelUpgradeOpen = true;
    $('set-channel-upgrade-steps').hidden = false;
    $('set-channel-upgrade-steps').scrollIntoView?.({ block: 'nearest' });
  }
  function cancelSlackChannelSetup() {
    slackChannelSetup.cancel();
    slackSetupRefreshGeneration++;
    channelUpgradeOpen = false;
    $('set-channel-upgrade-steps').hidden = true;
    $('set-channel-setup-feedback').textContent = '';
    $('set-channel-existing-manifest').value = '';
    $('set-channel-manifest-text').value = '';
  }
  function followSlackChannelConnection() {
    const generation = ++slackSetupRefreshGeneration;
    let remaining = 10;
    const current = () => generation === slackSetupRefreshGeneration && channelUpgradeOpen
      && ctx.settings.inbound.channelConnection?.enabled;
    const refresh = async () => {
      if (!current()) return;
      await renderInboundSettings();
      if (current() && slackConnection && !slackConnection.connected && --remaining > 0) {
        setTimeout(refresh, 3000);
      }
    };
    setTimeout(refresh, 1000);
  }
  const SLACK_SETUP_KEYS = Object.freeze({
    'checking': 'settings.channelSetup.checking',
    'needs-bot': 'settings.channelSetup.needs-bot',
    'needs-scopes': 'settings.channelSetup.needs-scopes',
    'needs-app': 'settings.channelSetup.needs-app',
    'invalid-bot': 'settings.channelSetup.invalid-bot',
    'network': 'settings.channelSetup.network',
    'user-error': 'settings.channelSetup.user-error',
    'workspace-mismatch': 'settings.channelSetup.workspace-mismatch',
    'unavailable': 'settings.channelSetup.unavailable',
    'save-error': 'settings.channelSetup.save-error',
    'enabled': 'settings.channelSetup.enabled',
  });
  const slackChannelSetup = createSlackChannelSetup({
    saveTokens: async () => {
      // A blur save and a click share the same promise, including its failure.
      const results = await Promise.all(SLACK_TOKEN_FIELDS.map(([slot, id]) => storeInboundSecret(slot, id)));
      return results.every(Boolean);
    },
    readStatus: async () => (slackConnection = await inv('slack_connection_status')),
    enable: proceed => commitSettings({ key: 'inbound', exclusive: true, proceed,
      edit: settings => { settings.inbound.channelConnection = { enabled: true, connectionId: 'default' }; },
      errorKey: 'error.inboundSave', apply: () => renderInboundSettings(),
      onCommit: () => { inv('inbound_check_now').catch(() => {}); },
    }),
    changed: state => {
      if (state === 'enabled') followSlackChannelConnection();
      const checking = state === 'checking';
      $('set-channel-enabled').disabled = checking;
      $('set-channel-upgrade').disabled = checking;
      $('set-channel-finish').disabled = checking;
      if (state !== 'enabled') {
        $('set-channel-enabled').checked = !!ctx.settings.inbound.channelConnection?.enabled;
        revealSlackChannelSetup();
      }
      if (state !== 'credential-error') $('set-channel-setup-feedback').textContent = t(SLACK_SETUP_KEYS[state]);
      if (!checking) $('set-channel-setup-feedback').focus();
    },
  });
  async function enableSlackChannels() {
    try { return await slackChannelSetup.run(); }
    finally {
      $('set-channel-enabled').disabled = false;
      $('set-channel-upgrade').disabled = false;
      $('set-channel-finish').disabled = false;
    }
  }
  async function openSlackAppSettings(page) {
    const entered = $('set-channel-app-id').value.trim();
    if (entered && !/^A[A-Z0-9]{1,63}$/.test(entered)) {
      $('set-channel-setup-feedback').textContent = t('settings.channelSetup.app-id');
      return;
    }
    try { await inv('open_target', { kind: 'url', value: slackAppSettingsUrl(entered || slackConnection?.appId, page, slackConnection?.workspaceId), cwd: ctx.HOME }); }
    catch (_) { $('set-channel-setup-feedback').textContent = t('error.inboundSetup'); }
  }

  const SLACK_STATE_KEYS = Object.freeze({
    unavailable: 'settings.slackState.unavailable',
    ready: 'settings.slackState.ready', off: 'settings.slackState.off',
    'not-enabled': 'settings.slackState.not-enabled',
    'upgrade-required': 'settings.slackState.upgrade-required',
    'needs-app': 'settings.slackState.needs-app', 'needs-user': 'settings.slackState.needs-user',
    'needs-scopes': 'settings.slackState.needs-scopes', invalid: 'settings.slackState.invalid',
    unverified: 'settings.slackState.unverified',
    'workspace-mismatch': 'settings.slackState.workspace-mismatch',
  });

  /* ---------- Slack connection (inbound): the switch and the tokens ----------
     Rules are the project's automations (automation.js, the ↻ drawer); this
     is only the account-level connection. Tokens live in the Keychain and are
     never read back into the page — the backend only reports whether a slot
     is filled. */

  function inboundSlackStatusText(status) {
    const slack = (status && status.sources || []).find(s => s.id === 'slack');
    if (!ctx.settings.inbound.sources.slack.enabled) return t('settings.inboundStatus.off');
    if (!slack) return '';
    const user = slack.secrets.find(x => x.slot === 'slack-user-token');
    if (!user || !user.present) return t('settings.inboundStatus.noToken');
    const parts = [t(slack.live ? 'settings.inboundStatus.live' : 'settings.inboundStatus.polling')];
    if (slack.lastPoll) {
      const ago = Math.max(0, Math.floor(Date.now() / 1000) - slack.lastPoll);
      const label = ago < 60
        ? t('settings.inboundAgoSeconds', { count: formatNumber(ago) })
        : t('settings.inboundAgoMinutes', { count: formatNumber(Math.floor(ago / 60)) });
      parts.push(t('settings.inboundStatus.lastPoll', { ago: label }));
    }
    if (slack.lastError) parts.push(t('settings.inboundStatus.error', { code: slack.lastError }));
    return parts.join(' · ');
  }

  async function renderInboundSettings() {
    const generation = ++slackRenderGeneration;
    $('set-inbound-slack').checked = !!ctx.settings.inbound.sources.slack.enabled;
    let status = null;
    try { status = await inv('inbound_status'); } catch (_) { status = null; }
    if (generation !== slackRenderGeneration) return;
    const present = slot => !!(status && status.sources || []).some(s => s.secrets.some(x => x.slot === slot && x.present));
    for (const [slot, id] of [['slack-user-token', 'set-inbound-slack-user'], ['slack-app-token', 'set-inbound-slack-app']]) {
      const box = $(id);
      box.placeholder = present(slot) ? t('settings.inboundTokenSaved') : (slot === 'slack-user-token' ? 'xoxp-…' : 'xapp-…');
      $(id + '-clear').style.display = present(slot) ? '' : 'none';
    }
    $('set-inbound-setup').hidden = present('slack-user-token') || present('slack-app-token');
    $('set-inbound-status').textContent = inboundSlackStatusText(status);
    let connection = null;
    try { connection = await inv('slack_connection_status'); } catch (_) { connection = null; }
    if (generation !== slackRenderGeneration) return;
    slackConnection = connection;
    const view = slackConnectionView(connection, ctx.settings);
    $('set-channel-enabled').checked = !!ctx.settings.inbound.channelConnection?.enabled;
    $('set-slack-workspace').textContent = view.workspace ? t('settings.slackWorkspace', { workspace: view.workspace }) : '';
    $('set-slack-transport').textContent = t(view.socket === 'connected' ? 'settings.slackSocketConnected' : 'settings.slackSocketDisconnected');
    $('set-slack-reaction-ready').textContent = t('settings.slackReactionState', { state: t(SLACK_STATE_KEYS[view.reaction]) });
    $('set-channel-status').textContent = t('settings.slackChannelState', { state: t(SLACK_STATE_KEYS[view.channel]) });
    if (view.channel === 'ready') $('set-channel-status').textContent += ' · ' + t('settings.slackChannelRules', { count: formatNumber(view.channelRules) });
    if (view.channel === 'ready' && view.socket === 'disconnected') $('set-channel-status').textContent += ' · ' + t('settings.channelStatus.disconnected');
    try {
      const channel = await inv('channel_status');
      if (generation !== slackRenderGeneration) return;
      const facts = [];
      if (channel.pendingCount) facts.push(t('settings.channelPending', { count: formatNumber(channel.pendingCount) }));
      if (channel.rejectedCount) facts.push(t('settings.channelRejected', { count: formatNumber(channel.rejectedCount) }));
      if (channel.gapUnresolved) facts.push(t('settings.channelGap'));
      if (channel.lastError) facts.push(t('settings.inboundStatus.error', { code: channel.lastError }));
      if (facts.length) $('set-channel-status').textContent += ' · ' + facts.join(' · ');
    } catch (_) { /* status remains explicit from credential/config state */ }
    $('set-slack-legacy').hidden = !view.legacyNotice;
    $('set-slack-legacy').textContent = view.legacyNotice
      ? t(view.legacyNotice === 'retained' ? 'settings.channelLegacyRetained' : 'settings.channelLegacy') : '';
    $('set-slack-legacy-clear').hidden = !view.legacy;
    $('set-channel-bot').placeholder = connection?.botPresent ? t('settings.inboundTokenSaved') : 'xoxb-…';
    $('set-channel-bot-clear').hidden = !connection?.botPresent;
    $('set-channel-upgrade-steps').hidden = !channelUpgradeOpen;
    $('set-channel-app-id').placeholder = connection?.appId || 'A…';
    $('set-channel-app-location').textContent = t(connection?.appId ? 'settings.channelAppLocated' : 'settings.channelAppChoose');
    $('set-channel-upgrade').textContent = t(ctx.settings.inbound.channelConnection?.enabled ? 'settings.channelManage' : 'settings.channelEnable');
    $('set-channel-upgrade-hint').textContent = t(!connection?.userPresent && !connection?.appPresent ? 'settings.channelNewSetupSteps' : 'settings.channelUpgradeSteps');
    $('set-channel-ids').textContent = [...new Set((ctx.settings.inbound.channelRules || []).flatMap(rule => rule.channelIds || []))].join(', ');

  }

  async function persistInboundSlackChoice() {
    const desired = $('set-inbound-slack').checked;
    const previous = !!ctx.settings.inbound.sources.slack.enabled;
    if (desired && !previous && !(await confirmDialog(t('settings.inboundEnableConfirm')))) {
      $('set-inbound-slack').checked = false;
      return;
    }
    const ok = await persistInbound({ ...ctx.settings.inbound, sources: { ...ctx.settings.inbound.sources, slack: { enabled: desired } } });
    if (ok) toast(t(desired ? 'settings.inboundEnabled' : 'settings.inboundDisabled'));
  }

  const INBOUND_TOKEN_ERRORS = { scope: 'error.inboundTokenScope', workspace: 'error.inboundWorkspace', 'other-credential': 'error.inboundOtherCredential', shape: 'error.inboundTokenShape', auth: 'error.inboundTokenAuth', network: 'error.inboundTokenNetwork', slack: 'error.inboundTokenSlack', keychain: 'error.inboundToken' };
  function storeInboundSecret(slot, inputId) {
    if (slackSecretWrites.has(slot)) return slackSecretWrites.get(slot);
    const box = $(inputId);
    const value = box.value.trim();
    if (!value) return Promise.resolve(true);
    box.disabled = true;
    const pending = (async () => {
      try {
        await inv('inbound_set_secret', { slot, value });
        if (box.value.trim() === value) box.value = '';
        toast(t('settings.inboundTokenStored'));
        return true;
      } catch (e) {
        const code = String(e);
        const message = code.startsWith('slack:') ? t('error.inboundTokenSlack', { code: code.slice(6) || '?' })
          : t(INBOUND_TOKEN_ERRORS[code] || 'error.inboundToken');
        toast(message);
        $('set-channel-setup-feedback').textContent = message;
        return false;
      } finally {
        box.disabled = false;
        slackSecretWrites.delete(slot);
        void renderInboundSettings();
      }
    })();
    slackSecretWrites.set(slot, pending);
    return pending;
  }

  async function clearInboundSecret(slot) {
    if (!(await confirmDialog(t(slot === 'slack-app-token' ? 'settings.slackAppClearConfirm' : 'settings.inboundTokenClearConfirm')))) return;
    try {
      await inv('inbound_set_secret', { slot, value: '' });
      toast(t('settings.inboundTokenCleared'));
    } catch (_) {
      toast(t('error.inboundToken'));
    }
    renderInboundSettings();
  }

  async function storeChannelSecret() { await storeInboundSecret('slack-bot-token', 'set-channel-bot'); }
  async function clearChannelSecret() { await clearInboundSecret('slack-bot-token'); }

  async function clearLegacySlackCredentials() {
    await removeLegacySlackCredentials({
      confirm: () => confirmDangerDialog(t('settings.channelLegacyClearConfirm'), t('settings.channelLegacyClear')),
      invoke: inv,
      refresh: renderInboundSettings,
      notice: outcome => toast(t(outcome === 'cleared' ? 'settings.channelLegacyCleared' : 'error.channelLegacyClear')),
    });
  }


  const bindings = {
    toggle: persistInboundSlackChoice,
    toggleChannel: async () => {
      if ($('set-channel-enabled').checked) return enableSlackChannels();
      cancelSlackChannelSetup();
      await persistInbound({ ...ctx.settings.inbound, channelConnection: { enabled: false, connectionId: 'default' } });
    },
    upgrade: () => ctx.settings.inbound.channelConnection?.enabled ? revealSlackChannelSetup() : enableSlackChannels(),
    finish: enableSlackChannels,
    cancel: cancelSlackChannelSetup,
    open: openSlackAppSettings,
    manifest: async () => {
      try {
        const required = JSON.parse(await inv('slack_manifest'));
        const merged = mergeSlackChannelManifest($('set-channel-existing-manifest').value, required);
        $('set-channel-manifest-text').value = merged;
        $('set-channel-manifest-text').hidden = false;
        await inv('write_clipboard', { text: merged });
        $('set-channel-setup-feedback').textContent = t('settings.channelSetup.copied');
        await openSlackAppSettings('app-manifest');
      } catch (_) { $('set-channel-setup-feedback').textContent = t('settings.channelSetup.manifest-error'); }
    },
    storeBot: () => storeChannelSecret(), clearBot: () => clearChannelSecret(),
    clearLegacy: clearLegacySlackCredentials,
    setup: async () => { try { await inv('inbound_setup', { source: 'slack' }); } catch (_) { toast(t('error.inboundSetup')); } },
    storeUser: () => storeInboundSecret('slack-user-token', 'set-inbound-slack-user'),
    storeApp: () => storeInboundSecret('slack-app-token', 'set-inbound-slack-app'),
    clearUser: () => clearInboundSecret('slack-user-token'),
    clearApp: () => clearInboundSecret('slack-app-token'),
    checkNow: () => { inv('inbound_check_now').catch(() => {}); setTimeout(renderInboundSettings, 4000); },
  };
  return {
    refresh: renderInboundSettings, inboundChanged: renderInboundSettings,
    close: cancelSlackChannelSetup, renderInboundSettings, persistInboundSlackChoice,
    mount() {
      const $ = host.$;
      $('set-inbound-slack').onchange = bindings.toggle;
      $('set-channel-enabled').onchange = bindings.toggleChannel;
      $('set-channel-upgrade').onclick = bindings.upgrade;
      $('set-channel-finish').onclick = bindings.finish;
      $('set-channel-cancel').onclick = bindings.cancel;
      $('set-channel-open-app').onclick = () => bindings.open('app-manifest');
      $('set-channel-open-oauth').onclick = () => bindings.open('oauth');
      $('set-channel-manifest').onclick = bindings.manifest;
      $('set-channel-bot').addEventListener('change', bindings.storeBot);
      $('set-channel-bot-clear').onclick = bindings.clearBot;
      $('set-slack-legacy-clear').onclick = bindings.clearLegacy;
      $('set-inbound-setup').onclick = bindings.setup;
      $('set-inbound-slack-user').addEventListener('change', bindings.storeUser);
      $('set-inbound-slack-app').addEventListener('change', bindings.storeApp);
      $('set-inbound-slack-user-clear').onclick = bindings.clearUser;
      $('set-inbound-slack-app-clear').onclick = bindings.clearApp;
      $('set-inbound-check').onclick = bindings.checkNow;
    },
  };
}
