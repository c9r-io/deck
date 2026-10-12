// What Pi's rows, notes and reasons LOOK like in the real window: the
// `pi-layout` WKWebView smoke. Debug carrier only.
//
// The production modules run in the bundled webview and paint the real
// Settings dialog, the real cards, the real Needs attention view, the real
// "Why?" dialog and the real shell
// service restart review, at the window's own size (1280×800, the supported
// minimum), in both languages and at the smallest, the default and the
// largest text size. What it proves is PRESENTATION: that the two switches
// and their note, a card's Pi status and its explanation, and the review's
// Pi row with its reason are visible, are where they belong, overflow
// nothing, overlap nothing and can be reached — also with long titles and
// with Pi, Claude, Codex and plain shells side by side.
//
// Controlled state, labelled as such: NO Pi, Claude or Codex runs here. The
// cards are real cards with real (empty) shell sessions, and what the BACKEND
// would answer about them is scripted in this page only, at the one place
// every invoke leaves it (`window.__TAURI__.core.invoke`):
//   - `agent_hooks_status`      each state of Pi's settings entry in turn;
//   - `agent_hooks_set`         refused with one closed code (the toast);
//   - `poll_sessions`           the real answer, with the Signal fields of
//                               this run's cards replaced;
//   - `tmux_server_status`, `agent_reentry_preview`, `restart_tmux_server`,
//     `agent_reentry_operation` the review and its result, as in
//                               reentry-smoke.mjs.
// It shows how a state READS, never that the backend reached it; the real
// backend with a real Pi is the `pi-live` mode. No shell service is
// restarted and no agent is started: `restart_tmux_server` never reaches the
// backend, and the page keeps refusing it when the carrier ends.
// Every scripted reply also carries fields shaped like a native session id,
// a pid, a digest and paths; none of them may be painted (`pilay-no-native`).
// Each scene reports by itself, so one failing scene never hides the next.
// Snapshots of the window land in <data dir>/evidence/.
export async function runPiLayoutSmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, pollNow } = await import('../js/board.js');
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { backToBoard } = await import('../js/layout.js');
  const { getLocale, setLocale, t } = await import('../js/i18n.js');
  const { applyFontScale, getFontScale } = await import('../js/font-scale.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) throw new Error('bounded layout condition timeout');
      await pause(40);
    }
  };

  const nativeTauri = window.__TAURI__;
  const nativeInvoke = nativeTauri.core.invoke;
  let failed = false;
  let details = 0;
  /* reports go straight to the backend, whatever the script does to other commands */
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  const metric = (name, a = 0, b = 0) => nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: Math.trunc(a), b: Math.trunc(b) });
  /* which combination of a scene failed and which of its parts: numbers only, and only when something failed */
  const tell = async (scene, index, mask) => {
    if (details++ < 24) await nativeInvoke('ui_event', { code: 'smoke-check', detail: 'pilay-detail', a: -(scene * 1000 + index + 1), b: mask });
  };
  const shots = { asked: 0, written: 0 };
  const snapshot = async name => {
    shots.asked += 1;
    await pause(120);
    const code = await nativeInvoke('smoke_native_snapshot', { name }).catch(() => -9);
    if (code === 0) shots.written += 1;
  };

  /* ---- geometry: what "fits" means here ---- */
  const rect = node => node.getBoundingClientRect();
  const shown = node => !!node && node.getClientRects().length > 0 && rect(node).height > 0
    && getComputedStyle(node).visibility !== 'hidden';
  const insideX = (inner, outer, slack = 1) => inner.left >= outer.left - slack && inner.right <= outer.right + slack;
  const inside = (inner, outer, slack = 1) => insideX(inner, outer, slack)
    && inner.top >= outer.top - slack && inner.bottom <= outer.bottom + slack;
  const view = () => ({ left: 0, top: 0, right: innerWidth, bottom: innerHeight });
  const noOverflowX = node => node.scrollWidth <= node.clientWidth + 1;
  /* nothing lies over the middle of `node`: what is there is the node, a part of it, or the label that wraps it */
  const reachable = node => {
    const r = rect(node);
    const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return !!top && (top === node || node.contains(top) || (top.tagName === 'LABEL' && top.contains(node)));
  };
  /* in reading order, one under the other, none on top of the next */
  const stacked = nodes => nodes.every((node, index) => index === 0 || rect(node).top >= rect(nodes[index - 1]).bottom - 1);
  const bits = parts => parts.reduce((mask, ok, index) => mask | (ok ? 0 : 1 << index), 0);

  /* planted values no surface may show */
  const UUID = '3f2b8c1e-9a4d-4c7b-8e21-0d5f6a7b8c9d';
  const DIGEST = '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08';
  const planted = {
    nativeSessionId: UUID, sessionId: UUID, pid: 48211, foregroundPid: 48211, birth: 1760000048211,
    binaryHash: DIGEST, executable: '/Users/someone/.local/lib/node_modules/pi', lease: 48211,
    transcriptPath: `/Users/someone/.pi/agent/sessions/x/${UUID}.jsonl`, sessionFile: '/Users/someone/work/s.jsonl',
  };
  const NATIVE = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32,}|48211|\/Users\/|\.jsonl|tok-pilay/i;
  const painted = [];
  const capture = (...nodes) => { for (const node of nodes) if (node) painted.push(node.textContent); };

  const LOCALES = ['en', 'zh-Hans'];
  const SCALES = [0.5, 1, 1.6];
  const tag = (language, scale) => `${language === 'en' ? 'en' : 'zh'}-${Math.round(scale * 100)}`;
  const each = async body => {
    for (const language of LOCALES) {
      if (getLocale() !== language) setLocale(language);
      for (const scale of SCALES) {
        applyFontScale(scale);
        await pause(140);
        await body(language, scale);
      }
    }
  };

  const LONG_EN = 'Pi · refactor the scheduler hold path, then verify every reader of the evidence store against the kernel facts it was bound to';
  const LONG_ZH = 'Pi · 重构调度器的保持路径，并逐一核对证据存储的每个读取方是否仍与其绑定的内核事实一致（含超长标题换行与截断）';
  const LONG_CLAUDE = 'Claude · a candidate whose title is long enough to push its launch profile note toward the edge of the review';

  /* ---- the script ---- */
  const scripted = { status: null, refusal: null, poll: {}, server: null, preview: null, previewReads: 0, restarts: [], operation: null };
  const REFUSED = new Set(['kill_session', 'pty_write', 'mcp_start_session', 'voice_deliver', 'defer_tmux_restart', 'agent_pi_lifecycle_set']);
  let refused = 0;
  const script = async (command, args) => {
    if (command === 'agent_hooks_status' && scripted.status) return { ...scripted.base, ...scripted.status, ...planted };
    if (command === 'agent_hooks_set') throw new Error(scripted.refusal || 'pi-config-unsupported-location');
    if (command === 'poll_sessions') {
      const real = await nativeInvoke(command, args);
      return real.map(entry => {
        const mine = scripted.poll[entry.name];
        if (!mine) return entry;
        const next = { ...entry, ...planted };
        for (const key of ['agent', 'episode', 'episode_viewed', 'pi_signal', 'pi_session', 'codex_signal', 'fg']) {
          if (mine[key] === undefined) delete next[key];
          else next[key] = mine[key];
        }
        return next;
      });
    }
    if (command === 'tmux_server_status' && scripted.server) return { ...scripted.server };
    if (command === 'agent_reentry_preview' && scripted.server) {
      scripted.previewReads += 1;
      if (!scripted.preview) throw new Error('reentry-preview-unavailable');
      return structuredClone(scripted.preview);
    }
    if (command === 'acknowledge_tmux_lifecycle_notice') return null;
    if (command === 'restart_tmux_server') {
      scripted.restarts.push(structuredClone(args));
      if (!scripted.operation) throw new Error('refused by the Pi layout smoke');
      return { replaced: true, reentry: structuredClone(scripted.operation), ...planted,
        server: { ...scripted.server, serverPid: scripted.server.serverPid + 1, serverStartedAt: scripted.server.serverStartedAt + 1 } };
    }
    if (command === 'agent_reentry_operation') return { known: true, result: structuredClone(scripted.operation) };
    if (REFUSED.has(command)) { refused += 1; throw new Error('refused by the Pi layout smoke'); }
    return nativeInvoke(command, args);
  };

  const locale = getLocale();
  const scale = getFontScale();
  const realStatus = ctx.tmuxServerStatus;
  let stage = 0;
  let installed = false;
  const cards = {};
  const cardNode = card => document.querySelector(`.card[data-sid="${card.id}"]`);
  const closeDialogs = () => {
    if ($('chd').style.display === 'flex') $('chd-actions').querySelector('button')?.click();
    if ($('cfm').style.display === 'flex') $('cfm-no').click();
  };
  try {
    stage = 1;
    await metric('pilay-m-window', innerWidth, innerHeight);
    // the cards: real cards with real, empty shell sessions; what runs in them is scripted below
    state.view === 'board' || backToBoard();
    const project = provider.projects()[0];
    const column = project.columns[0];
    const make = async title => (await provider.createStarted({ projectId: project.id, columnId: column.id, title, cmd: '', dir: '/tmp' })).card;
    cards.piLong = await make(LONG_EN);
    cards.piZh = await make(LONG_ZH);
    cards.piOld = await make('Pi · an older release');
    cards.claude = await make(LONG_CLAUDE);
    cards.claudeTwo = await make('Claude · ordinary');
    cards.codex = await make('Codex · shared service');
    cards.shell = await make('plain shell');
    scripted.base = await nativeInvoke('agent_hooks_status');
    window.__TAURI__ = { ...nativeTauri, core: { ...nativeTauri.core, invoke: script } };
    scripted.status = { pi: false, pi_lifecycle: false, pi_installation: 'absent' };
    installed = window.__TAURI__.core.invoke === script && (await inv('agent_hooks_status')).pi_installation === 'absent';
    if (!installed) throw new Error('the script is not installed');

    // ---- 1. the Board: a Pi's status on its card, beside Claude, Codex and a shell ----
    stage = 2;
    const words = (session, fields) => { scripted.poll[session] = fields; };
    words(cards.piLong.session, { fg: 'node', agent: 'working', episode: 1, episode_viewed: false, pi_signal: 'connected', pi_session: 'unsupported' });
    words(cards.piZh.session, { fg: 'pi', pi_signal: 'unavailable' });
    words(cards.piOld.session, { fg: 'pi', pi_signal: 'incompatible' });
    words(cards.claude.session, { fg: 'claude', agent: 'turn-done', episode: 2, episode_viewed: false });
    words(cards.claudeTwo.session, { fg: 'claude', agent: 'needs-input', episode: 3, episode_viewed: false });
    words(cards.codex.session, { fg: 'codex', codex_signal: 'unavailable' });
    await pollNow();
    await until(() => ctx.attention.get(cards.piZh)?.piSignal === 'unavailable' && !!cardNode(cards.piLong));
    const statusOf = card => cardNode(card).querySelector('.card-status-text').textContent;
    const helpOf = card => cardNode(card).querySelector('.card-signal-help');
    let boardOk = 0, boardAll = 0;
    await each(async (language, size) => {
      await pollNow();
      await pause(60);
      const all = Object.values(cards).map(cardNode);
      const parts = [
        all.every(node => shown(node)),
        statusOf(cards.piLong) === t('attention.working'),
        statusOf(cards.piZh) === t('signal.pi.unavailable'),
        statusOf(cards.piOld) === t('signal.pi.incompatible'),
        statusOf(cards.codex) === t('signal.codex.unavailable'),
        [cards.piLong, cards.piZh, cards.piOld, cards.codex].every(card => shown(helpOf(card)) && helpOf(card).textContent === t('signal.details')),
        [cards.claude, cards.claudeTwo, cards.shell].every(card => helpOf(card).hidden),
        // every card stays inside its column, and its status line inside the card
        all.every(node => insideX(rect(node), rect(node.parentElement)) && insideX(rect(node.querySelector('.card-status')), rect(node))),
        // what a Pi adds: a status text that is cut short rather than run over, and a link that is whole and can be reached
        [cards.piLong, cards.piZh, cards.piOld, cards.codex].every(card => {
          const text = cardNode(card).querySelector('.card-status-text'), help = helpOf(card);
          // the column scrolls: a card is brought into it before anything is asked about what lies over its link
          cardNode(card).scrollIntoView({ block: 'nearest' });
          return insideX(rect(help), rect(cardNode(card))) && rect(text).right <= rect(help).left + 1
            && help.scrollWidth <= help.clientWidth + 1 && reachable(help);
        }),
        // a card without an attention badge holds everything it shows, long title included
        [cards.piZh, cards.piOld, cards.codex, cards.shell].every(card => noOverflowX(cardNode(card))
          && insideX(rect(cardNode(card).querySelector('.card-title')), rect(cardNode(card)))),
        // coverage is no attention: only the two Claude cards ask for the user
        ctx.attention.counts(store.cards).input === 1 && ctx.attention.counts(store.cards).done === 1,
      ];
      boardAll += 1;
      if (parts.every(Boolean)) boardOk += 1; else await tell(1, boardAll, bits(parts));
      // Not Pi's, and judged by nothing here — a measurement of the card's own chrome at this window size:
      // a = cards whose top row runs over the card (an attention badge beside a title), b = Pi/Codex status texts cut short
      await metric('pilay-m-card',
        all.filter(node => node.querySelector('.card-top').scrollWidth > node.querySelector('.card-top').clientWidth + 1).length,
        [cards.piLong, cards.piZh, cards.piOld, cards.codex].filter(card => {
          const text = cardNode(card).querySelector('.card-status-text');
          return text.scrollWidth > text.clientWidth + 1;
        }).length);
      capture(...all);
      cardNode(cards.piLong).scrollIntoView({ block: 'nearest' });
      if (size !== 0.5 || language === 'en') await snapshot(`pilay-board-${tag(language, size)}`);
    });
    await report('pilay-board', boardOk === boardAll, boardAll, boardOk);

    // ---- 1b. Needs attention: a Pi's unread ending is listed like any other; a Pi with nothing but coverage asks for nothing ----
    stage = 12;
    const { showAttention, attentionStatusText } = await import('../js/attention.js');
    words(cards.piLong.session, { fg: 'node', agent: 'turn-done', episode: 4, episode_viewed: false, pi_signal: 'connected', pi_session: 'observed' });
    await pollNow();
    await until(() => ctx.attention.get(cards.piLong)?.agent === 'turn-done');
    showAttention();
    await until(() => state.view === 'attention' && !$('attention-view').hidden && !!$('attention-list').querySelector('.attention-row'));
    const attentionRow = card => [...$('attention-list').children].find(el => el.dataset.sid === card.id);
    const attentionRows = () => [...$('attention-list').children].filter(el => el.classList.contains('attention-row'));
    const pickFilter = async name => {
      [...$('attention-tools').querySelectorAll('button')].find(button => button.dataset.filter === name).click();
      await pause(100);
    };
    /* a row holds its cells, its cells hold their text, and its one button can be reached */
    const rowFits = row => {
      row.scrollIntoView({ block: 'nearest' });
      const open = row.querySelector('.attention-open');
      const cells = [...row.querySelectorAll(':scope > span')];
      return insideX(rect(row), rect($('attention-list'))) && noOverflowX(row)
        && cells.every(cell => insideX(rect(cell), rect(row)) && noOverflowX(cell))
        && shown(open) && insideX(rect(open), rect(row)) && reachable(open);
    };
    const asking = [cards.claudeTwo, cards.piLong, cards.claude];       // an input request, then the two unread endings
    const quiet = [cards.piZh, cards.piOld, cards.codex, cards.shell];  // nothing but coverage, or nothing at all
    let attentionOk = 0, attentionAll = 0;
    await each(async (language, size) => {
      await pollNow();
      await pickFilter('pending');
      const listed = asking.map(attentionRow);
      const pi = listed[1];
      const first = [
        listed.every(Boolean) && quiet.every(card => !attentionRow(card)) && attentionRows().length === 3,
        // the Pi's row: its own title, the word its card shows, and an ending nobody has looked at
        !!pi && pi.querySelector('.attention-card-title').textContent === cards.piLong.title
          && pi.querySelector('.attention-reason').textContent === attentionStatusText(cards.piLong)
          && attentionStatusText(cards.piLong) === t('attention.doneUnread')
          && pi.querySelector('.attention-viewed').textContent === t('attention.notViewed'),
        noOverflowX($('attention-list')) && inside(rect($('attention-view')), view()),
        listed.every(row => !!row && rowFits(row)),
        // in the order the list promises: what waits for input first
        stacked(attentionRows()) && attentionRows()[0] === listed[0],
        // the count in the window's top bar is those three
        ctx.attention.counts(store.cards).input === 1 && ctx.attention.counts(store.cards).done === 2
          && $('attention-count').textContent.includes('3'),
      ];
      capture($('attention-list'), $('attention-tools'));
      if (size !== 0.5) { if (pi) pi.scrollIntoView({ block: 'center' }); await snapshot(`pilay-attention-${tag(language, size)}`); }
      // what only has coverage is found where the list keeps what it cannot see into — and nowhere else
      await pickFilter('unavailable');
      const reason = card => attentionRow(card)?.querySelector('.attention-reason').textContent;
      const second = [
        quiet.every(card => !!attentionRow(card)) && asking.every(card => !attentionRow(card)),
        reason(cards.piZh) === t('signal.pi.unavailable') && reason(cards.piOld) === t('signal.pi.incompatible')
          && reason(cards.codex) === t('signal.codex.unavailable'),
        quiet.every(card => !!attentionRow(card) && rowFits(attentionRow(card))),
        noOverflowX($('attention-list')) && stacked(attentionRows()),
      ];
      capture($('attention-list'));
      if (size === 1.6) await snapshot(`pilay-attention-quiet-${tag(language, size)}`);
      const parts = [...first, ...second];
      attentionAll += 1;
      if (parts.every(Boolean)) attentionOk += 1; else await tell(11, attentionAll, bits(parts));
    });
    await pickFilter('pending');
    $('attention-back').click();                                        // the view's own way back to the Board
    await until(() => state.view === 'board' && !!cardNode(cards.piLong));
    await report('pilay-attention', attentionOk === attentionAll, attentionAll, attentionOk);

    // ---- 2. "Why?": what a card says about its Pi, for every coverage and session word ----
    stage = 3;
    const WHY = [
      ['connected', undefined, ['signal.pi.connectedHint', 'signal.pi.hold'], 'connected'],
      ['connected', 'observed', ['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.session'], 'observed'],
      ['connected', 'unsupported', ['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.sessionUnsupported'], 'unsupported'],
      ['unavailable', undefined, ['signal.pi.unavailableHint', 'signal.pi.hold'], 'unavailable'],
      ['unavailable', 'unsupported', ['signal.pi.unavailableHint', 'signal.pi.hold', 'signal.pi.sessionUnsupported'], 'lifecycle-only'],
      ['incompatible', undefined, ['signal.pi.incompatibleHint', 'signal.pi.hold'], 'incompatible'],
    ];
    let whyOk = 0, whyAll = 0;
    for (const [coverage, session, keys, name] of WHY) {
      words(cards.piLong.session, { fg: 'node', pi_signal: coverage, pi_session: session });
      await pollNow();
      await until(() => ctx.attention.get(cards.piLong)?.piSignal === coverage && (ctx.attention.get(cards.piLong)?.piSession ?? undefined) === session);
      await each(async (language, size) => {
        helpOf(cards.piLong).click();
        await until(() => $('chd').style.display === 'flex');
        await pause(80);
        const box = rect($('chd-box'));
        const buttons = [...$('chd-actions').querySelectorAll('button')];
        const parts = [
          $('chd-msg').textContent === keys.map(key => t(key)).join('\n\n'),
          inside(box, view()),
          noOverflowX($('chd-msg')) && insideX(rect($('chd-msg')), box),
          buttons.length === 2 && buttons.every(button => shown(button) && inside(rect(button), box) && reachable(button)),
          rect($('chd-msg')).bottom <= rect($('chd-actions')).top + 1,
          buttons[1].textContent === t('signal.settings'),
        ];
        whyAll += 1;
        if (parts.every(Boolean)) whyOk += 1; else await tell(2, whyAll, bits(parts));
        capture($('chd-box'));
        if (size !== 0.5 && (name === 'unsupported' || name === 'unavailable' || (name === 'incompatible' && size === 1))) {
          await snapshot(`pilay-why-${name}-${tag(language, size)}`);
        }
        buttons[0].click();
        await until(() => $('chd').style.display !== 'flex');
      });
    }
    await report('pilay-why', whyOk === whyAll, whyAll, whyOk);
    words(cards.piLong.session, { fg: 'node', agent: 'working', episode: 1, episode_viewed: false, pi_signal: 'connected', pi_session: 'unsupported' });

    // ---- 3. Settings: the two switches and their note, in every state of the entry ----
    stage = 4;
    const STATES = [
      ['absent', false, false, null, true, 'absent'],
      ['current', true, false, 'settings.pi.note.current', true, 'signal'],
      ['current', true, true, 'settings.pi.note.current', true, 'both'],
      ['needs-reload', true, true, 'settings.pi.note.needsReload', true, 'reload'],
      ['conflict', false, false, 'settings.pi.note.conflict', true, 'conflict'],
      ['unreadable', false, false, 'settings.pi.note.unreadable', true, 'unreadable'],
      ['unsupported-location', false, false, 'settings.pi.note.unsupportedLocation', false, 'location'],
      ['a-word-of-a-later-build', true, true, 'settings.pi.note.unknown', false, 'unknown'],
    ];
    let settingsOk = 0, settingsAll = 0;
    for (const [installation, signal, lifecycle, note, usable, name] of STATES) {
      scripted.status = { pi: signal, pi_lifecycle: lifecycle, pi_installation: installation };
      await openSettings();
      selectSettingsSection('agents');
      await until(() => $('set-pi-note').hidden === !note && $('set-pi-hooks').disabled === !usable);
      await each(async (language, size) => {
        const content = $('set-content');
        const boxes = [$('set-pi-hooks'), $('set-pi-lifecycle')];
        const labels = boxes.map(box => document.querySelector(`label[for="${box.id}"]`));
        const noteNode = $('set-pi-note');
        boxes[0].scrollIntoView({ block: 'center' });
        await pause(40);
        const known = installation !== 'a-word-of-a-later-build';
        const first = [
          boxes[0].checked === (known && signal) && boxes[1].checked === (known && lifecycle),
          boxes.every(box => box.disabled === !usable),
          noteNode.hidden === !note && noteNode.textContent === (note ? t(note) : ''),
          labels[0].textContent === t('settings.piSignal') && labels[1].textContent === t('settings.piLifecycle'),
          inside(rect($('settings-box')), view()) && noOverflowX(content),
          // each label is beside its switch or, at the large text size, above it; never under or over it
          boxes.every((box, index) => shown(box) && shown(labels[index])
            && (rect(labels[index]).right <= rect(box).left + 1 || rect(labels[index]).bottom <= rect(box).top + 1)
            && insideX(rect(box), rect(content)) && insideX(rect(labels[index]), rect(content))),
          !note || (shown(noteNode) && noOverflowX(noteNode) && insideX(rect(noteNode), rect(content))
            && rect(noteNode).top >= rect(boxes[0]).bottom - 1),
          reachable(boxes[0]),
        ];
        boxes[1].scrollIntoView({ block: 'center' });
        await pause(40);
        const second = [reachable(boxes[1]), rect(boxes[1]).top > rect(noteNode.hidden ? boxes[0] : noteNode).bottom - 1];
        const parts = [...first, ...second];
        settingsAll += 1;
        if (parts.every(Boolean)) settingsOk += 1; else await tell(3, settingsAll, bits(parts));
        capture(noteNode, ...labels);
        if (size !== 0.5 && ['reload', 'conflict', 'both', 'location'].includes(name)) {
          boxes[0].scrollIntoView({ block: 'center' });
          await snapshot(`pilay-settings-${name}-${tag(language, size)}`);
        }
      });
      $('set-close').click();
      await pause(80);
    }
    await report('pilay-settings', settingsOk === settingsAll, settingsAll, settingsOk);

    // the "Learn more" of both switches, open: the long explanations fit as well
    stage = 5;
    scripted.status = { pi: true, pi_lifecycle: true, pi_installation: 'needs-reload' };
    await openSettings();
    selectSettingsSection('agents');
    await until(() => !$('set-pi-note').hidden);
    const hints = ['settings.piSignalHint', 'settings.piLifecycleHint']
      .map(key => document.querySelector(`#set-content [data-i18n="${key}"]`));
    for (const hint of hints) hint.closest('details').open = true;
    let hintsOk = 0, hintsAll = 0;
    await each(async (language, size) => {
      $('set-pi-hooks').scrollIntoView({ block: 'start' });
      await pause(40);
      const content = $('set-content');
      const parts = [
        hints.every(hint => shown(hint) && noOverflowX(hint) && insideX(rect(hint), rect(content))),
        hints[0].textContent === t('settings.piSignalHint') && hints[1].textContent === t('settings.piLifecycleHint'),
        noOverflowX(content) && inside(rect($('settings-box')), view()),
        stacked([$('set-pi-hooks').closest('.set-row'), $('set-pi-note'), hints[0], $('set-pi-lifecycle').closest('.set-row'), hints[1]]),
        // the dialog's own way out stays where it is
        shown($('set-close')) && inside(rect($('set-close')), rect($('settings-box'))) && reachable($('set-close')),
      ];
      hintsAll += 1;
      if (parts.every(Boolean)) hintsOk += 1; else await tell(4, hintsAll, bits(parts));
      capture(...hints);
      if (size !== 0.5) await snapshot(`pilay-settings-details-${tag(language, size)}`);
    });
    for (const hint of hints) hint.closest('details').open = false;
    await report('pilay-settings-details', hintsOk === hintsAll, hintsAll, hintsOk);

    // a switch the backend refuses: its own sentence, as a toast, and the box back where it was
    stage = 6;
    let toastOk = 0, toastAll = 0;
    for (const [code, key] of [['pi-config-conflict', 'error.pi.conflict'], ['pi-never-ran', 'error.pi.neverRan'], ['some other failure', 'error.agentHooks']]) {
      for (const language of LOCALES) {
        if (getLocale() !== language) setLocale(language);
        applyFontScale(1.6);
        scripted.refusal = code;
        const box = $('set-pi-hooks');
        const seen = $('toasts').children.length;
        box.checked = false;
        box.dispatchEvent(new Event('change'));
        await until(() => $('toasts').children.length > seen && !box.disabled);
        await pause(120);
        const toast = $('toasts').lastElementChild;
        const parts = [
          toast.textContent === t(key),
          inside(rect(toast), view()) && noOverflowX(toast),
          box.checked === true,
        ];
        toastAll += 1;
        if (parts.every(Boolean)) toastOk += 1; else await tell(5, toastAll, bits(parts));
        capture(toast);
        if (code === 'pi-config-conflict') await snapshot(`pilay-settings-refused-${tag(language, 1.6)}`);
        await pause(300);
      }
    }
    await report('pilay-settings-refused', toastOk === toastAll, toastAll, toastOk);
    $('set-close').click();
    scripted.status = null;
    applyFontScale(1);
    if (getLocale() !== 'en') setLocale('en');
    await pause(150);

    // ---- 4. the restart review: a Pi beside Claude, Codex and a shell ----
    stage = 7;
    const observed = await nativeInvoke('tmux_server_status');
    scripted.server = { ...observed, sessionCount: 7, paneCount: 7, attachedSessionCount: 0, foregroundSessionCount: 6, sessions: [],
      restartBlockers: [], pendingRestart: false, shouldPrompt: false, canRestart: true, notice: null,
      serverPid: Number(observed.serverPid) || 1, serverStartedAt: Number(observed.serverStartedAt) || 1, impactToken: 'pilay' };
    const entry = (card, kind, extra = {}) => ({ cardId: card.id, session: card.session, kind, reason: null, launchProfile: null, source: null, ...planted, ...extra });
    const PI = { reason: 'client-identity-unverifiable', source: 'pi' };
    const candidates = [
      entry(cards.claude, 'candidate', { launchProfile: 'bypass', source: 'claude-code' }),
      entry(cards.claudeTwo, 'candidate', { launchProfile: 'ordinary', source: 'claude-code' }),
    ];
    const others = [
      entry(cards.piLong, 'unsupported', PI), entry(cards.piZh, 'unsupported', PI),
      entry(cards.codex, 'unsupported', { reason: 'unsupported-agent' }), entry(cards.shell, 'shell'),
    ];
    const section = () => $('tmux-reentry');
    const all = (root, selector) => [...root.querySelectorAll(selector)];
    const modalOpen = () => $('tmux-lifecycle-modal').style.display === 'flex';
    const rows = () => all(section(), '.reentry-list > *');
    const rowOf = card => rows().find(row => row.querySelector('.reentry-choice-text')?.textContent === card.title);
    const openReview = async preview => {
      scripted.preview = preview;
      const reads = scripted.previewReads;
      ($('set-tmux-restart').disabled ? $('tmux-restart-btn') : $('set-tmux-restart')).click();
      await until(modalOpen);
      await until(() => scripted.previewReads > reads && (!preview || !section().hidden));
      await pause(120);
    };
    const closeReview = async () => { $('tmux-done').click(); await until(() => !modalOpen()); await pause(80); };
    /* what holds for every review that lists a Pi: the frame, the rows, the Pi's row and the way out */
    const reviewParts = () => {
      const frame = rect($('tmux-lifecycle-box'));
      const body = $('tmux-lifecycle-body');
      const listed = rows();
      const pis = [rowOf(cards.piLong), rowOf(cards.piZh)];
      return [
        inside(frame, view()),
        noOverflowX(section()) && noOverflowX(body),
        listed.every(row => insideX(rect(row), rect(section())) && noOverflowX(row)) && stacked(listed),
        // a row's name and its note do not run into one another
        listed.every(row => {
          const name = row.querySelector('.reentry-choice-text'), note = row.querySelector('.reentry-note');
          return !!name && !!note && shown(name) && shown(note)
            && (rect(note).left >= rect(name).right - 1 || rect(note).top >= rect(name).bottom - 1);
        }),
        // the Pi rows: named by their card, not selectable, with Pi's own reason and nothing else
        pis.every(row => !!row && row.classList.contains('reentry-off') && !row.querySelector('input, button')
          && row.querySelector('.reentry-note').textContent === t('reentry.reason.client-identity-unverifiable')),
        rowOf(cards.codex)?.querySelector('.reentry-note').textContent === t('reentry.reason.unsupported-agent')
          && rowOf(cards.shell)?.querySelector('.reentry-note').textContent === t('reentry.kind.shell'),
        // the two ways out are inside the frame, under the scrolling body, and nothing lies over them
        [$('tmux-later'), $('tmux-restart')].every(button => shown(button) && inside(rect(button), frame) && reachable(button)
          && rect(button).top >= rect(body).bottom - 1),
      ];
    };
    const scrollTo = async node => { node.scrollIntoView({ block: 'center' }); await pause(40); };
    /* One review per language, opened IN that language (its opening paragraph is written when it opens),
       then every text size; `prepare` is what a person would click first. */
    const eachReview = async (preview, prepare, body) => {
      for (const language of LOCALES) {
        if (getLocale() !== language) setLocale(language);
        applyFontScale(1);
        await pause(140);
        await openReview(preview);
        if (prepare) await prepare();
        for (const size of SCALES) {
          applyFontScale(size);
          await pause(140);
          await body(language, size);
        }
        await closeReview();
      }
    };
    const opening = () => $('tmux-lifecycle-message');

    // 4a. mixed: two Claude candidates, two Pis, Codex, a shell
    let mixedOk = 0, mixedAll = 0;
    const boxes = () => all(section(), 'input[type="checkbox"]');
    const radios = () => all(section(), 'input[type="radio"]');
    const chooseAll = async () => {
      boxes()[0].click();                             // "select all"
      await pause(100);
      radios()[1].click();                            // policy B: bypass, where it was the launch profile
      await pause(100);
    };
    const MIXED = { token: 'tok-pilay-1', lifecycleObservation: true, ...planted, entries: [candidates[0], others[0], candidates[1], others[1], others[2], others[3]] };
    await eachReview(MIXED, chooseAll, async (language, size) => {
      const parts = [
        ...reviewParts(),
        shown(opening()) && noOverflowX(opening()) && insideX(rect(opening()), rect($('tmux-lifecycle-box'))),
        rows().length === 6 && boxes().length === 3 && boxes().every(box => box.checked),
        $('tmux-restart').textContent === t('reentry.confirm', { count: 2 }),
        // choosing everything and the bypass policy chose no Pi and counted none
        all(section(), '.reentry-choice-text').some(node => node.textContent === t('reentry.policy.bypass', { count: 1 })),
        rowOf(cards.claude)?.querySelector('.reentry-note').textContent === t('reentry.profile.bypass'),
        noOverflowX(section().querySelector('.reentry-policy')) && insideX(rect(section().querySelector('.reentry-policy')), rect(section())),
      ];
      mixedAll += 1;
      if (parts.every(Boolean)) mixedOk += 1; else await tell(6, mixedAll, bits(parts));
      capture(section(), $('tmux-lifecycle-title'), $('tmux-restart'));
      if (size !== 0.5 || language === 'en') {
        await scrollTo(rowOf(cards.piLong));
        await snapshot(`pilay-review-mixed-${tag(language, size)}`);
        if (size === 1.6) { await scrollTo(section().querySelector('.reentry-policy')); await snapshot(`pilay-review-policy-${tag(language, size)}`); }
      }
    });
    await report('pilay-review-mixed', mixedOk === mixedAll, mixedAll, mixedOk);

    // 4b. no candidate at all: only what cannot be reopened, each with its reason
    stage = 8;
    let noneOk = 0, noneAll = 0;
    await eachReview({ token: 'tok-pilay-2', lifecycleObservation: true, ...planted, entries: others }, null, async (language, size) => {
      const parts = [
        ...reviewParts(),
        rows().length === 4 && all(section(), 'input, fieldset, details').length === 0,
        all(section(), '.reentry-summary')[0]?.textContent === t('reentry.summary.none'),
        $('tmux-restart').textContent === t('tmux.restart'),
      ];
      noneAll += 1;
      if (parts.every(Boolean)) noneOk += 1; else await tell(7, noneAll, bits(parts));
      capture(section());
      if (size !== 0.5) await snapshot(`pilay-review-none-${tag(language, size)}`);
    });
    await report('pilay-review-none', noneOk === noneAll, noneAll, noneOk);

    // 4c. failure states of the review: observation not set up, and a preview that cannot be read
    stage = 9;
    let failOk = 0, failAll = 0;
    await eachReview({ token: 'tok-pilay-3', lifecycleObservation: false, ...planted, entries: [candidates[0], others[0], candidates[1], others[3]] }, null, async (language, size) => {
      const setup = section().querySelector('.reentry-setup');
      const parts = [
        inside(rect($('tmux-lifecycle-box')), view()) && noOverflowX(section()),
        rows().length === 4 && all(section(), 'input').length === 0 && stacked(rows()),
        !!setup && shown(setup) && noOverflowX(setup) && insideX(rect(setup), rect(section()))
          && reachable(setup.querySelector('button')),
        rowOf(cards.piLong)?.querySelector('.reentry-note').textContent === t('reentry.reason.client-identity-unverifiable'),
        all(section(), '.reentry-summary')[0]?.textContent === t('reentry.summary.unavailable', { count: 2 }),
        [$('tmux-later'), $('tmux-restart')].every(button => shown(button) && reachable(button)),
      ];
      failAll += 1;
      if (parts.every(Boolean)) failOk += 1; else await tell(8, failAll, bits(parts));
      capture(section());
      if (size !== 0.5) await snapshot(`pilay-review-setup-${tag(language, size)}`);
    });
    // a preview that cannot be read leaves the plain restart: nothing half-painted, the way out reachable
    await eachReview(null, null, async (language, size) => {
      const parts = [
        section().hidden && all(section(), '*').length === 0,
        inside(rect($('tmux-lifecycle-box')), view()),
        shown(opening()) && noOverflowX(opening()),
        [$('tmux-later'), $('tmux-restart')].every(button => shown(button) && reachable(button)),
        $('tmux-restart').textContent === t('tmux.restart'),
      ];
      failAll += 1;
      if (parts.every(Boolean)) failOk += 1; else await tell(9, failAll, bits(parts));
      if (size === 1.6) await snapshot(`pilay-review-unreadable-${tag(language, size)}`);
    });
    await report('pilay-review-failure', failOk === failAll, failAll, failOk);
    applyFontScale(1);
    if (getLocale() !== 'en') setLocale('en');
    await pause(150);

    // 4d. the result: what was reopened and what could not be confirmed; no Pi is among it
    stage = 10;
    const result = (card, outcome, extra = {}) => ({ cardId: card.id, session: card.session, outcome, reason: null, warnings: [],
      shell: true, clientObserved: true, settled: true, ...planted, ...extra });
    scripted.operation = { requestId: UUID, state: 'complete', serverReplaced: true, ...planted, entries: [
      result(cards.claude, 'confirmed', { warnings: ['bypass-reauthorized'] }),
      result(cards.claudeTwo, 'unconfirmed', { reason: 'context-changed' }),
    ] };
    await openReview({ ...MIXED, token: 'tok-pilay-4' });
    await chooseAll();
    $('tmux-restart').click();
    const shownResult = () => $('tmux-result');
    const resultRows = () => all(shownResult(), '.reentry-result-row');
    await until(() => !shownResult().hidden && resultRows().length === 2);
    const sent = scripted.restarts[0]?.reentry;
    let resultOk = 0, resultAll = 0;
    await each(async (language, size) => {
      const frame = rect($('tmux-lifecycle-box'));
      const parts = [
        inside(frame, view()) && noOverflowX(shownResult()),
        resultRows().every(row => insideX(rect(row), rect(shownResult())) && noOverflowX(row)) && stacked(resultRows()),
        shown($('tmux-done')) && inside(rect($('tmux-done')), frame) && reachable($('tmux-done')),
        // only what was asked for is spoken of: the two Claude cards, and no Pi
        !/\bPi\b/.test(shownResult().textContent) && !shownResult().textContent.includes(cards.piZh.title),
        // one way to its card per row, inside the row and reachable; nothing that would do the restart again
        all(shownResult(), 'input, a').length === 0 && all(shownResult(), 'button').length === 2
          && resultRows().every(row => {
            const open = row.querySelector('button');
            return !!open && open.textContent === t('reentry.result.open') && insideX(rect(open), rect(row)) && reachable(open);
          }),
      ];
      resultAll += 1;
      if (parts.every(Boolean)) resultOk += 1; else await tell(10, resultAll, bits(parts));
      capture(shownResult(), $('tmux-lifecycle-title'));
      if (size !== 0.5) await snapshot(`pilay-review-result-${tag(language, size)}`);
    });
    // a = one request, with the two candidates and no Pi; b = how many cards it named
    const asked = scripted.restarts.length === 1 && !!sent && sent.policy === 'reapply-bypass'
      && JSON.stringify(sent.cards) === JSON.stringify([cards.claude.id, cards.claudeTwo.id]);
    await report('pilay-review-result', resultOk === resultAll && asked, resultAll, resultOk);
    await closeReview();

    // ---- 5. nothing shaped like a native id, a pid, a digest, a path or a token was painted ----
    stage = 11;
    const leaked = painted.filter(text => NATIVE.test(text)).length;
    await report('pilay-no-native', painted.length >= 200 && leaked === 0, painted.length, leaked);
    // the backend was never asked to restart, and nothing was typed, killed or switched
    await report('pilay-ipc', installed && scripted.restarts.length === 1 && refused === 0, 1, refused);
    // a = the snapshots that were written; every one that was asked for
    await report('pilay-shots', shots.written === shots.asked && shots.asked >= 60, shots.written, shots.asked);
    await report('done', !failed, 1, 0);
  } catch (_) {
    await report('pilay-exception', false, stage || 1, details);
    await report('done', false, 1, stage);
  } finally {
    try { closeDialogs(); } catch (_) { /* nothing is open */ }
    try { if ($('tmux-lifecycle-modal').style.display === 'flex') $('tmux-done').click(); } catch (_) { /* the modal stays as it is */ }
    try { $('set-close').click(); } catch (_) { /* Settings is closed */ }
    /* the script goes, the refusal of a real restart stays (this page only ever scripted it) */
    const guarded = (command, args) => (command === 'restart_tmux_server'
      ? Promise.reject(new Error('refused by the Pi layout smoke')) : nativeInvoke(command, args));
    window.__TAURI__ = { ...nativeTauri, core: { ...nativeTauri.core, invoke: guarded } };
    ctx.tmuxServerStatus = realStatus;
    applyFontScale(scale);
    setLocale(locale);
  }
}
