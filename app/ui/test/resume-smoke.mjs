// Real WKWebView + IPC + tmux test; harmless printf output stands in for agents.
export async function runResumeSmoke() {
  const { $, ctx, inv, state } = await import('../js/state.js');
  const { provider, render, stopPolling, prepareCardsForServerRestart, markSessionsStoppedForServerRestart } = await import('../js/board.js');
  const { openSession, leaveSessionView } = await import('../js/layout.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { renderSuggest, resetSuggest, suggestions, updateGhost, acceptGhost } = await import('../js/terminal.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  let failed = false, stage = 0;
  const report = async (name, ok) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? 1 : -1, b: 0 });
  };
  const waitFor = async predicate => {
    for (let i = 0; i < 100; i++) { if (await predicate()) return true; await pause(50); }
    return false;
  };
  const ids = ['01a0b76c-b577-7493-86ef-a1f6209b823e', '0f3ab19c-4d2e-4a71-9b8c-1d2e3f4a5b6c'];
  const cards = [];
  const write = (card, text) => inv('pty_write', { name: card.session, dataB64: strToB64(text) });
  try {
    stopPolling();
    const project = provider.projects()[0];
    for (let i = 0; i < 2; i++) {
      const { card } = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
        title: 'resume-smoke-' + i, cmd: '', dir: '/tmp' });
      cards.push(card); render(); await openSession(card.id); stopPolling();
      await pause(1100);
      stage = 1;
      // A narrow real PTY soft-wraps both exit commands. -J must recover IDs.
      await inv('pty_resize', { name: card.session, cols: 32, rows: 20 });
      await write(card, "printf '\\nTo continue this session, run:\\n\\n  codex resume " + ids[i]
        + "\\nResume this session with:\\nclaude --resume " + ids[i] + "\\n'\r");
      await report('resume-capture-' + i, await waitFor(async () => {
        const hints = await inv('terminal_resume_hints', { name: card.session });
        return hints.length === 2 && hints.every(h => h.id === ids[i]);
      }));
    }
    stage = 2;
    await openSession(cards[0].id); stopPolling();
    provider.get(cards[0].id).fg = 'zsh';
    ctx.histCache = ['codex --yolo resume ' + ids[1]];
    ctx.lineBuf = 'codex --yolo resume ';
    renderSuggest();
    await report('resume-priority', await waitFor(() => suggestions()[0] === ctx.lineBuf + ids[0]));
    await report('resume-chip', $('quick-bar').querySelector('.qb-chip')?.textContent === ctx.lineBuf + ids[0]);
    // Fill the actual shell line; the UI mirror was set separately above.
    await write(cards[0], ctx.lineBuf);
    await pause(200); updateGhost(); acceptGhost();
    await report('resume-ghost', ctx.lineBuf === 'codex --yolo resume ' + ids[0]);
    await pause(150);
    await report('resume-no-execution', (await inv('terminal_resume_hints', { name: cards[0].session })).length === 2);
    await write(cards[0], '\x15');
    stage = 3;
    await pause(200);
    const status = await inv('tmux_server_status');
    ctx.tmuxRestarting = true;
    leaveSessionView({ detach: false });
    state.view = 'board'; state.sessionId = null;
    markSessionsStoppedForServerRestart(); render();
    await prepareCardsForServerRestart(status.sessions);
    await inv('restart_tmux_server', {
      expectedPid: status.serverPid, expectedStartedAt: status.serverStartedAt,
      expectedImpactToken: status.impactToken, expectedSessionCount: status.sessionCount,
      expectedPaneCount: status.paneCount, force: true, restoreShells: true, requestId: 'resume-smoke',
    });
    ctx.tmuxRestarting = false; ctx.settings.sessionRestore = true;
    await openSession(cards[0].id); stopPolling();
    await report('resume-restored', await waitFor(async () => {
      const hints = await inv('terminal_resume_hints', { name: cards[0].session });
      return hints.length === 2 && hints.every(h => h.id === ids[0]);
    }));
    await openSession(cards[1].id); stopPolling();
    provider.get(cards[1].id).fg = 'zsh';
    ctx.histCache = []; ctx.lineBuf = 'claude -r '; renderSuggest();
    await report('resume-pane-isolation', await waitFor(() => suggestions()[0] === 'claude -r ' + ids[1])
      && !suggestions().some(c => c.includes(ids[0])));
    resetSuggest(); provider.get(cards[1].id).fg = 'claude';
    ctx.lineBuf = 'claude -r '; renderSuggest();
    await pause(200);
    await report('resume-agent-hidden', suggestions().length === 0);
    provider.get(cards[1].id).fg = 'zsh'; resetSuggest();
    // Clear the visible screen too: clear-history only removes off-screen rows.
    await write(cards[1], "printf '\\033[2J\\033[H'\r");
    await pause(300);
    await inv('clear_history', { name: cards[1].session });
    ctx.lineBuf = 'claude -r '; renderSuggest(); await pause(250);
    await report('resume-cleared', suggestions().length === 0);
    stage = 4;
  } catch (_) {
    await report('resume-exception', false);
  } finally {
    ctx.tmuxRestarting = false;
    resetSuggest();
    for (const card of cards) await inv('kill_session', { name: card.session }).catch(() => {});
    await inv('ui_event', { code: 'smoke-check', detail: 'resume-done', a: failed ? -1 : 1, b: stage });
  }
}
