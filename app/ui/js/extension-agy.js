// agy's (Google Antigravity CLI) first-party Settings contribution: one
// switch for Deck's one entry in ~/.gemini/config/hooks.json
// (agent_agy.rs). agy reports a turn's start and end only, never a question
// to the user, so the card explains its limited coverage (attention.js).
// Part of deck's no-build frontend: native ES modules, no bundler.

export const CONTRIBUTION = Object.freeze({
  id: 'agy',
  /* an older carrier without the `agents` map has no agy: its switch reads
     off and cannot be used there, and it does not make the shared Agent
     status group unknown */
  agentSource: Object.freeze({ source: 'agy',
    legacy: () => ({ signal: false, lifecycle: null, installation: null, absent: true }),
    project: entry => ({ signal: entry?.signal, lifecycle: null,
      usable: entry?.absent !== true, known: true, note: null }),
  errorKey: message => ({
    'agy-entry-foreign': 'settings.agyErrorForeign',
    'agy-disabled-by-user': 'settings.agyErrorDisabled',
    'agy-hooks-unreadable': 'settings.agyErrorUnreadable',
  })[String(message || '').match(/agy-[a-z-]+/)?.[0]] || 'error.agentHooks' }),
  settings: Object.freeze([
    Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
      labelKeys: ['settings.agyHooks'],
      keywords: ['agy', 'antigravity', 'gemini', 'hook', 'hooks', '钩子'] }),
  ]),
  agentControls: Object.freeze([
    Object.freeze({
      item: 'agent-status', source: 'agy', capability: 'signal', boxId: 'set-agy-hooks',
      labelKey: 'settings.agyHooks', hintKey: 'settings.agyHooksHint',
      confirmKey: 'settings.agyHooksEnableConfirm', feedbackLabelKey: 'settings.agyHooks',
      feedbackKey: 'hook:agy', notifyDependency: true, toast: 'agent-hooks', lock: 'agent-signals',
    }),
  ]),
});
