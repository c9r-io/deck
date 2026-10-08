// A phone task preset's approval through the project defaults dialog while
// its digest is still being computed. The production dialog and the
// production digest run over the test document; the ONLY scripted thing is
// the moment a SHA-256 completes (or that it fails). Every assertion is on
// what the dialog returns to be saved and on whether that approval is valid
// for the returned preset, never on the tick box alone.
import test, { mock } from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };
const { projectDefaultsDialog } = await import('../js/dialogs.js');
const { presetApproved, withPresetApproval } = await import('../js/connector-model.js');
const { t } = await import('../js/i18n.js');

const el = id => fakeDocument.getElementById(id);
const turn = async (times = 4) => { for (let i = 0; i < times; i += 1) await new Promise(resolve => setImmediate(resolve)); };
/* hashes complete when the test lets them: `hold` keeps every hash asked
   for from now on waiting; `failing` makes them reject */
let hold = null; let hashing = 0; let failing = false;
const digest = crypto.subtle.digest.bind(crypto.subtle);
crypto.subtle.digest = async (...args) => {
  hashing += 1;
  try {
    if (hold) await hold.promise;
    if (failing) throw new Error('hash');
    return await digest(...args);
  } finally { hashing -= 1; }
};
const holdHashes = () => { let resolve; const promise = new Promise(done => { resolve = done; }); hold = { promise, resolve }; };
const releaseHashes = async () => { const held = hold; hold = null; held?.resolve(); do { await turn(); } while (hashing); await turn(); };
const failures = [];
process.on('unhandledRejection', error => failures.push(error));

/* a save that never returns is a failed test, not a hung run */
const LIMIT = { timeout: 10_000 };
const COLUMNS = [{ id: 'C1', name: 'Working' }];
const M0 = { id: 'R1', name: 'Fix issue', columnId: 'C1', title: 'Remote fix', dir: '~/work', cmd: 'claude', steps: ['inspect', 'fix'] };
const OTHER = { id: 'R2', name: 'Other', columnId: 'C1', title: 'Other', dir: '~/work', cmd: 'claude', steps: ['look'] };
const approved = await withPresetApproval('P1', M0, true);
const open = presets => projectDefaultsDialog({ projectId: 'P1', name: 'Atlas', dir: '~/work', cmd: 'codex', recent: [], presets, columns: COLUMNS });
const openPreset = index => el('pdf-presets').children[index].fire('click');
const box = () => el('pdf-preset-auto-send');
/* what a user does: a field edit fires input, a click on the box changes it */
const type = (id, value) => { el(id).value = value; el(id).oninput?.(); };
const click = () => { const input = box(); input.checked = !input.checked; input.indeterminate = false; input.fire('change'); };
const save = () => { el('pdf-preset-done').fire('click'); el('pdf-yes').fire('click'); };
const pending = async promise => await Promise.race([promise.then(() => false), turn().then(() => true)]);
const only = (result, id = 'R1') => result.presets.find(preset => preset.id === id);
/* each test starts clean, whatever the one before left waiting or open */
const reset = async () => {
  mock.timers.reset(); failing = false; await releaseHashes();
  el('pdf-no').fire('click'); await turn();
  el('toasts').replaceChildren();
};

test('control (C): a finished check and an ordinary save keep the stored approval, digest unchanged', LIMIT, async () => {
  await reset();
  const dialog = open([approved]); openPreset(0); await releaseHashes();
  assert.equal(box().checked, true);
  type('pdf-preset-name', 'Renamed');
  save(); await releaseHashes();
  const saved = only(await dialog);
  assert.equal(saved.autoSend.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', saved), true);
});

test('race A: steps edited while the stored approval is being checked are never approved by its late answer', LIMIT, async () => {
  for (const edit of [() => type('pdf-preset-steps', 'inspect\nfix it'), () => { el('pdf-preset-steps').value = 'inspect\nfix it'; }]) {
    await reset(); holdHashes();
    const dialog = open([approved]); openPreset(0);
    edit();
    await releaseHashes();          // the check of M0 answers "valid" now
    save(); await releaseHashes();
    const saved = only(await dialog);
    assert.deepEqual(saved.steps, ['inspect', 'fix it']);
    assert.equal(await presetApproved('P1', saved), false, 'no valid approval for the edited steps');
    assert.equal('autoSend' in saved, false);
  }
});

test('race B: saving unchanged before the check finishes keeps the stored approval, in one save', LIMIT, async () => {
  await reset(); holdHashes();
  const dialog = open([approved]); openPreset(0);
  save();                           // the check has not answered
  await releaseHashes();
  const saved = only(await dialog);
  assert.equal(saved.autoSend?.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', saved), true);
});

test('a check that fails is neither a withdrawal nor an approval: nothing is saved until it can be told', LIMIT, async () => {
  await reset(); failing = true;
  const dialog = open([approved]); openPreset(0); await releaseHashes();
  assert.equal(box().checked, false, 'not shown as approved');
  el('pdf-yes').fire('click'); await releaseHashes();
  assert.equal(await pending(dialog), true, 'not saved without the approval');
  assert.equal(el('toasts').children.length, 1, 'and says so');
  // the same save, once the digest can be computed, keeps the stored approval
  failing = false;
  el('pdf-yes').fire('click'); await releaseHashes();
  const saved = only(await dialog);
  assert.equal(saved.autoSend?.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', saved), true);
});

test('a check that never answers is bounded: the save ends unsaved, the approval untouched', LIMIT, async () => {
  await reset(); holdHashes();
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const dialog = open([approved]); openPreset(0);
    save(); await turn();
    mock.timers.tick(60_000); await turn();
    assert.equal(await pending(dialog), true, 'not saved');
    assert.equal(el('toasts').children.length, 1);
    el('pdf-no').fire('click');
    assert.equal(await dialog, null);
  } finally { mock.timers.reset(); await releaseHashes(); }
});

test('the user’s own choice during the check is what is saved, whichever way the check answers', LIMIT, async () => {
  // explicit off: tick, then untick, while the check is under way
  await reset(); holdHashes();
  let dialog = open([approved]); openPreset(0);
  click(); click();
  assert.equal(box().checked, false);
  await releaseHashes();
  assert.equal(box().checked, false, 'the late answer does not tick it again');
  save(); await releaseHashes();
  assert.equal('autoSend' in only(await dialog), false);
  // explicit on, for edited steps: a new approval of exactly those steps
  await reset(); holdHashes();
  dialog = open([approved]); openPreset(0);
  type('pdf-preset-steps', 'inspect\nfix it'); click();
  await releaseHashes();
  save(); await releaseHashes();
  const saved = only(await dialog);
  assert.deepEqual(saved.steps, ['inspect', 'fix it']);
  assert.notEqual(saved.autoSend.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', saved), true);
  // ticked first and edited after: the tick was for the earlier text
  await reset();
  dialog = open([approved]); openPreset(0); await releaseHashes();
  click(); click();                 // off, then on again by hand
  type('pdf-preset-steps', 'inspect\nsomething else');
  save(); await releaseHashes();
  assert.equal('autoSend' in only(await dialog), false);
});

test('a late answer belongs to the edit that asked: not to another preset, a reopened one or a new dialog', LIMIT, async () => {
  // cancelled, then another dialog on an unapproved preset
  await reset(); holdHashes();
  const first = open([approved]); openPreset(0);
  el('pdf-no').fire('click');
  assert.equal(await first, null);
  let dialog = open([OTHER]); openPreset(0);
  await releaseHashes();
  assert.equal(box().checked, false);
  save(); await releaseHashes();
  assert.equal('autoSend' in only(await dialog, 'R2'), false, 'the other preset gains no approval');
  // switched to another preset inside the same dialog
  await reset(); holdHashes();
  dialog = open([approved, OTHER]); openPreset(0); openPreset(1);
  await releaseHashes();
  assert.equal(box().checked, false);
  save(); await releaseHashes();
  let result = await dialog;
  assert.equal('autoSend' in only(result, 'R2'), false);
  assert.equal(only(result).autoSend.digest, approved.autoSend.digest, 'the one left alone keeps its own');
  // the same preset opened again, then edited: the first opening's answer is not for this edit
  await reset(); holdHashes();
  dialog = open([approved]); openPreset(0); openPreset(0);
  type('pdf-preset-steps', 'inspect\nfix it');
  await releaseHashes();
  save(); await releaseHashes();
  result = await dialog;
  assert.equal(await presetApproved('P1', only(result)), false);
  assert.equal('autoSend' in only(result), false);
});

test('a save still waiting when the dialog is cancelled saves nothing and leaves a later dialog open', LIMIT, async () => {
  await reset(); holdHashes();
  const first = open([approved]); openPreset(0);
  save();
  el('pdf-no').fire('click');
  assert.equal(await first, null);
  const second = open([OTHER]);
  await releaseHashes();
  assert.equal(el('pdf').style.display, 'flex', 'the waiting save did not close the new dialog');
  el('pdf-yes').fire('click'); await releaseHashes();
  assert.deepEqual((await second).presets, [OTHER]);
  assert.deepEqual(failures, []);
});

/* ---------- which edit a save request belongs to ---------- */
const note = () => el('pdf-preset-auto-send-state');
const editorOpen = () => el('pdf').style.display === 'flex' && el('pdf-preset-editor').hidden === false;

test('a save queued behind a waiting one belongs to the edit it was asked for, never to a preset opened since', LIMIT, async () => {
  await reset(); holdHashes();
  const dialog = open([approved, OTHER]); openPreset(0);
  type('pdf-preset-name', 'Renamed');
  el('pdf-preset-done').fire('click');   // waits for the held digest
  el('pdf-yes').fire('click');           // asked for the same edit, behind it
  openPreset(1);
  type('pdf-preset-steps', 'look\nand change');
  await releaseHashes();                 // nothing else is clicked
  assert.equal(await pending(dialog), true, 'the old request did not end the dialog');
  assert.equal(editorOpen(), true, 'nor close the editor of the other preset');
  assert.equal(el('pdf-preset-steps').value, 'look\nand change');
  // the user's own save of the other preset works as usual
  save(); await releaseHashes();
  const result = await dialog;
  assert.deepEqual(only(result, 'R2').steps, ['look', 'and change']);
  assert.equal('autoSend' in only(result, 'R2'), false);
  assert.equal(only(result).name, 'Fix issue', 'the abandoned edit saved nothing');
  assert.equal(only(result).autoSend.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', only(result)), true);
});

test('a queued save does not take over the same preset opened again, a new preset after a delete, or a later dialog', LIMIT, async () => {
  // the same preset id, another edit of it
  await reset(); holdHashes();
  let dialog = open([approved]); openPreset(0);
  el('pdf-preset-done').fire('click'); el('pdf-yes').fire('click');
  openPreset(0);
  type('pdf-preset-steps', 'inspect\nfix it');
  await releaseHashes();
  assert.equal(await pending(dialog), true);
  assert.equal(editorOpen(), true);
  save(); await releaseHashes();
  let result = await dialog;
  assert.deepEqual(only(result).steps, ['inspect', 'fix it']);
  assert.equal('autoSend' in only(result), false);
  // the edit deleted, a new preset begun
  await reset(); holdHashes();
  dialog = open([approved]); openPreset(0);
  el('pdf-preset-done').fire('click'); el('pdf-yes').fire('click');
  el('pdf-preset-delete').fire('click');
  el('pdf-preset-add').fire('click');
  type('pdf-preset-name', 'Brand new'); type('pdf-preset-title', 'New card'); type('pdf-preset-steps', 'draft');
  await releaseHashes();
  assert.equal(await pending(dialog), true);
  assert.equal(editorOpen(), true);
  assert.equal(el('pdf-preset-name').value, 'Brand new');
  save(); await releaseHashes();
  result = await dialog;
  assert.deepEqual(result.presets.map(preset => preset.name), ['Brand new']);
  // cancelled, then another dialog with its own edit
  await reset(); holdHashes();
  const first = open([approved]); openPreset(0);
  el('pdf-preset-done').fire('click'); el('pdf-yes').fire('click');
  el('pdf-no').fire('click');
  assert.equal(await first, null);
  dialog = open([OTHER]); openPreset(0);
  type('pdf-preset-steps', 'look\nlater');
  await releaseHashes();
  assert.equal(await pending(dialog), true);
  assert.equal(editorOpen(), true);
  save(); await releaseHashes();
  assert.deepEqual(only(await dialog, 'R2').steps, ['look', 'later']);
});

test('Done then Save on one edit, however often clicked, is one save that ends the dialog', LIMIT, async () => {
  await reset(); holdHashes();
  const dialog = open([approved]); openPreset(0);
  type('pdf-preset-name', 'Renamed');
  el('pdf-preset-done').fire('click'); el('pdf-preset-done').fire('click');
  el('pdf-yes').fire('click'); el('pdf-yes').fire('click');
  await releaseHashes();
  const result = await dialog;
  assert.deepEqual(result.presets.map(preset => preset.name), ['Renamed']);
  assert.equal(only(result).autoSend.digest, approved.autoSend.digest);
  assert.equal(await presetApproved('P1', only(result)), true);
  assert.equal(el('toasts').children.length, 0);
});

/* ---------- a new approval that cannot be computed ---------- */
test('a new approval whose digest fails saves nothing, says so in the edit, and the same save works once it can', LIMIT, async () => {
  await reset();
  const dialog = open([M0]); openPreset(0); await turn();
  click();                               // the user approves these steps
  type('pdf-preset-name', 'Renamed');
  failing = true;
  el('pdf-yes').fire('click'); await releaseHashes();
  assert.equal(await pending(dialog), true, 'not saved');
  assert.equal(editorOpen(), true, 'the draft stays open');
  assert.equal(el('pdf-preset-name').value, 'Renamed');
  assert.equal(box().checked, true, 'the choice is kept');
  assert.equal(note().hidden, false, 'the edit says it was not saved');
  assert.equal(note().textContent, t('presets.autoSend.approveFailedSave'));
  assert.equal(el('toasts').children.length, 1);
  // no second tick, no second confirmation: the same save again
  failing = false;
  el('pdf-yes').fire('click'); await releaseHashes();
  const saved = only(await dialog);
  assert.equal(saved.name, 'Renamed');
  assert.equal(await presetApproved('P1', saved), true);
});

test('a new approval whose digest never answers is bounded the same way, and its late answer saves nothing', LIMIT, async () => {
  await reset(); holdHashes();
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const dialog = open([M0]); openPreset(0); await turn();
    click();
    el('pdf-yes').fire('click'); await turn();
    mock.timers.tick(60_000); await turn();
    assert.equal(await pending(dialog), true, 'not saved');
    assert.equal(box().checked, true);
    assert.equal(note().hidden, false);
    assert.equal(note().textContent, t('presets.autoSend.approveFailedSave'));
    assert.equal(el('toasts').children.length, 1);
    await releaseHashes();               // the digest that timed out answers now
    assert.equal(await pending(dialog), true, 'a late digest is not a save');
    el('pdf-yes').fire('click'); await releaseHashes();
    assert.equal(await presetApproved('P1', only(await dialog)), true);
  } finally { mock.timers.reset(); await releaseHashes(); }
});

test('a failed new approval is reported only in the edit that asked for it', LIMIT, async () => {
  // another preset opened while the digest waited
  await reset();
  let dialog = open([M0, OTHER]); openPreset(0); await turn();
  click(); holdHashes();
  el('pdf-yes').fire('click'); await turn();
  failing = true;
  openPreset(1);
  await releaseHashes();
  assert.equal(note().hidden, true, 'no error in the other preset’s editor');
  assert.equal(el('toasts').children.length, 0);
  assert.equal(await pending(dialog), true);
  // the dialog closed, another one open
  await reset();
  const first = open([M0]); openPreset(0); await turn();
  click(); holdHashes();
  el('pdf-yes').fire('click'); await turn();
  failing = true;
  el('pdf-no').fire('click');
  assert.equal(await first, null);
  dialog = open([OTHER]); openPreset(0);
  await releaseHashes();
  assert.equal(note().hidden, true);
  assert.equal(el('toasts').children.length, 0);
  failing = false;
  save(); await releaseHashes();
  assert.equal('autoSend' in only(await dialog, 'R2'), false);
});
