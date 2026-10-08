// Isolated real-WKWebView carrier for the information layers of the
// automation editor, the lists panel, the inspection dialog, project
// defaults with phone task presets, the reminder dialog and the template
// manager. The production modules render in the bundled webview against the
// isolated data directory. What it proves is PRESENTATION and interface
// behaviour: which notes are visible for which trigger and option, that a
// risk is outside every disclosure, that opening a disclosure saves nothing
// and keeps fields, focus and drafts, and that the layout holds in both
// languages at the default and the largest font size.
// Controlled state, labelled as such: no Slack connection exists, no agent
// is started, and nothing is delivered (lists are queued a day ahead into an
// empty-command shell card). The "states" scene repaints the panel from a
// copy of the real queue whose stages are set here; it shows how a state
// READS, never that the backend reached it.
// Disclosures and boxes are activated by AppKit events handed to Deck's own
// window (`smoke_native_input`). Every scene reports by itself, so one
// failing scene never hides the next; snapshots land in <data dir>/evidence/.
export async function runUxLayersSmoke() {
  const { $, ctx, inv, state } = await import('../js/state.js');
  const { openProjectDefaults, provider, pollNow, render } = await import('../js/board.js');
  const { openSession } = await import('../js/layout.js');
  const { closeAutomations, openAutomations } = await import('../js/automation.js');
  const { refreshQueue, renderQueueUI, toggleQueuePanel } = await import('../js/scheduler.js');
  const { listStartCalls } = await import('../js/scheduler-model.js');
  const { cfmDone, confirmDialog, projectDefaultsDialog, reminderDialog } = await import('../js/dialogs.js');
  const { presetApproved, withPresetApproval } = await import('../js/connector-model.js');
  const { openTemplates } = await import('../js/templates.js');
  const { getLocale, setLocale, t } = await import('../js/i18n.js');
  const { applyFontScale, getFontScale } = await import('../js/font-scale.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) throw new Error('bounded condition timeout');
      await pause(50);
    }
  };
  let failed = false; let trace = 0;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -a, b });
  };
  const metric = (name, a, b = 0) => inv('ui_event', { code: 'smoke-check', detail: name, a, b });
  const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
  /* a scene: its own verdict, never the end of the run (a scene that does
     not return in two minutes has failed), and no dialog of its own left
     open over the next one */
  const attempt = async body => {
    try { return (await Promise.race([body(), pause(120000).then(() => { throw new Error('scene timeout'); })])) === true; } catch (_) { return false; } finally {
      if ($('cfm').style.display === 'flex') cfmDone(false);
      if ($('pdf').style.display === 'flex') $('pdf-no').click();
      document.querySelector('.reminder-overlay .cfm-actions .btn')?.click();
      if ($('tpl-modal').style.display === 'flex') $('tpl-done').click();
    }
  };
  const shown = el => !!el && el.getClientRects().length > 0 && el.getBoundingClientRect().height > 0;
  const disclosedBy = el => { const details = el && el.closest('details'); return !!details && !el.closest('summary'); };
  const chars = el => (el ? el.innerText.replace(/\s+/g, '').length : 0);
  const locale = getLocale(); const scale = getFontScale();
  const systemFetch = window.fetch; let writes = 0;
  const press = async el => {
    if (!el) throw new Error('missing control');
    el.scrollIntoView({ block: 'center' });
    await pause(120);
    let rect; let last = null;
    await until(() => {
      rect = el.getBoundingClientRect();
      const still = last !== null && last === `${rect.x},${rect.y}`;
      last = `${rect.x},${rect.y}`;
      return still && el.contains(document.elementFromPoint(rect.x + Math.min(rect.width / 2, 12), rect.y + rect.height / 2));
    }, 12000);
    for (const kind of ['down', 'up']) {
      await inv('smoke_native_input', { input: { kind, x: rect.x + Math.min(rect.width / 2, 12), y: rect.y + rect.height / 2, viewport: innerHeight } });
    }
    await pause(150);
  };
  const trigger = async value => { $('auto-trigger').querySelector(`button[data-v="${value}"]`).click(); await pause(80); };
  const type = (el, value) => { el.value = value; el.dispatchEvent(new Event('input', { bubbles: true })); };
  try {
    window.fetch = function counted(url, init) {
      const command = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]+)/.exec(String(url?.url || url));
      if (command && ['save_settings', 'save_state', 'queue_review_confirm', 'queue_review_mode'].includes(decodeURIComponent(command[1]))) writes += 1;
      return systemFetch.call(window, url, init);
    };
    await until(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    state.projectId = project.id; state.view = 'board'; render();
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    await until(async () => (await inv('smoke_native_input', { input: { kind: 'state' } }) & 3) === 3, 10000);
    await until(() => !document.hidden, 5000);
    setLocale('zh-Hans'); applyFontScale(1);
    await metric('ux-m-window', innerWidth, innerHeight);
    await provider.saveTemplate(project.id, 'ux layers', ['Run the checks', 'Summarize {{msg.text}}']);

    /* ---------- automations: the empty state and the editor ---------- */
    await openAutomations();
    await until(() => !$('auto-editor').hidden);
    const drawer = $('auto-drawer'); const empty = () => document.querySelector('#auto-list .auto-empty');
    await snapshot('ux-auto-clock-zh');
    await metric('ux-m-auto-head', chars(document.querySelector('.auto-hint')), chars(drawer));
    await report('ux-empty-create', await attempt(async () => {
      const whileCreating = !shown(empty());
      $('auto-cancel').click(); await pause(120);
      const afterCancel = shown(empty());
      await snapshot('ux-auto-empty-zh');
      $('auto-new').click(); await pause(120);
      return whileCreating && afterCancel && !shown(empty());
    }));
    await report('ux-auto-head', await attempt(async () => {
      const help = document.querySelector('.auto-help');
      return !!help && !help.open && shown(help.querySelector('summary')) && chars(document.querySelector('.auto-hint')) < 140
        && [...document.querySelectorAll('#auto-editor details')].every(details => !details.open);
    }));
    await report('ux-auto-clock', await attempt(async () => {
      await trigger('clock');
      const close = shown($('auto-first-send-hint')) && shown($('auto-first-send-state')) && !disclosedBy($('auto-first-send-hint'))
        && !shown($('auto-send-hint')) && !shown($('auto-channel-first-send-facts')) && shown($('auto-finish-hint')) && !shown($('auto-finish-keep-hint'));
      $('auto-finish').querySelector('button[data-v="keep"]').click(); await pause(80);
      const keep = shown($('auto-finish-keep-hint')) && !shown($('auto-finish-hint'));
      $('auto-finish').querySelector('button[data-v="close"]').click(); await pause(80);
      return close && keep;
    }));
    await report('ux-auto-slack', await attempt(async () => {
      await trigger('slack');
      await snapshot('ux-auto-slack-zh');
      return shown($('auto-send-hint')) && !disclosedBy($('auto-send-hint')) && shown($('auto-first-send-hint')) && shown($('auto-slack-state'))
        && !shown($('auto-channel-first-send-facts')) && !shown($('auto-finish-hint')) && !shown($('auto-finish-keep-hint'));
    }));
    await report('ux-auto-channel', await attempt(async () => {
      await trigger('channel');
      $('auto-channel-first-send').scrollIntoView({ block: 'center' }); await pause(120);
      await snapshot('ux-auto-channel-zh');
      const facts = [...document.querySelectorAll('#auto-channel-first-send-facts li')];
      return facts.length === 5 && facts.every(shown) && !disclosedBy($('auto-channel-first-send-facts')) && shown($('auto-channel-first-send-hint'))
        && !shown($('auto-first-send-hint')) && !shown($('auto-send-hint')) && !shown($('auto-finish')) && !shown($('auto-review'));
    }));
    /* the first-step box: the risk is on screen before the choice, the
       confirmation still asks, and the state line follows the command */
    await report('ux-first-send', await attempt(async () => {
      await trigger('clock');
      type($('auto-cmd'), 'codex');
      const before = shown($('auto-first-send-hint')) && /回车/.test($('auto-first-send-hint').textContent)
        && $('auto-first-send-state').textContent === t('automation.current', { state: t('automation.firstSend.off') });
      await press($('auto-first-send'));
      await until(() => $('cfm').style.display === 'flex');
      const asked = !$('auto-first-send').checked && /回车/.test($('cfm-msg').textContent);
      cfmDone(true); await pause(150);
      const unsupported = $('auto-first-send').checked && $('auto-first-send-state').classList.contains('warn')
        && $('auto-first-send-state').textContent === t('automation.current', { state: t('automation.firstSend.unsupported') });
      $('auto-first-send').scrollIntoView({ block: 'center' }); await pause(120);
      await snapshot('ux-auto-first-send-unsupported-zh');
      type($('auto-cmd'), 'claude');
      const on = !$('auto-first-send-state').classList.contains('warn')
        && $('auto-first-send-state').textContent === t('automation.current', { state: t('automation.firstSend.on') });
      await snapshot('ux-auto-first-send-on-zh');
      await press($('auto-first-send'));
      const off = !$('auto-first-send').checked && $('cfm').style.display !== 'flex'
        && $('auto-first-send-state').textContent === t('automation.current', { state: t('automation.firstSend.off') });
      return before && asked && unsupported && on && off;
    }));
    /* a disclosure opens by pointer and by keyboard, and touches nothing */
    await report('ux-disclosure', await attempt(async () => {
      type($('auto-name'), 'draft name'); type($('auto-dir'), '/tmp/ux-draft');
      const fields = () => JSON.stringify([...document.querySelectorAll('#auto-editor input, #auto-editor select')].map(el => [el.id, el.type === 'checkbox' ? el.checked : el.value]));
      const before = fields(); const at = writes;
      const details = $('auto-first-send-hint').parentElement.querySelector('details.q-p-clock');
      const summary = details.querySelector('summary');
      await press(summary);
      const byPointer = details.open && shown(details.querySelector('p'));
      await snapshot('ux-auto-disclosure-open-zh');
      summary.focus();
      await inv('smoke_native_input', { input: { kind: 'key', keyCode: 49, text: ' ' } }); await pause(200);
      const byKey = !details.open && document.activeElement === summary;
      const top = drawer.scrollTop; await pause(200);
      return byPointer && byKey && fields() === before && writes === at && !$('auto-editor').hidden && drawer.scrollTop === top;
    }));
    /* both languages, the default and the largest font size */
    await report('ux-auto-layout', await attempt(async () => {
      let ok = true;
      for (const [language, size, value] of [['zh-Hans', 1.6, 'slack'], ['en', 1, 'slack'], ['en', 1.6, 'channel'], ['en', 1, 'clock']]) {
        setLocale(language); applyFontScale(size); await trigger(value); await pause(200);
        const edge = drawer.getBoundingClientRect().right + 1;
        ok = ok && drawer.scrollWidth <= drawer.clientWidth + 1
          && [...drawer.querySelectorAll('.opt-note, .q-review-option, .auto-section, .set-row')].filter(shown).every(el => el.getBoundingClientRect().right <= edge)
          && $('auto-first-send-hint').textContent === t('automation.firstSend.hint');
        $(value === 'channel' ? 'auto-channel-first-send' : 'auto-first-send').scrollIntoView({ block: 'center' }); await pause(150);
        await snapshot(`ux-auto-${value}-${language === 'en' ? 'en' : 'zh'}-${size === 1 ? '100' : '160'}`);
      }
      setLocale('zh-Hans'); applyFontScale(1);
      return ok;
    }));
    closeAutomations();

    /* ---------- lists: one list, several lists, stability ---------- */
    const created = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: 'ux layers', cmd: '', dir: '/tmp', desc: '' });
    const card = created.card;
    await openSession(card.id); await pollNow();
    toggleQueuePanel(true);
    const base = { session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd, reviewEach: false };
    const tomorrow = Math.floor(Date.now() / 1000) + 86400;
    const addList = async steps => { for (const [command, payload] of listStartCalls(base, { mode: 'at', at: tomorrow }, steps)) await inv(command, payload); };
    const body = $('queue-body'); const list = $('queue-list');
    const groups = () => [...list.querySelectorAll('.q-group')];
    await addList(['Run the regression suite\nand report the failures', 'Summarize what changed', 'Draft the release note']);
    await refreshQueue(); await until(() => groups().length === 1);
    await pause(200); await snapshot('ux-queue-one-zh');
    await metric('ux-m-queue-one', chars(list), body.scrollHeight);
    await report('ux-queue-one', await attempt(async () => {
      const group = groups()[0]; const rows = [...group.querySelectorAll('.qg-row')];
      const plan = group.querySelector('.q-send-plan');
      const risk = plan?.querySelector('.q-risk');
      const expected = ctx.queueCache.items.find(item => item.session === card.session && item.mode === 'at')?.expected_process;
      const bits = [
        !!plan && rows.length === 3 && rows.every(row => row.compareDocumentPosition(plan) & Node.DOCUMENT_POSITION_FOLLOWING),
        list.querySelectorAll('.q-session-facts').length === 1 && !list.innerText.includes(t('queue.plan.others', { count: 1 })),
        /* the backend's fact decides (context.rs captures whatever is in the
           foreground when the list is added): a compatibility target is said
           outside the disclosure, an expected program only inside it */
        expected ? !risk && [...plan.querySelectorAll('details p')].some(p => p.textContent === t('queue.plan.expected', { process: expected }))
          : shown(risk) && !disclosedBy(risk) && risk.textContent === t('queue.plan.compatibility'),
        shown($('q-shell-risk')),
        [...list.querySelectorAll('details')].every(details => !details.open),
        !rows[1].innerText.includes(t('queue.stage.previous')) && rows[0].querySelector('.row-meta').textContent.length > 0,
      ];
      trace = bits.reduce((mask, ok, index) => mask | (ok ? 1 << index : 0), 0);
      return bits.every(Boolean);
    }), 1, trace);
    await addList(['Check the dashboards']);
    await refreshQueue(); await until(() => groups().length === 2);
    await pause(200); await snapshot('ux-queue-many-zh');
    await report('ux-queue-many', await attempt(async () => {
      const said = [...list.querySelectorAll('p')].filter(p => p.textContent === t('queue.plan.others', { count: 2 }));
      return said.length === 1 && shown(said[0]) && list.querySelectorAll('.q-session-facts').length === 1
        && groups().every(group => group.querySelectorAll('.q-send-plan').length === 1);
    }));
    await report('ux-queue-stable', await attempt(async () => {
      const at = writes;
      const summary = () => groups()[0].querySelector('.q-send-plan details summary');
      await press(summary());
      const open = () => groups()[0].querySelector('.q-send-plan details').open;
      const opened = open();
      const draft = groups()[0].querySelector('.qg-add textarea');
      type(draft, 'a draft row'); draft.blur();
      body.scrollTop = 40; const top = body.scrollTop;
      await refreshQueue(); renderQueueUI(); await pause(150);
      await snapshot('ux-queue-conditions-open-zh');
      const kept = open() && groups()[0].querySelector('.qg-add textarea').value === 'a draft row' && Math.abs(body.scrollTop - top) <= 1;
      summary().focus();
      await inv('smoke_native_input', { input: { kind: 'key', keyCode: 49, text: ' ' } }); await pause(200);
      await refreshQueue(); await pause(100);
      return opened && kept && !open() && writes === at;
    }));
    /* how the states read — a repaint from a copy of the real queue. The
       panel's own refresh may replace the copy at any moment, so a picture
       counts only if the copy was still what was painted after it was taken */
    await report('ux-queue-states', await attempt(async () => {
      await refreshQueue();
      const copy = structuredClone(ctx.queueCache);
      const mine = copy.items.filter(item => item.session === card.session).sort((a, b) => a.seq - b.seq);
      const now = () => Math.floor(Date.now() / 1000);
      const stage = (item, value, extra = {}) => { copy.plans = [...(copy.plans || []).filter(plan => plan.item !== item.id), { item: item.id, stage: value, checked_at: now() + 600, ...extra }]; };
      const paint = async (name, check, prepare = () => {}) => {
        for (let turn = 0; turn < 6; turn += 1) {
          prepare(); ctx.queueCache = copy; renderQueueUI(); body.scrollTop = 0; await pause(120);
          const ok = check();
          await snapshot(name);
          if (ctx.queueCache === copy) return ok;
        }
        return false;
      };
      stage(mine[0], 'authority-unverified', { authorized: true }); stage(mine[1], 'first-send'); stage(mine[2], 'previous');
      mine[3].state = 'ambiguous'; stage(mine[3], 'ambiguous');
      const held = () => {
        const text = list.innerText;
        return text.includes(t('queue.stage.authorityUnverified')) && text.includes(t('queue.stage.firstSend')) && text.includes(t('queue.ambiguousDetail'))
          && !text.includes(t('queue.stage.previous')) && !!list.querySelector('.q-ack') && !!list.querySelector('.q-risk-retry')
          && [...list.querySelectorAll('.row-meta')].every(meta => !disclosedBy(meta));
      };
      const zh = await paint('ux-queue-states-zh', held);
      const en = await paint('ux-queue-states-en-160', () => held() && body.scrollWidth <= body.clientWidth + 1, () => { setLocale('en'); applyFontScale(1.6); });
      setLocale('zh-Hans'); applyFontScale(1);
      mine[0].state = 'review'; mine[0].review_each = true; stage(mine[0], 'review');
      const review = await paint('ux-queue-review-zh', () => {
        const row = list.querySelector('.qg-review');
        return shown(row) && row.innerText.includes(t('queue.stage.review')) && row.innerText.includes(t('queue.review.boundary'));
      });
      await refreshQueue();
      trace = (zh ? 1 : 0) | (en ? 2 : 0) | (review ? 4 : 0);
      return zh && en && review;
    }), 1, trace);
    /* the inspection dialog's text, shown by the production dialog (the
       release itself is review-smoke's subject, not this carrier's) */
    await report('ux-review-dialog', await attempt(async () => {
      const message = t('queue.review.confirm', { prompt: 'Summarize what changed', current: 'zsh', expected: t('queue.review.compatibility') });
      const answer = confirmDialog(message);
      await until(() => $('cfm').style.display === 'flex');
      const box = $('cfm-box').getBoundingClientRect();
      await snapshot('ux-review-dialog-zh');
      const lines = $('cfm-msg').innerText.split('\n').filter(Boolean).length;
      cfmDone(false);
      return (await answer) === false && lines >= 4 && box.bottom <= innerHeight && box.right <= innerWidth;
    }));
    toggleQueuePanel(false);

    /* ---------- project defaults and phone task presets ---------- */
    await report('ux-presets', await attempt(async () => {
      const column = project.columns[0].id;
      const plain = { id: 'Rux1', name: 'Fix issue', columnId: column, title: 'Remote fix', dir: '/tmp', cmd: 'claude', steps: ['inspect', 'fix'] };
      const approved = await withPresetApproval(project.id, plain, true);
      const stale = { ...approved, id: 'Rux2', name: 'Edited later', steps: ['inspect', 'fix it differently'] };
      const first = { ...plain, id: 'Rux3', name: 'Direct first step', cmd: 'codex', firstSend: true };
      const done = projectDefaultsDialog({ projectId: project.id, name: project.name, dir: '/tmp', cmd: 'claude', recent: [],
        presets: [approved, stale, first], columns: project.columns });
      await until(() => $('pdf').style.display === 'flex'); await pause(300);
      const lines = [...document.querySelectorAll('#pdf-presets .preset-summary')].map(el => el.textContent);
      await snapshot('ux-defaults-zh');
      await metric('ux-m-defaults', chars($('pdf-box')));
      const scope = $('pdf-box').querySelector('details');
      const layered = !!scope && !scope.open && chars(document.querySelector('#pdf-box > .set-hint')) < 80;
      document.querySelectorAll('#pdf-presets button')[0].click(); await pause(300);
      $('pdf-preset-auto-send').scrollIntoView({ block: 'center' }); await pause(150);
      await snapshot('ux-preset-editor-zh');
      const editor = shown($('pdf-preset-auto-send-hint')) && !disclosedBy($('pdf-preset-auto-send-hint')) && shown($('pdf-preset-first-send-hint'))
        && !disclosedBy($('pdf-preset-first-send-hint')) && $('pdf-preset-auto-send').checked === true;
      $('pdf-no').click();
      return (await done) === null && layered && editor && lines.length === 3
        && lines[0] === `${t('presets.later.auto')} · ${t('presets.first.wait')}`
        && lines[1] === `${t('presets.later.stale')} · ${t('presets.first.wait')}`
        && lines[2] === `${t('presets.later.manual')} · ${t('presets.first.unsupported')}`;
    }));

    /* ---------- a preset's approval through the real save path ----------
       The dialog is opened by the Board's own entry and what is judged is
       the preset the Board holds after the save (the isolated deck.json),
       with the production digest. Boxes and buttons take native clicks. */
    const presetColumn = project.columns[0].id;
    const presetPlain = { id: 'Rux5', name: 'Fix issue', columnId: presetColumn, title: 'Remote fix', dir: '/tmp', cmd: 'claude', steps: ['inspect', 'fix'] };
    const presetApprovedOnce = await withPresetApproval(project.id, presetPlain, true);
    const defaultsBefore = { dir: project.dir || '', cmd: project.cmd || '' };
    const seedPreset = () => provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [presetApprovedOnce] });
    const storedPreset = () => (provider.project(project.id).presets || []).find(preset => preset.id === 'Rux5');
    const presetBox = () => $('pdf-preset-auto-send');
    const openPresetEditor = async () => {
      const done = openProjectDefaults(project.id);
      await until(() => $('pdf').style.display === 'flex' && !!document.querySelector('#pdf-presets button'));
      await press(document.querySelector('#pdf-presets button'));
      await until(() => !$('pdf-preset-editor').hidden);
      return [done];   // boxed: an async function would wait for the dialog itself
    };
    /* a dialog that does not close is this scene's failure, not a hung run */
    const closed = promise => Promise.race([promise, pause(10000).then(() => { throw new Error('dialog still open'); })]);
    await report('ux-preset-keep', await attempt(async () => {
      await seedPreset();
      trace = 1;
      // an ordinary save keeps the approval it had, digest unchanged
      let [done] = await openPresetEditor();
      await until(() => presetBox().checked && !presetBox().indeterminate);
      type($('pdf-preset-title'), 'Remote fix 2');
      await press($('pdf-yes')); await closed(done);
      const kept = storedPreset().title === 'Remote fix 2' && storedPreset().autoSend?.digest === presetApprovedOnce.autoSend.digest
        && await presetApproved(project.id, storedPreset());
      trace = 2;
      // an edit to a step unticks it; Cancel leaves the Board's preset alone
      [done] = await openPresetEditor();
      await until(() => presetBox().checked);
      type($('pdf-preset-steps'), 'inspect\nfix it');
      const unticked = !presetBox().checked && !presetBox().indeterminate;
      presetBox().scrollIntoView({ block: 'center' }); await pause(150);
      await snapshot('ux-preset-withdrawn-zh');
      await press($('pdf-no'));
      const cancelled = (await closed(done), storedPreset().steps.join('|') === 'inspect|fix'
        && storedPreset().autoSend?.digest === presetApprovedOnce.autoSend.digest);
      trace = 3;
      // reopened: the cancelled edit is gone and the approval shows again
      [done] = await openPresetEditor();
      await until(() => presetBox().checked);
      const reopened = $('pdf-preset-steps').value === 'inspect\nfix';
      trace = 4;
      // the same edit saved without a new tick: no approval for the new steps
      type($('pdf-preset-steps'), 'inspect\nfix it');
      await press($('pdf-yes')); await closed(done);
      const dropped = storedPreset().steps.join('|') === 'inspect|fix it' && !('autoSend' in storedPreset());
      trace = 5;
      // ticked by hand for the steps as they are now: a new approval, valid for them
      [done] = await openPresetEditor(); await pause(300);
      const off = !presetBox().checked && !presetBox().indeterminate;
      await press(presetBox());
      await until(() => presetBox().checked);
      await press($('pdf-yes')); await closed(done);
      const again = !!storedPreset().autoSend && storedPreset().autoSend.digest !== presetApprovedOnce.autoSend.digest
        && await presetApproved(project.id, storedPreset());
      trace = [kept, unticked, cancelled, reopened, dropped, off, again].reduce((bits, ok, i) => bits | (ok ? 0 : 1 << i), 0) + 100;
      return kept && unticked && cancelled && reopened && dropped && off && again;
    }), 1, trace);

    /* CONTROLLED: this page's SHA-256 is held back by the carrier, so the
       check of a stored approval is still under way while the user acts.
       Everything else is production: the dialog, the digest once released,
       the Board save. It is not an external chain of any kind. */
    await report('ux-preset-race', await attempt(async () => {
      const subtle = crypto.subtle; const real = subtle.digest.bind(subtle); let gate = null; let held = 0;
      subtle.digest = async (...args) => { if (gate) { held += 1; await gate.promise; } return real(...args); };
      const hold = () => { let open; const promise = new Promise(resolve => { open = resolve; }); gate = { promise, open }; };
      const release = async () => { const g = gate; gate = null; g?.open(); await pause(250); };
      try {
        trace = 6;
        // A: steps edited while the check waits; its late "valid" approves nothing
        await seedPreset(); hold(); held = 0;
        let [done] = await openPresetEditor();
        const checking = presetBox().indeterminate === true && !presetBox().checked && shown($('pdf-preset-auto-send-state')) && held > 0;
        presetBox().scrollIntoView({ block: 'center' }); await pause(150);
        await snapshot('ux-preset-checking-zh');
        type($('pdf-preset-steps'), 'inspect\nfix it');
        await release();
        const stays = !presetBox().checked && !presetBox().indeterminate;
        await press($('pdf-yes')); await closed(done);
        const a = storedPreset().steps.join('|') === 'inspect|fix it' && !('autoSend' in storedPreset())
          && !(await presetApproved(project.id, storedPreset()));
        trace = 7;
        // B: saved unchanged while the check waits; one save, the approval kept
        await seedPreset(); hold(); held = 0;
        [done] = await openPresetEditor();
        let returned = false; done.then(() => { returned = true; });
        await press($('pdf-yes')); await pause(400);
        const waited = !returned && held > 0 && $('pdf').style.display === 'flex';
        await release(); await closed(done);
        const b = storedPreset().autoSend?.digest === presetApprovedOnce.autoSend.digest && await presetApproved(project.id, storedPreset());
        trace = 8;
        // a save still waiting when the dialog is cancelled writes nothing
        hold();
        [done] = await openPresetEditor();
        type($('pdf-preset-title'), 'Never saved');
        $('pdf-yes').click(); $('pdf-no').click();
        await release(); await closed(done); await pause(200);
        const c = storedPreset().title !== 'Never saved' && storedPreset().autoSend?.digest === presetApprovedOnce.autoSend.digest;
        trace = [checking, stays, a, waited, b, c].reduce((bits, ok, i) => bits | (ok ? 0 : 1 << i), 0) + 100;
        return checking && stays && a && waited && b && c;
      } finally {
        gate?.open(); gate = null; delete subtle.digest;
        await provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [] }).catch(() => {});
      }
    }), 1, trace);

    /* CONTROLLED, as above: the carrier holds this page's SHA-256 back, or
       makes it fail. A save asked for one preset's edit must not take the
       preset opened while it waited; a new approval that cannot be
       computed says so in the edit and the same save works afterwards. */
    const presetById = id => (provider.project(project.id).presets || []).find(preset => preset.id === id);
    const gatedDigest = () => {
      const subtle = crypto.subtle; const real = subtle.digest.bind(subtle); const state = { gate: null, failing: false };
      subtle.digest = async (...args) => { if (state.gate) await state.gate.promise; if (state.failing) throw new Error('digest'); return real(...args); };
      state.hold = () => { let open; const promise = new Promise(resolve => { open = resolve; }); state.gate = { promise, open }; };
      state.release = async () => { const g = state.gate; state.gate = null; g?.open(); await pause(300); };
      state.restore = () => { state.gate?.open(); state.gate = null; delete subtle.digest; };
      return state;
    };
    await report('ux-preset-queue', await attempt(async () => {
      const digest = gatedDigest();
      const otherPreset = { id: 'Rux6', name: 'Other', columnId: presetColumn, title: 'Other card', dir: '/tmp', cmd: 'claude', steps: ['look'] };
      try {
        trace = 9;
        await provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [presetApprovedOnce, otherPreset] });
        digest.hold();
        const [done] = await openPresetEditor();          // the approved one
        let returned = false; done.then(() => { returned = true; });
        type($('pdf-preset-name'), 'Queued rename');
        await press($('pdf-preset-done'));                // waits for the held digest
        await press($('pdf-yes'));                        // asked for the same edit
        await press(document.querySelectorAll('#pdf-presets button')[1]);
        await until(() => $('pdf-preset-name').value === 'Other');
        type($('pdf-preset-steps'), 'look\nand change');
        await digest.release(); await pause(400);         // nothing else is clicked
        const open = !returned && $('pdf').style.display === 'flex' && !$('pdf-preset-editor').hidden
          && $('pdf-preset-steps').value === 'look\nand change';
        const untouched = presetById('Rux6').steps.join('|') === 'look' && presetById('Rux5').name === 'Fix issue';
        $('pdf-preset-steps').scrollIntoView({ block: 'center' }); await pause(150);
        await snapshot('ux-preset-queue-zh');
        trace = 10;
        // the user's own save of the other preset then works as usual
        await press($('pdf-yes')); await closed(done);
        const saved = presetById('Rux6').steps.join('|') === 'look|and change' && !('autoSend' in presetById('Rux6'))
          && presetById('Rux5').name === 'Fix issue' && presetById('Rux5').autoSend?.digest === presetApprovedOnce.autoSend.digest
          && await presetApproved(project.id, presetById('Rux5'));
        trace = [open, untouched, saved].reduce((bits, ok, i) => bits | (ok ? 0 : 1 << i), 0) + 100;
        return open && untouched && saved;
      } finally {
        digest.restore();
        await provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [] }).catch(() => {});
      }
    }), 1, trace);
    await report('ux-preset-mint', await attempt(async () => {
      const digest = gatedDigest();
      try {
        trace = 11;
        await provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [presetPlain] });
        const [done] = await openPresetEditor(); await pause(300);
        let returned = false; done.then(() => { returned = true; });
        await press(presetBox());
        await until(() => presetBox().checked);
        type($('pdf-preset-title'), 'Remote fix 3');
        digest.failing = true;
        await press($('pdf-yes')); await pause(400);
        const note = $('pdf-preset-auto-send-state');
        const said = shown(note) && note.textContent === t('presets.autoSend.approveFailedSave');
        const kept = !returned && $('pdf').style.display === 'flex' && !$('pdf-preset-editor').hidden
          && presetBox().checked && $('pdf-preset-title').value === 'Remote fix 3';
        const unsaved = presetById('Rux5').title === 'Remote fix' && !('autoSend' in presetById('Rux5'));
        presetBox().scrollIntoView({ block: 'center' }); await pause(150);
        await snapshot('ux-preset-mint-zh');
        trace = 12;
        // the digest works again: the same save, no second tick
        digest.failing = false;
        await press($('pdf-yes')); await closed(done);
        const retried = presetById('Rux5').title === 'Remote fix 3' && !!presetById('Rux5').autoSend
          && await presetApproved(project.id, presetById('Rux5'));
        trace = [said, kept, unsaved, retried].reduce((bits, ok, i) => bits | (ok ? 0 : 1 << i), 0) + 100;
        return said && kept && unsaved && retried;
      } finally {
        digest.restore();
        await provider.setProjectDefaults(project.id, { ...defaultsBefore, presets: [] }).catch(() => {});
      }
    }), 1, trace);

    /* ---------- reminder: the note limit is bytes, and is feedback ---------- */
    await report('ux-reminder-bytes', await attempt(async () => {
      const done = reminderDialog(null);
      await until(() => !!$('reminder-note'));
      const note = $('reminder-note'); const feedback = $('reminder-note-feedback'); const save = $('reminder-save');
      const quiet = !!feedback && !shown(feedback);
      const long = '提醒'.repeat(50);   // 100 characters, 300 UTF-8 bytes
      type(note, long);
      const over = shown(feedback) && feedback.textContent === t('reminder.noteOver', { used: 300, max: 280 }) && save.disabled && note.value === long;
      await snapshot('ux-reminder-over-zh');
      type(note, 'x'.repeat(280));
      const full = shown(feedback) && feedback.textContent === t('reminder.noteUsed', { used: 280, max: 280 }) && !save.disabled;
      type(note, 'short');
      const fine = !shown(feedback) && !save.disabled;
      await snapshot('ux-reminder-zh');
      document.querySelector('.reminder-editor .cfm-actions .btn').click();
      return (await done) === null && quiet && over && full && fine;
    }));

    /* ---------- templates: placeholders behind a named way in ---------- */
    await report('ux-templates', await attempt(async () => {
      trace = 0;
      openTemplates(null);
      await until(() => shown($('tpl-form')));
      const details = $('tpl-form').querySelector('details');
      const body = () => $('tpl-form').querySelector('[data-i18n="templates.placeholders"]');
      /* WebKit keeps a closed disclosure's content in layout, so `open` is the fact read here */
      const closed = !!details && !details.open && shown(details.querySelector('summary'));
      trace |= closed ? 1 : 0;
      await snapshot('ux-templates-zh');
      await press(details.querySelector('summary'));
      trace |= details.open ? 2 : 0; trace |= shown(body()) ? 4 : 0;
      await snapshot('ux-templates-open-zh');
      const opened = details.open && shown(body());
      $('tpl-done').click();
      return closed && opened;
    }), 1, trace);
  } catch (_) {
    await snapshot('ux-layers-exception');
    await report('ux-exception', false);
  } finally {
    window.fetch = systemFetch;
    setLocale(locale); applyFontScale(scale);
  }
  await report('done', !failed);
}
