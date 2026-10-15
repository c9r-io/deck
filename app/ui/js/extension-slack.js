// Slack's first-party custom Settings contribution. Account connection,
// credentials and Channel Monitoring remain one panel and one mount lifecycle.
// Part of deck's no-build frontend: native ES modules, no bundler.
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
  mount(host) {
    const $ = host.$;
    $('set-inbound-slack').onchange = host.slack.toggle;
    $('set-channel-enabled').onchange = host.slack.toggleChannel;
    $('set-channel-upgrade').onclick = host.slack.upgrade;
    $('set-channel-finish').onclick = host.slack.finish;
    $('set-channel-cancel').onclick = host.slack.cancel;
    $('set-channel-open-app').onclick = () => host.slack.open('app-manifest');
    $('set-channel-open-oauth').onclick = () => host.slack.open('oauth');
    $('set-channel-manifest').onclick = host.slack.manifest;
    $('set-channel-bot').addEventListener('change', host.slack.storeBot);
    $('set-channel-bot-clear').onclick = host.slack.clearBot;
    $('set-slack-legacy-clear').onclick = host.slack.clearLegacy;
    $('set-inbound-setup').onclick = host.slack.setup;
    $('set-inbound-slack-user').addEventListener('change', host.slack.storeUser);
    $('set-inbound-slack-app').addEventListener('change', host.slack.storeApp);
    $('set-inbound-slack-user-clear').onclick = host.slack.clearUser;
    $('set-inbound-slack-app-clear').onclick = host.slack.clearApp;
    $('set-inbound-check').onclick = host.slack.checkNow;
  },
});
