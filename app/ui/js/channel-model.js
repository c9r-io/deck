// DOM-free Slack channel monitor model. Native code performs the same closed
// validation before connecting; this side preserves only shapes it can edit.
//
// Admission is a separate, runtime policy (`channelBlockReason`): a rule's
// command must launch `claude` or `codex` with shell-safe arguments, and every line of its
// template must begin with user-written text rather than a `{{msg.*}}`
// placeholder (a message starting with `!` or `/` must never become the
// first character the agent reads). Normalizing keeps a structurally valid
// rule that fails admission, so a rule saved by an older deck is shown as
// blocked and can be edited, never silently dropped on the next save.
// Channel first-send permission is native-issued, independently revocable
// authorization for a new run's head. The editor accepts all first-input
// risks in one save; this module compares semantics for that action, never
// grants authority. Native pending proof freezes the claim and skeleton;
// replay does not consult today's rule or template. Empty first steps remain
// legal for legacy plans but cannot receive first-step permission.
// Applied scratchpad evidence is checked before collecting/expiry, so an
// ACK retry cannot promote a later event to a new run.
import { fillInboundTemplate, inboundTitle, LOCAL_ID_RE, normalizeTemplateStep } from './pure.js';

const CHANNEL_ID = /^[CG][A-Z0-9_-]{0,63}$/;
const USER_ID = /^[UW][A-Z0-9_-]{0,63}$/;
const BOT_ID = /^B[A-Z0-9_-]{0,63}$/;
export const CHANNEL_IDLE_DEFAULT = 30;
export const CHANNEL_IDLE_MAX = 7 * 24 * 60;
/* inbound_channel.rs bounds, mirrored through test/fixtures/limits.json */
export const CHANNEL_RULES_MAX = 64;
export const CHANNEL_IDS_MAX = 64;
export const CHANNEL_KEYWORDS_MAX = 32;
export const CHANNEL_KEYWORD_MAX_CHARS = 64;

// Twin of admission::channel_agent_command: a bare agent or simple arguments,
// without environment prefixes, paths, quoting or shell syntax.
export const channelAgentCommand = command => {
  const match = /^(claude|codex)(?: [-A-Za-z0-9_./:=+,@]+)*$/.exec(command || '');
  return match && match[0] === command ? match[1] : null;
};

const LEADING_MESSAGE = /^\s*\{\{\s*msg\.[a-z]+\s*\}\}/;

// Why a channel rule may not run: 'command' | 'template' | null. The
// template check needs the project's templates; without them only the
// command is judged (the plan re-checks the template when it expands it).
export function channelBlockReason(rule, project) {
  if (!channelAgentCommand(rule?.cmd)) return 'command';
  const template = (project?.templates || []).find(value => value.name === rule.template);
  if (template && template.steps.some(step => LEADING_MESSAGE.test(String(step)))) return 'template';
  return null;
}

export async function channelDigestId(prefix, value) {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
  return prefix + [...new Uint8Array(bytes)].slice(0, 16).map(byte => byte.toString(16).padStart(2, '0')).join('');
}

const unique = (values, re, max) => [...new Set((Array.isArray(values) ? values : [])
  .map(String).map(value => value.trim()).filter(value => re.test(value)))].slice(0, max);

export function normalizeChannelRule(raw) {
  if (!raw || typeof raw !== 'object') return null;
  const kind = String(raw.match?.kind || 'contains');
  const matcher = { kind, caseSensitive: raw.match?.caseSensitive === true };
  if (kind === 'keywords') matcher.keywords = unique(raw.match?.keywords, new RegExp(`^.{1,${CHANNEL_KEYWORD_MAX_CHARS}}$`, 'u'), CHANNEL_KEYWORDS_MAX);
  else matcher.value = String(raw.match?.value || '').slice(0, 1024);
  if (kind === 'regex' && LOCAL_ID_RE.test(String(raw.match?.groupCapture || ''))) {
    matcher.groupCapture = String(raw.match.groupCapture);
  }
  const rule = {
    id: String(raw.id || ''), enabled: raw.enabled !== false, connectionId: 'default',
    channelIds: unique(raw.channelIds, CHANNEL_ID, CHANNEL_IDS_MAX),
    senderUserIds: unique(raw.senderUserIds, USER_ID, 128),
    senderBotIds: unique(raw.senderBotIds, BOT_ID, 128), match: matcher,
    includeThreads: raw.includeThreads !== false,
    projectId: String(raw.projectId || ''), columnId: String(raw.columnId || ''),
    dir: String(raw.dir || '').trim(), cmd: String(raw.cmd || '').trim(),
    template: String(raw.template || ''),
    idleMinutes: Number.isInteger(Number(raw.idleMinutes))
      && Number(raw.idleMinutes) >= 0 && Number(raw.idleMinutes) <= CHANNEL_IDLE_MAX
      ? Number(raw.idleMinutes) : CHANNEL_IDLE_DEFAULT,
  };
  const validMatch = kind === 'contains' ? !!matcher.value && matcher.value.length <= 256
    : kind === 'keywords' ? matcher.keywords.length > 0
      : kind === 'regex' ? !!matcher.value : false;
  if (!LOCAL_ID_RE.test(rule.id) || rule.id.length > 64 || !rule.channelIds.length
    || (!rule.senderUserIds.length && !rule.senderBotIds.length) || !validMatch
    || !LOCAL_ID_RE.test(rule.projectId) || !LOCAL_ID_RE.test(rule.columnId)
    || rule.dir.length > 1024 || /[\r\n\0]/.test(rule.dir)
    || !rule.cmd || rule.cmd.length > 200 || /[\r\n\0]/.test(rule.cmd)
    || !rule.template || rule.template.length > 120) return null;
  // This is a native-issued claim, never a frontend authorization decision.
  // Keep it intact across ordinary settings saves; native code revalidates it.
  if (raw.firstSend === true) rule.firstSend = true;
  if (raw.firstSendGrant && typeof raw.firstSendGrant === 'object' && !Array.isArray(raw.firstSendGrant)) {
    rule.firstSendGrant = structuredClone(raw.firstSendGrant);
  }
  return rule;
}

// Display-only semantic comparison. Native code owns the canonical grant
// digest and independently checks all sources. Sets are unordered; a template
// name is only a lookup key, and follow-up edits do not change the first step.
export function channelFirstSendScope(rule, project) {
  const template = project?.templates?.find(value => value.name === rule.template);
  const sorted = values => [...new Set(values || [])].sort();
  return JSON.stringify({ id: rule.id, projectId: rule.projectId, dir: rule.dir, cmd: rule.cmd,
    channelIds: sorted(rule.channelIds), senderUserIds: sorted(rule.senderUserIds), senderBotIds: sorted(rule.senderBotIds),
    match: { kind: rule.match.kind, value: rule.match.value || '', keywords: sorted(rule.match.keywords),
      caseSensitive: rule.match.caseSensitive === true, groupCapture: rule.match.groupCapture || '' },
    includeThreads: rule.includeThreads, idleMinutes: rule.idleMinutes,
    first: template ? normalizeTemplateStep(template.steps[0]) : null });
}

export function channelFirstSendRecipe(rule, project) {
  const template = project?.templates?.find(value => value.name === rule.template);
  const first = template && normalizeTemplateStep(template.steps[0]);
  // Do not sign steps[0] then send a later step after filtering empty text.
  if (!first || LEADING_MESSAGE.test(first)) return null;
  return first;
}

export async function channelFirstSendNeedsUpdate(rule, previous, project) {
  if (!previous?.firstSend || !previous.firstSendGrant
    || channelFirstSendScope(rule, project) !== channelFirstSendScope(previous, project)) return true;
  const recipe = channelFirstSendRecipe(rule, project);
  if (!recipe) return true;
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(recipe));
  return [...new Uint8Array(hash)].map(byte => byte.toString(16).padStart(2, '0')).join('') !== previous.firstSendGrant.stepHash;
}

export function normalizeChannelConfig(raw) {
  const source = raw && typeof raw === 'object' ? raw : {};
  const seen = new Set();
  const channelRules = [];
  for (const value of Array.isArray(source.channelRules) ? source.channelRules : []) {
    const rule = normalizeChannelRule(value);
    if (!rule || seen.has(rule.id)) continue;
    seen.add(rule.id); channelRules.push(rule);
    if (channelRules.length === CHANNEL_RULES_MAX) break;
  }
  return {
    channelConnection: { enabled: source.channelConnection?.enabled === true, connectionId: 'default' },
    channelRules,
  };
}

// Why a pending event could not be placed is said once per run. The event
// stays in the inbox and Settings shows the count, so repeating the sentence
// at every drain is noise. `tell(id, sentence, visible)` answers whether to
// show it now: never twice for one event, once per drain for one sentence
// however many events share it, and not at all while the page is hidden — a
// toast nobody could see does not count, the next drain says it. `drain`
// starts a pass and forgets events that left the inbox.
export function createPendingNotices() {
  const told = new Set();
  let said = new Set();
  return {
    drain(pendingIds) {
      said = new Set();
      for (const id of told) if (!pendingIds.has(id)) told.delete(id);
    },
    tell(id, sentence, visible) {
      if (!visible || told.has(id)) return false;
      told.add(id);
      if (said.has(sentence)) return false;
      said.add(sentence);
      return true;
    },
  };
}

export const channelRunExpired = (run, nowSecs) => !!run?.collecting
  && run.idleMinutes > 0 && run.lastCollectedAt + run.idleMinutes * 60 < nowSecs;

export const nextCollectedAt = (run, nowSecs) => Math.max(run?.lastCollectedAt || 0, nowSecs);

export function collectingCard(cards, item) {
  return (cards || []).find(card => card.projectId === item.target.projectId
    && card.channelRun?.collecting === true && card.channelRun.groupKey === item.groupKey);
}

// Application evidence outlives collection. Consult it before expiry/routing:
// a failed ACK must never turn a collected event into a new run's first event.
export function appliedChannelCard(cards, item) {
  return (cards || []).find(card => (card.origin?.source === 'channel'
    && card.origin.key === item.operationKey) || (card.buffer?.entries || []).some(entry => {
    const source = entry.source;
    return entry.kind === 'external' && source?.type === 'channel'
      && source.eventId === item.eventId && source.rule === item.ruleId
      && source.workspaceId === item.workspaceId && source.connection === item.connectionId
      && source.channel === item.channelId;
  }));
}

export const unfinishedChannelPlans = cards => (cards || [])
  .filter(card => card.channelRun && !card.channelRun.initialQueued);

export function channelTemplatePlan(item, project, nowSecs) {
  if (!channelAgentCommand(item?.target?.cmd)) return { error: 'command' };
  const template = (project?.templates || []).find(value => value.name === item.target.template);
  if (!template) return { error: 'template' };
  // A native staged grant freezes the head recipe before frontend draining.
  // Later template edits cannot rewrite that accepted event's first input.
  const skeleton = item.firstSendGrant ? normalizeTemplateStep(item.firstSendGrant.skeleton) : null;
  if (item.firstSendGrant && !skeleton) return { error: 'template' };
  const steps = skeleton === null ? template.steps : [skeleton, ...template.steps.slice(1)];
  if (steps.some(step => LEADING_MESSAGE.test(String(step)))) return { error: 'template-leading-message' };
  const msg = { text: item.body, from: item.senderUserId || item.senderBotId || '', where: item.channelId, link: '' };
  const texts = steps.map(step => fillInboundTemplate(step, msg)).filter(Boolean);
  if (!texts.length) return { error: 'template' };
  return {
    title: inboundTitle(item.body) || item.channelId,
    texts, template: template.name, at: nowSecs,
    ...(item.firstSendGrant ? { firstSend: { inboxId: item.id,
      grantId: item.firstSendGrant.id, grantDigest: item.firstSendGrant.digest,
      skeleton } } : {}),
  };
}

export function channelSource(item) {
  return {
    type: 'channel', eventId: item.eventId, connection: item.connectionId,
    channel: item.channelId, rule: item.ruleId, at: item.occurredAt, links: [],
    workspaceId: item.workspaceId, messageTs: item.messageTs,
    ...(item.threadTs ? { threadTs: item.threadTs } : {}),
    ...(item.senderUserId ? { senderUserId: item.senderUserId } : {}),
    ...(item.senderBotId ? { senderBotId: item.senderBotId } : {}),
  };
}
