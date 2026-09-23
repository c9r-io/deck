// buffer-model.js — pure card scratchpad schema, bounds and delivery evidence.
// A buffer belongs to one persisted card. Queueing stores an immutable copy;
// later source edits never alter text already handed to the scheduler. An
// external message whose first visible character is `!`, `/` or `#` never
// queues as-is (`leadingCommand`), on the desktop or through the Connector.

export const BUFFER_MAX_ENTRIES = 256;
export const BUFFER_MAX_COPIES = 256;
export const BUFFER_MAX_ENTRY_BYTES = 32 * 1024;
export const BUFFER_MAX_BYTES = 1024 * 1024;
export const BUFFER_MAX_SERIALIZED_BYTES = 2 * 1024 * 1024;

const bytes = text => new TextEncoder().encode(String(text || '')).byteLength;
export const emptyBuffer = () => ({ revision: 0, collecting: false, entries: [] });
export const retainedBuffer = card => !!(card?.buffer?.collecting || card?.buffer?.entries?.length);

export function bufferBytes(buffer) {
  return (buffer?.entries || []).reduce((total, entry) => total + bytes(entry.text)
    + (entry.copies || []).reduce((n, copy) => n + bytes(copy.text), 0), 0);
}

export function bufferLimitError(buffer) {
  const entries = buffer?.entries || [];
  if (entries.length > BUFFER_MAX_ENTRIES) return 'entries';
  if (entries.reduce((n, entry) => n + (entry.copies || []).length, 0) > BUFFER_MAX_COPIES) return 'copies';
  if (entries.some(entry => bytes(entry.text) > BUFFER_MAX_ENTRY_BYTES)) return 'entry';
  if (entries.some(entry => (entry.copies || []).some(copy => bytes(copy.text) > BUFFER_MAX_ENTRY_BYTES))) return 'entry';
  if (bufferBytes(buffer) > BUFFER_MAX_BYTES) return 'total';
  if (bytes(JSON.stringify(buffer || emptyBuffer())) > BUFFER_MAX_SERIALIZED_BYTES) return 'total';
  return null;
}

export function addManual(buffer, { id, text, now }) {
  const next = structuredClone(buffer || emptyBuffer());
  next.entries.push({ id, kind: 'manual', text, revision: 1, createdAt: now, updatedAt: now, copies: [] });
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, error: bufferLimitError(next) };
}

/** Add or revise one external event without changing its stable entry id. */
export function upsertExternal(buffer, { id, text, source, now }) {
  const next = structuredClone(buffer || emptyBuffer());
  const existing = next.entries.find(entry => entry.kind === 'external'
    && entry.source?.type === source.type && entry.source?.eventId === source.eventId);
  if (existing) {
    if (existing.text === text && JSON.stringify(existing.source) === JSON.stringify(source)) {
      return { buffer: next, entry: existing, noop: true };
    }
    return { buffer: next, entry: existing, error: 'immutable' };
  }
  const entry = {
    id, kind: 'external', text, revision: 1, createdAt: now, updatedAt: now,
    source: structuredClone(source), copies: [],
  };
  next.entries.push(entry);
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, entry, error: bufferLimitError(next) };
}

export function editEntry(buffer, id, text, now) {
  const next = structuredClone(buffer || emptyBuffer());
  const entry = next.entries.find(item => item.id === id);
  if (!entry) return { buffer: next, error: 'missing' };
  if (entry.kind !== 'manual') return { buffer: next, error: 'immutable' };
  entry.text = text;
  entry.revision = (entry.revision || 0) + 1;
  entry.updatedAt = now;
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, error: bufferLimitError(next) };
}

export function deleteEntry(buffer, id) {
  const next = structuredClone(buffer || emptyBuffer());
  const before = next.entries.length;
  next.entries = next.entries.filter(item => item.id !== id);
  if (next.entries.length !== before) next.revision = (next.revision || 0) + 1;
  return next;
}

// An external (Slack) message is untrusted agent input: like a channel
// template, it may never make `!` (shell mode), `/` (slash command) or `#`
// (Claude Code memory shortcut) the first character the agent reads, not
// even behind whitespace or invisible format characters. The native
// `scheduler::ops::leading_command` is the authority (a row queued with
// `externalText` is refused there); this is its UI twin over the same set:
// Unicode whitespace (`\s` plus U+0085) and format characters (`\p{Cf}`).
// `@` stays allowed: Slack sends a mention as `<@U…>`, and an `@path`
// reference works anywhere in a prompt, so a leading-only refusal would
// close nothing. The user adopts such text by copying it into a manual
// note, which is theirs to queue.
const LEADING_COMMAND = /^[\s\u0085\p{Cf}]*[!/#]/u;
export const leadingCommand = entry => entry?.kind === 'external' && LEADING_COMMAND.test(String(entry.text || ''));

export function addQueueCopy(buffer, id, operationId, now) {
  const next = structuredClone(buffer || emptyBuffer());
  const entry = next.entries.find(item => item.id === id);
  if (!entry) return { buffer: next, error: 'missing' };
  entry.copies ||= [];
  const prior = entry.copies.find(copy => copy.operationId === operationId);
  if (prior) return { buffer: next, copy: prior };
  if (leadingCommand(entry)) return { buffer: next, error: 'leading-command' };
  const copy = {
    operationId, entryRevision: entry.revision || 0, text: entry.text,
    createdAt: now, state: 'uncertain',
  };
  entry.copies.push(copy);
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, copy, error: bufferLimitError(next) };
}

export function copyEvidence(copy, queue) {
  const operation = (queue?.operations || []).find(row => row.id === copy.operationId);
  if (operation && ['delivered', 'canceled'].includes(operation.state)) return operation.state;
  const item = (queue?.items || []).find(row => row.operation_id === copy.operationId);
  if (item) return item.state === 'ambiguous' ? 'uncertain' : 'queued';
  if (operation) return operation.state;
  if ((queue?.deliveries || []).some(row => row.operation_id === copy.operationId)) return 'delivered';
  return copy.state === 'canceled' ? 'canceled' : 'uncertain';
}
