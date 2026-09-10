// Real WKWebView + isolated tmux integration; no microphone access. Exercises
// production composer, IPC, target capture, literal paste and separate Enter.
// Recording-phase button checks simulate frontend state without opening a mic.
// Language preferences exercise real settings IPC and enhanced dropdowns.
export async function runVoiceSmoke() {
  const { $, inv, state } = await import('../js/state.js');
  const { provider, render, stopPolling, panes } = await import('../js/board.js');
  const { openSession, backToBoard } = await import('../js/layout.js');
  const { voiceComposer: voice } = await import('../js/voice.js');
  const { setLocale } = await import('../js/i18n.js');
  const { activateTheme } = await import('../js/theme.js');
  const { openSettings, selectSettingsSection, persistVoicePreferences } = await import('../js/dialogs.js');
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
  try {
    stopPolling(); setLocale('zh-Hans');
    const project = provider.projects()[0];
    const { card } = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: '语音输入验证', cmd: '', dir: '/tmp' });
    render(); await openSession(card.id);
    let pane = panes.get(card.session);
    const target = { session: card.session, cardId: card.id, title: card.title };
    await pause(800); stage = 1;
    voice.show(); await voice.bind(target); voice.edit('请检查模块结构，并补充单元测试。');
    const draft = $('voice-draft'); draft.focus(); draft.setSelectionRange(2, 7);
    const attachment = pane.term;
    for (const layout of ['bottom', 'floating', 'right', 'bottom']) {
      voice.layout(layout); await pause(250);
      const rect = $('voice-panel').getBoundingClientRect(), term = $('terminal').getBoundingClientRect();
      await report(`voice-layout-${layout}`, !draft.readOnly && draft === $('voice-draft')
        && draft.selectionStart === 2 && draft.selectionEnd === 7 && document.activeElement === draft
        && rect.width > 250 && rect.height > 100 && term.width > 150 && term.height > 60
        && rect.right <= innerWidth + 1 && rect.bottom <= innerHeight + 1 && pane.term === attachment);
    }
    stage = 2;
    const bindingA = voice.state.target.id;
    voice.language('ja-JP');
    const second = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: '独立语音草稿', cmd: '', dir: '/tmp' });
    render(); await openSession(second.card.id);
    await waitFor(() => !!voice.state.target && !voice.state.switching);
    await report('voice-session-new', voice.state.open && voice.state.session === second.card.session && voice.state.draft === '');
    voice.edit('DECK_VOICE_SESSION_B'); voice.language('en-US');
    const bindingB = voice.state.target.id;
    await openSession(card.id); await waitFor(() => !voice.state.switching);
    pane = panes.get(card.session);
    await report('voice-session-restore', voice.state.open && voice.state.draft === '请检查模块结构，并补充单元测试。'
      && voice.state.target.id === bindingA && voice.state.language === 'ja-JP' && !$('voice-bind') && $('voice-retry').hidden);
    // Neither restoring A nor completing a send may invalidate B's binding.
    await openSession(second.card.id); await waitFor(() => !voice.state.switching);
    await voice.deliver(false);
    const secondPane = panes.get(second.card.session);
    const secondVisible = () => Array.from({ length: secondPane.term.buffer.active.length }, (_, i) => secondPane.term.buffer.active.getLine(i)?.translateToString(true) || '').join('\n');
    await report('voice-session-delivery', voice.state.notice === 'inserted' && voice.state.target?.id === bindingB
      && await waitFor(() => secondVisible().includes('DECK_VOICE_SESSION_B')));
    await inv('pty_write', { name: second.card.session, dataB64: btoa('\x15') });
    await openSession(card.id); await waitFor(() => !voice.state.switching);
    pane = panes.get(card.session);
    // A draft's Enter / IME confirmation must never submit or leave the view.
    draft.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', isComposing: true, metaKey: true, bubbles: true }));
    await report('voice-ime', voice.state.draft.length > 0 && state.view === 'session');
    // Exercise the production handlers while displaying a recording phase.
    // Native capture itself is covered by the microphone test, not this smoke.
    voice.edit(''); voice.state.phase = 'recording'; voice.layout('bottom');
    await report('voice-actions-empty', $('voice-clear').disabled && $('voice-insert').disabled && $('voice-send').disabled);
    draft.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', metaKey: true, bubbles: true }));
    await pause(50);
    await report('voice-actions-empty-noop', voice.state.phase === 'recording' && !voice.state.draft);
    voice.state.phase = 'idle'; voice.edit('清空测试'); voice.state.phase = 'recording'; voice.layout('bottom');
    await report('voice-actions-recording', !$('voice-clear').disabled && !$('voice-insert').disabled && !$('voice-send').disabled && draft.readOnly);
    $('voice-clear').click();
    await report('voice-actions-clear', await waitFor(() => !voice.state.draft && voice.state.phase === 'idle') && voice.state.open);
    voice.edit('DECK_VOICE_INSERT_ONLY'); voice.state.phase = 'recording'; voice.layout('bottom');
    $('voice-insert').click(); await waitFor(() => voice.state.notice === 'inserted');
    await report('voice-insert', voice.state.notice === 'inserted' && !voice.state.draft);
    const visible = () => Array.from({ length: pane.term.buffer.active.length }, (_, i) => pane.term.buffer.active.getLine(i)?.translateToString(true) || '').join('\n');
    await report('voice-insert-visible', await waitFor(() => visible().includes('DECK_VOICE_INSERT_ONLY')));
    // Clear the pending harmless text without execution, then prove explicit send.
    await inv('pty_write', { name: card.session, dataB64: btoa('\x15') }); await pause(150);
    voice.edit("printf 'VOICE_SENT_OK\\n'"); voice.state.phase = 'stopping'; voice.layout('bottom');
    draft.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', metaKey: true, bubbles: true }));
    await waitFor(() => voice.state.notice === 'submitted');
    await report('voice-submit', voice.state.notice === 'submitted' && await waitFor(() => visible().includes('VOICE_SENT_OK')));
    stage = 3;
    // Change the foreground program inside the already-bound session. cat has
    // no bracketed paste: refusal must retain the binding, and a corrected
    // single-line insertion must work without any explicit target confirmation.
    const retainedBinding = voice.state.target.id;
    await inv('pty_write', { name: card.session, dataB64: btoa('cat\r') }); await pause(600);
    voice.edit('DECK_REFUSED_FIRST\nDECK_REFUSED_SECOND'); await voice.deliver(false);
    await report('voice-binding-refusal', voice.state.error === 'multiline-unsupported'
      && voice.state.target?.id === retainedBinding && !voice.state.needsConfirmation && $('voice-retry').hidden
      && !visible().includes('DECK_REFUSED_FIRST'));
    voice.edit('DECK_NEW_FOREGROUND'); await voice.deliver(false);
    await report('voice-binding-current-program', voice.state.notice === 'inserted' && voice.state.target?.id === retainedBinding
      && await waitFor(() => visible().includes('DECK_NEW_FOREGROUND')));
    await inv('pty_write', { name: card.session, dataB64: btoa('\x15\x03') }); await pause(400);
    await voice.bind(target); voice.edit('保留草稿');
    await inv('kill_session', { name: card.session });
    await voice.deliver(true);
    await report('voice-target-ended', voice.state.draft === '保留草稿' && voice.state.phase === 'error');
    backToBoard(); await report('voice-leave', !voice.state.open && voice.state.draft === '保留草稿');
    stage = 4;
    const sample = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: '重构认证模块', cmd: '', dir: '/tmp' });
    render(); await openSession(sample.card.id); await pause(500);
    voice.show(); await voice.bind({ session: sample.card.session, cardId: sample.card.id, title: sample.card.title });
    voice.edit('请把认证中间件拆成独立模块，保留现有接口，并补充权限检查的单元测试。');
    await report('voice-preferences-defaults', $('voice-language').options.length === 3
      && voice.state.languages.join(',') === 'zh-CN,en-US,ja-JP');
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
    await report('voice-preferences-single', saved.voice.languages.join(',') === 'en-US'
      && saved.voice.defaultLanguage === 'en-US' && voice.state.language === 'en-US'
      && $('voice-language').hidden && $('voice-language').closest('.dd').hidden
      && $('voice-language-label').hidden);
    const choices = [...$('set-voice-languages').querySelectorAll('input')];
    await report('voice-preferences-last', choices.length === 8
      && choices.filter(input => input.checked).length === 1
      && choices.find(input => input.checked).disabled && $('set-voice-default').options.length === 2);
    await persistVoicePreferences({ languages: ['zh-CN', 'en-US', 'ja-JP'], defaultLanguage: 'system' });
    await pause(80);
    await report('voice-preferences-restore', !$('voice-language').hidden
      && !$('voice-language').closest('.dd').hidden && $('voice-language').options.length === 3
      && voice.state.language === 'en-US' && !voice.state.recordingId && voice.state.draft.startsWith('请把认证'));
    $('set-close').click();
    for (const theme of ['light', 'high-contrast', 'deck-dark']) {
      activateTheme({ theme, accent: 'teal' }); await pause(80);
      await report(`voice-theme-${theme}`, getComputedStyle($('voice-panel')).color !== getComputedStyle($('voice-panel')).backgroundColor);
    }
  } catch (_) { await report(`voice-exception-${stage}`, false); }
  await inv('ui_event', { code: 'smoke-check', detail: 'done', a: failed ? -1 : 1, b: 0 });
}
