// quota-model.js — pure helpers for the Quota area and a card's quota chip
// (quota.js). The backend (quota.rs) sends each agent account once with its
// usage windows; this file only orders, labels and grades them. It never
// decides anything: quota is presentation only, never a hold, a release,
// an alert or a card colour.
// Part of deck's no-build frontend: native ES modules, no bundler.

/* the percent at which a window turns to the warning and the danger colour */
export const WARN_AT = 80;
export const DANGER_AT = 95;

/* the window still counting (not past its reset) that is most used */
export function tightest(account) {
  let best = null;
  for (const w of account?.windows || []) {
    if (w.expired) continue;
    if (!best || w.usedPercent > best.usedPercent) best = w;
  }
  return best;
}

export function level(percent) {
  if (percent >= DANGER_AT) return 'danger';
  if (percent >= WARN_AT) return 'warn';
  return 'ok';
}

/* "5h", "7d", "1d", "90m" from the window's length; its name otherwise */
export function windowShort(w) {
  const m = w?.minutes;
  if (!m) return String(w?.name || '');
  if (m % 1440 === 0) return `${m / 1440}d`;
  if (m % 60 === 0) return `${m / 60}h`;
  return `${m}m`;
}

const AGENT_NAMES = Object.freeze({ claude: 'Claude', codex: 'Codex', agy: 'agy' });

/* "Claude · .claude-work": the agent and its folders (never an email) */
export function accountLabel(account) {
  const agent = AGENT_NAMES[account?.kind] || String(account?.kind || '');
  const folders = (account?.folders || []).map(f => f.name).filter(Boolean);
  return folders.length ? `${agent} · ${folders.join(', ')}` : agent;
}

/* "in 2 h 14 min", "in 3 d" — or the past-reset word */
export function resetIn(resetsAt, now, t) {
  const ms = resetsAt - now;
  if (ms <= 0) return t('quota.resetDone');
  const min = Math.ceil(ms / 60000);
  if (min < 60) return t('quota.resetMinutes', { m: min });
  const h = Math.floor(min / 60);
  if (h < 48) return t('quota.resetHours', { h, m: min % 60 });
  return t('quota.resetDays', { d: Math.floor(h / 24), h: h % 24 });
}

/* HH:MM of an epoch-ms instant, in the local zone */
export function clock(ms) {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/* the account a card's session is linked to, or null */
export function accountForSession(snapshot, session) {
  if (!session) return null;
  return (snapshot?.accounts || []).find(a => (a.sessions || []).includes(session)) || null;
}

/* the most used window of all accounts (the collapsed sidebar's ring) */
export function tightestOverall(snapshot) {
  let best = null;
  for (const a of snapshot?.accounts || []) {
    const w = tightest(a);
    if (w && (!best || w.usedPercent > best.usedPercent)) best = w;
  }
  return best;
}

/* one card chip: "Claude .claude-work 62%" with its level, or null */
export function chipFor(snapshot, session) {
  const account = accountForSession(snapshot, session);
  const w = account && tightest(account);
  if (!w) return null;
  return {
    account: account.id,
    text: `${accountLabel(account).replace(' · ', ' ')} ${Math.round(w.usedPercent)}%`,
    level: level(w.usedPercent),
    stale: !!account.stale,
  };
}
