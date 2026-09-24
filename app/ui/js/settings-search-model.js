// settings-search-model.js — the DOM-free half of Settings navigation and
// search: the ONE list of sections (in sidebar order) and of searchable
// settings, each with a stable id that the markup carries as
// `id="set-item-<id>"` / `data-setting-id`. Identity never depends on DOM
// order or on visible text.
//
// # Search contract
// A query is trimmed, lower-cased and split on whitespace; every term must
// be a substring of an item's searchable text (AND, no scoring, results in
// sidebar order). An item's searchable text is its title and control
// labels in EVERY dictionary (so either language finds it whatever the UI
// locale) plus a short, bounded bilingual keyword list. Hints and "Learn
// more" text are never indexed, so a sentence inside a collapsed details
// block cannot surface a group. Section titles are a fallback tier, per
// section: where some item matches on its own text, only those items show;
// where none does, the section title counts as part of each item's text. So
// "通知" shows the notification group and not the agent-status group in the
// same "Agent 与通知" section, while "terminal" shows Terminal & sessions
// (whose items never say "terminal") beside MCP terminal control, and
// "terminal voice" narrows to Voice input.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const SETTINGS_SECTIONS = Object.freeze([
  { id: 'general', titleKey: 'settings.general' },
  { id: 'shortcuts', titleKey: 'settings.shortcuts' },
  { id: 'terminal', titleKey: 'settings.terminal' },
  { id: 'agents', titleKey: 'settings.agents' },
  { id: 'integrations', titleKey: 'settings.integrations' },
  { id: 'remote', titleKey: 'settings.remote' },
  { id: 'data', titleKey: 'settings.data' },
  { id: 'about', titleKey: 'settings.about' },
].map(Object.freeze));

const item = (id, section, labelKeys, keywords) =>
  Object.freeze({ id, section, labelKeys: Object.freeze(labelKeys), keywords: Object.freeze(keywords) });

export const SETTING_ITEMS = Object.freeze([
  item('language', 'general', ['settings.language'],
    ['locale', 'english', 'chinese', '语言', '中文', '英文']),
  item('appearance', 'general', ['settings.group.appearance', 'settings.theme', 'settings.accent', 'settings.fontSize'],
    ['dark', 'light', 'color', 'colour', 'contrast', 'text size', 'zoom', '深色', '浅色', '颜色', '字号', '缩放']),
  item('shortcuts', 'shortcuts', ['settings.shortcuts', 'settings.shortcut.newSession', 'settings.shortcut.toggleSidebar',
    'settings.shortcut.splitRight', 'settings.shortcut.splitDown'],
  ['keyboard', 'hotkey', 'key binding', 'keybinding', '键盘', '热键']),
  item('voice', 'terminal', ['settings.voice', 'settings.voiceDefault'],
    ['speech', 'dictation', 'microphone', 'mic', '听写', '麦克风', '识别']),
  item('editor', 'terminal', ['settings.openFiles'],
    ['editor', 'ide', 'vs code', 'vscode', 'cursor', 'zed', '编辑器']),
  item('session-restore', 'terminal', ['settings.shellRecovery'],
    ['restore', 'resume', 'snapshot', '恢复', '快照']),
  item('shell-service', 'terminal', ['tmux.service', 'tmux.restart'],
    ['tmux', 'server', '服务', '重启']),
  item('agent-status', 'agents', ['settings.group.agentStatus', 'settings.agentHooks', 'settings.codexHooks'],
    ['hook', 'hooks', 'needs input', 'turn finished', '钩子', '等待输入']),
  item('away-notifications', 'agents', ['settings.group.awayNotifications', 'settings.notifyAway', 'settings.notifySound'],
    ['notification', 'notifications', 'notify', 'alert', 'badge', 'dock', 'sound', '通知', '提醒', '徽标', '提示', '声音']),
  item('slack-reactions', 'integrations', ['settings.slack', 'settings.inboundUserToken', 'settings.inboundAppToken'],
    ['slack', 'reaction', 'emoji', 'badge', 'token', 'keychain', '标记', '表情', '令牌']),
  item('slack-channel', 'integrations', ['settings.channelMonitor', 'settings.channelBotToken', 'settings.channelAppToken'],
    ['slack', 'channel', 'monitor', 'bot', 'token', 'keychain', '频道', '监控', '令牌']),
  item('connector', 'remote', ['connector.title', 'connector.address', 'connector.pair'],
    ['phone', 'iphone', 'mobile', 'pair', 'pairing', 'qr', 'device', 'remote', '手机', '配对', '设备', '二维码', '远程']),
  item('mcp', 'remote', ['mcp.title', 'mcp.add', 'mcp.outputRetention'],
    ['mcp', 'chatgpt', 'codex', 'client', 'authorize', 'authorization', 'tunnel', 'remote', 'terminal control',
      '远程', '控制', '授权', '客户端']),
  item('history', 'data', ['settings.history'],
    ['command history', 'completion', '历史', '补全']),
  item('shell-data', 'data', ['settings.shellData'],
    ['snapshot', 'recovery', '快照', '恢复']),
  item('logs', 'data', ['settings.logs', 'settings.exportLogs', 'settings.resetLogs'],
    ['log', 'logs', 'app.log', 'diagnostic', 'diagnostics', '日志', '诊断']),
  item('updates', 'about', ['settings.updateChannel', 'settings.updates', 'settings.checkUpdates'],
    ['update', 'version', 'nightly', 'stable', 'release', '更新', '版本', '通道']),
]);

const ITEMS_BY_ID = new Map(SETTING_ITEMS.map(entry => [entry.id, entry]));

export const isSettingsSection = id => SETTINGS_SECTIONS.some(section => section.id === id);

/** The item with this id, or null — never a guess. */
export const settingItem = id => (typeof id === 'string' && ITEMS_BY_ID.get(id)) || null;

export const sectionItems = section => SETTING_ITEMS.filter(entry => entry.section === section);

export function normalizeSearchQuery(query) {
  return String(query ?? '').trim().replace(/\s+/g, ' ').toLocaleLowerCase();
}

const translations = (dictionaries, key) => Object.values(dictionaries || {})
  .map(dictionary => dictionary?.[key]).filter(text => typeof text === 'string');

/** null for an empty query (normal navigation); otherwise the matches as
 *  [{ section, items: [id…] }] in sidebar order, possibly empty. */
export function searchSettings(query, dictionaries) {
  const terms = normalizeSearchQuery(query).split(' ').filter(Boolean);
  if (!terms.length) return null;
  const own = entry => [...entry.labelKeys.flatMap(key => translations(dictionaries, key)), ...entry.keywords]
    .join('\n').toLocaleLowerCase();
  const matches = hay => terms.every(term => hay.includes(term));
  return SETTINGS_SECTIONS.map(section => {
    const entries = sectionItems(section.id);
    let found = entries.filter(entry => matches(own(entry)));
    if (!found.length) {
      const title = translations(dictionaries, section.titleKey).join('\n').toLocaleLowerCase();
      found = entries.filter(entry => matches(title + '\n' + own(entry)));
    }
    return { section: section.id, items: found.map(entry => entry.id) };
  }).filter(result => result.items.length);
}
