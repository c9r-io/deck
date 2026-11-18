// The sidebar's update button: what it offers and what a click does.
//
// The offer (`ctx.pendingUpdate`) comes from the periodic or manual check in
// app.js. A click asks the backend to install exactly the offered version;
// the backend re-reads the feed first and refuses a version that is no
// longer the feed's answer (updater.rs `install_update`). After a refusal
// this module asks the feed once more, so a release published between the
// check and the click replaces the offer instead of reading as a failure.
// It never installs a version the button did not show: the user clicks
// again for the new one.
import { $, ctx, inv, uev } from './state.js';
import { toast } from './dialogs.js';
import { t } from './i18n.js';

export function renderPendingUpdate() {
  if (!ctx.pendingUpdate) return;
  const btn = $('update-btn');
  btn.querySelector('.label').textContent = t('update.available', { version: ctx.pendingUpdate.version });
  btn.title = t('update.availableTitle', { version: ctx.pendingUpdate.version });
}

export async function installPendingUpdate() {
  if (!ctx.pendingUpdate) return;
  const btn = $('update-btn');
  const label = btn.querySelector('.label');
  btn.disabled = true;
  ctx.updateDownloadBytes = 0;
  try {
    await inv('install_update', {
      channel: ctx.settings.updateChannel,
      expectedVersion: ctx.pendingUpdate.version,
    });
    label.textContent = t('update.restarting');
    await inv('relaunch_after_update');
  } catch (e) {
    btn.disabled = false;
    // the backend's fixed refusal for an admin-installed bundle (updater.rs)
    const notWritable = String(e).includes('not writable');
    // The feed can move on between the periodic check and the click, and
    // the backend then refuses the stale version before any download.
    // Ask the feed again: a different answer replaces the button's offer
    // (the user clicks once more, for the version now shown); the same
    // answer, or no answer, leaves this an ordinary failure.
    const offered = ctx.pendingUpdate.version;
    const now = notWritable ? undefined : await inv('check_for_update', {
      channel: ctx.settings.updateChannel,
    }).catch(() => undefined);
    if (now && now.version !== offered) {
      ctx.pendingUpdate = now;
      renderPendingUpdate();
      toast(t('update.changed', { version: now.version }));
      uev('update-install-fail', 'changed');
      return;
    }
    if (now === null) {
      ctx.pendingUpdate = null;
      btn.style.display = 'none';
      toast(t('update.current', { version: ctx.buildIdentity.version || '?' }));
      uev('update-install-fail', 'changed');
      return;
    }
    label.textContent = t('update.failedRetry');
    toast(notWritable ? t('update.bundleNotWritable')
                      : t('error.operation', { operation: t('settings.updates') }));
    uev('update-install-fail', notWritable ? 'not-writable' : null);
  }
}
