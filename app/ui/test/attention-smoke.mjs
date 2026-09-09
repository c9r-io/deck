// Isolated WKWebView integration gate. Real production DOM, layout and PTY
// attach; only closed status observations/failures are synthetic. All shells
// have empty commands under the fresh smoke data dir/socket. No agent hooks.
// Leaves the approved fixture in the real UI for implementation screenshots.
export async function runAttentionSmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, render, pollNow, stopPolling, panes } = await import('../js/board.js');
  const { mutateBoard, boardData } = await import('../js/persistence.js');
  const { openSession, backToBoard, addSplit, leaveSessionView } = await import('../js/layout.js');
  const { showAttention, openAttentionCard, refreshAttention } = await import('../js/attention.js');
  const { createAttentionTracker } = await import('../js/attention-model.js');
  const { setLocale, t } = await import('../js/i18n.js');
  const { applyFontScale } = await import('../js/font-scale.js');
  const { activateTheme } = await import('../js/theme.js');
  const nativeTauri = window.__TAURI__;
  const nativeInvoke = nativeTauri.core.invoke;
  let stage = 0, failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  try {
    stopPolling(); await pollNow();
    setLocale('en');
    const fixture = (await import('./fixtures/attention-fixture.mjs')).default.cards;
    const projects = new Map();
    for (const name of ['Atlas', 'Beacon', 'Cedar']) {
      const project = projects.size === 0 ? provider.projects()[0] : await provider.createProject(name);
      await provider.renameProject(project.id, name);
      for (const [index, column] of project.columns.entries()) {
        await provider.renameColumn(project.id, column.id, ['Attention', 'Working', 'Queued', 'Parked'][index]);
      }
      projects.set(name, provider.project(project.id));
    }
    const samples = new Map();
    for (const spec of fixture) {
      const project = projects.get(spec.project);
      const column = project.columns.find(c => c.name === spec.group);
      let card;
      if (spec.state === 'stopped') {
        card = { id: 'attention-stopped', session: 'deck-attention-stopped', projectId: project.id, columnId: column.id, title: spec.title, cmd: '', dir: '/tmp', desc: '', pinned: false, status: 'stopped' };
        await mutateBoard(draft => { draft.cards.push(card); });
      } else card = await provider.create({ projectId: project.id, columnId: column.id, title: spec.title, cmd: '', dir: '/tmp' });
      if (spec.pin) await provider.togglePinned(card.id);
      samples.set(spec.id, { spec, card: provider.get(card.id) });
    }
    stage = 1;
    let failPoll = false, missing = null, failAttach = null, starts = 0;
    let attachGate = null;
    const statuses = new Map();
    for (const { spec, card } of samples.values()) statuses.set(card.session, {
      name: card.session, alive: spec.state !== 'stopped', agent: spec.source === 'hook' ? spec.state : null,
      idle_secs: spec.state === 'quiet' ? 240 : 3, fg: spec.source === 'hook' ? 'fixture-agent' : 'zsh',
      mem_mb: spec.state === 'stopped' ? null : 24, tail: ['', '', '', '', '[isolated sample]', spec.detail],
    });
    const wrappedInvoke = async (command, args) => {
      if (command === 'start_session') starts++;
      if (command === 'attach_session' && args.name === failAttach) throw new Error('isolated attach failure');
      if (command !== 'poll_sessions') {
        const result = await nativeInvoke(command, args);
        if (command === 'attach_session' && attachGate) await attachGate;
        return result;
      }
      if (failPoll) throw new Error('isolated poll failure');
      return args.names.filter(name => name !== missing).map(name => statuses.get(name)).filter(Boolean);
    };
    window.__TAURI__ = { ...nativeTauri, core: { ...nativeTauri.core, invoke: wrappedInvoke } };
    const reset = async () => {
      leaveSessionView();
      ctx.attentionReturn = null; ctx.attentionFilter = 'pending';
      ctx.attention = createAttentionTracker();
      state.projectId = projects.get('Atlas').id; state.view = 'board'; state.sessionId = null;
      await pollNow();
      for (const { spec, card } of samples.values()) if (spec.read) ctx.attention.saw(card);
      render();
    };
    await reset(); setLocale('zh-Hans'); render();
    const original = JSON.stringify(boardData());
    const counts = ctx.attention.counts(store.cards);
    await report('attention-fixture', counts.pending === 4 && counts.input === 2 && counts.done === 2 && counts.unavailable === 3 && counts.stopped === 1, counts.pending, 4);
    const visible = [...document.querySelectorAll('#columns .card')].filter(el => !el.hidden);
    await report('attention-board', !$('board-attention') && !$('board-view').querySelector('.attention-tools')
      && visible.length === 6 && visible.every(el => el.draggable) && !!$('attention-btn')
      && JSON.stringify(boardData()) === original);
    showAttention(); await pollNow();
    await report('attention-origin', $('attention-list').querySelectorAll('.attention-row').length === 4
      && $('attention-list').textContent.includes('Beacon / Parked') && document.querySelectorAll('#side-list .side-item').length === 6);
    stage = 2;
    const ending = samples.get('07').card;
    const input = samples.get('01').card;
    failAttach = ending.session;
    await openAttentionCard(ending.id); await pollNow();
    await report('attention-attach-fail', ctx.attention.category(ending) === 'done' && !panes.get(ending.session)?.attached && starts === 0);
    backToBoard(); await pollNow();
    failAttach = null;
    $('attention-list').style.maxHeight = '140px';
    $('attention-list').scrollTop = 80;
    const priorScroll = $('attention-list').scrollTop;
    let releaseAttach;
    attachGate = new Promise(resolve => { releaseAttach = resolve; });
    const opening = openAttentionCard(ending.id);
    for (let i = 0; i < 100 && !panes.get(ending.session)?.renderedGen; i++) await pause(20);
    await pollNow();
    await report('attention-attach-pending', ctx.attention.category(ending) === 'done' && !panes.get(ending.session)?.attached);
    releaseAttach(); attachGate = null; await opening;
    for (let i = 0; i < 100 && !ctx.attention.get(ending)?.seen; i++) await pause(20);
    await pollNow();
    await report('attention-seen', panes.get(ending.session)?.attached && ctx.attention.counts(store.cards).done === 1);
    await nativeTauri.event.emit('pty-exit', { name: ending.session, gen: panes.get(ending.session).attachedGen - 1 });
    await pause(20);
    await report('attention-exit-generation', !!panes.get(ending.session)?.attached);
    backToBoard(); await pollNow();
    await report('attention-return', state.view === 'attention' && ctx.attentionFilter === 'pending' && state.projectId === projects.get('Atlas').id
      && $('attention-list').scrollTop === priorScroll && $('attention-tools').contains(document.activeElement));
    $('attention-list').style.maxHeight = '';
    await openAttentionCard(input.id);
    for (let i = 0; i < 100 && !ctx.attention.get(input)?.seen; i++) await pause(20);
    await pollNow();
    await report('attention-input-seen', ctx.attention.category(input) === 'input' && ctx.attention.get(input).seen);
    // A failed split must not acknowledge a newly observed ending.
    statuses.get(ending.session).agent = 'working'; await pollNow();
    statuses.get(ending.session).agent = 'turn-done'; await pollNow();
    failAttach = ending.session;
    await addSplit(input.id, 'row', false, ending.id); await pollNow();
    await report('attention-split-fail', ctx.attention.category(ending) === 'done' && !panes.get(ending.session)?.attached);
    failAttach = null;
    backToBoard(); await pollNow();
    stage = 3;
    await reset(); showAttention(); await pollNow();
    const button = [...$('attention-list').querySelectorAll('.attention-row')].find(el => el.dataset.sid === input.id).querySelector('button');
    button.focus();
    await pollNow();
    await report('attention-keyed-focus', document.activeElement === button && button.isConnected);
    button.dispatchEvent(new Event('pointerdown', { bubbles: true }));
    statuses.get(input.session).agent = 'working'; await pollNow();
    const held = button.isConnected;
    document.dispatchEvent(new Event('pointerup', { bubbles: true })); await pause(20);
    await report('attention-pointer', held && !button.isConnected && $('attention-tools').contains(document.activeElement));
    statuses.get(input.session).agent = 'needs-input';
    await reset(); showAttention(); await pollNow();
    failPoll = true; await pollNow();
    await report('attention-stale', ctx.attention.counts(store.cards).pending === 4 && !$('attention-tools').querySelector('.attention-notice').hidden
      && [...$('attention-list').querySelectorAll('.attention-row button')].every(el => el.textContent === t('attention.locate')));
    failPoll = false; missing = ending.session; await pollNow();
    await report('attention-partial', ctx.attention.get(ending).stale && !ctx.attention.get(input).stale && ctx.attention.freshness(store.cards).kind === 'stale');
    missing = null; await pollNow();
    ctx.attentionFilter = 'stopped'; refreshAttention();
    await openAttentionCard(samples.get('05').card.id); await pollNow();
    await report('attention-locate-only', starts === 0 && state.view === 'board' && document.activeElement?.dataset.sid === samples.get('05').card.id);
    stage = 4;
    // Real WebKit layout at the native window size, 12 locale/theme/font combinations.
    showAttention(); ctx.attentionFilter = 'pending'; refreshAttention();
    let layoutChecks = 0;
    for (const locale of ['en', 'zh-Hans']) for (const theme of ['deck-dark', 'light', 'high-contrast']) for (const scale of [1, 1.6]) {
      setLocale(locale); activateTheme({ theme, accent: 'teal' }); applyFontScale(scale); render(); await pause(25);
      const list = $('attention-list');
      const buttons = [...list.querySelectorAll('button')];
      if (list.scrollWidth <= list.clientWidth + 1 && buttons.every(el => el.getBoundingClientRect().right <= list.getBoundingClientRect().right + 1)) layoutChecks++;
    }
    await report('attention-layout', layoutChecks === 12, layoutChecks, 12);
    activateTheme({ theme: 'deck-dark', accent: 'teal' }); applyFontScale(1); setLocale('zh-Hans');
    await reset(); showAttention(); await pollNow();
    await report('attention-persistence', JSON.stringify(boardData()) === original && starts === 0);
    // 06 B v01: the Board's entry points, the empty-project start, and one
    // vocabulary per object — real DOM and real focus, no session starts.
    stage = 5;
    const { openAutomations, closeAutomations } = await import('../js/automation.js');
    const { openTemplates, closeTemplates } = await import('../js/templates.js');
    const { persistInbound } = await import('../js/dialogs.js');
    const { switchProject } = await import('../js/board.js');
    await reset();
    const more = $('board-new-more');
    const atlas = projects.get('Atlas');
    await report('entry-head', !!more && !$('board-tpl') && $('board-auto').hidden && $('board-empty').hidden
      && $('board-view').querySelectorAll('.board-head .btn').length === 3 && starts === 0);
    const emptyProject = await provider.createProject('Empty');
    switchProject(emptyProject.id); await pollNow();
    const emptyTarget = emptyProject.columns.find(c => c.semantic === 'working');
    const emptyShown = !$('board-empty').hidden && $('board-empty-body').textContent.includes(emptyTarget.name)
      && $('board-empty-key').textContent === '⌘N'
      && [...document.querySelectorAll('.attention-column-empty')].every(el => el.hidden);
    switchProject(atlas.id); await pollNow();
    await report('entry-empty', emptyShown && $('board-empty').hidden && starts === 0);
    more.click(); await pause(20);
    const items = [...$('ctx').querySelectorAll('button')];
    const opened = $('ctx').style.display === 'block' && items.length === 5 && document.activeElement === items[0] && more.getAttribute('aria-expanded') === 'true';
    $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    const moved = document.activeElement === items[1];
    $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await report('entry-menu', opened && moved && $('ctx').style.display !== 'block' && document.activeElement === more && more.getAttribute('aria-expanded') === 'false');
    more.click(); await pause(20);
    [...$('ctx').querySelectorAll('button')][2].click(); await pause(60);
    const slackPreset = !$('auto-drawer').hidden && !$('auto-editor').hidden
      && $('auto-trigger').querySelector('button[aria-pressed="true"]')?.dataset.v === 'slack' && $('ctx').style.display !== 'block';
    closeAutomations();
    await report('entry-automations', slackPreset && $('auto-drawer').hidden && document.activeElement === more);
    const ruleId = 'entry' + Date.now().toString(36);
    const rule = { id: ruleId, source: 'clock', badge: ruleId, projectId: atlas.id, columnId: atlas.columns[1].id,
      cmd: '', template: 'none', dir: '', name: 'Entry smoke', enabled: true,
      schedule: { unit: 'day', days: [], minute: 540 }, finish: 'keep', since: 0 };
    const before = ctx.settings.inbound;
    await persistInbound({ ...before, rules: [...before.rules, rule] }); render(); await pause(20);
    const chipShown = !$('board-auto').hidden && $('board-auto-count').textContent.includes('1');
    $('board-auto').click(); await pause(60);
    const chipOpens = !$('auto-drawer').hidden && $('board-auto').classList.contains('on');
    closeAutomations();
    await persistInbound({ ...before, rules: before.rules.filter(r => r.id !== ruleId) }); render(); await pause(20);
    await report('entry-chip', chipShown && chipOpens && $('board-auto').hidden);
    openTemplates(more);
    const tplOpen = $('tpl-modal').style.display === 'flex';
    closeTemplates();
    const tplBack = document.activeElement === more;
    await openSession(samples.get('02').card.id); await pollNow();
    $('queue-btn').click(); await pause(20);
    $('q-tpl').click(); await pause(20);
    const rows = [...$('tpl-pop').querySelectorAll('.t-row')];
    const manage = rows[rows.length - 1];
    const manageShown = $('tpl-pop').style.display === 'block' && !!manage && manage.textContent.includes(t('queue.manageTemplates'));
    manage.click(); await pause(20);
    const manageOpens = $('tpl-modal').style.display === 'flex' && $('tpl-pop').style.display !== 'block';
    closeTemplates();
    const manageBack = document.activeElement === $('q-tpl');
    $('queue-btn').click();
    backToBoard(); await pollNow();
    await report('entry-templates', tplOpen && tplBack && manageShown && manageOpens && manageBack);
    setLocale('zh-Hans'); render();
    const zhNames = $('home-btn').querySelector('.label').textContent === '看板'
      && document.querySelector('label[for="auto-column"]').textContent === '栏目'
      && t('automation.when') !== t('automation.trigger') && $('side-count').previousElementSibling.textContent === 'session';
    setLocale('en'); render();
    const enNames = document.querySelector('label[for="auto-column"]').textContent === 'Group' && $('home-btn').querySelector('.label').textContent === 'Board';
    setLocale('zh-Hans'); render();
    await report('entry-vocabulary', zhNames && enNames);
    await reset(); showAttention(); await pollNow();
    // Buttons in a test-only strip select real implemented views for screenshots.
    const strip = document.createElement('div');
    strip.style.cssText = 'padding:6px 12px;border-bottom:1px solid var(--border);display:flex;gap:12px;color:var(--muted);font-size:12px';
    const label = document.createElement('span'); label.textContent = '隔离验证 · 真实实现 / 虚构状态'; strip.append(label);
    for (const mode of ['看板', '空项目', '待关注', '更新失败']) {
      const btn = document.createElement('button'); btn.textContent = mode;
      btn.onclick = async () => {
        failPoll = false; missing = null;
        await reset();
        if (mode === '空项目') { switchProject(emptyProject.id); await pollNow(); return; }
        if (mode !== '看板') { showAttention(); await pollNow(); }
        if (mode === '更新失败') { failPoll = true; await pollNow(); }
      };
      strip.append(btn);
    }
    $('main').insertBefore(strip, $('tabs'));
    await report('done', !failed);
  } catch (error) {
    await nativeInvoke('ui_event', { code: 'js-reject', detail: error.name || 'error', a: stage, b: Number((String(error.stack).match(/attention-smoke\.mjs:(\d+)/) || [])[1]) || 0 });
    await report('attention-stage', false, stage);
    await report('done', false);
  }
}
