// Bergamot's first-party custom Settings contribution. The extension owns
// the local translation panel's declaration and binding lifecycle.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const CONTRIBUTION = Object.freeze({
  id: 'bergamot',
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
  mount(host) {
    host.$('set-local-translation').onchange = host.bergamot.toggle;
    host.$('set-translation-document-limit').onchange = host.bergamot.changeLimit;
    host.$('set-translation-download').onclick = host.bergamot.download;
    host.$('set-translation-delete').onclick = host.bergamot.remove;
  },
});
