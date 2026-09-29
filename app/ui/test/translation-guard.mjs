// Hard gate for every test write to a shared (or named) pasteboard in the
// translation smokes. `withGuard` is the only way a carrier gets a guard:
// a refused begin throws BEFORE the body runs (no Cmd+C, no fixture /copy,
// no driver write can follow), every write/permit/adoption that is refused
// throws, and the guard is always settled in `finally` — before the error
// propagates and before the carrier can finish or be torn down.
// `call(action, extra)` is the smoke_pasteboard IPC (or a test double).
export class GuardRefused extends Error {
  constructor(stage, code) { super(`pasteboard guard ${stage} refused (${code})`); this.stage = stage; this.code = code; }
}

export async function withGuard(call, board, body, settled = () => {}) {
  const id = await call('guard-begin', { board });
  if (!(id > 0)) { await settled({ id: 0, result: 15, stage: 'begin', code: id }); throw new GuardRefused('begin', id); }
  const guard = {
    id,
    async write(text) { const r = await call('write', { text }); if (!(r > 0)) throw new GuardRefused('write', r); return r; },
    async permit() { const r = await call('permit'); if (r < 0) throw new GuardRefused('permit', r); return r; },
    async adopt(receipt) { const r = await call('adopt', { receipt }); if (r !== 0) throw new GuardRefused('adopt', r); },
  };
  let failure = null;
  try { return await body(guard); }
  catch (error) { failure = error; throw error; }
  finally {
    const result = await call('guard-end').catch(() => -9);
    await settled({ id, result, stage: failure instanceof GuardRefused ? failure.stage : failure ? 'error' : 'done',
      code: failure instanceof GuardRefused ? failure.code : 0 });
  }
}
