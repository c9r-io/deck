// buffer-model.js — pure card scratchpad schema, bounds and delivery evidence.
// A buffer belongs to one persisted card. Queueing stores an immutable copy;
// later source edits never alter text already handed to the scheduler. An
// external message whose first visible character is `!`, `/` or `#` never
// queues as-is (`leadingCommand`), on the desktop or through the Connector.
//
// Three kinds of entry, each with its author shown: `manual` (the user's,
// editable), `external` (a Slack channel message, read-only) and `agent`
// (approved 2026-11-23): a note an MCP client left from the card it runs in
// for the user to judge (`mcp/notes.rs`, written through `putAgentNote` in
// the webview's Board transaction). An Agent note has a one-line title, a
// text, `acted` (the Agent already went ahead with its recommendation), its
// writer's key and client; the user never edits or queues it — they mark it
// handled (`handledAt`) or answer it with a manual note of their own. Open
// notes (not handled) are what the card's badge and Needs attention count.

export const BUFFER_MAX_ENTRIES = 256;
export const BUFFER_MAX_COPIES = 256;
export const BUFFER_MAX_ENTRY_BYTES = 32 * 1024;
export const BUFFER_MAX_BYTES = 1024 * 1024;
export const BUFFER_MAX_SERIALIZED_BYTES = 2 * 1024 * 1024;
/* an Agent note (documents.rs AGENT_NOTE_*, limits.json agent_note) */
export const AGENT_TITLE_MAX = 120;
export const AGENT_TEXT_MAX_BYTES = 8 * 1024;
export const AGENT_OPEN_MAX = 32;

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

export const isAgentNote = entry => entry?.kind === 'agent';
export const openAgentNote = entry => isAgentNote(entry) && !entry.handledAt;
/** The open Agent notes of a card, in scratchpad order. */
export const openAgentNotes = card => (card?.buffer?.entries || []).filter(openAgentNote);

const sameNote = (entry, plan) => entry.title === plan.title && entry.text === plan.text
  && !!entry.acted === !!plan.acted;

/** Write, revise or withdraw one Agent note as `mcp/notes.rs` planned it
 *  ({ entryId, key, clientId, client, title, text, acted, withdraw }). A
 *  note the user handled is closed to its writer; a new note needs room. */
export function putAgentNote(buffer, plan, now) {
  const next = structuredClone(buffer || emptyBuffer());
  const index = next.entries.findIndex(entry => isAgentNote(entry)
    && entry.source?.clientId === plan.clientId && entry.source?.eventId === plan.key);
  const existing = index >= 0 ? next.entries[index] : null;
  if (existing?.handledAt) return { buffer: next, error: 'note-handled' };
  if (plan.withdraw) {
    if (!existing) return { buffer: next, noop: true };
    next.entries.splice(index, 1);
  } else if (existing) {
    if (sameNote(existing, plan)) return { buffer: next, noop: true };
    Object.assign(existing, { title: plan.title, text: plan.text, acted: !!plan.acted,
      revision: (existing.revision || 0) + 1, updatedAt: now });
  } else {
    if (next.entries.filter(openAgentNote).length >= AGENT_OPEN_MAX) return { buffer: next, error: 'note-capacity' };
    if (next.entries.some(entry => entry.id === plan.entryId)) return { buffer: next, error: 'note-conflict' };
    next.entries.push({
      id: plan.entryId, kind: 'agent', title: plan.title, text: plan.text, acted: !!plan.acted,
      revision: 1, createdAt: now, updatedAt: now, copies: [],
      source: { type: 'agent', eventId: plan.key, clientId: plan.clientId, client: plan.client },
    });
  }
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, error: bufferLimitError(next) ? 'note-capacity' : null };
}

/** The user handled one Agent note: it stays, folded, and leaves the count. */
export function markHandled(buffer, id, now) {
  const next = structuredClone(buffer || emptyBuffer());
  const entry = next.entries.find(item => item.id === id);
  if (!isAgentNote(entry)) return { buffer: next, error: 'missing' };
  if (entry.handledAt) return { buffer: next, noop: true };
  entry.handledAt = now;
  next.revision = (next.revision || 0) + 1;
  return { buffer: next, error: null };
}

/** Whether `proposed` changes an Agent note the user may not change: only
 *  `handledAt` may be set, and only once. */
export function agentNoteChanged(entry, proposed) {
  if (!isAgentNote(entry)) return false;
  if (!isAgentNote(proposed)) return true;
  if (entry.handledAt && proposed.handledAt !== entry.handledAt) return true;
  const without = value => JSON.stringify(Object.fromEntries(Object.entries(value).filter(([key]) => key !== 'handledAt')));
  return without(entry) !== without(proposed);
}

export function addQueueCopy(buffer, id, operationId, now) {
  const next = structuredClone(buffer || emptyBuffer());
  const entry = next.entries.find(item => item.id === id);
  if (!entry) return { buffer: next, error: 'missing' };
  if (isAgentNote(entry)) return { buffer: next, error: 'agent-note' };
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
