// smoke-mcp-helpers.mjs — the MCP client dialog steps the WKWebView smoke
// carriers share (workers-smoke.mjs, agent-live-smoke.mjs). It reports no
// checkpoint of its own: each carrier names its checks
// (smoke-manifest.test.mjs reads them from the carrier files).

export const pause = ms => new Promise(resolve => setTimeout(resolve, ms));

/* the first truthy answer of `test` within `ms`, else null; a throw counts
   as no answer */
export async function until(test, ms, step = 1000) {
  const deadline = Date.now() + ms;
  for (;;) {
    let value = null;
    try { value = await test(); } catch (_) { value = null; }
    if (value) return value;
    if (Date.now() >= deadline) return null;
    await pause(step);
  }
}

/* the client row's Permissions… dialog opened as the person does it
   (extension-mcp.js); throws when the row's button or the dialog is missing */
export async function openPermissions(clientName) {
  const { $ } = await import('../js/state.js');
  const { openSettings, renderMcpSettings } = await import('../js/settings.js');
  const { t } = await import('../js/i18n.js');
  await openSettings({ section: 'remote' });
  await renderMcpSettings();
  const row = [...document.querySelectorAll('.mcp-client-row')].find(value => value.textContent.includes(clientName));
  const open = row?.querySelector('button.mcp-permissions');
  if (!open || open.textContent !== t('mcp.permissions')) throw new Error('permissions button');
  open.click();
  if (!(await until(() => $('mcp-auth').style.display === 'flex', 3000, 50))) throw new Error('permissions dialog');
}

/* one choice changed through that dialog and its Save */
export async function setPermission(clientName, checkbox, on) {
  const { $ } = await import('../js/state.js');
  await openPermissions(clientName);
  if ($(checkbox).checked !== on) $(checkbox).click();
  $('mcp-auth-yes').click();
  await until(() => $('mcp-auth').style.display === 'none', 3000, 50);
}
