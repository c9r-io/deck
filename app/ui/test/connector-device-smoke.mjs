// Isolated real-WKWebView host for the physical-iPhone Connector acceptance
// (scripts/connector-device-acceptance.py). Debug smoke only: process-local
// smoke credentials, an isolated data directory and tmux socket.
//
// Steps `trust`/`untrust` use the per-device trust controls.
// Setup creates disposable cards (a shell where the driver later starts
// Claude by typing, a plain shell, a stopped card and a never-launched saved
// `claude` card), keeps the legacy "All card summaries" scope, enables the
// listener on the private address the driver names and shows the pairing QR,
// all through the production Settings controller in this page: the select's
// production change handler, the production confirmation dialog (its Confirm
// button is clicked in the DOM), the Enable/Pair/Revoke buttons. The window
// is not activated, so the developer's foreground is left alone; this is not
// a hand on a mouse. The pairing code reaches the phone only as the rendered
// QR image (`smoke_native_snapshot`), which the driver decodes and types into
// the phone's paste field.
//
// Afterwards the driver writes one closed step word at a time
// (`<seq>:<step>`, read with `smoke_native_scenario`); each is performed once
// and answered with `connector-device-step` (a = seq, b = step code) plus a
// snapshot of the Settings panel in <data dir>/evidence/.
export async function runConnectorDeviceSmoke() {
  const { $, inv } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { openSettings, selectSettingsSection, renderConnectorSettings } = await import('../js/settings.js');
  const { addManual, emptyBuffer } = await import('../js/buffer-model.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 15000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) return false;
      await pause(100);
    }
    return true;
  };
  const report = (name, ok, a = 1, b = 0) => inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.abs(a || 1), b });
  const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
  const status = () => inv('connector_status');
  const confirmShown = () => !!$('cfm') && getComputedStyle($('cfm')).display !== 'none' && $('cfm-msg').textContent.length > 0;
  const confirm = async () => {
    if (!await until(confirmShown, 5000)) return false;
    $('cfm-yes').click();
    return true;
  };
  const showRemote = async () => {
    await openSettings(); selectSettingsSection('remote'); await renderConnectorSettings();
  };
  const chooseScope = async scope => {
    await showRemote();
    const control = $('set-connector-scope');
    if (!await until(() => !control.disabled)) return false;
    control.value = scope;
    const changed = control.onchange();
    const before = (await status()).scope;
    const expands = { agentCards: 0, boardMetadata: 1, boardAccess: 2 };
    if (expands[scope] > expands[before] && !await confirm()) return false;
    await changed;
    await renderConnectorSettings();
    return (await status()).scope === scope;
  };
  // `configure` runs after the panel render, which resets the address and
  // port fields to the host's saved values.
  const toggle = async (enable, configure = () => true) => {
    await showRemote();
    if (!configure()) return false;
    const button = $('set-connector-toggle');
    if ((button.dataset.enabled === 'true') === enable) return true;
    const clicked = button.onclick();
    if (enable && !await confirm()) return false;
    await clicked;
    return until(async () => { const s = await status(); return s.enabled === enable && s.running === enable; });
  };

  let stage = 0;
  try {
    stage = 1;
    await until(() => provider.projects().length > 0);
    const project = provider.projects()[0]; const column = project.columns[0];
    const word = async () => (await inv('smoke_native_scenario').catch(() => '')) || '';
    if (!await until(async () => (await word()).startsWith('0:setup-'), 120000)) throw new Error('no setup step');
    const [, setup] = (await word()).split(':');
    const parts = setup.slice('setup-'.length).split('-');
    const address = parts.slice(0, 4).join('.'); const port = parts[4];
    stage = 2;
    await provider.createStarted({ projectId: project.id, columnId: column.id, title: 'qa manual agent', cmd: '', dir: '/tmp' });
    await provider.createStarted({ projectId: project.id, columnId: column.id, title: 'qa shell', cmd: '', dir: '/tmp',
      buffer: addManual(emptyBuffer(), { id: 'Nqaseeded', text: 'QA seeded note', now: Date.now() }).buffer });
    await provider.create({ projectId: project.id, columnId: column.id, title: 'qa stopped', cmd: '', dir: '/tmp' });
    const saved = await provider.create({ projectId: project.id, columnId: column.id, title: 'qa saved claude', cmd: 'claude', dir: '/tmp' });
    await provider.markLaunched(saved.id);
    stage = 3;
    // The legacy choice the field Mac kept: summaries only.
    const legacy = await chooseScope('boardMetadata');
    const enabled = legacy && await toggle(true, () => {
      $('set-connector-address').value = address;
      $('set-connector-port').value = port;
      return $('set-connector-address').value === address && $('set-connector-port').value === port;
    });
    stage = 4;
    const listening = enabled && (await status()).origin === `https://${address}:${port}`;
    $('set-connector-pair').onclick();
    const qr = await until(() => !$('set-connector-pairing').hidden && $('set-connector-qr').src.startsWith('data:image/svg'));
    $('set-connector-qr').scrollIntoView({ block: 'center' });
    await pause(500);
    await snapshot('pairing-qr');
    // The kept summaries-only scope is explained beside the code, with the
    // confirmed full-access shortcut, while the user is still at the Mac.
    const offered = !$('set-connector-pair-scope').hidden
      && $('set-connector-pair-scope-text').textContent.length > 0 && !$('set-connector-withheld').hidden;
    await report('connector-device-ready', listening && qr && offered, stage,
      (legacy ? 1 : 0) | (enabled ? 2 : 0) | (qr ? 4 : 0) | (offered ? 8 : 0));

    const steps = {
      'snap-settings': async () => { await showRemote(); return true; },
      pair: async () => {
        await showRemote(); $('set-connector-pair').onclick();
        const shown = await until(() => !$('set-connector-pairing').hidden);
        $('set-connector-qr').scrollIntoView({ block: 'center' });
        await pause(500);
        await snapshot('pairing-qr');
        return shown;
      },
      grant: () => chooseScope('boardAccess'),
      downgrade: () => chooseScope('boardMetadata'),
      regrant: () => chooseScope('boardAccess'),
      disable: () => toggle(false),
      enable: () => toggle(true),
      // Per-device trust: the offer shown right after pairing when it is still
      // visible, otherwise the device row; both use the production handler
      // and its confirmation. Stopping trust needs no confirmation.
      trust: async () => {
        await showRemote();
        const offer = $('set-connector-new-device');
        const button = !offer.hidden ? $('set-connector-new-device-trust')
          : $('set-connector-devices').querySelector('button[data-device-trust="trust"]');
        if (!button) return false;
        const clicked = button.onclick();
        if (!await confirm()) return false;
        await clicked;
        const devices = (await status()).devices.filter(device => !device.revoked);
        return devices.length === 1 && devices[0].trusted && (await status()).scope === 'boardMetadata';
      },
      untrust: async () => {
        await showRemote();
        const button = $('set-connector-devices').querySelector('button[data-device-trust="untrust"]');
        if (!button) return false;
        await button.onclick();
        return (await status()).devices.every(device => !device.trusted);
      },
      fault: async () => (await inv('smoke_fault_set', { kind: 'connector-after-accept', count: 1 })).remaining === 1,
      revoke: async () => {
        await showRemote();
        const button = $('set-connector-devices').querySelector('button[data-device-revoke="revoke"]');
        if (!button) return false;
        const clicked = button.onclick();
        if (!await confirm()) return false;
        await clicked;
        return (await status()).devices.every(device => device.revoked);
      },
    };
    const codes = Object.keys(steps);
    let seq = 0;
    for (;;) {
      const current = await word();
      const [number, step] = current.split(':');
      if (Number(number) === seq + 1 && Object.hasOwn(steps, step)) {
        seq += 1;
        let ok = false;
        try { ok = await steps[step](); } catch (_) { ok = false; }
        await pause(400);
        await snapshot(`step-${seq}-${step}`);
        await report('connector-device-step', ok, seq, codes.indexOf(step) + 1);
      } else if (step === 'done') {
        return;
      }
      await pause(200);
    }
  } catch (_) {
    await report('connector-device-ready', false, stage, 0);
  }
}
