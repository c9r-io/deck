// Channel setup is a user-started, cancelable Settings operation. It waits for
// credential saves before checking capability and uses the ONE settings writer.
// No token, manifest or setup progress is persisted here. App URLs are navigation
// hints only; only native validation can establish credential/connection facts.
export function slackAppSettingsUrl(appId, page = 'app-manifest', workspaceId = null) {
  if (!['app-manifest', 'oauth', 'install-on-team'].includes(page)) throw new Error('page');
  // The legacy api.slack.com/apps/<id>/<page> route can return 404.
  // Current settings need both identities; otherwise use Slack's App picker.
  return /^A[A-Z0-9]{1,63}$/.test(appId || '') && /^T[A-Z0-9]{1,63}$/.test(workspaceId || '')
    ? `https://app.slack.com/app-settings/${workspaceId}/${appId}/${page}` : 'https://api.slack.com/apps';
}

// Merge into the user's exported JSON, never replace it with Deck's template.
// The required fragment comes from the native manifest (one scope/event owner).
export function mergeSlackChannelManifest(text, required) {
  if (text.length > 131072) throw new Error('manifest');
  const doc = JSON.parse(text);
  const object = value => value && typeof value === 'object' && !Array.isArray(value);
  if (!object(doc) || !object(doc.display_information) || typeof doc.display_information.name !== 'string') throw new Error('manifest');
  const child = (parent, key) => {
    if (parent[key] === undefined) parent[key] = {};
    if (!object(parent[key])) throw new Error('manifest');
    return parent[key];
  };
  const append = (parent, key, values) => {
    const current = parent[key] ?? [];
    if (!Array.isArray(current) || !current.every(v => typeof v === 'string')) throw new Error('manifest');
    parent[key] = [...new Set([...current, ...values])];
  };
  const features = child(doc, 'features');
  if (!features.bot_user) features.bot_user = structuredClone(required.features.bot_user);
  else if (!object(features.bot_user)) throw new Error('manifest');
  append(child(child(doc, 'oauth_config'), 'scopes'), 'bot', required.oauth_config.scopes.bot);
  const settings = child(doc, 'settings');
  settings.socket_mode_enabled = true;
  append(child(settings, 'event_subscriptions'), 'bot_events', required.settings.event_subscriptions.bot_events);
  return JSON.stringify(doc, null, 2);
}

export function slackChannelSetupIssue(s) {
  if (!s || typeof s.botPresent !== 'boolean' || typeof s.appPresent !== 'boolean') return 'unavailable';
  if (!s.botPresent) return 'needs-bot';
  if (!s.botValid) return s.botError === 'scope' ? 'needs-scopes'
    : ['network', 'timeout', 'http', 'ratelimited'].includes(s.botError) ? 'network' : 'invalid-bot';
  if (!s.appPresent) return 'needs-app';
  if (s.userPresent && !s.userValid) return 'user-error';
  if (s.workspaceMatch !== true) return 'workspace-mismatch';
  // An idle socket after restart is not an invalid App token. Enabling
  // collection may start the existing owner; its live status stays separate.
  return null;
}

export function createSlackChannelSetup({ saveTokens, readStatus, enable, changed }) {
  let generation = 0, pending = null;
  return {
    cancel() { generation++; },
    run() {
      if (pending) return pending;
      const mine = ++generation;
      const current = () => generation === mine;
      changed('checking');
      pending = (async () => {
        try {
          if (!await saveTokens()) { if (current()) changed('credential-error'); return false; }
          if (!current()) return false;
          const status = await readStatus();
          if (!current()) return false;
          const issue = slackChannelSetupIssue(status);
          if (issue) { changed(issue); return false; }
          const ok = await enable(current);
          if (current()) changed(ok ? 'enabled' : 'save-error');
          return !!ok;
        } catch (_) {
          if (current()) changed('unavailable');
          return false;
        } finally { pending = null; }
      })();
      return pending;
    },
  };
}
