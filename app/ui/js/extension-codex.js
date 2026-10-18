// Codex's first-party Settings contribution.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const CONTRIBUTION = Object.freeze({
  id: 'codex',
  agentSource: Object.freeze({ source: 'codex', legacy: raw => ({
    signal: raw.codex, lifecycle: null, installation: null,
  }) }),
  settings: Object.freeze([
    Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
      labelKeys: ['settings.codexHooks'], keywords: ['codex', 'hook', 'hooks', '钩子'] }),
    Object.freeze({ id: 'codex-reentry', section: 'agents', order: 1,
      labelKeys: ['settings.codexReentry'],
      keywords: ['codex', 're-entry', 'reentry', 'reopen', 'lifecycle', '重入', '重新进入'] }),
  ]),
  agentControls: Object.freeze([
    Object.freeze({
      item: 'agent-status', source: 'codex', capability: 'signal', boxId: 'set-codex-hooks',
      labelKey: 'settings.codexHooks', hintKey: 'settings.codexHooksHint',
      confirmKey: 'settings.codexHooksEnableConfirm', feedbackLabelKey: 'settings.codexHooks',
      feedbackKey: 'hook:codex', notifyDependency: true, toast: 'agent-hooks', lock: 'agent-signals',
    }),
    // the separate re-entry observation opt-in: Codex's own hooks file, and
    // Codex's own trust step for the two entries
    Object.freeze({ item: 'codex-reentry', source: 'codex', capability: 'lifecycle',
      boxId: 'set-codex-reentry', labelKey: 'settings.codexReentry', hintKey: 'settings.codexReentryHint',
      confirmKey: 'settings.codexReentryEnableConfirm', feedbackLabelKey: 'settings.codexReentry',
      onKey: 'settings.codexReentryEnabled', offKey: 'settings.codexReentryDisabled',
      feedbackKey: 'hook:codex-reentry',
      write: (host, enable) => { const inv = host.inv; return inv('agent_lifecycle_hooks_set', { enable, source: 'codex' }); } }),
  ]),
});
