// Pi's first-party Settings contribution. Both switches name the same Pi
// settings entry, so `lock: pi-settings` makes the host serialize them.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { piErrorKey, piSettingsView } from './pi-model.js';

export const CONTRIBUTION = Object.freeze({
  id: 'pi',
  agentSource: Object.freeze({
    source: 'pi',
    legacy: raw => ({ signal: raw.pi, replies: raw.pi_replies, installation: raw.pi_installation }),
    project(entry) {
      const status = entry && { pi: entry.signal, pi_replies: entry.replies, pi_installation: entry.installation };
      const view = piSettingsView(status);
      return {
        signal: typeof entry?.signal === 'boolean' ? view.signal : undefined,
        /* an older answer without it reads as off */
        replies: typeof entry?.signal === 'boolean' ? view.replies : undefined,
        usable: view.usable,
        known: view.note !== 'settings.pi.note.unknown', note: view.note };
    },
    errorKey: piErrorKey,
    readBackAfterWrite: true,
  }),
  settings: Object.freeze([Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
    labelKeys: ['settings.piSignal', 'settings.piReplies'],
    keywords: ['pi', 'work status', 'final message', 'reply', '工作状态', '最终回复'] })]),
  agentControls: Object.freeze([
    Object.freeze({ item: 'agent-status', source: 'pi', capability: 'signal', boxId: 'set-pi-hooks',
      labelKey: 'settings.piSignal', hintKey: 'settings.piSignalHint', noteId: 'set-pi-note',
      confirmKey: 'settings.piSignalEnableConfirm', feedbackLabelKey: 'settings.piSignal',
      feedbackKey: 'hook:pi-signal', onKey: 'settings.piSignalEnabled', offKey: 'settings.piSignalDisabled', lock: 'pi-settings' }),
    // the final message of a turn that ended, for the card's line and the
    // readers that keep it (card snapshot, phone): the entry that reads it
    // carries the work status too, so only while that is on
    Object.freeze({ item: 'agent-status', source: 'pi', capability: 'replies', requires: 'signal',
      boxId: 'set-pi-replies', labelKey: 'settings.piReplies', hintKey: 'settings.piRepliesHint',
      confirmKey: 'settings.piRepliesEnableConfirm', feedbackLabelKey: 'settings.piReplies',
      onKey: 'settings.piRepliesEnabled', offKey: 'settings.piRepliesDisabled',
      feedbackKey: 'hook:pi-replies', lock: 'pi-settings',
      write: (host, enable) => { const inv = host.inv; return inv('agent_pi_replies_set', { enable }); } }),
  ]),
});
