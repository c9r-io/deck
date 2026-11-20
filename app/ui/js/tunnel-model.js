// tunnel-model.js — what the MCP settings row says about a Secure Tunnel,
// DOM-free. The backend (`tunnel_helper.rs`, contract in its header) passes
// on only the helper's own closed failure codes and remembers the last failed
// action per client until it succeeds or the runtime reports ready; this file
// turns a code into what the user can do about it, and says which states let
// the user (re)enter the Tunnel ID and Runtime key. Setup replaces a stored
// key, so every configured state offers it: a key OpenAI no longer accepts
// leaves the runtime configured, and without the button there would be no
// way to put a new one in.

/* the reason a code stands for; each names its own way out */
const FAILURE_GROUPS = {
  connect: ['tunnel_client_readiness_timeout', 'tunnel_not_ready', 'tunnel_client_connect_failed'],
  key: ['key_missing', 'key_invalid', 'keychain_unavailable', 'not_configured', 'runtime_config_invalid'],
  client: [
    'tunnel_client_missing', 'tunnel_client_untrusted', 'tunnel_client_replaced', 'executable_missing',
    'executable_untrusted', 'development_executable_missing', 'adapter_untrusted', 'adapter_unsupported_platform',
  ],
};

/* i18n keys are literal so the dictionary check sees each one */
const FAILURE_KEYS = {
  connect: 'mcp.tunnelFailure.connect',
  key: 'mcp.tunnelFailure.key',
  client: 'mcp.tunnelFailure.client',
  other: 'mcp.tunnelFailure.other',
};
const ACTION_KEYS = { start: 'mcp.tunnelAction.start', stop: 'mcp.tunnelAction.stop', remove: 'mcp.tunnelAction.remove' };

export function tunnelFailureKey(code) {
  for (const [group, codes] of Object.entries(FAILURE_GROUPS)) if (codes.includes(code)) return FAILURE_KEYS[group];
  return FAILURE_KEYS.other;
}

export const tunnelActionKey = action => ACTION_KEYS[action] || ACTION_KEYS.start;

/* the setup button's label for a state, or null when setup cannot help */
export function tunnelSetupKey(state) {
  if (state === 'not_configured' || state === 'key_missing') return 'mcp.tunnelSetup';
  if (['stopped', 'starting', 'ready', 'unhealthy', 'stale', 'error'].includes(state)) return 'mcp.tunnelResetup';
  return null;
}

/* a code is shown only when it is a plain closed word */
export const tunnelCode = code => (typeof code === 'string' && /^[a-z][a-z0-9_]{0,63}$/.test(code) ? code : 'helper_action_failed');
