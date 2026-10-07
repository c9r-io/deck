// The first rule save of a run (F3.3). Each scene runs in a process of its
// own (fixtures/first-save-scenes.mjs): no earlier test, refresh or save has
// told the settings writer what the file holds, only the scene's own
// production `loadSettings()`, as at launch.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const scenes = fileURLToPath(new URL('./fixtures/first-save-scenes.mjs', import.meta.url));
for (const [scene, title] of [
  ['clock-first-save-fails', 'a clock rule save refused as the first save of a run is in no later save\'s write'],
  ['badge-first-save-fails', 'a badge rule approval refused as the first save of a run is in no later save\'s write'],
  ['first-save-fails-then-last-fails', 'with the last queued save failing too, the refused first save is nowhere in the file'],
  ['first-save-succeeds', 'a first rule save that lands is kept by the saves made while it was under way'],
  ['given-up-before-its-turn', 'a rule save given up before its turn is in no write when no save of the run has succeeded'],
  ['grant-kept', 'after a load, a save of another setting keeps a channel permission and asks for none'],
  ['backup', 'a backup is loaded without writing, and no later save or load brings its channel permission back'],
  ['unreadable', 'settings that cannot be read are not written over by loading'],
  ['first-run', 'a first run saves its first setting with one save'],
  /* F3.4: a rule save that landed, then another setting's save that failed */
  ['landed-clock-then-font-fails', 'a clock rule save that landed is kept in memory and in the next rule save when a font save fails'],
  ['landed-clock-then-shortcut-fails', 'a clock rule save that landed is kept when a shortcut save fails'],
  ['landed-approval-then-font-fails', 'a badge approval that landed is kept in memory and in the next rule save when a font save fails'],
  ['withdrawn-channel-permission-then-font-fails', 'a channel permission withdrawn by a save that landed is not brought back by a failed font save'],
  ['landed-clock-then-font-lands', 'a rule save and a font save that both landed are both kept by the next rule save'],
  ['font-fails-alone', 'a font save that fails alone takes back its own change and leaves every automation and permission'],
]) {
  test(title, () => {
    const run = spawnSync(process.execPath, [scenes, scene], { encoding: 'utf8', timeout: 20000 });
    assert.equal(run.status, 0, run.stderr || run.stdout || String(run.error));
  });
}
