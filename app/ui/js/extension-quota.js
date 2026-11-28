// Quota Settings contribution: the one switch for Deck's statusLine entry in
// Claude Code's and agy's settings (quota/install.rs). Switching on wraps
// the statusLine command already there (it keeps running, with the same
// input) or adds a silent one; switching off puts the original back. The
// state is read from the two files; a command the user put there after
// Deck wrapped one shows as off. Codex needs no switch (its own session
// files are read). What the windows show lives in quota.js.
// Part of deck's no-build frontend: native ES modules, no bundler.

const PANEL = `<div class="set-group" id="set-item-quota" data-setting-id="quota" tabindex="-1" role="group" aria-labelledby="set-item-quota-title">
<h3 class="set-group-title" id="set-item-quota-title" data-i18n="settings.quota">Usage quota</h3>
<div class="set-row"><label for="set-quota" data-i18n="settings.quotaSwitch">Show usage quota</label><input type="checkbox" id="set-quota"></div>
<div class="set-hint" data-i18n="settings.quotaHint">Shows each Claude Code, Codex and agy account’s usage windows once in the sidebar and on the cards that run under it. Turning this on changes the statusLine in ~/.claude/settings.json and agy’s settings.json: a statusLine command you already have keeps running with the same input, behind Deck’s helper; without one Deck adds a silent entry. Turning it off puts back what was there. Codex is read from its own session files. Quota never holds, sends or notifies anything.</div>
<div class="set-hint" id="set-quota-state" role="status" aria-live="polite"></div>
</div>
`;

/* one agent's state word → its sentence key */
const STATE_KEYS = Object.freeze({
  on: 'settings.quotaOn', off: 'settings.quotaOff', user: 'settings.quotaUser',
  absent: 'settings.quotaAbsent', unreadable: 'settings.quotaUnreadable',
});

/* The status line under the switch: the change under way, unknown, or
   each agent's state. */
export function quotaLine(status, busy, { t }) {
  if (busy) return t('settings.quotaChanging');
  if (!status) return t('settings.readFailed');
  const parts = [['Claude Code', status.claude], ['agy', status.agy]]
    .map(([name, state]) => `${name}: ${t(STATE_KEYS[state] || 'settings.quotaUnreadable')}`);
  if (!status.writable) parts.push(t('settings.quotaReleaseOnly'));
  return parts.join(' · ');
}

/* the switch reads as on when every agent that is here is on */
export function quotaOn(status) {
  if (!status) return false;
  const present = [status.claude, status.agy].filter(s => s !== 'absent');
  return present.length > 0 && present.every(s => s === 'on');
}

function createController(host) {
  const { $, inv, t, toast, confirmDialog } = host;
  let status = null; let busy = false; let reads = 0;
  function paint() {
    const box = $('set-quota');
    box.checked = quotaOn(status);
    box.disabled = busy || !status || !status.writable;
    $('set-quota-state').textContent = quotaLine(status, busy, { t });
  }
  async function render() {
    const read = ++reads;
    const next = await inv('quota_status').catch(() => null);
    if (read !== reads) return;
    status = next && typeof next === 'object' ? next : null;
    paint();
  }
  async function set(enable) {
    if (busy) return false;
    /* switching on rewrites two files the user owns: they are asked first */
    if (enable && confirmDialog && !(await confirmDialog(t('settings.quotaEnableConfirm')))) {
      paint();
      return false;
    }
    busy = true; paint();
    try {
      status = await inv('quota_set', { enable });
      toast(t(enable ? 'settings.quotaEnabled' : 'settings.quotaDisabled'));
      window.dispatchEvent(new Event('deck-quota-changed'));
      return true;
    } catch (error) {
      toast(t('settings.quotaFailed', { reason: String(error) }));
      return false;
    } finally {
      busy = false;
      await render();
    }
  }
  return {
    refresh: render, localeChanged: paint, isPending: () => busy, set,
    mount() {
      const box = $('set-quota');
      box.addEventListener('change', () => set(box.checked));
    },
  };
}

export const CONTRIBUTION = Object.freeze({
  id: 'quota',
  slots: Object.freeze([Object.freeze({ slotId: 'set-extension-agents', html: PANEL })]),
  settings: Object.freeze([Object.freeze({
    id: 'quota', section: 'agents', order: 20,
    labelKeys: ['settings.quota', 'settings.quotaSwitch'],
    keywords: ['quota', 'usage', 'rate limit', 'limit', 'statusline', 'status line', '额度', '用量', '限额'],
  })]),
  create: createController,
});
