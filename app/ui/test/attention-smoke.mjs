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
      // A live fixture card owns a real empty shell on the isolated socket, so
      // attachments stream a real prompt and never end before their reply.
      if (spec.state !== 'stopped') await nativeInvoke('start_session', { name: card.session, dir: '/tmp', cmd: '', restoreShell: false });
      samples.set(spec.id, { spec, card: provider.get(card.id) });
    }
    stage = 1;
    let failPoll = false, missing = null, failAttach = null, starts = 0, attaches = 0, polls = 0;
    let attachGate = null, pendingGen = null, pollGate = null;
    const statuses = new Map();
    for (const { spec, card } of samples.values()) statuses.set(card.session, {
      name: card.session, alive: spec.state !== 'stopped', agent: spec.source === 'hook' ? spec.state : null,
      idle_secs: spec.state === 'quiet' ? 240 : 3, fg: spec.source === 'hook' ? 'fixture-agent' : 'zsh',
      mem_mb: spec.state === 'stopped' ? null : 24,
    });
    const wrappedInvoke = async (command, args) => {
      if (command === 'start_session') starts++;
      if (command === 'attach_session' && args.name === failAttach) throw new Error('isolated attach failure');
      if (command === 'attach_session') attaches++;
      if (command !== 'poll_sessions') {
        const result = await nativeInvoke(command, args);
        if (command === 'attach_session' && attachGate) { pendingGen = result; await attachGate; }
        return result;
      }
      polls++;
      if (failPoll) throw new Error('isolated poll failure');
      // The snapshot is taken when the request leaves, as a real poll's is.
      const snapshot = args.names.filter(name => name !== missing).map(name => ({ ...statuses.get(name) })).filter(Boolean);
      if (pollGate) await pollGate;
      return snapshot;
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
    // A pane whose shell exited is only focused again: no re-attach, no restart.
    await openSession(input.id); await pollNow();
    await nativeTauri.event.emit('pty-exit', { name: input.session, gen: panes.get(input.session).attachedGen });
    await pause(20);
    const attachesBefore = attaches, startsBefore = starts;
    const reopened = await openSession(input.id);
    const bits = checks => checks.reduce((mask, ok, index) => mask | (ok ? 0 : 1 << index), 0);
    const reopenBits = bits([reopened === false, !panes.get(input.session)?.attached, attaches === attachesBefore,
      starts === startsBefore, state.view === 'session', ctx.attachedName === input.session]);
    await report('attention-reopen-detached', reopenBits === 0, 1, reopenBits);
    backToBoard(); await pollNow();
    // An exit that lands before the attach reply keeps the pane detached and unseen.
    statuses.get(input.session).agent = 'working'; await pollNow();
    statuses.get(input.session).agent = 'needs-input'; await pollNow();
    attachGate = new Promise(resolve => { releaseAttach = resolve; });
    pendingGen = null;
    const racing = openSession(input.id);
    for (let i = 0; i < 100 && pendingGen == null; i++) await pause(20);
    await nativeTauri.event.emit('pty-exit', { name: input.session, gen: pendingGen });
    await pause(20);
    releaseAttach(); attachGate = null;
    const raced = await racing; await pollNow();
    const raceBits = bits([raced === false, !panes.get(input.session)?.attached,
      ctx.ptyGens.get(input.session) === pendingGen, !ctx.attention.get(input)?.seen]);
    await report('attention-exit-before-reply', raceBits === 0, 1, raceBits);
    backToBoard(); await pollNow();
    // A poll requested during an in-flight poll runs again after it and hands
    // its callers the follow-up, never the pre-event snapshot.
    let releaseFirst, releaseSecond;
    pollGate = new Promise(resolve => { releaseFirst = resolve; });
    const pollsBefore = polls;
    const first = pollNow();
    for (let i = 0; i < 100 && polls === pollsBefore; i++) await pause(20);
    statuses.get(input.session).agent = 'turn-done';
    const second = pollNow();
    const third = pollNow();
    // The follow-up holds on its own gate so the first result is read before it records.
    pollGate = new Promise(resolve => { releaseSecond = resolve; });
    releaseFirst();
    await first;
    const firstStale = ctx.attention.get(input)?.agent === 'needs-input';
    releaseSecond(); pollGate = null;
    await second;
    const followBits = bits([firstStale, second !== first, third === second,
      polls === pollsBefore + 2, ctx.attention.get(input)?.agent === 'turn-done']);
    await report('attention-poll-followup', followBits === 0, 1, followBits);
    statuses.get(input.session).agent = 'needs-input'; await pollNow();
    stage = 3;
    await reset(); showAttention(); await pollNow();
    const button = [...$('attention-list').querySelectorAll('.attention-row')].find(el => el.dataset.sid === input.id).querySelector('button');
    button.focus();
    await pollNow();
    await report('attention-keyed-focus', document.activeElement === button && button.isConnected);
    // A reorder moves a connected row (remove + insert); focus must survive it.
    const inputRows = [...$('attention-list').querySelectorAll('.attention-row')].filter(el => ctx.attention.category(provider.get(el.dataset.sid)) === 'input');
    const laterButton = inputRows[1].querySelector('button'); laterButton.focus();
    const earlier = provider.get(inputRows[0].dataset.sid);
    statuses.get(earlier.session).agent = 'turn-done'; await pollNow();
    const reordered = $('attention-list').querySelector('.attention-row') === inputRows[1] && inputRows[0].isConnected;
    await report('attention-reorder-focus', reordered && document.activeElement === laterButton);
    statuses.get(earlier.session).agent = 'needs-input'; await pollNow();
    button.focus();
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
    const opened = $('ctx').style.display === 'block' && items.length === 6 && document.activeElement === items[0] && more.getAttribute('aria-expanded') === 'true';
    $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    const moved = document.activeElement === items[1];
    $('ctx').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await report('entry-menu', opened && moved && $('ctx').style.display !== 'block' && document.activeElement === more && more.getAttribute('aria-expanded') === 'false');
    // Dismissal by a click elsewhere or the global Escape clears the menu's keyboard handler.
    more.click(); await pause(20);
    const armed = typeof $('ctx').onkeydown === 'function';
    document.body.click(); await pause(20);
    const clickCleared = $('ctx').style.display !== 'block' && $('ctx').onkeydown === null && more.getAttribute('aria-expanded') === 'false';
    more.click(); await pause(20);
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    const escCleared = $('ctx').style.display !== 'block' && $('ctx').onkeydown === null;
    await report('entry-menu-dismiss', armed && clickCleared && escCleared);
    more.click(); await pause(20);
    [...$('ctx').querySelectorAll('button')][2].click(); await pause(60);
    const slackPreset = !$('auto-drawer').hidden && !$('auto-editor').hidden
      && $('auto-trigger').querySelector('button[aria-pressed="true"]')?.dataset.v === 'slack' && $('ctx').style.display !== 'block';
    closeAutomations();
    await report('entry-automations', slackPreset && $('auto-drawer').hidden && document.activeElement === more);
    // The project tab menu hands both managers a tab resolved at close time.
    const tabMenu = () => document.querySelector('#tabs .tab.active')
      .dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 20, clientY: 20 }));
    tabMenu(); $('ctx').querySelector('[data-a="automations"]').click(); await pause(60);
    const autoOpen = !$('auto-drawer').hidden;
    closeAutomations();
    const autoBack = document.activeElement?.dataset?.pid === atlas.id;
    tabMenu(); $('ctx').querySelector('[data-a="templates"]').click(); await pause(20);
    const tplOpenTab = $('tpl-modal').style.display === 'flex';
    closeTemplates();
    const tplBackTab = document.activeElement?.dataset?.pid === atlas.id;
    await report('entry-project-menu', autoOpen && autoBack && tplOpenTab && tplBackTab);
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
    // 04 A v01: project defaults. The Board's own entries take the project's
    // directory and command (sent once, at creation); context entries never
    // do; a directory that no longer exists asks and leaves no card behind.
    // Real sessions start on the isolated socket with a harmless echo.
    stage = 6;
    const { openProjectDefaults } = await import('../js/board.js');
    const { newSession, newDefaultSession } = await import('../js/terminal.js');
    const { openEditor } = await import('../js/automation.js');
    const { renderSuggest } = await import('../js/terminal.js');
    await reset();
    const base = JSON.stringify(boardData());
    const menuButtons = () => [...$('ctx').querySelectorAll('button')];
    more.click(); await pause(20);
    const plain = menuButtons();
    const plainMenu = plain.length === 6 && !plain.some(b => b.textContent.includes(t('menu.newShellOnly')))
      && plain[0].querySelector('.ctx-hint')?.textContent.includes(atlas.columns[1].name)
      && plain[3].textContent.includes(t('menu.projectDefaults')) && plain[3].querySelector('.ctx-hint')?.textContent === t('projectDefaults.none');
    document.body.click(); await pause(20);
    const smokeCmd = 'echo deck-04-smoke';
    await provider.setProjectDefaults(atlas.id, { dir: '/tmp', cmd: smokeCmd }); await pause(20);
    more.click(); await pause(20);
    const withDefaults = menuButtons();
    const hint0 = withDefaults[0].querySelector('.ctx-hint')?.textContent || '';
    const defaultsMenu = withDefaults.length === 7 && withDefaults[1].textContent.includes(t('menu.newShellOnly'))
      && hint0.includes('/tmp') && hint0.includes(smokeCmd) && hint0.includes(atlas.columns[1].name)
      && withDefaults[4].querySelector('.ctx-hint')?.textContent.includes(smokeCmd)
      && $('board-new').title.includes('/tmp') && $('board-new').title.includes(smokeCmd);
    document.body.click(); await pause(20);
    await report('defaults-menu', plainMenu && defaultsMenu);
    // ＋ with defaults: one start with the command, the card marked launched,
    // no fresh-shell chips (a program owns the pane), the session really alive.
    let startsMark = starts;
    const created = await newDefaultSession(); await pause(80);
    const c1 = created && provider.get(created.id);
    const live1 = c1 ? await nativeInvoke('poll_sessions', { names: [c1.session], tailFor: [], checkpointShells: false }) : [];
    const createOk = !!c1 && c1.dir === '/tmp' && c1.cmd === smokeCmd && c1.launched === true && c1.columnId === atlas.columns[1].id
      && starts === startsMark + 2 && live1[0]?.alive === true && ctx.freshShell === false && state.view === 'session'
      && boardData().cards.find(c => c.id === c1.id)?.launched === true;
    if (c1) await provider.close(c1.id);
    backToBoard(); await pollNow();
    await report('defaults-create', createOk && !provider.get(created?.id));
    // "New shell only": same directory, no command, one real start.
    startsMark = starts;
    const shell = await newDefaultSession({ shellOnly: true }); await pause(80);
    const c2 = shell && provider.get(shell.id);
    const live2 = c2 ? await nativeInvoke('poll_sessions', { names: [c2.session], tailFor: [], checkpointShells: false }) : [];
    // A fresh empty shell offers its recent-command chips (fixed under 04: the
    // flag survives focusPane); history is injected because the isolated data
    // dir has none, and the chips only FILL the line.
    const freshFlag = ctx.freshShell === true;
    ctx.histCache = ['echo deck-04-chip']; renderSuggest(); await pause(60);
    const chipsShown = $('quick-bar').style.display === 'flex' && [...$('quick-bar').querySelectorAll('.qb-chip')].some(b => b.textContent === 'echo deck-04-chip');
    const shellOk = !!c2 && c2.dir === '/tmp' && c2.cmd === '' && c2.launched === true && state.view === 'session'
      && starts === startsMark + 2 && live2[0]?.alive === true && freshFlag && chipsShown;
    if (c2) await provider.close(c2.id);
    backToBoard(); await pollNow();
    await report('defaults-shell-only', shellOk);
    // A shell that exits retires its card through the poll's live→dead
    // transition, exactly as before 04. This harness replaces poll_sessions
    // with a synthetic snapshot that knows only the fixture cards, so the new
    // card is registered in it first; the close itself is the real one.
    const exiting = await newDefaultSession({ shellOnly: true }); await pause(80);
    const c4 = exiting && provider.get(exiting.id);
    if (c4) statuses.set(c4.session, { name: c4.session, alive: true, agent: null, idle_secs: 1, fg: 'zsh', mem_mb: 5 });
    await pollNow();
    const wentLive = !!c4 && c4.status !== 'stopped';
    if (c4) { statuses.get(c4.session).alive = false; statuses.get(c4.session).mem_mb = null; }
    await pollNow(); await pause(80);
    const retired = !!c4 && !provider.get(c4.id) && state.view === 'board' && !panes.has(c4.session);
    if (c4) statuses.delete(c4.session);
    await report('defaults-exit', wentLive && retired);
    // A default directory that does not exist: the dialog names it, nothing
    // was written, cancel leaves it so; the home-shell choice creates a shell.
    await provider.setProjectDefaults(atlas.id, { dir: '/nonexistent/deck-04-smoke', cmd: smokeCmd }); await pause(20);
    const beforeFail = JSON.stringify(boardData());
    startsMark = starts;
    newDefaultSession(); await pause(200);
    const dialogShown = $('chd').style.display === 'flex' && $('chd-msg').textContent.includes('/nonexistent/deck-04-smoke')
      && $('chd-actions').querySelectorAll('button').length === 3 && document.activeElement === $('chd-actions').lastElementChild;
    const noGhost = JSON.stringify(boardData()) === beforeFail && starts === startsMark + 1 && state.view === 'board';
    $('chd-actions').querySelectorAll('button')[0].click(); await pause(20);
    const cancelled = $('chd').style.display !== 'flex' && JSON.stringify(boardData()) === beforeFail;
    newDefaultSession(); await pause(200);
    $('chd-actions').lastElementChild.click(); await pause(200);
    const homeCard = provider.list(atlas.id).find(c => c.dir === ctx.HOME && c.cmd === '');
    const homeOk = !!homeCard && homeCard.launched === true && state.view === 'session' && ctx.freshShell === true;
    if (homeCard) await provider.close(homeCard.id);
    backToBoard(); await pollNow();
    newDefaultSession(); await pause(200);
    $('chd-actions').querySelectorAll('button')[1].click(); await pause(80);
    const editOffered = $('pdf').style.display === 'flex' && $('pdf-dir').value === '/nonexistent/deck-04-smoke';
    $('pdf-no').click(); await pause(20);
    await report('defaults-not-dir', dialogShown && noGhost && cancelled && homeOk && editOffered);
    // A context entry keeps its own directory and is a shell even with a
    // default command; its missing directory asks with two choices only.
    await provider.setProjectDefaults(atlas.id, { dir: '/tmp', cmd: smokeCmd }); await pause(20);
    const here = await newSession('/tmp', { projectId: atlas.id }); await pause(80);
    const c3 = here && provider.get(here.id);
    const hereOk = !!c3 && c3.cmd === '' && c3.dir === '/tmp' && c3.launched === true && state.view === 'session' && ctx.freshShell === true;
    if (c3) await provider.close(c3.id);
    backToBoard(); await pollNow();
    const beforeHere = JSON.stringify(boardData());
    newSession('/nonexistent/deck-04-here', { projectId: atlas.id }); await pause(200);
    const hereDialog = $('chd').style.display === 'flex' && $('chd-actions').querySelectorAll('button').length === 2 && JSON.stringify(boardData()) === beforeHere;
    $('chd-actions').querySelectorAll('button')[0].click(); await pause(20);
    await report('defaults-here', hereOk && hereDialog && $('chd').style.display !== 'flex');
    // A NEW automation rule starts from the defaults; an existing rule keeps its own.
    await openAutomations({ from: more }); await pause(20);
    $('auto-new').click(); await pause(20);
    const prefilled = $('auto-dir').value === '/tmp' && $('auto-cmd').value === smokeCmd;
    openEditor({ ...rule, dir: '/elsewhere', cmd: 'codex' }); await pause(20);
    const existingKept = $('auto-dir').value === '/elsewhere' && $('auto-cmd').value === 'codex';
    closeAutomations();
    await report('defaults-editor', prefilled && existingKept);
    // The dialog: current values, Save clears, Escape and the tab menu entry.
    let pdfPromise = openProjectDefaults(atlas.id, more); await pause(60);
    const dialogValues = $('pdf').style.display === 'flex' && $('pdf-dir').value === '/tmp' && $('pdf-cmd').value === smokeCmd && document.activeElement === $('pdf-dir');
    $('pdf-dir').value = ''; $('pdf-cmd').value = '';
    $('pdf-yes').click(); await pdfPromise; await pause(20);
    const atlasNow = provider.project(atlas.id);
    const cleared = !('dir' in atlasNow) && !('cmd' in atlasNow) && $('pdf').style.display !== 'flex' && document.activeElement === more;
    pdfPromise = openProjectDefaults(atlas.id, more); await pause(60);
    $('pdf').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    await pdfPromise;
    const escaped = $('pdf').style.display !== 'flex' && document.activeElement === more;
    tabMenu(); $('ctx').querySelector('[data-a="defaults"]').click(); await pause(60);
    const fromTab = $('pdf').style.display === 'flex';
    $('pdf-no').click(); await pause(20);
    await report('defaults-dialog', dialogValues && cleared && escaped && fromTab);
    // The empty project: the link without defaults, the promise with them.
    switchProject(emptyProject.id); await pollNow();
    const linkShown = !$('board-empty').hidden && !$('board-empty-defaults').hidden
      && $('board-empty-body').textContent === t('board.emptyBody', { column: emptyTarget.name });
    $('board-empty-defaults').click(); await pause(60);
    const linkOpens = $('pdf').style.display === 'flex';
    $('pdf-no').click(); await pause(20);
    await provider.setProjectDefaults(emptyProject.id, { dir: '/tmp', cmd: 'echo e' }); await pause(20);
    const promised = $('board-empty-defaults').hidden && $('board-empty-body').textContent.includes('/tmp') && $('board-empty-body').textContent.includes('echo e');
    await provider.setProjectDefaults(emptyProject.id, { dir: '', cmd: '' }); await pause(20);
    switchProject(atlas.id); await pollNow();
    await report('defaults-empty', linkShown && linkOpens && promised);
    // Persistence: present values are written as strings; cleared ones leave
    // the project byte-identical to one written before the fields existed.
    await provider.setProjectDefaults(atlas.id, { dir: '~/work/atlas', cmd: 'claude' });
    const saved = boardData().projects.find(p => p.id === atlas.id);
    await provider.setProjectDefaults(atlas.id, { dir: '', cmd: '' });
    const savedAfter = boardData().projects.find(p => p.id === atlas.id);
    await report('defaults-persistence', saved.dir === '~/work/atlas' && saved.cmd === 'claude'
      && !('dir' in savedAfter) && !('cmd' in savedAfter) && JSON.stringify(boardData()) === base);
    await reset(); showAttention(); await pollNow();
    // Buttons in a test-only strip select real implemented views for screenshots.
    const strip = document.createElement('div');
    strip.style.cssText = 'padding:6px 12px;border-bottom:1px solid var(--border);display:flex;gap:12px;color:var(--muted);font-size:12px';
    const label = document.createElement('span'); label.textContent = '隔离验证 · 真实实现 / 虚构状态'; strip.append(label);
    for (const mode of ['看板', '空项目', '待关注', '更新失败', '项目默认值示例', '真实轮询']) {
      const btn = document.createElement('button'); btn.textContent = mode;
      btn.onclick = async () => {
        failPoll = false; missing = null;
        if (mode === '真实轮询') {
          // Hand the page back to the real backend: polls, exits and closes
          // behave as in production from here on ("更新失败" no longer injects).
          window.__TAURI__ = nativeTauri;
          await reset(); switchProject(atlas.id); await pollNow(); return;
        }
        await reset();
        if (mode === '项目默认值示例') {
          // Atlas gets a harmless demo default (a directory that exists and an
          // echo), so ＋ / ▾ / the tab menu can be inspected; the fixture
          // cards are untouched and the values are cleared by 看板.
          await provider.setProjectDefaults(atlas.id, { dir: '/tmp', cmd: 'echo deck-04-demo' });
          switchProject(atlas.id); await pollNow(); return;
        }
        if (mode === '看板') await provider.setProjectDefaults(atlas.id, { dir: '', cmd: '' });
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
