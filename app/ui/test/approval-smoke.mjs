// Isolated real-WKWebView carrier for a badge rule's approval box in the
// automation editor (automation.js `checkApproval` / `decideApproval` /
// `withdrawDrifted`). The production drawer, editor and settings writer run
// in the bundled webview and save to the isolated data directory; no Slack
// connection exists and no agent is started (a badge rule fires only on a
// Slack reaction). Two things are scripted, in this page only: when a
// SHA-256 completes or fails (`crypto.subtle.digest` is held back, so the
// "being checked" state can be looked at), and the count of `save_settings`
// requests leaving the page.
// Controls are activated by AppKit events handed to Deck's own window
// (`smoke_native_input`: mouse down/up at the control, a Space key to the
// focused box): WebKit's own default actions run, nothing here sets
// `checked` or dispatches `change`. This is not a hand on a mouse.
// Snapshots (`smoke_native_snapshot`) land in <data dir>/evidence/.
export async function runApprovalSmoke() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { provider, render } = await import('../js/board.js');
  const { openAutomations } = await import('../js/automation.js');
  const { persistInbound } = await import('../js/settings.js');
  const { approveRule } = await import('../js/automation-model.js');
  const { getLocale, setLocale, t } = await import('../js/i18n.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) throw new Error('bounded approval condition timeout');
      await pause(50);
    }
  };
  let failed = false;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -a, b });
  };
  const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
  const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

  /* hashes complete when this carrier lets them, or fail */
  const subtle = crypto.subtle; const digest = subtle.digest;
  let hold = null; let failing = false; let hashing = 0;
  /* the page's own settings writes, counted where every invoke leaves it */
  const systemFetch = window.fetch; let writes = 0;
  const locale = getLocale();
  let stage = 0;
  try {
    stage = 1;
    subtle.digest = async function held(...args) {
      hashing += 1;
      try {
        if (hold) await hold.promise;
        if (failing) throw new Error('hash');
        return await digest.apply(subtle, args);
      } finally { hashing -= 1; }
    };
    if (subtle.digest === digest) throw new Error('digest cannot be held');
    window.fetch = function counted(url, init) {
      const command = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]+)/.exec(String(url?.url || url));
      if (command && decodeURIComponent(command[1]) === 'save_settings') writes += 1;
      return systemFetch.call(window, url, init);
    };
    const hashed = async () => { await pause(80); await until(() => hashing === 0); await pause(80); };
    const letGo = async () => { const held = hold; hold = null; if (held) held.resolve(); await hashed(); };

    await until(() => provider.projects().length > 0);
    const project = provider.projects()[0];
    state.projectId = project.id; state.view = 'board'; render();
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    await until(async () => (await inv('smoke_native_input', { input: { kind: 'state' } }) & 3) === 3, 10000);
    await until(() => !document.hidden, 5000);
    const TEMPLATE = 'approval smoke';
    await provider.saveTemplate(project.id, TEMPLATE, ['Run the checks', 'Summarize']);
    const template = () => provider.projects()[0].templates.find(value => value.name === TEMPLATE);
    await openAutomations();

    /* a real pointer press and release on the control, in Deck's own window */
    const press = async el => {
      if (!el) throw new Error('missing control');
      el.scrollIntoView({ block: 'center' });
      await pause(120);
      /* the control has come to rest and nothing (a toast) lies over the point that is pressed */
      let rect; let last = null;
      await until(() => {
        rect = el.getBoundingClientRect();
        const still = last !== null && last === `${rect.x},${rect.y}`;
        last = `${rect.x},${rect.y}`;
        return still && el.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2));
      }, 12000);
      for (const kind of ['down', 'up']) {
        await inv('smoke_native_input', { input: { kind, x: rect.x + rect.width / 2, y: rect.y + rect.height / 2, viewport: innerHeight } });
      }
      await pause(150);
    };
    const send = () => $('auto-send'); const note = () => $('auto-send-check');
    const box = () => `${send().checked ? 'c' : '-'}${send().indeterminate ? 'i' : '-'}${note().hidden ? '-' : 'n'}`;
    const fileRule = async id => JSON.parse((await inv('load_settings')).data).inbound.rules.find(rule => rule.id === id);
    const editorOpen = () => !$('auto-editor').hidden;
    /* an approval field for field (the file's keys come back sorted) */
    const same = (a, b) => !!a && !!b && a.digest === b.digest && a.external === b.external
      && JSON.stringify(a.steps) === JSON.stringify(b.steps) && JSON.stringify(a.classes) === JSON.stringify(b.classes);
    /* One approved badge rule, the only rule in the settings. Each has a
       directory of its own, so nothing was ever computed for it; `cold`
       holds every hash from before it is saved. */
    let serial = 0;
    const seed = async ({ cold = true } = {}) => {
      if (editorOpen()) await press($('auto-cancel'));
      serial += 1;
      const id = `ap${serial}`;
      const rule = await approveRule({ id, source: 'slack', badge: id, name: '', projectId: project.id,
        columnId: project.columns[0].id, template: TEMPLATE, cmd: 'claude', dir: `/tmp/approval-${serial}`,
        enabled: true, finish: 'keep' }, template());
      if (cold) hold = deferred();
      if (!(await persistInbound({ ...ctx.settings.inbound, rules: [rule] }))) throw new Error('seed save failed');
      await openAutomations();
      if (!cold) await hashed();
      await until(() => document.querySelectorAll('#auto-list .auto-rule').length === 1);
      return rule;
    };
    const openRule = async () => {
      await press(document.querySelector('#auto-list .auto-rule .ar-edit'));
      await until(editorOpen);
      send().scrollIntoView({ block: 'center' });
      await pause(150);
    };
    /* the waiting line and the controls around it: nothing cut off or covered */
    const layout = () => {
      const line = note().getBoundingClientRect();
      const label = send().closest('label').getBoundingClientRect();
      const hint = $('auto-send-hint').getBoundingClientRect();
      const drawer = $('auto-drawer').getBoundingClientRect();
      const apart = (a, b) => a.bottom <= b.top + 1 || b.bottom <= a.top + 1 || a.right <= b.left + 1 || b.right <= a.left + 1;
      const buttons = [$('auto-save'), $('auto-cancel')].map(el => el.getBoundingClientRect());
      return line.width > 0 && line.height > 0 && note().scrollWidth <= note().clientWidth + 1
        && line.left >= drawer.left - 1 && line.right <= drawer.right + 1
        && label.bottom <= line.top + 1 && line.bottom <= hint.top + 1
        && buttons.every(rect => rect.width > 0 && rect.height > 0 && apart(rect, line) && apart(rect, label)
          && rect.left >= drawer.left - 1 && rect.right <= drawer.right + 1);
    };
    let laidOut = true;

    // 1. the waiting state, then the answer
    stage = 2;
    setLocale('en');
    await pause(200);
    await seed();
    await openRule();
    const waiting = box() === 'cin' && note().textContent === t('automation.autoSend.checking');
    laidOut = layout() && laidOut;
    await snapshot('approval-pending-en');
    await letGo();
    const settled = box() === 'c--';
    await snapshot('approval-checked-en');
    await report('approval-pending', waiting && settled, waiting ? 1 : 2, settled ? 1 : 0);

    // 2. one press on the box takes the approval away; a late valid changes nothing
    stage = 3;
    let rule = await seed();
    await openRule();
    await press(send());
    const unticked = box() === '---';
    await letGo();
    const stays = box() === '---';
    await snapshot('approval-unticked');
    let before = writes;
    await press($('auto-save'));
    await until(() => !editorOpen());
    await hashed();
    const saved = await fileRule(rule.id);
    await report('approval-press-box', unticked && stays && writes - before === 1 && !!saved && !saved.autoSend,
      unticked ? 1 : 2, writes - before);

    // 3. the same through the label's text
    stage = 4;
    rule = await seed();
    await openRule();
    await press(send().closest('label').querySelector('span'));
    const viaLabel = box() === '---';
    await letGo();
    const labelStays = box() === '---' && editorOpen();
    await report('approval-press-label', viaLabel && labelStays, viaLabel ? 1 : 2, send().checked ? 1 : 0);

    // 4. the Space key on the focused box
    stage = 5;
    rule = await seed();
    await openRule();
    send().focus();
    await pause(100);
    await inv('smoke_native_input', { input: { kind: 'key', keyCode: 49, text: ' ' } });
    await pause(200);
    const viaSpace = box() === '---';
    await letGo();
    await report('approval-key-space', viaSpace && box() === '---', viaSpace ? 1 : 2, document.activeElement === send() ? 1 : 0);

    // 5. Save pressed while the approval is unknown goes on by itself
    stage = 6;
    rule = await seed();
    await openRule();
    $('auto-name').value = 'Renamed'; $('auto-name').dispatchEvent(new Event('input', { bubbles: true }));
    $('auto-name').blur();                             // a focused field is scrolled back into view by WebKit
    await pause(200);
    before = writes;
    await press($('auto-save'));
    await pause(400);
    const waited = writes === before && editorOpen();
    await letGo();
    await until(() => !editorOpen());
    await hashed();
    const kept = await fileRule(rule.id);
    await report('approval-save-through', waited && writes - before === 1 && kept?.name === 'Renamed'
      && same(kept.autoSend, rule.autoSend), waited ? 1 : 2, writes - before);

    // 6. a check that fails: said, not saved on a guess, and unticking still saves
    stage = 7;
    rule = await seed();
    failing = true;
    await openRule();
    await letGo();
    const said = box() === 'cin' && note().textContent === t('automation.autoSend.checkFailed');
    laidOut = layout() && laidOut;
    await snapshot('approval-failed-en');
    before = writes;
    await press($('auto-save'));
    await pause(500);
    const refused = writes === before && editorOpen()
      && [...$('toasts').children].some(el => el.textContent === t('automation.autoSend.checkFailedSave'));
    await press(send());
    const decided = box() === '---';
    await press($('auto-save'));
    await until(() => !editorOpen());
    failing = false;
    await hashed();
    const dropped = await fileRule(rule.id);
    await report('approval-check-failed', said && refused && decided && writes - before === 1 && !!dropped && !dropped.autoSend,
      said ? (refused ? 1 : 3) : 2, writes - before);

    // 7. the same lines in Chinese, at this window's size
    stage = 8;
    setLocale('zh-Hans');
    await pause(200);
    rule = await seed();
    await openRule();
    const zhWaiting = box() === 'cin' && note().textContent === t('automation.autoSend.checking');
    laidOut = layout() && laidOut;
    await snapshot('approval-pending-zh');
    failing = true;
    await letGo();
    const zhFailed = box() === 'cin' && note().textContent === t('automation.autoSend.checkFailed');
    laidOut = layout() && laidOut;
    await snapshot('approval-failed-zh');
    failing = false;
    setLocale('en');
    await pause(200);
    await report('approval-layout', laidOut && zhWaiting && zhFailed, innerWidth, innerHeight);

    // 8. the template's steps change while the editor holds a checked approval
    stage = 9;
    rule = await seed({ cold: false });
    await openRule();
    /* a repaint may still be computing: the box settles within the check's own time */
    await until(() => !send().indeterminate, 3000);
    const held = box() === 'c--';
    await provider.saveTemplate(project.id, TEMPLATE, ['Run other checks', 'Summarize']);
    await pause(200);
    const withdrawn = box() === '--n' && note().textContent === t('automation.autoSend.templateChanged');
    laidOut = layout() && laidOut;
    await snapshot('approval-template-changed');
    before = writes;
    await press($('auto-save'));
    await until(() => !editorOpen());
    await hashed();
    const after = await fileRule(rule.id);
    await report('approval-drift', held && withdrawn && laidOut && writes - before === 1 && !!after && !after.autoSend,
      held ? (withdrawn ? 1 : 3) : 2, writes - before);

    await report('done', !failed, 1, 0);
  } catch (_) {
    await report('approval-exception', false, stage || 1, hashing);
    await snapshot('approval-failure');
    await report('done', false, 1, stage);
  } finally {
    failing = false;
    if (hold) hold.resolve();
    subtle.digest = digest;
    window.fetch = systemFetch;
    setLocale(locale);
  }
}
