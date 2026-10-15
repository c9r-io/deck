// Claude Code's first-party Settings contribution. The shared extension host
// owns status reads and write serialization; this file owns Claude's controls,
// copy/search metadata, native command mapping and DOM binding declaration.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const CONTRIBUTION = Object.freeze({
  id: 'claude-code',
  agentSource: Object.freeze({ source: 'claude-code', legacy: raw => ({
    signal: raw.claude, lifecycle: raw.claude_reentry, installation: null,
  }) }),
  settings: Object.freeze([
    Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
      labelKeys: ['settings.group.agentStatus', 'settings.agentHooks'],
      keywords: ['hook', 'hooks', 'needs input', 'turn finished', '钩子', '等待输入'] }),
    Object.freeze({ id: 'claude-reentry', section: 'agents', order: 1,
      labelKeys: ['settings.claudeReentry'],
      keywords: ['re-entry', 'reentry', 'reopen', 'lifecycle', '重入', '重新进入'] }),
  ]),
  agentControls: Object.freeze([
    Object.freeze({ item: 'agent-status', source: 'claude-code', capability: 'signal',
      boxId: 'set-agent-hooks', labelKey: 'settings.agentHooks', hintKey: 'settings.agentHooksHint',
      confirmKey: 'settings.agentHooksEnableConfirm', feedbackLabelKey: 'settings.agentHooks',
      feedbackKey: 'hook:claude', notifyDependency: true, toast: 'agent-hooks', lock: 'agent-signals' }),
    Object.freeze({ item: 'claude-reentry', source: 'claude-code', capability: 'lifecycle',
      boxId: 'set-claude-reentry', labelKey: 'settings.claudeReentry', hintKey: 'settings.claudeReentryHint',
      confirmKey: 'settings.claudeReentryEnableConfirm', feedbackLabelKey: 'settings.claudeReentry',
      onKey: 'settings.claudeReentryEnabled', offKey: 'settings.claudeReentryDisabled',
      feedbackKey: 'hook:claude-reentry',
      write: (host, enable) => { const inv = host.inv; return inv('agent_lifecycle_hooks_set', { enable }); } }),
  ]),
});
