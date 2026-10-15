// Bergamot's first-party custom Settings contribution. The extension owns
// the local translation panel's declaration and binding lifecycle.
// Controller owns feature state and actions; the host retains the one settings writer.
// Part of deck's no-build frontend: native ES modules, no bundler.

import { packStatus, packInstall, packDelete, unload } from './local-intelligence.js';

export const CONTRIBUTION = Object.freeze({
  id: 'bergamot',
  feedback: {
    'local-translation': ['local-translation', 'settings.localTranslation'],
    'translation-limit': ['local-translation', 'settings.translationDocumentLimit'],
  },
  slots: Object.freeze([Object.freeze({ slotId: 'set-extension-terminal', html: `
    <div class="set-group" id="set-item-local-translation" data-setting-id="local-translation" tabindex="-1">
      <h3 class="set-group-title" data-i18n="settings.localIntelligence">Local Intelligence</h3>
      <div class="set-row"><label for="set-local-translation" data-i18n="settings.localTranslation">Local Translation</label><input type="checkbox" id="set-local-translation"></div>
      <div class="set-hint" data-i18n="settings.localTranslationHint">Fast, private on-device translation for everyday reading. For important content, verify with a professional translation service.</div>
      <div class="set-row"><span data-i18n="settings.translationTarget">Target language</span><span id="set-translation-target"></span></div>
      <div class="set-row"><label for="set-translation-document-limit" data-i18n="settings.translationDocumentLimit">Long-text translation limit</label><select id="set-translation-document-limit" class="col-select" disabled></select></div>
      <div class="set-row"><span data-i18n="settings.translationPack">Offline language pack · English → Simplified Chinese</span><span id="set-translation-pack-status"></span></div>
      <div class="set-row"><span id="set-translation-pack-size"></span><button class="btn" id="set-translation-download" type="button" data-i18n="settings.translationDownload">Download offline translation support</button><button class="btn" id="set-translation-delete" type="button" hidden data-i18n="settings.translationDelete">Delete language pack</button></div>
    </div>` })]),
  settings: Object.freeze([Object.freeze({
    id: 'local-translation', section: 'terminal', order: 1,
    labelKeys: ['settings.localIntelligence', 'settings.localTranslation'],
    keywords: ['translation', 'offline', 'local', '本地', '翻译', '离线'],
  })]),
  create: createController,
});

function createController(host) {
  const { $, ctx, t, toast, confirmDialog, confirmDangerDialog, formatNumber, commitSettings } = host;
  let translationPack = null;
  function renderLocalTranslationSettings() {
    const pref = ctx.settings.localIntelligence.translation;
    $('set-local-translation').checked = pref.enabled === true;
    const choices = $('set-translation-document-limit');
    choices.replaceChildren();
    for (const size of translationPack?.documentChoices || []) {
      if (![8192, 16384].includes(size)) continue;
      const option = document.createElement('option');
      option.value = String(size); option.textContent = `${size / 1024} KiB`;
      choices.appendChild(option);
    }
    choices.disabled = choices.options.length === 0;
    choices.value = String(pref.documentLimitBytes);
    $('set-translation-target').textContent = translationPack?.targetLanguage === 'zh-Hans'
      ? t('settings.locale.zh-Hans') : '';
    if (!translationPack) return;
    const state = translationPack.corrupt ? 'corrupt' : translationPack.installed ? 'installed' : 'missing';
    $('set-translation-pack-status').textContent = t(`settings.translationPackState.${state}`);
    $('set-translation-pack-size').textContent = t(translationPack.installed
      ? 'settings.translationPackInstalledSize' : 'settings.translationPackSize', {
      size: formatNumber(Math.round((translationPack.installed
        ? translationPack.installedBytes : translationPack.downloadBytes) / 1048576)),
    });
    $('set-translation-download').hidden = translationPack.installed;
    $('set-translation-delete').hidden = !translationPack.installed && !translationPack.corrupt;
  }
  async function refreshTranslationPack() {
    try { translationPack = await packStatus(); renderLocalTranslationSettings(); }
    catch { translationPack = null; $('set-translation-pack-status').textContent = t('settings.translationPackState.unavailable'); }
  }
  function translationChoice(patch) {
    return settings => { Object.assign(settings.localIntelligence.translation, patch); };
  }
  async function saveTranslationEnabled(enabled) {
    const current = ctx.settings.localIntelligence.translation.enabled;
    if (current === enabled) return true;
    if (!enabled) {
      window.dispatchEvent(new Event('deck-translation-disabled'));
      try { await unload(); }
      catch { toast(t('settings.translationUnloadFailed')); renderLocalTranslationSettings(); return false; }
    }
    const ok = await commitSettings({ key: 'local-translation', exclusive: true,
      edit: translationChoice({ enabled }),
      locked: ['set-local-translation', 'set-translation-download', 'set-translation-delete'],
      apply: renderLocalTranslationSettings,
      onCommit: () => window.dispatchEvent(new Event('deck-translation-enabled-changed')),
    });
    renderLocalTranslationSettings();
    return ok;
  }
  let translationInstallPending = false;
  async function installTranslationPack(enableAfter, { confirmed = false } = {}) {
    if (translationInstallPending) return false;
    translationInstallPending = true;
    const controls = ['set-local-translation', 'set-translation-download', 'set-translation-delete'].map($);
    controls.forEach(control => { control.disabled = true; });
    try { return await performTranslationInstall(enableAfter, confirmed); }
    finally { translationInstallPending = false; controls.forEach(control => { control.disabled = false; }); }
  }
  async function performTranslationInstall(enableAfter, confirmed) {
    await refreshTranslationPack();
    if (translationPack?.corrupt) { toast(t('settings.translationPackCorrupt')); return false; }
    if (translationPack?.installed) return enableAfter ? saveTranslationEnabled(true) : true;
    const size = formatNumber(Math.round((translationPack?.downloadBytes || 36745493) / 1048576));
    if (!confirmed && !(await confirmDialog(t(enableAfter ? 'settings.translationDownloadConfirm'
      : 'settings.translationDownloadOnlyConfirm', { size })))) {
      renderLocalTranslationSettings(); return false;
    }
    try {
      translationPack = await packInstall();
      renderLocalTranslationSettings();
      return enableAfter ? saveTranslationEnabled(true) : true;
    } catch {
      toast(t('settings.translationDownloadFailed'));
      await refreshTranslationPack();
      return false;
    } finally {
      renderLocalTranslationSettings();
    }
  }


  const bindings = {
    toggle: async event => {
      if (event.target.checked) await installTranslationPack(true);
      else await saveTranslationEnabled(false);
      renderLocalTranslationSettings();
    },
    changeLimit: async event => {
      const documentLimitBytes = Number(event.target.value);
      if (![8192, 16384].includes(documentLimitBytes)) { renderLocalTranslationSettings(); return; }
      await commitSettings({ key: 'translation-limit', exclusive: true,
        edit: translationChoice({ documentLimitBytes }),
        locked: ['set-translation-document-limit'], apply: renderLocalTranslationSettings });
      renderLocalTranslationSettings();
    },
    download: () => installTranslationPack(false),
    remove: async () => {
      if (!(await confirmDangerDialog(t('settings.translationDeleteConfirm'), t('settings.translationDelete')))) return;
      if (ctx.settings.localIntelligence.translation.enabled && !(await saveTranslationEnabled(false))) return;
      try { await packDelete(); await refreshTranslationPack(); }
      catch { toast(t('settings.translationDeleteFailed')); }
    },
  };
  return {
    refresh() { renderLocalTranslationSettings(); return refreshTranslationPack(); }, installTranslationPack,
    mount() {
      host.$('set-local-translation').onchange = bindings.toggle;
      host.$('set-translation-document-limit').onchange = bindings.changeLimit;
      host.$('set-translation-download').onclick = bindings.download;
      host.$('set-translation-delete').onclick = bindings.remove;
    },
  };
}
