// Derived Slack capability state. No migration boolean is persisted.
export function slackConnectionView(status, settings) {
  const s = status || {};
  const inbound = settings?.inbound || {};
  const reactionEnabled = !!inbound.sources?.slack?.enabled;
  const channelEnabled = !!inbound.channelConnection?.enabled;
  const reaction = !reactionEnabled ? 'off' : !s.userPresent ? 'needs-user' : !s.userValid ? (s.userError === 'scope' ? 'needs-scopes' : 'invalid') : 'ready';
  const channel = !s.botPresent && (s.legacyPresent || Number(s.channelRules) > 0) ? 'upgrade-required'
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
