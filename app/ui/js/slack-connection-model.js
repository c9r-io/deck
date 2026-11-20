// Derived Slack capability state. No migration boolean is persisted.
// An unavailable status read is unknown, never a missing credential.
export function slackConnectionView(status, settings) {
  const s = status || {};
  const inbound = settings?.inbound || {};
  const reactionEnabled = !!inbound.sources?.slack?.enabled;
  const channelEnabled = !!inbound.channelConnection?.enabled;
  const reaction = !status ? 'unavailable' : !reactionEnabled ? 'off' : !s.userPresent ? 'needs-user' : !s.userValid ? (s.userError === 'scope' ? 'needs-scopes' : 'invalid') : 'ready';
  const channel = !status ? 'unavailable' : !s.botPresent && (s.legacyPresent || Number(s.channelRules) > 0) ? 'upgrade-required'
    : !channelEnabled && !s.botPresent ? 'not-enabled'
      : !s.appPresent ? 'needs-app'
      : !s.appValid ? (s.appError ? 'invalid' : 'unverified')
      : !s.botPresent ? 'upgrade-required'
        : !s.botValid ? (s.botError === 'scope' ? 'needs-scopes' : 'invalid')
          : !s.workspaceMatch ? 'workspace-mismatch'
            : !channelEnabled ? 'off' : 'ready';
  const legacyNotice = !s.legacyPresent ? null : channel === 'ready' || channel === 'off' ? 'retained' : 'upgrade-required';
  return { reaction, channel, legacy: !!s.legacyPresent, legacyNotice, socket: s.connected ? 'connected' : 'disconnected',
    workspace: s.workspace || '', channelRules: Number(s.channelRules) || 0 };
}

// What a closed Slack failure word means for the user (slack_api.rs maps
// Slack's answers to `auth`, `scope`, `ratelimited`, `slack`; the transport
// adds its socket words). Each reason names its own way out: a refused token
// is replaced in the field above it, a network word fixes itself.
const FAILURE_KEYS = {
  auth: 'settings.slackFailure.auth',
  scope: 'settings.slackFailure.scope',
  ratelimited: 'settings.slackFailure.ratelimited',
  network: 'settings.slackFailure.network',
};
const NETWORK_WORDS = ['network', 'timeout', 'http', 'socket', 'stalled', 'closed', 'reconnect'];
export function slackFailureKey(code) {
  if (NETWORK_WORDS.includes(code)) return FAILURE_KEYS.network;
  return Object.hasOwn(FAILURE_KEYS, code) ? FAILURE_KEYS[code] : 'settings.slackFailure.other';
}
