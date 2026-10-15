// Pi's first-party Settings contribution. Both switches name the same Pi
// settings entry, so `lock: pi-settings` makes the host serialize them.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { piErrorKey, piSettingsView } from './pi-model.js';

export const CONTRIBUTION = Object.freeze({
  id: 'pi',
  agentSource: Object.freeze({
    source: 'pi',
    legacy: raw => ({ signal: raw.pi, lifecycle: raw.pi_lifecycle, installation: raw.pi_installation }),
    project(entry) {
      const status = entry && { pi: entry.signal, pi_lifecycle: entry.lifecycle, pi_installation: entry.installation };
      const view = piSettingsView(status);
      return {
        signal: typeof entry?.signal === 'boolean' ? view.signal : undefined,
        lifecycle: typeof entry?.lifecycle === 'boolean' ? view.lifecycle : undefined,
        usable: view.usable,
        known: view.note !== 'settings.pi.note.unknown', note: view.note };
    },
    errorKey: piErrorKey,
    readBackAfterWrite: true,
  }),
  settings: Object.freeze([Object.freeze({ id: 'agent-status', section: 'agents', order: 0,
    labelKeys: ['settings.piSignal', 'settings.piLifecycle'],
    keywords: ['pi', 'work status', 'session observation', '工作状态', '会话观察'] })]),
  agentControls: Object.freeze([
    Object.freeze({ item: 'agent-status', source: 'pi', capability: 'signal', boxId: 'set-pi-hooks',
      labelKey: 'settings.piSignal', hintKey: 'settings.piSignalHint', noteId: 'set-pi-note',
      confirmKey: 'settings.piSignalEnableConfirm', feedbackLabelKey: 'settings.piSignal',
      feedbackKey: 'hook:pi-signal', onKey: 'settings.piSignalEnabled', offKey: 'settings.piSignalDisabled', lock: 'pi-settings' }),
    Object.freeze({ item: 'agent-status', source: 'pi', capability: 'lifecycle', boxId: 'set-pi-lifecycle',
      labelKey: 'settings.piLifecycle', hintKey: 'settings.piLifecycleHint',
      confirmKey: 'settings.piLifecycleEnableConfirm', feedbackLabelKey: 'settings.piLifecycle',
      onKey: 'settings.piLifecycleEnabled', offKey: 'settings.piLifecycleDisabled',
      feedbackKey: 'hook:pi-lifecycle', lock: 'pi-settings',
      write: (host, enable) => { const inv = host.inv; return inv('agent_pi_lifecycle_set', { enable }); } }),
  ]),
});
