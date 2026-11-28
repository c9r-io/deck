// Usage quota in the real window (quota.rs, quota.js, extension-quota.js).
// Debug carrier only, on an isolated data root and tmux socket.
//
// `quota-switch`: the Settings switch on the GLOBAL Claude Code and agy
// settings files of a home the operator gave this carrier (`open --env
// HOME=…`, a carrier at <home>/Applications/deck.app, like hooks-live): a
// statusLine of the "user's" own in Claude's file, none in agy's. The page
// clicks the box, answers the question, and reports; the operator copies
// both files after every checkpoint (each is followed by a pause) and
// judges them: wrapped with the user's command kept, the other bytes
// untouched, and byte for byte the original after the switch is off.
//
// `quota-live`: the host's own signed-in Claude Code and a Codex home of
// the run's own. The operator prepared the fixture: `claude.sh` (Claude
// Code with the fixture's settings: Signal hooks and Deck's statusLine tee
// into the fixture's `user-line.sh`, which reads its input and prints
// QUOTA-USER-LINE), `codex.sh` when the scenario says `codex`. Two Claude
// cards (one account: one entry with both cards), a Codex card, the area,
// the chip. The operator reads the panes for QUOTA-USER-LINE afterwards.
const HOLD = 6000; // the operator's time to copy the files after a checkpoint

async function harness() {
  const { $, inv } = await import('../js/state.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitFor = async (test, timeout = 8000, every = 200) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { const value = await test(); if (value) return value; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return null;
      await pause(every);
    }
  };
  const report = async (name, ok, a = 1, b = 0, hold = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? Math.max(a, 1) : -Math.max(a, 1), b });
    if (hold) await pause(hold);
    return ok;
  };
  return { $, inv, pause, waitFor, report, failed: () => failed };
}

export async function runQuotaSwitchSmoke() {
  const { $, inv, pause, waitFor, report, failed } = await harness();
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { t } = await import('../js/i18n.js');
  const status = () => inv('quota_status');
  const asked = () => $('cfm').style.display === 'flex';
  const set = async (on, answer) => {
    const box = $('set-quota');
    box.checked = on;
    box.dispatchEvent(new Event('change'));
    let question = '';
    if (await waitFor(asked, on ? 3000 : 600)) {
      question = $('cfm-msg').textContent;
      $(answer ? 'cfm-yes' : 'cfm-no').click();
    }
    await waitFor(() => !box.disabled && !asked(), 15000);
    await pause(300);
    return question;
  };
  const open = async () => {
    await openSettings(); selectSettingsSection('agents');
    await waitFor(async () => {
      const now = await status();
      return !$('set-quota').disabled && $('set-quota').checked === (now.claude === 'on' && now.agy === 'on');
    });
  };
  try {
    await open();
    const first = await status();
    await report('quota-sw-clean', first.claude === 'user' && first.agy === 'off' && first.writable === true
      && !$('set-quota').checked, 1, 0, HOLD);
    const declined = await set(true, false);
    const afterNo = await status();
    await report('quota-sw-decline', declined === t('settings.quotaEnableConfirm')
      && afterNo.claude === 'user' && afterNo.agy === 'off' && !$('set-quota').checked, 1, 0, HOLD);
    const accepted = await set(true, true);
    const on = await status();
    await report('quota-sw-on', accepted === t('settings.quotaEnableConfirm')
      && on.claude === 'on' && on.agy === 'on' && $('set-quota').checked, 1, 0, HOLD);
    $('set-close').click();
    await open();
    await report('quota-sw-reopen', $('set-quota').checked);
    const quiet = await set(false, true);
    const off = await status();
    await report('quota-sw-off', quiet === '' && off.claude === 'user' && off.agy === 'off' && !$('set-quota').checked,
      1, 0, HOLD);
    await report('done', !failed(), 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 20, b: 0 });
    await report('quota-exception', false, 20);
    await report('done', false, 0, 20);
  }
}

export async function runQuotaLiveSmoke() {
  const { $, inv, pause, waitFor, report, failed } = await harness();
  const { state, store } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { backToBoard, openSession } = await import('../js/layout.js');
  const { refreshQuota } = await import('../js/quota.js');
  const snapshot = () => inv('quota_snapshot');
  const opened = [];
  try {
    if (store.cards.length) throw new Error('nonempty isolated board');
    const marker = await inv('smoke_native_fixture');
    const fixture = marker.slice(0, marker.lastIndexOf('/'));
    const work = `${fixture}/work`;
    const scenario = await inv('smoke_native_scenario');
    const codex = scenario.includes('codex');
    const first = await snapshot();
    await refreshQuota();
    await report('quota-clean', first.accounts.length === 0 && $('quota-area').hidden === true);

    state.view === 'board' || backToBoard();
    const project = provider.projects()[0];
    const column = project.columns[0];
    const start = async (title, cmd) => {
      const card = (await provider.createStarted({ projectId: project.id, columnId: column.id, title, cmd, dir: work })).card;
      opened.push(card);
      return card;
    };
    const a = await start('Quota Claude A', `/bin/sh '${fixture}/claude.sh'`);
    const b = await start('Quota Claude B', `/bin/sh '${fixture}/claude.sh'`);
    const c = codex ? await start('Quota Codex', `/bin/sh '${fixture}/codex.sh'`) : null;

    const claude = snap => snap.accounts.filter(x => x.kind === 'claude');
    const linked = await waitFor(async () => {
      const snap = await snapshot();
      const mine = claude(snap);
      return mine.length === 1 && mine[0].windows.length > 0
        && mine[0].sessions.includes(a.session) && mine[0].sessions.includes(b.session) ? snap : null;
    }, 300_000, 3000);
    const account = linked && claude(linked)[0];
    await report('quota-claude', !!account && account.byAccount === true
      && account.windows.every(w => typeof w.usedPercent === 'number' && w.resetsAt > Date.now() - 86400e3),
    account ? account.windows.length : 0, account ? account.windows.filter(w => w.minutes === 300 || w.minutes === 10080).length : 0);
    await report('quota-dedupe', !!account && account.sessions.length === 2, account ? account.sessions.length : 0);

    if (codex) {
      const got = await waitFor(async () => {
        const snap = await snapshot();
        const mine = snap.accounts.filter(x => x.kind === 'codex');
        return mine.length === 1 && mine[0].windows.length > 0 && mine[0].sessions.includes(c.session) ? mine[0] : null;
      }, 360_000, 5000);
      await report('quota-codex', !!got && got.source === 'rollout' && got.byAccount === true, got ? got.windows.length : 0);
    }

    await refreshQuota();
    const accounts = (await snapshot()).accounts.length;
    const rows = $('quota-list').children.length;
    await report('quota-area', $('quota-area').hidden === false && rows === accounts && rows >= 1, rows, accounts);

    await openSession(a.id);
    const chip = await waitFor(() => {
      const head = document.querySelector('.spane-head .quotachip');
      return head && /\d+%/.test(head.textContent) ? head : null;
    }, 30_000, 500);
    await report('quota-chip', !!chip, chip ? 1 : 0);
    if (chip) {
      chip.click();
      const pop = await waitFor(() => document.getElementById('quota-pop'), 3000);
      const cards = pop ? pop.querySelectorAll('.quota-pop-card').length : 0;
      await report('quota-detail', !!pop && cards === 2, cards);
      document.body.click();
    }
    // the operator reads the panes now (QUOTA-USER-LINE in Claude's status line)
    await report('quota-armed', true, 1, 0, 8000);
    backToBoard();
    await report('done', !failed(), 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 20, b: 0 });
    await report('quota-exception', false, 20);
    await report('done', false, 0, 20);
  }
}
