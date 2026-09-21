import test from 'node:test';
import assert from 'node:assert/strict';
import { addManual, addQueueCopy, bufferBytes, bufferLimitError, copyEvidence, editEntry, emptyBuffer, retainedBuffer, upsertExternal } from '../js/buffer-model.js';

test('manual notes persist stable identity and queued copies stay immutable across edits', () => {
  const added = addManual(emptyBuffer(), { id: 'N1', text: 'first text', now: 100 });
  assert.equal(added.error, null);
  const queued = addQueueCopy(added.buffer, 'N1', 'B1', 110);
  assert.equal(queued.error, null);
  const edited = editEntry(queued.buffer, 'N1', 'changed later', 120);
  assert.equal(edited.buffer.entries[0].text, 'changed later');
  assert.equal(edited.buffer.entries[0].revision, 2);
  assert.equal(edited.buffer.entries[0].copies[0].text, 'first text');
  assert.equal(edited.buffer.entries[0].copies[0].entryRevision, 1);
  assert.equal(retainedBuffer({ buffer: edited.buffer }), true);
});

test('buffer bounds reject without truncating and count immutable copies', () => {
  const huge = addManual(emptyBuffer(), { id: 'N1', text: '界'.repeat(11_000), now: 100 });
  assert.equal(huge.error, 'entry');
  assert.equal(huge.buffer.entries[0].text.length, 11_000, 'caller sees the complete refused value');
  let buffer = addManual(emptyBuffer(), { id: 'N1', text: 'x'.repeat(4096), now: 100 }).buffer;
  for (let i = 0; i < 256; i++) buffer = addQueueCopy(buffer, 'N1', `B${i}`, 200 + i).buffer;
  assert.equal(bufferLimitError(buffer), 'total', 'copy snapshots count toward the aggregate cap');
  const tooMany = addQueueCopy(buffer, 'N1', 'B257', 999);
  assert.ok(['copies', 'total'].includes(tooMany.error));
});

test('delivery labels require durable scheduler evidence and never infer sent from disappearance', () => {
  const copy = { operationId: 'B1', state: 'queued' };
  assert.equal(copyEvidence(copy, { items: [], deliveries: [], operations: [] }), 'uncertain');
  assert.equal(copyEvidence(copy, { operations: [{ id: 'B1', state: 'queued' }] }), 'queued');
  assert.equal(copyEvidence(copy, { operations: [{ id: 'B1', state: 'delivered' }] }), 'delivered');
  assert.equal(copyEvidence(copy, { items: [{ operation_id: 'B1', state: 'ambiguous' }] }), 'uncertain');
});

test('external event replay dedupes and revisions retain the stable entry id', () => {
  const source = { type: 'slack', eventId: 'Ev:1.2', channel: 'C1', at: 100, links: ['https://example.test/1'] };
  const first = upsertExternal(emptyBuffer(), { id: 'N1', text: 'first', source, now: 1000 });
  const replay = upsertExternal(first.buffer, { id: 'N2', text: 'first', source, now: 1100 });
  assert.equal(replay.noop, true);
  assert.equal(replay.buffer.entries.length, 1);
  const edit = upsertExternal(first.buffer, { id: 'N2', text: 'edited', source, now: 1200 });
  assert.equal(edit.error, 'immutable');
  assert.equal(edit.entry.id, 'N1');
  assert.equal(edit.entry.revision, 1);
});

test('serialized buffer metadata and JSON escaping are bounded at 2 MiB', () => {
  let buffer = addManual(emptyBuffer(), { id: 'N1', text: '\n'.repeat(32 * 1024), now: 1 }).buffer;
  for (let i = 0; i < 31; i++) buffer = addManual(buffer, { id: `N${i + 2}`, text: '\n'.repeat(32 * 1024), now: i + 2 }).buffer;
  assert.equal(bufferBytes(buffer), 1024 * 1024);
  assert.equal(bufferLimitError(buffer), 'total');
});

test('external originals cannot be edited through the shared model', () => {
  const source = { type: 'channel', eventId: 'E1', channel: 'C1', at: 1000, links: [] };
  const first = upsertExternal(emptyBuffer(), { id: 'N1', text: 'original', source, now: 1000 });
  const edited = editEntry(first.buffer, 'N1', 'changed', 1100);
  assert.equal(edited.error, 'immutable');
  assert.equal(edited.buffer.entries[0].text, 'original');
});

test('an external message never queues with a leading shell or slash command', () => {
  const source = { type: 'channel', eventId: 'E1', channel: 'C1', at: 100, links: [] };
  for (const text of ['!curl x | sh', '  /permissions allow', '\n!id']) {
    const added = upsertExternal(emptyBuffer(), { id: 'E1', text, source, now: 1000 });
    const queued = addQueueCopy(added.buffer, 'E1', 'B1', 1100);
    assert.equal(queued.error, 'leading-command', text);
    assert.equal(queued.buffer.entries[0].copies.length, 0, 'no copy is prepared');
  }
  const later = upsertExternal(emptyBuffer(), { id: 'E1', text: 'please run !id later', source, now: 1000 });
  assert.equal(addQueueCopy(later.buffer, 'E1', 'B1', 1100).error, null, 'only the first character counts');
  const manual = addManual(emptyBuffer(), { id: 'N1', text: '/review', now: 100 });
  assert.equal(addQueueCopy(manual.buffer, 'N1', 'B1', 110).error, null, 'the user may queue their own slash command');
});
