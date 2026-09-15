// The session header's microphone is the whole voice UI: idle starts a
// recording for the focused pane, recording stops it, stopping waits. There is
// no panel, draft or per-session state: committed speech lands in the pane
// like keystrokes and the terminal keeps focus, so keyboard and voice share
// one input. A translucent caption under the button shows what was heard but
// not yet typed; it is display only. Errors and notices are toasts. A session
// switch, leaving the view, the pane's exit, a hidden window or page unload
// ends capture.
// Dependencies from the view core are injected at boot, no cycle.
import { $, ctx, inv, listen, uev } from './state.js';
import { onLocaleChange, t } from './i18n.js';
import { createVoiceInput, voiceBusy, voiceRecording } from './voice-model.js';

export let voiceInput = null;

export function initVoice(deps) {
  const button = $('voice-btn'), label = $('voice-time'), caption = $('voice-caption');
  const render = s => {
    button.dataset.phase = s.phase;
    button.classList.toggle('voice-active', voiceRecording(s.phase));
    button.disabled = s.phase === 'stopping';
    button.setAttribute('aria-pressed', String(voiceBusy(s.phase)));
    const title = t(`voice.phase.${s.phase}`);
    button.title = title; button.setAttribute('aria-label', title);
    const elapsed = s.startedAt ? Math.floor((Date.now() - s.startedAt) / 1000) : 0;
    const time = `${String(Math.floor(elapsed / 60)).padStart(2, '0')}:${String(elapsed % 60).padStart(2, '0')}`;
    label.textContent = s.startedAt ? time : voiceBusy(s.phase) ? '…' : '';
    label.hidden = !label.textContent;
    const preview = voiceRecording(s.phase) ? s.preview : '';
    caption.textContent = preview.length > 160 ? `…${preview.slice(-160)}` : preview;
    caption.hidden = !preview;
    if (!caption.hidden && button.getBoundingClientRect && button.parentElement?.getBoundingClientRect) {
      caption.style.right = `${Math.max(0, button.parentElement.getBoundingClientRect().right - button.getBoundingClientRect().right)}px`;
    }
  };
  const controller = createVoiceInput({ invoke: inv, changed: render, language: navigator.language, preferences: ctx.settings.voice,
    prepareTarget: deps.prepareTarget, afterDelivery: deps.afterDelivery,
    report: (kind, code) => deps.toast(t(`voice.${kind}.${code}`)) });
  window.addEventListener('deck-voice-preferences-changed', () => controller.configure(ctx.settings.voice));
  // The button never takes focus from the terminal: speech and keys go to one input.
  button.addEventListener('mousedown', event => event.preventDefault());
  button.onclick = () => { deps.focusTerminal?.(); return controller.toggle(deps.selectedTarget()); };
  window.addEventListener('deck-voice-session-changed', () => controller.select(deps.selectedTarget()));
  window.addEventListener('deck-session-leave', () => controller.cancel());
  window.addEventListener('deck-voice-target-exit', event => controller.targetExit(event.detail));
  window.addEventListener('pagehide', () => controller.cancel());
  listen('voice-window-hidden', () => controller.cancel()).catch(() => uev('listen-fail', 'voice-window-hidden'));
  onLocaleChange(() => render(controller.state));
  render(controller.state);
  voiceInput = controller;
  return controller;
}
