// Compatibility carrier for the baseline negative control ONLY: the control
// flow of translation-smoke.mjs at 9a2e8b1 (B05 Cmd+C), where a failed
// guard-begin was reported and the shared write still followed. It is run
// against the F01 test by scripts/translation-lens-verify.py to prove that
// test catches that defect; production carriers use translation-guard.mjs.
export class GuardRefused extends Error {}
export async function withGuard(call, board, body, settled = () => {}) {
  const id = await call('guard-begin', { board });
  const guard = {
    id,
    write: text => call('write', { text }),
    permit: () => call('permit'),
    adopt: receipt => call('adopt', { receipt }),
  };
  const out = await body(guard); // report(guard >= 0) and continue
  const result = id > 0 ? await call('guard-end') : -1;
  await settled({ id, result, stage: 'done', code: 0 });
  return out;
}
