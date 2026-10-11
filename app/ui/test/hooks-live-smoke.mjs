// The two Claude hook families through the real Settings switches, on the
// GLOBAL Claude settings file of the home this carrier runs under. Debug
// carrier only. Hooks are installed only from a stable location
// (/Applications/deck.app or ~/Applications/deck.app), so this mode needs a
// carrier at <home>/Applications/deck.app; the operator gives it a home of
// its own (`open --env HOME=…`) holding a `.claude/settings.json` with hooks
// of the user's own, and never the home of a person.
//
// The page clicks what a person clicks: the box, then Confirm or Cancel in
// the confirmation. What the file then holds is not the page's to judge: the
// operator copies it after every checkpoint (each is followed by a pause)
// and compares it with the file before.
const HOLD = 6000; // the operator's time to copy the file after a checkpoint

export async function runHooksLiveSmoke() {
  const { $, inv } = await import('../js/state.js');
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitFor = async (test, timeout = 8000) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { if (await test()) return true; } catch (_) { /* not yet */ }
      if (Date.now() >= deadline) return false;
      await pause(100);
    }
  };
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(a, 1), b });
    await pause(HOLD);
    return ok;
  };
  const status = () => inv('agent_hooks_status');
  const asked = () => $('cfm').style.display === 'flex';
  /* Set a box the way a click does and answer the confirmation, if one is
     asked: returns the question shown ('' when none was asked). */
  const set = async (id, on, answer) => {
    const box = $(id);
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
      return $('set-agent-hooks').checked === (now.claude === true)
        && $('set-claude-reentry').checked === (now.claude_reentry === true);
    });
  };
  try {
    await open();
    const first = await status();
    if (!await report('hooks-clean', first.claude === false && first.claude_reentry === false
      && !$('set-agent-hooks').checked && !$('set-claude-reentry').checked)) throw new Error('state');

    // declining the question writes nothing
    const declined = await set('set-claude-reentry', true, false);
    const afterNo = await status();
    await report('hooks-decline', declined === t('settings.claudeReentryEnableConfirm')
      && !$('set-claude-reentry').checked && afterNo.claude_reentry === false && afterNo.claude === false);

    // re-entry observation on: its own opt-in, the status hooks stay off
    const accepted = await set('set-claude-reentry', true, true);
    const lifeOn = await status();
    await report('hooks-life-on', accepted === t('settings.claudeReentryEnableConfirm')
      && $('set-claude-reentry').checked && lifeOn.claude_reentry === true && lifeOn.claude === false
      && !$('set-agent-hooks').checked);

    // the status hooks on: re-entry observation stays on
    const signal = await set('set-agent-hooks', true, true);
    const bothOn = await status();
    await report('hooks-signal-on', signal === t('settings.agentHooksEnableConfirm')
      && $('set-agent-hooks').checked && bothOn.claude === true && bothOn.claude_reentry === true
      && $('set-claude-reentry').checked);

    // re-entry observation off asks nothing and leaves the status hooks
    const quiet = await set('set-claude-reentry', false, true);
    const lifeOff = await status();
    await report('hooks-life-off', quiet === '' && !$('set-claude-reentry').checked
      && lifeOff.claude_reentry === false && lifeOff.claude === true && $('set-agent-hooks').checked);

    // closed and opened again, the boxes say what the file says
    $('set-close').click();
    await open();
    await report('hooks-reopen', $('set-agent-hooks').checked && !$('set-claude-reentry').checked);

    // the status hooks off: nothing of Deck's is left
    await set('set-agent-hooks', false, true);
    const none = await status();
    await report('hooks-signal-off', none.claude === false && none.claude_reentry === false
      && !$('set-agent-hooks').checked && !$('set-claude-reentry').checked);
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 20, b: 0 });
    await report('done', false, 0, 20);
  }
}
