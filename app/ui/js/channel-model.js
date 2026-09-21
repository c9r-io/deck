// DOM-free Slack channel monitor model. Native code performs the same closed
// validation before connecting; this side preserves only shapes it can edit.
//
// Admission is a separate, runtime policy (`channelBlockReason`): a rule's
// command must be exactly `claude` or `codex`, and every line of its
// template must begin with user-written text rather than a `{{msg.*}}`
// placeholder (a message starting with `!` or `/` must never become the
// first character the agent reads). Normalizing keeps a structurally valid
// rule that fails admission, so a rule saved by an older deck is shown as
// blocked and can be edited, never silently dropped on the next save.
import { fillInboundTemplate, inboundTitle } from './pure.js';

const LOCAL_ID = /^[A-Za-z0-9_-]{1,128}$/;
const CHANNEL_ID = /^[CG][A-Z0-9_-]{0,63}$/;
const USER_ID = /^[UW][A-Z0-9_-]{0,63}$/;
const BOT_ID = /^B[A-Z0-9_-]{0,63}$/;
export const CHANNEL_IDLE_DEFAULT = 30;
export const CHANNEL_IDLE_MAX = 7 * 24 * 60;

// Twin of inbound_channel::channel_agent_command: exactly `claude` or
// `codex`, no arguments, environment prefix, path or shell syntax.
export const channelAgentCommand = command =>
  command === 'claude' || command === 'codex' ? command : null;

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
  if (kind === 'keywords') matcher.keywords = unique(raw.match?.keywords, /^.{1,64}$/u, 32);
  else matcher.value = String(raw.match?.value || '').slice(0, 1024);
  if (kind === 'regex' && LOCAL_ID.test(String(raw.match?.groupCapture || ''))) {
    matcher.groupCapture = String(raw.match.groupCapture);
  }
  const rule = {
    id: String(raw.id || ''), enabled: raw.enabled !== false, connectionId: 'default',
    channelIds: unique(raw.channelIds, CHANNEL_ID, 64),
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
  if (!LOCAL_ID.test(rule.id) || rule.id.length > 64 || !rule.channelIds.length
    || (!rule.senderUserIds.length && !rule.senderBotIds.length) || !validMatch
    || !LOCAL_ID.test(rule.projectId) || !LOCAL_ID.test(rule.columnId)
    || rule.dir.length > 1024 || /[\r\n\0]/.test(rule.dir)
    || !rule.cmd || rule.cmd.length > 200 || /[\r\n\0]/.test(rule.cmd)
    || !rule.template || rule.template.length > 120) return null;
  return rule;
}

export function normalizeChannelConfig(raw) {
  const source = raw && typeof raw === 'object' ? raw : {};
  const seen = new Set();
  const channelRules = [];
  for (const value of Array.isArray(source.channelRules) ? source.channelRules : []) {
    const rule = normalizeChannelRule(value);
    if (!rule || seen.has(rule.id)) continue;
    seen.add(rule.id); channelRules.push(rule);
    if (channelRules.length === 64) break;
  }
  return {
    channelConnection: { enabled: source.channelConnection?.enabled === true, connectionId: 'default' },
    channelRules,
  };
}

export const channelRunExpired = (run, nowSecs) => !!run?.collecting
  && run.idleMinutes > 0 && run.lastCollectedAt + run.idleMinutes * 60 < nowSecs;

export const nextCollectedAt = (run, nowSecs) => Math.max(run?.lastCollectedAt || 0, nowSecs);

export function collectingCard(cards, item) {
  return (cards || []).find(card => card.projectId === item.target.projectId
    && card.channelRun?.collecting === true && card.channelRun.groupKey === item.groupKey);
}

export const unfinishedChannelPlans = cards => (cards || [])
  .filter(card => card.channelRun && !card.channelRun.initialQueued);

export function channelTemplatePlan(item, project, nowSecs) {
  if (!channelAgentCommand(item?.target?.cmd)) return { error: 'command' };
  const template = (project?.templates || []).find(value => value.name === item.target.template);
  if (!template) return { error: 'template' };
  if (template.steps.some(step => LEADING_MESSAGE.test(String(step)))) return { error: 'template-leading-message' };
  const msg = { text: item.body, from: item.senderUserId || item.senderBotId || '', where: item.channelId, link: '' };
  const texts = template.steps.map(step => fillInboundTemplate(step, msg)).filter(Boolean);
  if (!texts.length) return { error: 'template' };
  return {
    title: inboundTitle(item.body) || item.channelId,
    texts, template: template.name, at: nowSecs,
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
