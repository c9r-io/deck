// The zone contract for a clock slot's instant, pinned to the same fixture
// the backend reads (procinfo.rs `wall_times_follow_the_zone_contract`):
// a wall time the spring switch skips moves forward by the gap, one the
// autumn switch repeats is its first occurrence, every other wall time is
// itself — so the drawer's "next" and the preview name the instant the
// backend fires at. `TZ` is process-wide, which is why this test has a
// file of its own (node --test runs each file in its own process) and sets
// the zone before any Date is built.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const fixture = JSON.parse(readFileSync(new URL('./fixtures/schedule-cases.json', import.meta.url), 'utf8'));
const { nextScheduleSlot, nextScheduleSlots } = await import('../js/pure.js');

test('a slot instant follows the zone contract the backend is pinned to', () => {
  assert.ok(fixture.instants.length >= 4);
  for (const c of fixture.instants) {
    process.env.TZ = c.zone;
    const [y, m, d] = c.date.split('-').map(Number);
    const midnight = Math.floor(new Date(y, m - 1, d, 0, 0, 0, 0).getTime() / 1000);
    const daily = { unit: 'day', days: [], minute: c.minute };
    assert.equal(nextScheduleSlot(daily, midnight - 1), c.at, `${c.zone} ${c.date} minute ${c.minute}: ${c.reads}`);
    assert.equal(nextScheduleSlots(daily, midnight - 1, 0, 1)[0], c.at);
    /* the key the backend keeps for the slot is local midnight plus the
       minute, whatever the wall time's instant */
    assert.equal(midnight + c.minute * 60, c.key, `${c.zone} ${c.date}: key`);
    const read = new Date(c.at * 1000);
    assert.equal(read.getDate(), d, `${c.date}: the instant is on that day`);
  }
});
