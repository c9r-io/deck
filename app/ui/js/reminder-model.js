// Card-local attention intent. UTC instants never change with the host zone.
// Local calendar input enumerates offsets: gaps fail, folds require a choice.
// Viewing and agent observations cannot handle a reminder or authorize execution.
export const REMINDER_NOTE_BYTES = 280;
export const reminderDue = (card, now = Date.now()) => !!card?.reminder &&
  (card.reminder.due === true || card.reminder.dueAt <= now);
// The webview's periodic reconcile. A Board without a reminder has nothing
// for it: no due latch to set, no request to keep registered. It then makes
// no IPC at all; the boot, focus and visibility runs stay unconditional.
export const reminderTick = (cards, reconcile) => () => { if (cards().some(card => card.reminder)) reconcile(); };
export const reminderClaim = card => card?.reminder
  ? { cardId: card.id, id: card.reminder.id, revision: card.reminder.revision } : null;
export const sameReminder = (reminder, claim) => !!reminder && !!claim &&
  reminder.id === claim.id && reminder.revision === claim.revision;
export const noteValid = note => typeof note === 'string' && !/[\r\n\0]/.test(note)
  && new TextEncoder().encode(note).length <= REMINDER_NOTE_BYTES;

export function localParts(instant, timeZone) {
  const parts = new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit',
    day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(instant);
  const p = Object.fromEntries(parts.map(part => [part.type, part.value]));
  return `${p.year}-${p.month}-${p.day}T${p.hour}:${p.minute}`;
}
export function localCandidates(local, timeZone) {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(local)) return [];
  const utc = Date.parse(`${local}:00Z`);
  if (!Number.isFinite(utc) || new Date(utc).toISOString().slice(0, 16) !== local) return [];
  try {
    const offsets = new Set();
    // Sample both sides of transitions, then verify exact round trips.
    for (let h = -48; h <= 48; h += 6) {
      const sample = utc + h * 3600000;
      offsets.add(Date.parse(`${localParts(sample, timeZone)}:00Z`) - sample);
    }
    return [...offsets].map(offset => utc - offset).filter(value => localParts(value, timeZone) === local).sort((a, b) => a - b);
  } catch (_) { return []; }
}
export function shortcutTime(kind, now, timeZone) {
  if (kind === 'hour') return localParts(now + 3600000, timeZone);
  const local = localParts(now, timeZone);
  const date = new Date(`${local.slice(0, 10)}T09:00:00Z`);
  const days = kind === 'tomorrow' ? 1 : 8 - (date.getUTCDay() || 7);
  date.setUTCDate(date.getUTCDate() + days);
  return date.toISOString().slice(0, 16);
}
export function reminderAction(card, action, now) {
  if (!sameReminder(card?.reminder, action) || card.id !== action.cardId) return null;
  if (action.kind === 'open') return { open: card.id };
  if (action.kind !== 'snooze' || !Number.isSafeInteger(now)) return null;
  return { reminder: { ...card.reminder, revision: card.reminder.revision + 1, dueAt: now + 3600000, due: false } };
}
export const retirementKey = (kind, card, lifecycle) => kind === 'run' && card?.origin
  ? `run:${JSON.stringify([card.origin.source, card.origin.key, card.origin.badge])}`
  : kind === 'exit' && lifecycle ? `exit:${lifecycle}` : null;
export const retirementBlocked = (card, key) => !!key && (card?.reminderRetirements || []).includes(key);
export function rememberRetirement(card, key) {
  if (!key || retirementBlocked(card, key)) return;
  card.reminderRetirements = [...(card.reminderRetirements || []).filter(value => !value.startsWith(key.split(':')[0] + ':')), key];
}

export function reminderRequestId(card) {
  const hex = [...new TextEncoder().encode(card.id)].map(byte => byte.toString(16).padStart(2, '0')).join('');
  return `deck.reminder.${hex}.${card.reminder.id}.${card.reminder.revision}`;
}
