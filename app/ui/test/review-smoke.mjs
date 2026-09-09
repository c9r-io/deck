// C v01 isolated integration: real queue, persistence, WKWebView and tmux.
// Sends only shell no-ops into a newly created empty-command smoke session.
// Synthetic agent observations exercise display only, never delivery authority.
export async function runReviewSmoke(restart = false) {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, pollNow, stopPolling, render } = await import('../js/board.js');
  const { openSession } = await import('../js/layout.js');
  const { refreshQueue, toggleQueuePanel, renderQueueUI } = await import('../js/scheduler.js');
  const { setLocale } = await import('../js/i18n.js');
  const { activateTheme } = await import('../js/theme.js');
  const { applyFontScale } = await import('../js/font-scale.js');
  const { cfmDone } = await import('../js/dialogs.js');
  let failed = false, stage = 0;
  const pause = ms => new Promise(r => setTimeout(r, ms));
  const report = async (name, ok) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? 1 : -1, b: stage });
  };
  const wait = async (fn, timeout = 8000) => {
    const end = Date.now() + timeout;
    while (Date.now() < end) { if (await fn()) return true; await pause(80); }
    return false;
  };
  try {
    stopPolling(); await pollNow(); setLocale('zh-Hans');
    let card;
    if (restart) {
      card = store.cards.find(c => c.title === '回归检查 · C v01');
      if (!card) throw new Error('fixture unavailable');
    } else {
      const project = provider.projects()[0];
      await provider.renameProject(project.id, 'Atlas');
      const created = await provider.createStarted({ projectId: project.id, columnId: project.columns[1].id,
        title: '回归检查 · C v01', cmd: '', dir: '/tmp', desc: '' });
      card = created.card;
    }
    state.projectId = card.projectId; render(); await openSession(card.id); await pollNow();
    toggleQueuePanel(true);
    const signal = agent => {
      ctx.attention.record([card], [{ name: card.session, alive: true, agent, idle_secs: agent === 'working' ? 720 : 180 }]);
      renderQueueUI();
    };
    const bar = document.createElement('div');
    bar.id = 'review-smoke-bar'; bar.style.cssText = 'padding:8px;background:#293544;color:#e7ebf1;font:12px sans-serif;display:flex;gap:10px;flex-wrap:wrap';
    const label = document.createElement('span'); label.textContent = '隔离验证 · 真实实现 / 合成状态 · shell 仅执行冒号空操作'; bar.append(label);
    for (const [text, value] of [['无 hook', null], ['有 hook', 'turn-done'], ['长静默', 'working'], ['权限等待', 'needs-input']]) {
      const b = document.createElement('button'); b.textContent = text; b.onclick = () => signal(value); bar.append(b);
    }
    $('queue-panel').prepend(bar);
    await refreshQueue();
    const own = () => ctx.queueCache.items.filter(i => i.session === card.session);
    if (restart) {
      await report('review-restart', own().length === 1 && own()[0].state === 'review'
        && ctx.queueCache.deliveries.filter(d => d.session === card.session).length === 3
        && ctx.queueCache.reviews.filter(d => d.session === card.session).length === 3);
      signal('turn-done');
      await report('done', !failed); return;
    }
    stage = 1;
    await report('review-default', !$('q-review').checked);
    const base = { session: card.session, cardId: card.id, dir: card.dir, cmd: '', reviewEach: true,
      text: ': 修改代码', mode: 'at', at: Math.floor(Date.now() / 1000) + 3600 };
    await inv('queue_add_reviewed_list', { args: base, texts: [': 修改代码', ': 运行测试', ': 整理结果'] });
    await refreshQueue();
    await report('review-atomic-list', own().length === 3 && own().every(i => i.review_each)
      && new Set(own().map(i => i.group)).size === 1);
    const first = own().find(i => i.seq === 1).id;
    await inv('queue_send_now', { id: first, acceptProcessMismatch: false });
    await wait(async () => { await refreshQueue(); return own().some(i => i.id === first && i.state === 'review'); });
    const second = own().find(i => i.seq === 2).id;
    try { await inv('queue_send_now', { id: second, acceptProcessMismatch: false }); } catch {}
    await refreshQueue();
    await report('review-no-bypass', own().find(i => i.id === second)?.state === 'pending'
      && ctx.queueCache.deliveries.filter(d => d.session === card.session).length === 1);
    const preview = await inv('queue_review_preview', { id: first });
    await inv('smoke_fault_set', { kind: 'queue-save', count: 1 });
    let rejected = false;
    try { await inv('queue_review_confirm', { decision: preview.decision }); } catch { rejected = true; }
    await refreshQueue();
    await report('review-save-failure', rejected && own().find(i => i.id === first)?.state === 'review');
    await inv('queue_review_confirm', { decision: preview.decision });
    await inv('queue_review_confirm', { decision: preview.decision });
    await refreshQueue();
    await report('review-idempotent', own().find(i => i.id === first)?.state === 'review-approved'
      && ctx.queueCache.reviews.filter(r => r.session === card.session).length === 1);
    await inv('queue_update', { id: second, text: ': 运行测试（隔离空操作）' });
    await refreshQueue();
    await report('review-edit-revokes', own().find(i => i.id === first)?.state === 'review');
    stage = 2;
    let signalsHold = true;
    for (const a of [null, 'working', 'needs-input', 'turn-done', 'turn-done']) {
      signal(a); await refreshQueue(); signalsHold &&= own().find(i => i.id === first)?.state === 'review';
    }
    await report('review-signals', signalsHold);
    document.querySelector(`[data-queue-focus="${first}:confirm"]`).click();
    await wait(() => $('cfm').style.display === 'flex');
    await report('review-dialog', $('cfm-msg').textContent.includes('运行测试') && $('cfm-msg').textContent.includes('间隔'));
    cfmDone(false); await pause(80); await refreshQueue();
    await report('review-cancel-dialog', own().find(i => i.id === first)?.state === 'review');
    const refreshed = await inv('queue_review_preview', { id: first });
    await inv('queue_review_confirm', { decision: refreshed.decision });
    await refreshQueue();
    // Check layout in both languages and existing theme/font presets.
    let layoutOK = true;
    for (const locale of ['zh-Hans', 'en']) for (const theme of ['deck-dark', 'light', 'high-contrast']) for (const scale of [1, 1.6]) {
      setLocale(locale); activateTheme({ theme, accent: 'teal' }); applyFontScale(scale); renderQueueUI(); await pause(60);
      const panel = $('queue-panel');
      layoutOK &&= panel.scrollWidth <= panel.clientWidth + 1
        && !!document.querySelector('.q-execution-plan') && !!document.querySelector('.q-history');
    }
    setLocale('zh-Hans'); activateTheme({ theme: 'deck-dark', accent: 'teal' }); applyFontScale(1); renderQueueUI();
    await report('review-layout', layoutOK);
    // Real minimum-gap passage: never shorten the production timer.
    for (const seq of [2, 3]) {
      stage = seq + 1;
      await refreshQueue();
      const until = (ctx.queueCache.last_fired[card.session] + 61) * 1000;
      while (Date.now() < until) await pause(Math.min(1000, until - Date.now()));
      const id = own().find(i => i.seq === seq && !i.review).id;
      await inv('queue_send_now', { id, acceptProcessMismatch: false });
      await wait(async () => { await refreshQueue(); return own().some(i => i.id === id && i.state === 'review'); });
      await report(seq === 2 ? 'review-second' : 'review-last', own().some(i => i.id === id && i.state === 'review'));
      if (seq === 2) {
        const p = await inv('queue_review_preview', { id });
        await inv('queue_review_confirm', { decision: p.decision });
      }
    }
    await refreshQueue(); signal('turn-done');
    const final = own().find(i => i.state === 'review');
    const finalPreview = await inv('queue_review_preview', { id: final.id });
    await report('review-final-hold', finalPreview.next_text === null
      && !ctx.queueCache.review_completed.includes(card.session)
      && ctx.queueCache.deliveries.filter(d => d.session === card.session).length === 3);
    await report('done', !failed);
  } catch (e) {
    await inv('ui_event', { code: 'smoke-check', detail: 'review-stage', a: -1, b: stage });
    await report('done', false);
  }
}
