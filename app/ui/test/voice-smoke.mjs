// Real WKWebView + isolated tmux integration; no microphone access. Exercises
// the production button, IPC, target capture and the byte-literal paste path
// (no Enter, separators preserved, any foreground program, dead targets),
// then language preferences through real settings IPC and enhanced dropdowns.
export async function runVoiceSmoke() {
  const { $, ctx, inv, state } = await import('../js/state.js');
  const { provider, render, stopPolling, panes } = await import('../js/board.js');
  const { openSession, backToBoard } = await import('../js/layout.js');
  const { voiceInput: voice } = await import('../js/voice.js');
  const { setLocale, t } = await import('../js/i18n.js');
  const { activateTheme } = await import('../js/theme.js');
  const { openSettings, selectSettingsSection, persistVoicePreferences } = await import('../js/settings.js');
  let failed = false, stage = 0;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? 1 : -1, b: 0 });
  };
  const waitFor = async predicate => {
    for (let n = 0; n < 120; n++) { if (predicate()) return true; await pause(50); }
    return false;
  };
  const screen = pane => () => Array.from({ length: pane.term.buffer.active.length }, (_, i) => pane.term.buffer.active.getLine(i)?.translateToString(true) || '').join('\n');
  try {
    stopPolling(); setLocale('zh-Hans');
    const project = provider.projects()[0];
    const { card } = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: '语音输入验证', cmd: '', dir: '/tmp' });
    render(); await openSession(card.id);
    const pane = panes.get(card.session), visible = screen(pane);
    const target = { session: card.session, cardId: card.id, title: card.title };
    await pause(800); stage = 1;
    const button = $('voice-btn'), host = $('terminal-host').getBoundingClientRect(), term = $('terminal').getBoundingClientRect();
    await report('voice-workspace', !$('voice-panel') && !$('voice-draft') && term.height > host.height - 60 && term.width > host.width - 4);
    await report('voice-button-idle', button.dataset.phase === 'idle' && button.title === t('voice.phase.idle')
      && button.getAttribute('aria-pressed') === 'false' && $('voice-time').hidden && $('voice-caption').hidden && !button.disabled && voice.state.phase === 'idle');
    stage = 2;
    // The typing path: literal bytes, separators kept, never an Enter.
    await voice.typeText(target, 'DECK_VOICE_TYPED');
    await report('voice-type-visible', await waitFor(() => visible().includes('DECK_VOICE_TYPED')) && state.view === 'session');
    await voice.typeText(target, ' AFTER');
    await report('voice-type-separator', await waitFor(() => visible().includes('DECK_VOICE_TYPED AFTER')));
    await pause(400);
    await report('voice-type-no-enter', !visible().includes('not found') && document.activeElement !== button);
    await inv('pty_write', { name: card.session, dataB64: btoa('\x15') }); await pause(150);
    // A foreground program without bracketed paste still takes a single line.
    await inv('pty_write', { name: card.session, dataB64: btoa('cat\r') }); await pause(600);
    await voice.typeText(target, 'DECK_NEW_FOREGROUND');
    await report('voice-foreground-program', await waitFor(() => visible().includes('DECK_NEW_FOREGROUND')));
    await inv('pty_write', { name: card.session, dataB64: btoa('\x15\x03') }); await pause(400);
    stage = 3;
    await inv('kill_session', { name: card.session });
    let ended = '';
    try { await voice.typeText(target, 'DECK_DEAD'); } catch (error) { ended = String(error); }
    await report('voice-target-ended', ['target-unavailable', 'target-expired', 'target-changed'].includes(ended) && voice.state.phase === 'idle');
    backToBoard(); await report('voice-leave', voice.state.phase === 'idle' && state.view !== 'session');
    stage = 4;
    const sample = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: '重构认证模块', cmd: '', dir: '/tmp' });
    render(); await openSession(sample.card.id); await pause(500);
    await report('voice-preferences-defaults', ctx.settings.voice.languages.join(',') === 'zh-CN,en-US,ja-JP'
      && ctx.settings.voice.defaultLanguage === 'system');
    await openSettings(); selectSettingsSection('terminal');
    await pause(80);
    await report('voice-preferences-layout', [...$('set-voice-languages').querySelectorAll('label')].every(label => {
      const box = label.getBoundingClientRect(), text = label.querySelector('span').getBoundingClientRect();
      const checkbox = label.querySelector('input').getBoundingClientRect();
      return text.width > 0 && checkbox.right < text.left && text.right <= box.right + 1 && text.height < 35;
    }));
    await report('voice-preferences-saved', await persistVoicePreferences({ languages: ['en-US'], defaultLanguage: 'en-US' }));
    await pause(80);
    const saved = JSON.parse((await inv('load_settings')).data);
    await report('voice-preferences-single', saved.voice.languages.join(',') === 'en-US' && saved.voice.defaultLanguage === 'en-US');
    const choices = [...$('set-voice-languages').querySelectorAll('input')];
    await report('voice-preferences-last', choices.length === 8
      && choices.filter(input => input.checked).length === 1
      && choices.find(input => input.checked).disabled && $('set-voice-default').options.length === 2);
    await persistVoicePreferences({ languages: ['zh-CN', 'en-US', 'ja-JP'], defaultLanguage: 'system' });
    await pause(80);
    const restored = JSON.parse((await inv('load_settings')).data);
    await report('voice-preferences-restore', restored.voice.languages.join(',') === 'zh-CN,en-US,ja-JP'
      && restored.voice.defaultLanguage === 'system' && $('set-voice-default').options.length === 4 && voice.state.phase === 'idle');
    $('set-close').click();
    for (const theme of ['light', 'high-contrast', 'deck-dark']) {
      activateTheme({ theme, accent: 'teal' }); await pause(80);
      const style = getComputedStyle(button);
      await report(`voice-theme-${theme}`, style.color !== style.backgroundColor && button.getBoundingClientRect().width > 20);
    }
  } catch (_) { await report(`voice-exception-${stage}`, false); }
  await inv('ui_event', { code: 'smoke-check', detail: 'done', a: failed ? -1 : 1, b: 0 });
}
