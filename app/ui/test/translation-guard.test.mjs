import test from 'node:test';
import assert from 'node:assert/strict';

// TL_GUARD_FLOW=legacy runs the same assertions against the start version's
// control flow (negative control); the default is the production helper.
const flow = process.env.TL_GUARD_FLOW === 'legacy'
  ? await import('./fixtures/legacy-guard-flow.mjs') : await import('./translation-guard.mjs');

function fakeNative({ begin = 1, writeCode = 7 } = {}) {
  const log = [];
  const call = async (action, extra = {}) => {
    log.push(action);
    if (action === 'guard-begin') return begin;
    if (action === 'write') return writeCode;
    if (action === 'permit') return 3;
    if (action === 'adopt') return extra.receipt === 4 ? 0 : -4;
    if (action === 'guard-end') return 11;
    throw new Error('unknown action');
  };
  return { call, log };
}

test('[F01] a refused guard-begin stops the section before any shared write or Cmd+C', async () => {
  for (const code of [-2, -3, -1]) {
    const native = fakeNative({ begin: code }); const effects = [];
    const outcome = await flow.withGuard(native.call, 0, async guard => {
      effects.push('cmd-c'); await guard.write('x'); effects.push('pbcopy');
    }).then(() => 'completed', error => error);
    assert.ok(outcome instanceof Error, `begin ${code} must throw`);
    assert.deepEqual(effects, [], 'no Cmd+C, no pbcopy, no write');
    assert.deepEqual(native.log, ['guard-begin']);
  }
});

test('[F02] a refused write, permit or adoption stops the section and still settles', async () => {
  const native = fakeNative({ writeCode: -2 }); const effects = []; const settled = [];
  await assert.rejects(flow.withGuard(native.call, 1, async guard => {
    await guard.write('x'); effects.push('after-refused-write');
  }, info => settled.push(info)));
  assert.deepEqual(effects, []);
  assert.equal(native.log.at(-1), 'guard-end'); assert.equal(settled[0].stage, 'write');
  const adopt = fakeNative();
  await assert.rejects(flow.withGuard(adopt.call, 1, async guard => { await guard.permit(); await guard.adopt(99); }));
  assert.equal(adopt.log.at(-1), 'guard-end');
});

test('[F05] an assertion or IPC failure inside the section settles the guard before propagating', async () => {
  for (const fault of ['assertion', 'ipc']) {
    const native = fakeNative(); const order = [];
    const call = async (action, extra) => { order.push(action); return native.call(action, extra); };
    await assert.rejects(flow.withGuard(call, 1, async guard => {
      await guard.write('synthetic');
      if (fault === 'ipc') await call('bogus');
      throw new Error('assertion failed');
    }, info => order.push(`settled:${info.result}`)));
    assert.deepEqual(order.slice(-2), ['guard-end', 'settled:11']);
  }
});

test('[F02] a successful section settles exactly once after the body', async () => {
  const native = fakeNative(); const settled = [];
  const value = await flow.withGuard(native.call, 1, async guard => { await guard.permit(); await guard.adopt(4); return 'ok'; },
    info => settled.push(info));
  assert.equal(value, 'ok'); assert.deepEqual(native.log, ['guard-begin', 'permit', 'adopt', 'guard-end']);
  assert.deepEqual(settled.map(s => [s.result, s.stage]), [[11, 'done']]);
});
