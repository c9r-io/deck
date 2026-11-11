// MCP terminal control's first-party Settings contribution. This module owns
// the panel markup, authorization dialog, client/tunnel presentation and DOM
// bindings; the generic Settings extension host supplies native services.
// Part of deck's no-build frontend: native ES modules, no bundler.

const PANEL_HTML = `
    <div class="set-group" id="set-item-mcp" data-setting-id="mcp" tabindex="-1" role="group" aria-labelledby="set-item-mcp-title">
      <div class="set-group-head"><h3 class="set-group-title" id="set-item-mcp-title" data-i18n="mcp.title">MCP terminal control</h3><span class="set-group-status" id="set-mcp-status"></span></div>
      <div class="set-hint" data-i18n="mcp.hint">Off by default. An authorized MCP client can execute code with your account permissions and receive terminal output. This trusted-host mode is not an OS sandbox.</div>
      <div class="set-actions mcp-settings-actions"><button class="btn" id="set-mcp-toggle" data-i18n="mcp.enable">Enable</button><button class="btn" id="set-mcp-add" data-i18n="mcp.add">Authorize this project…</button></div>
      <div class="set-row mcp-retention-row"><label for="set-mcp-retention" data-i18n="mcp.outputRetention">Managed output retention</label><select id="set-mcp-retention"><option value="3600000" data-i18n="mcp.retention1h">1 hour</option><option value="86400000" data-i18n="mcp.retention24h">24 hours</option><option value="259200000" data-i18n="mcp.retention3d">3 days</option><option value="604800000" data-i18n="mcp.retention7d">7 days</option></select></div>
      <div id="set-mcp-clients"></div>
    </div>`;

import { defaultWorkerType, initialPermissions, permissionsPayload, permissionSummary, workerTypeErrorKey, workerTypesProblem } from './mcp-permissions-model.js';

const AUTHORIZATION_HTML = `<div id="mcp-auth" role="dialog" aria-modal="true" aria-labelledby="mcp-auth-title">
  <div id="mcp-auth-box">
    <h2 id="mcp-auth-title" data-i18n="mcp.authorizeTitle">Authorize MCP client</h2>
    <div class="set-row"><label for="mcp-auth-name" data-i18n="mcp.clientName">Name this MCP client</label><input type="text" id="mcp-auth-name" maxlength="120" autocomplete="off" spellcheck="false"></div>
    <div class="set-row"><label for="mcp-auth-project" data-i18n="mcp.project">Project</label><div class="mcp-auth-field"><select id="mcp-auth-project" aria-describedby="mcp-auth-project-error"></select><p class="mcp-field-error" id="mcp-auth-project-error" role="alert" hidden></p></div></div>
    <div class="set-row"><label for="mcp-auth-root" data-i18n="mcp.directory">Authorized directory</label><div class="mcp-auth-field"><input type="text" id="mcp-auth-root" autocomplete="off" spellcheck="false" aria-describedby="mcp-auth-root-error" data-i18n-placeholder="mcp.directoryPlaceholder" placeholder="/path/to/project"><p class="mcp-field-error" id="mcp-auth-root-error" role="alert" hidden></p></div></div>
    <p class="set-hint" id="mcp-auth-hint" data-i18n="mcp.authorizeHint">Choose the project and directory explicitly. A configured project directory is filled in but remains editable. Its canonical path will be shown again before Deck creates the authorization.</p>
    <div class="mcp-perms" id="mcp-auth-perms">
      <h3 class="mcp-perms-title" data-i18n="mcp.permsTitle">What it may do</h3>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-read" checked disabled> <span data-i18n="mcp.permRead">Read files in this directory</span></label>
      <p class="set-hint mcp-perm-hint" data-i18n="mcp.permReadHint">Read-only; nothing runs.</p>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-create"> <span data-i18n="mcp.permCreate">Create sessions and run commands in them</span></label>
      <p class="set-hint mcp-perm-hint mcp-perm-danger" data-i18n="mcp.permCreateHint">It may run any program as you.</p>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-automations"> <span data-i18n="mcp.permAutomations">Trigger Agent automations</span></label>
      <p class="set-hint mcp-perm-hint" data-i18n="mcp.permAutomationsHint">Items it hands over open cards.</p>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-workers"> <span data-i18n="mcp.permWorkers">Drive worker cards</span></label>
      <p class="set-hint mcp-perm-hint" data-i18n="mcp.permWorkersHint">It starts worker cards.</p>
      <div class="mcp-perm-sub" id="mcp-perm-worker-box">
        <div class="mcp-types-head"><span data-i18n="mcp.typeName">Name</span><span data-i18n="mcp.typeCommand">Command</span><span data-i18n="mcp.typeColumn">Column</span><span data-i18n="mcp.typeDescription">Good at</span><span></span></div>
        <div id="mcp-perm-types"></div>
        <button class="btn" id="mcp-perm-type-add" type="button" data-i18n="mcp.typeAdd">Add type</button>
        <label class="mcp-perm"><input type="checkbox" id="mcp-perm-first-send"> <span data-i18n="mcp.permFirstSend">Send the first line without waiting</span></label>
        <p class="set-hint mcp-perm-hint" data-i18n="mcp.permFirstSendHint">A startup screen may take it.</p>
        <label class="mcp-perm"><input type="checkbox" id="mcp-perm-replies"> <span data-i18n="mcp.permReplies">Share replies</span></label>
        <p class="set-hint mcp-perm-hint" data-i18n="mcp.permRepliesHint">Claude Code only.</p>
      </div>
      <div class="mcp-perm-sub" id="mcp-perm-follow-box">
        <label class="mcp-perm"><input type="checkbox" id="mcp-perm-follow"> <span data-i18n="mcp.permFollow">Open worker cards beside their orchestrator</span></label>
        <p class="set-hint mcp-perm-hint" data-i18n="mcp.permFollowHint">When the agent runs in a Deck card, the cards it starts open in that card's column and only it drives them. Off: each type's or automation's column.</p>
      </div>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-drafts"> <span data-i18n="mcp.permDrafts">Draft templates and automations</span></label>
      <p class="set-hint mcp-perm-hint" data-i18n="mcp.permDraftsHint">Drafts wait in Needs attention; nothing works until you approve.</p>
      <label class="mcp-perm"><input type="checkbox" id="mcp-perm-mentions"> <span data-i18n="mcp.permMentions">Read Slack mentions</span></label>
      <p class="set-hint mcp-perm-hint mcp-perm-danger" data-i18n="mcp.permMentionsHint">Other people wrote these messages, and this agent will read them. Covers your whole Slack account. With automations, it may hand them to its own.</p>
      <p class="mcp-field-error" id="mcp-perm-error" role="alert" hidden></p>
    </div>
    <div class="cfm-actions">
      <button class="btn" id="mcp-auth-no" data-i18n="common.cancel">Cancel</button>
      <button class="btn primary" id="mcp-auth-yes" data-i18n="mcp.authorize">Authorize</button>
    </div>
  </div>
</div>
`;

export function createMcpSettings(host) {
  const { $, inv, store, t, toast, confirmDangerDialog, choiceDialog, formatNumber } = host;
  let mcpAuthDone = null;

  async function renderMcpSettings() {
    let status = null;
    try { status = await inv('mcp_status'); } catch (_) {}
    /* The authorization data could not be loaded in this run: MCP stays off
       and nothing here can change that, so the row says why instead of
       offering controls that would fail. Ordinary terminals are unaffected
       (mcp/mod.rs). */
    const unavailable = status?.unavailable === true;
    $('set-mcp-status').textContent = t(unavailable ? 'mcp.unavailable' : status?.enabled ? 'mcp.on' : 'mcp.off');
    $('set-mcp-toggle').textContent = t(status?.enabled ? 'mcp.disable' : 'mcp.enable');
    $('set-mcp-toggle').dataset.enabled = String(status?.enabled === true);
    $('set-mcp-toggle').disabled = unavailable;
    $('set-mcp-add').disabled = unavailable || !status?.enabled;
    $('set-mcp-retention').disabled = unavailable;
    $('set-mcp-retention').value = String(status?.outputRetentionMs || 24 * 60 * 60 * 1000);
    const clients = $('set-mcp-clients'); clients.replaceChildren();
    if (unavailable) {
      const note = document.createElement('div'); note.className = 'set-hint';
      note.textContent = t('mcp.unavailableHint');
      clients.appendChild(note);
    }
    for (const client of status?.clients || []) {
      const row = document.createElement('div'); row.className = 'set-row mcp-client-row';
      const details = document.createElement('div'); details.className = 'mcp-client-details';
      const label = document.createElement('span');
      label.textContent = client.name;
      const scope = document.createElement('span'); scope.style.color = 'var(--muted)';
      scope.textContent = [t('mcp.projectCount', { count: formatNumber(client.projects?.length || 0) }),
        ...permissionSummary(client).map(key => t(key)),
        ...(client.revoked ? [t('mcp.revoked')] : [])].join(' · ');
      details.append(label, scope);
      const actions = document.createElement('div'); actions.className = 'mcp-client-actions';
      row.append(details, actions);
      if (!client.revoked) {
        const copy = document.createElement('button'); copy.className = 'btn'; copy.textContent = t('mcp.copyConfig');
        copy.onclick = async () => {
          try {
            const command = await inv('mcp_adapter_path');
            const config = `[mcp_servers.deck]\ncommand = ${JSON.stringify(command)}\nargs = ["--client-id", ${JSON.stringify(client.id)}]\ndefault_tools_approval_mode = "writes"\n`;
            await inv('write_clipboard', { text: config }); toast(t('mcp.configCopied'));
          } catch (_) { toast(t('mcp.actionFailed')); }
        };
        const revoke = document.createElement('button'); revoke.className = 'btn'; revoke.textContent = t('mcp.revoke');
        revoke.onclick = async () => {
          if (!await confirmDangerDialog(t('mcp.revokeConfirm', { name: client.name }), t('mcp.revoke'))) return;
          try { await inv('mcp_client_revoke', { clientId: client.id }); await renderMcpSettings(); }
          catch (_) { toast(t('mcp.actionFailed')); }
        };
        /* every choice in the one dialog it was authorized with
           (mcp-permissions-model.js); withdrawing takes effect at once */
        const permissions = document.createElement('button'); permissions.className = 'btn mcp-permissions';
        permissions.textContent = t('mcp.permissions');
        permissions.onclick = async () => {
          const project = store.projects.find(value => value.id === client.projects?.[0]?.projectId) || null;
          const chosen = await mcpAuthorizationDialog(store.projects, { client, project });
          if (!chosen) return;
          try { await inv('mcp_client_permissions', { clientId: client.id, permissions: chosen.permissions }); await renderMcpSettings(); }
          catch (error) { toast(t(workerTypeErrorKey(error) || 'mcp.actionFailed')); }
        };
        actions.append(copy, permissions, revoke);
      } else {
        const remove = document.createElement('button'); remove.className = 'btn set-danger'; remove.textContent = t('mcp.delete');
        remove.onclick = async () => {
          remove.disabled = true;
          let tunnel = null;
          try { tunnel = await inv('tunnel_helper_status', { clientId: client.id }); } catch (_) {}
          remove.disabled = false;
          if (tunnel?.helperState === 'installed' && tunnel.runtimeExists) {
            const choice = await choiceDialog(t('mcp.tunnelDeleteWarning'), [
              { id: 'stop', label: t('mcp.tunnelStop') },
              { id: 'remove-runtime', label: t('mcp.tunnelRemove') },
              { id: 'delete-client', label: t('mcp.tunnelDeleteAnyway'), primary: true },
            ]);
            if (choice === 'stop' || choice === 'remove-runtime') {
              const command = choice === 'stop' ? 'tunnel_helper_stop' : 'tunnel_helper_remove';
              try { await inv(command, { clientId: client.id }); await renderMcpSettings(); }
              catch (_) { toast(t('mcp.tunnelActionFailed')); }
              return;
            }
            if (choice !== 'delete-client') return;
          } else if (!await confirmDangerDialog(t('mcp.deleteConfirm', { name: client.name }), t('mcp.delete'))) return;
          try { await inv('mcp_client_delete', { clientId: client.id }); await renderMcpSettings(); }
          catch (_) { toast(t('mcp.actionFailed')); }
        };
        actions.append(remove);
      }
      /* its adapter speaks another control protocol and is being refused (kept
         by the backend for an authenticated request only, mcp/mod.rs): the one
         thing that helps is restarting that integration, so the row says so */
      if (client.adapterRefused === true && !client.revoked) {
        const note = document.createElement('div'); note.className = 'mcp-tunnel-status mcp-adapter-refused';
        note.style.color = 'var(--wait)';
        note.textContent = t('mcp.adapterRefused');
        row.appendChild(note);
      }
      clients.appendChild(row);
      renderTunnelForClient(client, row, actions);
    }
  }

  /* Tunnel is an optional enhancement. This async branch never delays MCP
     rendering and never participates in revoke/delete authority changes. */
  async function renderTunnelForClient(client, row, actions) {
    const line = document.createElement('div'); line.className = 'mcp-tunnel-status';
    line.textContent = t('mcp.tunnelChecking');
    row.appendChild(line);
    let status;
    try { status = await inv('tunnel_helper_status', { clientId: client.id }); }
    catch (_) { status = { helperState: 'helper_error' }; }
    if (!line.isConnected) return;
    if (status.helperState !== 'installed') {
      line.textContent = t(status.helperState === 'helper_missing' ? 'mcp.tunnelOptionalMissing' : 'mcp.tunnelUnavailable');
      if (status.helperState === 'helper_missing') {
        const instructions = document.createElement('button'); instructions.className = 'btn';
        instructions.textContent = t('mcp.tunnelInstallInstructions');
        instructions.onclick = async () => {
          instructions.disabled = true;
          try {
            await inv('write_clipboard', { text: 'docs/mcp-tunnel-helper.md' });
            toast(t('mcp.tunnelInstructionsCopied'));
          } catch (_) { toast(t('mcp.tunnelActionFailed')); }
          finally { instructions.disabled = false; }
        };
        actions.append(instructions);
      }
      return;
    }
    const stateKey = `mcp.tunnelState.${status.tunnelState || 'error'}`;
    line.textContent = `${t(status.developmentHelper ? 'mcp.tunnelDevelopmentHelper' : 'mcp.tunnelInstalled')} · ${t(stateKey)}`;
    const buttons = [];
    let pending = false;
    const addAction = (label, command) => {
      const button = document.createElement('button'); button.className = 'btn'; button.textContent = label;
      button.onclick = async () => {
        if (pending) return;
        pending = true;
        for (const item of buttons) item.disabled = true;
        try { await inv(command, { clientId: client.id }); await renderMcpSettings(); }
        catch (_) { pending = false; toast(t('mcp.tunnelActionFailed')); for (const item of buttons) item.disabled = false; }
      };
      buttons.push(button); actions.append(button);
    };
    if (status.tunnelState === 'stopped') addAction(t('mcp.tunnelStart'), 'tunnel_helper_start');
    if (['starting', 'ready', 'unhealthy', 'stale'].includes(status.tunnelState)) addAction(t('mcp.tunnelStop'), 'tunnel_helper_stop');
    if (['not_configured', 'key_missing'].includes(status.tunnelState)) {
      const setup = document.createElement('button'); setup.className = 'btn'; setup.textContent = t('mcp.tunnelSetup');
      setup.onclick = async () => {
        if (pending) return;
        pending = true;
        setup.disabled = true;
        try {
          const command = await inv('tunnel_helper_setup_command', { clientId: client.id });
          await inv('write_clipboard', { text: command }); toast(t('mcp.tunnelSetupCopied'));
        } catch (_) { toast(t('mcp.tunnelActionFailed')); }
        finally { pending = false; setup.disabled = false; }
      };
      buttons.push(setup); actions.append(setup);
    }
    if (status.tunnelId) {
      const copy = document.createElement('button'); copy.className = 'btn'; copy.textContent = t('mcp.tunnelCopyId');
      copy.onclick = async () => {
        if (pending) return;
        pending = true;
        copy.disabled = true;
        try { await inv('write_clipboard', { text: status.tunnelId }); toast(t('mcp.tunnelIdCopied')); }
        catch (_) { toast(t('mcp.tunnelActionFailed')); }
        finally { pending = false; copy.disabled = false; }
      };
      buttons.push(copy); actions.append(copy);
    }
  }

  /* MCP authorization must never inherit an invisible global project choice.
     This dialog starts with no selection and requires an explicit directory.
     A project's configured directory is only a visible, editable initial value;
     the native backend canonicalizes the submitted authorization root, and
     the canonical path is put back in the field (and must be confirmed with
     a second click) when it differs from what was typed.
     Every choice of the client is in the same dialog
     (mcp-permissions-model.js); with `options.client` it edits that client's
     choices, its name, project and directory shown but fixed. */
  function mcpAuthorizationDialog(projects, options = {}) {
    return new Promise(resolve => {
      if (mcpAuthDone) mcpAuthDone(null);
      const editing = options.client || null;
      const modal = $('mcp-auth');
      const name = $('mcp-auth-name');
      const select = $('mcp-auth-project');
      const root = $('mcp-auth-root');
      const save = $('mcp-auth-yes');
      const projectError = $('mcp-auth-project-error');
      const rootError = $('mcp-auth-root-error');
      const permError = $('mcp-perm-error');
      const previewRoot = options.previewRoot || (async value => ({ ok: true, root: value, error: null }));
      const projectExists = options.projectExists || (projectId => available.some(project => project.id === projectId));
      let generation = 0;
      let pending = false;
      let closed = false;
      const available = (projects || []).filter(project => project?.id && typeof project.name === 'string');
      select.replaceChildren();
      const placeholder = document.createElement('option');
      placeholder.value = ''; placeholder.textContent = t('mcp.chooseProject');
      select.appendChild(placeholder);
      for (const project of available) {
        const option = document.createElement('option');
        option.value = project.id; option.textContent = project.name;
        select.appendChild(option);
      }
      const selected = () => available.find(project => project.id === select.value) || null;
      const showError = (element, field, message) => {
        element.textContent = message || '';
        element.hidden = !message;
        field?.setAttribute('aria-invalid', message ? 'true' : 'false');
      };
      /* the permissions: state kept here, painted into the checkboxes and
         the worker type rows */
      const perms = initialPermissions(editing);
      const columns = () => (editing ? options.project : selected())?.columns || [];
      const paintTypes = () => {
        const list = $('mcp-perm-types'); list.replaceChildren();
        perms.workerTypes.forEach((type, index) => {
          const row = document.createElement('div'); row.className = 'mcp-type-row';
          const field = (cls, key, value) => {
            const input = document.createElement('input');
            input.type = 'text'; input.className = cls; input.value = value || '';
            input.spellcheck = false; input.autocomplete = 'off';
            input.setAttribute('aria-label', t(key));
            return input;
          };
          const typeName = field('mcp-type-name', 'mcp.typeName', type.name);
          typeName.oninput = () => { type.name = typeName.value; showError(permError, null, ''); };
          const cmd = field('mcp-type-cmd', 'mcp.typeCommand', type.cmd);
          cmd.oninput = () => { type.cmd = cmd.value; showError(permError, null, ''); };
          const column = document.createElement('select'); column.className = 'mcp-type-column';
          column.setAttribute('aria-label', t('mcp.typeColumn'));
          for (const value of columns()) {
            const option = document.createElement('option');
            option.value = value.id; option.textContent = value.name;
            column.appendChild(option);
          }
          if (!columns().some(value => value.id === type.columnId)) type.columnId = columns()[0]?.id || '';
          column.value = type.columnId;
          column.onchange = () => { type.columnId = column.value; };
          const description = field('mcp-type-desc', 'mcp.typeDescription', type.description);
          description.maxLength = 200;
          description.oninput = () => { type.description = description.value; };
          const remove = document.createElement('button');
          remove.type = 'button'; remove.className = 'btn mcp-type-remove'; remove.textContent = '✕';
          remove.title = t('mcp.typeRemove'); remove.setAttribute('aria-label', t('mcp.typeRemove'));
          remove.onclick = () => { perms.workerTypes.splice(index, 1); paintTypes(); };
          row.append(typeName, cmd, column, description, remove);
          list.appendChild(row);
        });
        $('mcp-perm-type-add').disabled = perms.workerTypes.length >= 8;
      };
      const paintPerms = () => {
        $('mcp-perm-create').checked = perms.allowCreate;
        $('mcp-perm-automations').checked = perms.allowAutomations;
        $('mcp-perm-workers').checked = perms.allowWorkers;
        $('mcp-perm-first-send').checked = perms.workerFirstSend;
        $('mcp-perm-replies').checked = perms.allowReplies;
        $('mcp-perm-drafts').checked = perms.allowDrafts;
        $('mcp-perm-mentions').checked = perms.allowMentions;
        $('mcp-perm-worker-box').hidden = !perms.allowWorkers;
        $('mcp-perm-follow').checked = !perms.ruleColumn;
        $('mcp-perm-follow-box').hidden = !(perms.allowWorkers || perms.allowAutomations);
        paintTypes();
      };
      $('mcp-perm-create').onchange = () => { perms.allowCreate = $('mcp-perm-create').checked; };
      $('mcp-perm-automations').onchange = () => { perms.allowAutomations = $('mcp-perm-automations').checked; paintPerms(); };
      $('mcp-perm-follow').onchange = () => { perms.ruleColumn = !$('mcp-perm-follow').checked; };
      $('mcp-perm-workers').onchange = () => {
        perms.allowWorkers = $('mcp-perm-workers').checked;
        /* turned on: one Claude type and shared replies, ready to use */
        if (perms.allowWorkers && !perms.workerTypes.length) {
          perms.workerTypes.push(defaultWorkerType(columns(), t('mcp.typeDefaultDescription')));
          perms.allowReplies = true;
        }
        showError(permError, null, '');
        paintPerms();
      };
      $('mcp-perm-first-send').onchange = () => { perms.workerFirstSend = $('mcp-perm-first-send').checked; };
      $('mcp-perm-replies').onchange = () => { perms.allowReplies = $('mcp-perm-replies').checked; };
      $('mcp-perm-drafts').onchange = () => { perms.allowDrafts = $('mcp-perm-drafts').checked; };
      $('mcp-perm-mentions').onchange = () => { perms.allowMentions = $('mcp-perm-mentions').checked; };
      $('mcp-perm-type-add').onclick = () => {
        perms.workerTypes.push({ name: '', cmd: '', columnId: columns()[0]?.id || '', description: '' });
        paintTypes();
        $('mcp-perm-types').lastElementChild?.querySelector?.('.mcp-type-name')?.focus?.();
      };
      /* identity: fixed when editing */
      if (editing) {
        $('mcp-auth-title').textContent = t('mcp.permissionsTitle', { name: editing.name });
        name.value = editing.name;
        select.value = options.project?.id || '';
        root.value = editing.projects?.[0]?.roots?.[0] || '';
        save.textContent = t('common.save');
      } else {
        $('mcp-auth-title').textContent = t('mcp.authorizeTitle');
        name.value = 'ChatGPT'; select.value = ''; root.value = '';
        save.textContent = t('mcp.authorize');
      }
      for (const field of [name, select, root]) field.disabled = !!editing;
      $('mcp-auth-hint').hidden = !!editing;
      save.disabled = false;
      showError(permError, null, ''); showError(projectError, select, ''); showError(rootError, root, '');
      paintPerms();
      const done = value => {
        if (closed) return;
        closed = true; generation++;
        modal.style.display = 'none'; modal.onkeydown = null;
        name.oninput = null; root.oninput = null; select.onchange = null; mcpAuthDone = null;
        for (const field of [name, select, root]) field.disabled = false;
        resolve(value);
      };
      mcpAuthDone = done;
      name.oninput = () => { generation++; };
      root.oninput = () => { generation++; showError(rootError, root, ''); };
      select.onchange = () => {
        generation++; showError(projectError, select, ''); showError(rootError, root, '');
        const project = selected();
        root.value = typeof project?.dir === 'string' ? project.dir.trim() : '';
        paintTypes();
      };
      save.onclick = async () => {
        if (pending || closed) return;
        const problem = workerTypesProblem(perms);
        if (problem) { showError(permError, null, t(problem)); return; }
        if (editing) { done({ permissions: permissionsPayload(perms) }); return; }
        const project = selected();
        const clientName = name.value.trim();
        const authorizationRoot = root.value.trim();
        if (!project) {
          showError(projectError, select, t('mcp.projectRequired')); select.focus(); return;
        }
        if (!authorizationRoot) {
          showError(rootError, root, t('mcp.rootRequired')); root.focus(); return;
        }
        if (!clientName) { name.focus(); return; }
        const requestGeneration = generation;
        pending = true; save.disabled = true;
        let preview;
        try { preview = await previewRoot(authorizationRoot); }
        catch (_) { preview = { ok: false, error: 'unavailable' }; }
        pending = false;
        if (closed || requestGeneration !== generation) { save.disabled = false; return; }
        save.disabled = false;
        if (!selected() || selected().id !== project.id || !projectExists(project.id)) {
          showError(projectError, select, t('mcp.projectUnavailable')); select.focus(); return;
        }
        if (!preview?.ok || !preview.root) {
          const key = ({ not_found: 'mcp.rootNotFound', not_directory: 'mcp.rootNotDirectory', not_accessible: 'mcp.rootNotAccessible', too_broad: 'mcp.rootTooBroad' })[preview?.error] || 'mcp.rootUnavailable';
          showError(rootError, root, t(key)); root.focus(); return;
        }
        /* the path Deck will authorize is the one shown: a resolved link or
           a relative spelling is put in the field to be confirmed */
        if (preview.root !== authorizationRoot) {
          root.value = preview.root;
          showError(rootError, root, t('mcp.rootResolved', { root: preview.root }));
          root.focus(); return;
        }
        done({ name: clientName, project, root: preview.root, permissions: permissionsPayload(perms) });
      };
      $('mcp-auth-no').onclick = () => done(null);
      modal.onkeydown = event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); done(null); }
      };
      modal.style.display = 'flex';
      (editing ? $('mcp-perm-create') : name).focus();
      if (!editing) name.select();
    });
  }

  function mount() {
    $('set-mcp-toggle').onclick = async () => {
      const enabled = $('set-mcp-toggle').dataset.enabled === 'true';
      if (!enabled && !(await confirmDangerDialog(t('mcp.enableConfirm'), t('mcp.enable')))) return;
      try { await inv(enabled ? 'mcp_disable' : 'mcp_enable'); }
      catch (_) { toast(t('mcp.actionFailed')); }
      await renderMcpSettings();
    };
    $('set-mcp-add').onclick = async () => {
      const authorization = await mcpAuthorizationDialog(store.projects, {
        previewRoot: root => inv('mcp_scope_preview', { root }),
        projectExists: projectId => store.projects.some(project => project.id === projectId),
      });
      if (!authorization) return;
      const { name, project, root, permissions } = authorization;
      if (!store.projects.some(candidate => candidate.id === project.id)) { toast(t('mcp.projectUnavailable')); return; }
      try {
        await inv('mcp_client_add', { name, projects: [{ projectId: project.id, roots: [root] }], permissions });
        await renderMcpSettings();
      } catch (error) {
        toast(String(error).includes('MCP project no longer exists') ? t('mcp.projectUnavailable')
          : t(workerTypeErrorKey(error) || 'mcp.actionFailed'));
      }
    };
    $('set-mcp-retention').onchange = async () => {
      try {
        await inv('mcp_output_retention', { durationMs: Number($('set-mcp-retention').value) });
      } catch (_) { toast(t('mcp.actionFailed')); }
      await renderMcpSettings();
    };
  }

  return Object.freeze({
    mount,
    refresh: renderMcpSettings,
    renderMcpSettings,
    renderTunnelForClient,
    mcpAuthorizationDialog,
  });
}

export const CONTRIBUTION = Object.freeze({
  id: 'mcp',
  slots: Object.freeze([
    Object.freeze({ slotId: 'set-extension-remote', html: PANEL_HTML }),
    Object.freeze({ slotId: 'set-extension-dialogs', html: AUTHORIZATION_HTML }),
  ]),
  settings: Object.freeze([Object.freeze({
    id: 'mcp', section: 'remote', order: 1,
    labelKeys: ['mcp.title', 'mcp.add', 'mcp.outputRetention'],
    keywords: ['mcp', 'chatgpt', 'codex', 'client', 'authorize', 'authorization', 'tunnel', 'remote', 'terminal control',
      '远程', '控制', '授权', '客户端'],
  })]),
  create: createMcpSettings,
});
