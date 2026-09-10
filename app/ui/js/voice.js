// A single mounted composer, placed by CSS in the bottom, floating or right
// position. Dependencies from the view core are injected at boot, no cycle.
// Each session owns its draft and target; selection survives layout changes. No global mic/keyboard capture.
// The session header identifies the recipient; only uncertain delivery shows
// a retry action alongside its message, without a duplicate target row.
import { $, ctx, inv, listen, uev } from './state.js';
import { onLocaleChange, t } from './i18n.js';
import { createVoiceComposer, voiceBusy, voiceRecording, voiceCanAct } from './voice-model.js';

import { VOICE_LANGUAGE_NAMES } from './voice-preferences-model.js';

export let voiceComposer = null;

export function initVoice(deps) {
  const panel = $('voice-panel'), draft = $('voice-draft'), workspace = $('voice-workspace');
  const button = $('voice-btn');
  let controller;
  const render = s => {
    panel.hidden = !s.open;
    workspace.dataset.voiceLayout = s.open ? s.layout : '';
    button.setAttribute('aria-expanded', String(s.open));
    button.classList.toggle('voice-active', voiceRecording(s.phase));
    const busy = voiceBusy(s.phase) || s.deliveryBusy || s.switching, recording = voiceRecording(s.phase);
    // Never replace the textarea node or its value on a layout-only render.
    if (draft.value !== s.draft) draft.value = s.draft;
    draft.readOnly = busy;
    $('voice-record').disabled = busy || !s.session;
    $('voice-record').hidden = recording;
    $('voice-stop').hidden = !recording;
    $('voice-stop').disabled = s.phase === 'stopping';
    $('voice-close').disabled = s.phase === 'sending';
    $('voice-clear').disabled = !voiceCanAct(s);
    $('voice-retry').disabled = !voiceCanAct(s) || !s.target;
    $('voice-retry').hidden = !s.needsConfirmation;
    const language = $('voice-language');
    if ([...language.options].map(option => option.value).join(',') !== s.languages.join(',')) {
      language.replaceChildren(...s.languages.map(code => {
        const option = document.createElement('option'); option.value = code; option.textContent = VOICE_LANGUAGE_NAMES[code]; return option;
      }));
    }
    language.hidden = s.languages.length === 1;
    $('voice-language-label').hidden = language.hidden;
    language.disabled = busy;
    $('voice-language').value = s.language;
    $('voice-insert').disabled = $('voice-send').disabled = !voiceCanAct(s) || !s.session || s.needsConfirmation;
    $('voice-record').textContent = t(s.draft ? 'voice.continue' : 'voice.record');
    const elapsed = s.startedAt ? Math.floor((Date.now() - s.startedAt) / 1000) : 0;
    const time = `${String(Math.floor(elapsed / 60)).padStart(2, '0')}:${String(elapsed % 60).padStart(2, '0')}`;
    $('voice-status').textContent = t(`voice.phase.${s.phase}`) + (s.phase === 'recording' ? ` · ${time}` : '');
    panel.dataset.phase = s.phase;
    $('voice-message').textContent = s.error ? t(`voice.error.${s.error}`) : s.notice ? t(`voice.notice.${s.notice}`) : t('voice.hint');
    $('voice-message').classList.toggle('voice-error', !!s.error || s.notice === 'ambiguous');
    panel.querySelectorAll('[data-voice-layout]').forEach(el => el.setAttribute('aria-pressed', String(el.dataset.voiceLayout === s.layout)));
  };
  controller = createVoiceComposer({ invoke: inv, changed: render, language: navigator.language, preferences: ctx.settings.voice,
    prepareTarget: deps.prepareTarget, afterDelivery: deps.afterDelivery });
  window.addEventListener('deck-voice-preferences-changed', () => controller.configure(ctx.settings.voice));
  const ensureTarget = () => controller.ensureTarget(deps.selectedTarget());
  button.onclick = async () => {
    if (controller.state.open) { draft.focus(); return; }
    controller.show();
    const session = deps.selectedTarget()?.session;
    if (await ensureTarget() && controller.state.open && controller.state.session === session) {
      draft.focus(); if (!controller.state.draft) await controller.start();
    }
  };
  $('voice-record').onclick = async () => {
    const owner = controller.state;
    if (owner.target) return controller.start();
    if (await ensureTarget() && controller.state === owner) await controller.start();
  };
  $('voice-stop').onclick = () => controller.stop();
  $('voice-close').onclick = () => { controller.close(); button.focus(); };
  $('voice-retry').onclick = () => controller.retry();
  $('voice-clear').onclick = async () => {
    const owner = controller.state;
    await controller.clear();
    if (controller.state === owner && owner.open) draft.focus();
  };
  $('voice-language').onchange = event => controller.language(event.target.value);
  const deliver = async submit => {
    if (!voiceCanAct(controller.state) || controller.state.needsConfirmation) return;
    const owner = controller.state;
    if (owner.target) return controller.deliver(submit);
    if (await ensureTarget() && controller.state === owner) await controller.deliver(submit);
  };
  $('voice-insert').onclick = () => deliver(false);
  $('voice-send').onclick = () => deliver(true);
  draft.oninput = () => controller.edit(draft.value);
  panel.querySelectorAll('[data-voice-layout]').forEach(el => {
    // Mouse layout switching retains the editor's caret/focus; keyboard users
    // retain the focused layout button. No DOM reparenting/recording restart.
    el.addEventListener('mousedown', event => event.preventDefault());
    el.onclick = () => controller.layout(el.dataset.voiceLayout);
  });
  panel.addEventListener('keydown', event => {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === 'Escape') {
      event.preventDefault(); event.stopPropagation();
      if (voiceRecording(controller.state.phase)) controller.stop();
      else { controller.close(); button.focus(); }
    }
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault(); event.stopPropagation(); deliver(true);
    }
  });
  window.addEventListener('deck-voice-session-changed', () => controller.select(deps.selectedTarget()));
  // Leaving the session view always releases capture and keeps an unsent draft.
  window.addEventListener('deck-session-leave', event => controller.close({ preserveOpen: !!event.detail?.switchingSession }));
  window.addEventListener('deck-voice-target-exit', event => {
    if (event.detail === controller.state.session) controller.close();
  });
  window.addEventListener('pagehide', () => controller.close());
  listen('voice-window-hidden', () => controller.close()).catch(() => uev('listen-fail', 'voice-window-hidden'));
  onLocaleChange(() => render(controller.state));
  render(controller.state);
  voiceComposer = controller;
  return controller;
}
