// Codex's first-party Settings contribution.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const CONTRIBUTION = Object.freeze({
  id: 'codex',
  agentSource: Object.freeze({ source: 'codex', legacy: raw => ({
    signal: raw.codex, lifecycle: null, installation: null,
  }) }),
  settings: Object.freeze([Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
    labelKeys: ['settings.codexHooks'], keywords: ['codex', 'hook', 'hooks', '钩子'] })]),
  agentControls: Object.freeze([Object.freeze({
    item: 'agent-status', source: 'codex', capability: 'signal', boxId: 'set-codex-hooks',
    labelKey: 'settings.codexHooks', hintKey: 'settings.codexHooksHint',
    confirmKey: 'settings.codexHooksEnableConfirm', feedbackLabelKey: 'settings.codexHooks',
    feedbackKey: 'hook:codex', notifyDependency: true, toast: 'agent-hooks', lock: 'agent-signals',
  })]),
});
