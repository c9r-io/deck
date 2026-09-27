# Controlled Execution calibration (CE1, CE1.1)

The ruler for Controlled Execution: it measures Full Local parity today and
will carry the Protected tracks later. Governance: Controlled Execution r2
(approved 2026-09-27). This file is an index plus the recorded baselines; each
contract lives in the header of the file that owns it.

| Piece | File | Owns |
|---|---|---|
| Plan v1 (frozen, historical) | `scripts/ce/plan-full-local-1.json` | the first calibration (accepted 2026-09-27); never edited — `test_ce_verdict.py` pins its digest |
| Plan v2 (current) | `scripts/ce/plan-full-local-2.json` | the calibrated comparison: case classes, registered transport differences, the reasons for every reclassification from v1 |
| Probe | `scripts/ce/probe.zsh` | one observation per call, no values or secrets (presence, digests, synthetic or hashed identities) |
| Harness | `scripts/ce_parity.py` | the ci lanes on a disposable bundled-tmux server and the designated lanes on an isolated smoke Deck; cleanup evidence |
| B-admission probe | `app/src-tauri/src/mcp/tests.rs` `ce1_probe_*` | how the production `deck_exec` route admits a Full Local cwd; observes, never asserts the outcome |
| Aggregator | `scripts/ce_verdict.py` (+ `scripts/test_ce_verdict.py`) | the ONLY verdict: closed classification, tracks, counters, exit code |

Raw evidence of each accepted run is archived read-only with `SHA256SUMS`
outside the repository (`~/c9r-io/deck-ce-evidence/<date>-plan-v<n>/`).

## Comparison contract (plan v2)

The comparison target is Deck's ordinary Terminal: a login interactive shell
in a Deck tmux server session. Not the Deck.app process environment.

- **host-capability** — what any program in the Deck session gets without
  shell startup files: the session base environment (the Deck tmux session
  environment as a process tmux starts directly sees it), filesystem,
  sockets, localhost, subprocesses, cwd. A structured direct job must have
  all of it; the direct lane is compared with the session base (`A-base`).
- **shell-semantic** — the developer environment startup files build
  (`.zshenv`, `.zprofile`, `.zshrc`, shadowing, `#!/usr/bin/env`
  interpreters). Obtained by the documented Terminal-semantic invocation and
  compared with the terminal.
- **transport** — registered differences that remove no work: no startup
  files in a direct launch (TD-1), no tty (TD-2), no persistent shell state
  (TD-3), no Deck-internal coordinates in a job (TD-4, checked).

The Terminal-semantic Full Local invocation is the user's login shell with
`-lic`: executable `$SHELL` (`/bin/zsh` here), arguments `-lic <command>`.
`/bin/zsh -lc` is NOT it: it is non-interactive and never reads `.zshrc`.
Only zsh has been calibrated.

## Run

```sh
cargo build --manifest-path app/src-tauri/Cargo.toml -p deck-mcp-runner
scripts/ce_parity.py --plan scripts/ce/plan-full-local-2.json --out /tmp/ce.jsonl \
    --app-probe --deck-build "$(git rev-parse --short HEAD)"
scripts/ce_verdict.py --plan scripts/ce/plan-full-local-2.json \
    --evidence /tmp/ce.jsonl --evidence /tmp/ce.jsonl.app.jsonl --out /tmp/verdict.json
```

The harness exits 0 when every planned observation of its mode was recorded
and cleanup was confirmed, whatever the observations show. The aggregator
exits 0 only for `overall = pass`.

### Designated lanes (maintainer approval required)

1. Build the smoke bundle (`DECK_SMOKE_DATA_DIR=<abs dir>
   DECK_SMOKE_TMUX_SOCKET=deck-smoke-ce app/run.sh`, then quit it), and
   launch it through `/usr/bin/open -n …/deck-smoke.app --args
   --smoke-data-dir … --smoke-tmux-socket deck-smoke-ce` with an environment
   holding ONLY the GUI-session variables (HOME, USER, LOGNAME, SHELL, PATH,
   TMPDIR, SSH_AUTH_SOCK, `__CF_USER_TEXT_ENCODING`, COMMAND_MODE,
   SECURITYSESSIONID). `open` passes the caller's environment to the app, so
   launching from an agent or terminal shell contaminates the baseline;
   compare the smoke server's variable names with a normally launched
   Deck's before trusting the run.
2. `scripts/ce_parity.py --plan … --mode designated --smoke-socket
   deck-smoke-ce --runner …/deck-smoke.app/Contents/MacOS/deck-mcp-runner
   --out d.jsonl` — refuses a server without Deck's metadata.
3. Quit the smoke Deck, `scripts/edr_runtime.py --cleanup --socket
   deck-smoke-ce`, remove the data directory.

## Baselines

### Plan v1 on fe19a89 (accepted, frozen)

`full_local_parity = fail`: ci 13 fail / 11 pass; designated 8 missing.
Unchanged; re-aggregating the archived evidence with the current aggregator
reproduces it case for case.

### Plan v2 on fe19a89 (CE1.1)

`full_local_parity = fail`; no case missing or unknown; every counter 0.

- ci: 10 fail / 15 pass (two runs, identical classifications).
- designated: 6 fail / 14 pass.

Shell semantics under `-lic` are at parity with the terminal in both
environments: synthetic `.zshenv`/`.zprofile`/`.zshrc`, shadowing and
`#!/usr/bin/env` launchers; real node, npm, cargo, rustc, python3 and git
(same identity, version and interpreter). The invocation completed
unattended every time (ci 45–69 ms, real 0.4–0.5 s, exit 0, no output).

Confirmed Full Local regressions on fe19a89 (all `FULL_LOCAL_PARITY_REGRESSION`;
repaired by CE1b, below):

| Defect | Evidence |
|---|---|
| The runner replaces the session PATH with a fixed Deck PATH | ci `FL2-HOST-path-value`, `-tool-base`, `-env-shebang`; real `FL2-D-HOST-tool-{node,npm,git}`: the session base has no node, the direct job gets Homebrew node v26.7.0 and npm on it (the terminal: nvm node v24.19.0); the base's system git 2.54.0 becomes Homebrew git 2.55.0 |
| `SSH_AUTH_SOCK` is stripped | ci and real, direct and `-lic` (`ssh-add -l` exit 2 / socket variable absent) |
| Session environment coordinates are stripped | ci `CE_DEV_VAR`; `SHELL` in ci and real |
| An existing cwd outside the project root is refused | `deck_exec` answers `PERMISSION_DENIED` |

### CE1b repair on the same plan v2 (host-terminal-env-v1)

Plan v2 unchanged (digest sha256:42afe7fa…). `full_local_parity = pass`,
45/45 mandatory cases, every counter 0:

- ci: 25/25 pass — `--gate full_local_parity:ci` exits 0.
- designated (clean smoke launch, runner rebuilt from this source): 20/20
  pass — `--gate full_local_parity:designated` exits 0.
- `overall` stays `blocked` because the protected / authority / EDR tracks
  have no cases yet; nothing about Protected exists.

Real tools after the repair: the direct job now resolves exactly what the
session base resolves — node and npm absent (they live on the PATH `.zshrc`
builds), cargo and rustc absent (added by `.zshenv`), python3 and git the
system ones (Apple git 2.54.0, no Homebrew injection) — and `/bin/zsh -lic`
still reproduces the terminal (nvm node v24.19.0 with npm 11.17.0 on it,
cargo/rustc 1.97.1, Homebrew git 2.55.0). The SSH agent is reachable in both
forms; `SHELL` is present.

Remaining differences are the registered transport differences only (TD-1
to TD-4) plus the uncalibrated shells (anything but zsh).

## CI gate

`.github/workflows/gate.yml` runs the real workload on every gate, not only
the aggregator's tests, in three steps that each fail the job on their own
exit code (no pipe, no `|| true`, no retry — a lane error is UNKNOWN and
blocks):

1. build `deck-mcp-runner`;
2. `ce_parity.py --plan scripts/ce/plan-full-local-2.json --mode ci
   --app-probe` — evidence generation; exits 1 unless every planned ci
   observation was recorded and cleanup was confirmed;
3. `ce_verdict.py … --gate full_local_parity:ci` — exits 0 only when the
   ci sub-status of `full_local_parity` passes and every ci run counter
   (cleanup, friction, prompts, fallback, run errors) is 0.

The ci corpus is host-independent (synthetic tools and variables only), so it
runs on any macOS arm64 runner with the bundled tmux. The designated cases
stay a separate certification on a real user machine (`--gate
full_local_parity:designated`); ordinary CI never needs a real user
environment and CI evidence never substitutes for it.
