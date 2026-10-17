// Phone Connector's first-party Settings contribution. This module owns the
// panel markup, pairing/device presentation and DOM bindings; the generic
// Settings extension host supplies native and dialog services.
// The hint under the access selector always describes the CURRENT host scope
// (what a paired phone can and cannot do now, and which choice adds output),
// so a kept legacy choice is never mistaken for a sync fault. Device rows add
// the memory-only last-sync time and app build the host saw, and per-device
// trust: a trusted device gets full output/notes/message access whatever the
// global scope is. Trust is granted only here, with a confirmation naming the
// device; right after a pairing the new device is offered trust by name, so
// the decision is made while the user is still at the Mac. A phone never
// widens its own access. Stopping trust needs no confirmation.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { newlyPairedDevice } from './connector-model.js';

const PANEL_HTML = `
    <div class="set-group" id="set-item-connector" data-setting-id="connector" tabindex="-1" role="group" aria-labelledby="set-item-connector-title">
      <div class="set-group-head"><h3 class="set-group-title" id="set-item-connector-title" data-i18n="connector.title">Phone Connector</h3><span class="set-group-status" id="set-connector-status"></span></div>
      <div class="set-hint" data-i18n="connector.hint">Off by default. When enabled, deck listens only on the selected local address over pinned HTTPS.</div>
      <div class="set-row"><label for="set-connector-address" data-i18n="connector.address">Local address</label><select id="set-connector-address"></select><input id="set-connector-port" type="number" min="1024" max="65535" value="47631"><button class="btn" id="set-connector-toggle" data-i18n="connector.enable">Enable</button></div>
      <div class="set-row"><label for="set-connector-scope" data-i18n="connector.scope">Phone access</label><select id="set-connector-scope" disabled><option value="agentCards" data-i18n="connector.scope.agentCards">Saved Codex/Claude cards</option><option value="boardMetadata" data-i18n="connector.scope.boardMetadata">All card summaries</option><option value="boardAccess" data-i18n="connector.scope.boardAccess">All card output and notes</option></select></div>
      <div class="set-hint" id="set-connector-scope-effect" aria-live="polite"></div>
      <div class="set-hint" id="set-connector-withheld" hidden></div>
      <div class="set-actions"><button class="btn" id="set-connector-pair" data-i18n="connector.pair">Pair a device…</button><button class="btn" id="set-connector-reset" data-i18n="connector.reset">Reset and re-pair…</button></div>
      <div id="set-connector-pairing" hidden><img id="set-connector-qr" alt=""><div class="set-hint" id="set-connector-expiry"></div>
        <div id="set-connector-pair-scope" hidden><div class="set-hint" id="set-connector-pair-scope-text"></div></div></div>
      <div id="set-connector-new-device" hidden><div class="set-hint" id="set-connector-new-device-text"></div><button class="btn" id="set-connector-new-device-trust" data-i18n="connector.trust">Trust this device…</button></div>
      <div id="set-connector-devices"></div>
    </div>`;

export function createConnectorSettings(host) {
  const { $, inv, listen, uev, t, toast, confirmDialog, confirmDangerDialog, formatDateTime, formatNumber } = host;
  // Devices seen by the last settings render; a pairing is the difference.
  let connectorDevices = [];
  let connectorScope = 'agentCards';
  let scopeChangePending = false;
  const scopeRank = Object.freeze({ agentCards: 0, boardMetadata: 1, boardAccess: 2 });
  const knownScope = scope => Object.hasOwn(scopeRank, scope) ? scope : 'agentCards';
  const scopeEffectKeys = Object.freeze({
    agentCards: 'connector.scopeEffect.agentCards',
    boardMetadata: 'connector.scopeEffect.boardMetadata',
    boardAccess: 'connector.scopeEffect.boardAccess',
  });
  const pairedTime = seconds => formatDateTime(new Date(seconds * 1000), { dateStyle: 'medium', timeStyle: 'short' });

  // A pairing emits `connector-changed`. While a QR code is shown, the new
  // device is announced by name and time and the spent code is hidden, so a
  // pairing by someone who saw the code cannot go unnoticed.
  async function connectorPairingChanged() {
    if ($('set-connector-pairing').hidden) return;
    const before = connectorDevices;
    let status;
    try { status = await inv('connector_status'); } catch (_) { return; }
    const device = newlyPairedDevice(before, status?.devices);
    if (!device) return;
    $('set-connector-pairing').hidden = true;
    newDeviceId = device.id;
    toast(t('connector.pairedNotice', { name: device.name, time: pairedTime(device.pairedAt) }));
    await renderConnectorSettings();
  }

  // Shown only beside a visible pairing code while full access is not shared.
  function renderPairScope() {
    const show = !$('set-connector-pairing').hidden && connectorScope !== 'boardAccess';
    $('set-connector-pair-scope').hidden = !show;
    $('set-connector-pair-scope-text').textContent = show ? t('connector.pairScope') : '';
  }

  // The device paired while this Settings view showed its code, offered trust
  // by name until it is trusted, revoked or the global scope already covers it.
  let newDeviceId = null;
  function renderNewDevice() {
    const device = connectorDevices.find(entry => entry.id === newDeviceId);
    const show = !!device && !device.revoked && !device.trusted && connectorScope !== 'boardAccess';
    $('set-connector-new-device').hidden = !show;
    $('set-connector-new-device-text').textContent = show ? t('connector.newDeviceTrust', { name: device.name }) : '';
    $('set-connector-new-device-trust').onclick = show ? () => setTrusted(device, true) : null;
  }

  async function setTrusted(device, trusted) {
    if (trusted && !(await confirmDialog(t('connector.trustConfirm', { name: device.name })))) return;
    try { await inv('connector_set_trusted', { deviceId: device.id, trusted }); }
    catch (_) { toast(t('connector.actionFailed')); }
    await renderConnectorSettings();
  }

  // The one global scope change path: an expansion is confirmed before the
  // native write.
  async function changeScope(requested) {
    const control = $('set-connector-scope');
    if (scopeChangePending || requested === connectorScope) { control.value = connectorScope; return; }
    scopeChangePending = true;
    control.disabled = true;
    if (scopeRank[requested] > scopeRank[connectorScope]) {
      const key = requested === 'boardAccess' ? 'connector.boardAccessConfirm' : 'connector.boardMetadataConfirm';
      if (!(await confirmDialog(t(key)))) {
        control.value = connectorScope;
        scopeChangePending = false;
        control.disabled = false;
        return;
      }
    }
    try {
      await inv('connector_set_scope', { scope: requested });
    } catch (_) {
      control.value = connectorScope;
      toast(t('connector.actionFailed'));
    }
    scopeChangePending = false;
    await renderConnectorSettings();
  }

  async function renderConnectorSettings() {
    const scopeControl = $('set-connector-scope');
    scopeControl.disabled = true;
    let status = null; let addresses = [];
    try { [status, addresses] = await Promise.all([inv('connector_status'), inv('connector_addresses')]); } catch (_) {}
    connectorDevices = status?.devices || [];
    const select = $('set-connector-address'); select.replaceChildren();
    for (const address of addresses || []) {
      const option = document.createElement('option'); option.value = address; option.textContent = address; select.appendChild(option);
    }
    if (status?.address && !(addresses || []).includes(status.address)) {
      const option = document.createElement('option'); option.value = status.address; option.textContent = status.address; select.appendChild(option);
    }
    if (status?.address) select.value = status.address;
    $('set-connector-port').value = String(status?.port || 47631);
    $('set-connector-status').textContent = !status?.enabled ? t('connector.off')
      : status.running ? t('connector.listening', { origin: status.origin || '' }) : t('connector.notRunning');
    $('set-connector-toggle').textContent = t(status?.enabled ? 'connector.disable' : 'connector.enable');
    $('set-connector-toggle').dataset.enabled = String(status?.enabled === true);
    if (status) connectorScope = knownScope(status.scope);
    scopeControl.value = connectorScope;
    scopeControl.disabled = !status || scopeChangePending;
    $('set-connector-scope-effect').textContent = t(scopeEffectKeys[connectorScope]);
    const withheld = status?.withheldCards || 0;
    $('set-connector-withheld').hidden = !(withheld > 0);
    $('set-connector-withheld').textContent = withheld > 0 ? t('connector.withheld', { count: formatNumber(withheld) }) : '';
    renderPairScope();
    $('set-connector-pair').disabled = !status?.running;
    $('set-connector-reset').disabled = status?.enabled === true;
    const devices = $('set-connector-devices'); devices.replaceChildren();
    for (const device of status?.devices || []) {
      const row = document.createElement('div'); row.className = 'set-row';
      const label = document.createElement('span'); label.textContent = device.name;
      const state = document.createElement('span');
      state.textContent = device.revoked ? t('connector.revoked') : [
        ...(device.trusted ? [t('connector.trusted')] : []),
        t('connector.pairedAt', { time: pairedTime(device.pairedAt) }),
        device.lastSeenAt ? t('connector.lastSeen', { time: pairedTime(device.lastSeenAt) }) : t('connector.notSeen'),
        ...(device.clientBuild ? [t('connector.clientBuild', { build: device.clientBuild })] : []),
      ].join(' · ');
      row.append(label, state);
      if (!device.revoked) {
        const trust = document.createElement('button'); trust.className = 'btn';
        trust.textContent = t(device.trusted ? 'connector.untrust' : 'connector.trust');
        trust.dataset.deviceTrust = device.trusted ? 'untrust' : 'trust';
        trust.onclick = () => setTrusted(device, !device.trusted);
        row.appendChild(trust);
        const revoke = document.createElement('button'); revoke.className = 'btn'; revoke.textContent = t('connector.revoke');
        revoke.dataset.deviceRevoke = 'revoke';
        revoke.onclick = async () => {
          if (!await confirmDialog(t('connector.revokeConfirm', { name: device.name }))) return;
          try { await inv('connector_revoke', { deviceId: device.id }); await renderConnectorSettings(); }
          catch (_) { toast(t('connector.actionFailed')); }
        };
        row.appendChild(revoke);
      }
      devices.appendChild(row);
    }
    renderNewDevice();
  }

  function mount() {
    listen('connector-changed', connectorPairingChanged).catch(() => uev('listen-fail', 'connector-changed'));
    $('set-connector-toggle').onclick = async () => {
      const enabled = $('set-connector-toggle').dataset.enabled === 'true';
      if (!enabled && !(await confirmDangerDialog(t('connector.enableConfirm'), t('connector.enable')))) return;
      try {
        if (enabled) await inv('connector_disable');
        else await inv('connector_enable', { address: $('set-connector-address').value, port: Number($('set-connector-port').value) });
      } catch (_) { toast(t('connector.actionFailed')); }
      await renderConnectorSettings();
    };
    $('set-connector-scope').onchange = async () => {
      const control = $('set-connector-scope');
      if (control.disabled || scopeChangePending) { control.value = connectorScope; return; }
      await changeScope(knownScope(control.value));
    };
    $('set-connector-pair').onclick = async () => {
      try {
        const pairing = await inv('connector_pairing');
        $('set-connector-qr').src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(pairing.svg)}`;
        $('set-connector-qr').alt = t('connector.qrAlt');
        $('set-connector-expiry').textContent = t('connector.expires', { seconds: formatNumber(Math.max(0, pairing.expiresAt - Math.floor(Date.now() / 1000))) });
        $('set-connector-pairing').hidden = false;
        renderPairScope();
      } catch (_) { toast(t('connector.actionFailed')); }
    };
    $('set-connector-reset').onclick = async () => {
      if (!await confirmDangerDialog(t('connector.resetConfirm'), t('connector.reset'))) return;
      try { await inv('connector_reset_identity'); $('set-connector-pairing').hidden = true; }
      catch (_) { toast(t('connector.actionFailed')); }
      await renderConnectorSettings();
    };
  }

  return Object.freeze({
    mount,
    refresh: renderConnectorSettings,
    renderConnectorSettings,
    connectorPairingChanged,
    pairedTime,
  });
}

export const CONTRIBUTION = Object.freeze({
  id: 'connector',
  slots: Object.freeze([Object.freeze({ slotId: 'set-extension-remote', html: PANEL_HTML })]),
  settings: Object.freeze([Object.freeze({
    id: 'connector', section: 'remote', order: 0,
    labelKeys: ['connector.title', 'connector.address', 'connector.scope', 'connector.scope.boardMetadata', 'connector.scope.boardAccess', 'connector.pair'],
    keywords: ['phone', 'iphone', 'mobile', 'pair', 'pairing', 'qr', 'device', 'remote', '手机', '配对', '设备', '二维码', '远程'],
  })]),
  create: createConnectorSettings,
});
