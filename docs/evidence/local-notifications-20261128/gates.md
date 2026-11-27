# Gates (this Mac, macOS 27.2.1, final code)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | clean |
| CI-form coverage `cargo llvm-cov --workspace --all-targets --all-features --locked --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` (isolated HOME, empty TMPDIR) | exit 0; deck-app 1548 passed 0 failed 14 ignored; every integration target passed (EDR-quiet 20, external admission 8, IPC census 3, …; adapter 119, CLI 6); lines 84.92 %, functions 82.45 %, regions 85.78 % |
| `scripts/ui-tests` | 1057/1057, coverage gate ok |
| `python3 scripts/check-js-syntax.py`, `node app/ui/js/check.mjs` | 244 modules; 102 modules, no cycles outside the view core |
| `python3 scripts/extension_catalog.py --check` | current |
| `python3 -m unittest discover -s scripts -p 'test_*.py'` | 202 OK |
| `scripts/smoke-verdict` (via the runners) | see README §2 |

New tests: `src/local_notify/tests.rs` (21: input bounds, normalization before
fingerprint, duplicate/conflict, write failure accepts nothing, revisions,
occurrences, claim exclusivity incl. 8 concurrent claims, restart re-offer,
consumption facts, skipped claims, orphan retirement, dismissal, capacity,
retention, status, wire, socket permissions and bounds, failed ack re-offer);
`scheduler/tests.rs` local trigger authority/first-send/revocation;
`inbound.rs` local rule validation and the built-in source pin; `storage.rs`
sticky v14; `notify-cli` (6: argument closure, socket override, stdin bounds,
exit codes, UNAVAILABLE, NO_RECEIPT); UI `notices.test.mjs`,
`local-rules.test.mjs` (digest twin against `ui/test/fixtures/local-grant.json`).
