# Interjected worker lines — evidence (2026-11-27)

`deck_worker_interject`: the kernel-proven orchestrator card sends a line to
a worker card it started without waiting for the card to fall quiet; the
agent takes it through its own way of taking a message while it works.
Contract: `scheduler/select.rs` "Interjected lines", `mcp/workers.rs`,
`agent_pi/handover.rs` "Interjected lines", `docs/mcp.md` "Interjecting.".
Every row below names where it ran and what it ran against; a row that says
mock or isolated is not real-agent evidence.

All of it ran on uncommitted changes over `01ff6dbf` (the commit that holds
this directory is the change). Closed facts only: step names, states,
stages, seconds, counts, whether files exist. No agent text, session id,
credential or process argument is kept here.

## 1. Isolated agent probes (this Mac, macOS 27.2.1)

`agent-probes.md`: what Claude Code 2.1.332, Codex 0.172.0 (`--no-daemon`)
and agy 1.5.18 do with Deck's delivery sequence (paste, 600 ms, Enter) while
they work, and with the same keystrokes on their own dialogs; when their
hooks report a dialog. They decided the design: the `PermissionRequest`
mapping for Claude Code, the pre-paste and pre-Enter re-reads, and the agy
refusal.

## 2. Pi native (this Mac, real Pi 1.6.0, fixture model)

`scripts/pi_native.py` (Deck's production status listener, Pi transport,
mailbox, observer extension and Pi's own message path, against real
interactive Pi, npm install and official standalone build, a deterministic
local provider in place of a model):

| Run | Result |
|---|---|
| full suite, `deck_app-8f169971261f42d9` | 234 checks passed, 0 failed; the run stopped at the new `steer` scenario's start (it relaunched Pi with the work status the scenario before had switched off: a scenario defect, fixed by `S00`); cleanup exit 0, no process or socket left |
| `--only exec_peer,steer` after the fix (`steer` right after the scenario that switches the work status off) | 37/37, cleanup exit 0, nothing left (`runs/pi-native-exec-peer-steer-result.json`; the full run's summary is `runs/pi-native-full-result.json`) |
| `--only steer` (earlier, same code path) | 26/26: S01 an ordinary worker line is never offered while Pi runs a tool; S02 an interjected one is handed over during the run; S03 a later provider request of the same run carries it; S04 one run, one settlement, nothing synthesized; S05 between runs it is an ordinary message |

## 3. Deck-level, real agents (air2, macOS 27.2.1, debug carrier of this checkout)

`worker-interject` (app/SMOKE.md): the page authorizes one client and opens
the orchestrator card; `scripts/mcp-interject-e2e.py`, typed into that
card's pane, starts one worker on a task in two phases — a 70-paragraph
story in its reply (model output, no tool), then `python3 phase1.py`
(100 s, writes `p1-started` / `p1-done`), then a result file — and, past
the 60 s session gap, queues line A and interjects line B1 while the
story is written, then B2 past the next gap while `phase1.py` runs. The
runner and its raw outputs are outside the repository
(`~/c9r-io/deck-interject-20261127/`).

| Run | Agent | Page | Client |
|---|---|---|---|
| c1 | Codex 0.169.0, `codex --no-daemon --dangerously-bypass-hook-trust`, gpt-6.1-sol low, a `CODEX_HOME` of the run's own (Deck's four Signal hooks only, the sign-in copied and removed after, on-request approvals, workspace-write sandbox without /tmp) | PASS 6/6 | PASS 18/18 |
| i3 | Claude Code 2.1.332, Opus 5.5 (the machine's Claude sign-in), `--setting-sources project --permission-mode acceptEdits --settings <fixture>` (Deck's Claude hook table incl. `PermissionRequest`; only `python3 phase1.py` allowed) | PASS 6/6 | 17/18: the line sent after the dialog was still held by the session gap (60 s after the line that raised the dialog) when it was read at 30 s, not yet by the agent; the client now waits for the stage to become `agent` |
| i4 | the same, after that client fix | PASS 6/6 | PASS 18/18 |
| p1 | Pi 1.6.0 (official standalone build) on MiniMax-M3, air2's own Pi credential borrowed for the run (`pi-interject`, `scripts/pi-live-verify.py --model default`, relocated home) | green | 15/16: B1 was sent while the 80-paragraph story was written, but the model had finished it and started `phase1.py` by the first view that showed B1 delivered (4.7 s later); Pi showed it as its steering message during the program |
| p3 | the same, a 200-paragraph story | green | PASS 16/16 (no dialog step: Deck sees no question of Pi's) |

What c1, i3, i4 and p3 measured:

- B1 sent while the story was written (`p1-started` absent): seen
  delivered 1.1 s after the call (Claude, Codex) and 4.7 s (Pi: the
  observer takes it at its next exchange), the agent still `working`, the
  program not started; A at `quiet` (Pi: `pi-handover`, the pause between
  two runs) at first look, then at `gap` after B1 went.
- B2 sent while `phase1.py` ran: delivered 1.1 s (Claude, Codex) and 2.6 s
  (Pi) after the call, the program still running.
- The agent followed both: `result-interject-<tag>.txt` and
  `extra-<tag>.txt` exist, `result-<tag>.txt` never did. Claude Code queued
  B1 and B2 and read them after the program's tool call, in the same turn;
  Codex steered its running turn; Pi held them as its steering messages
  and gave them to the model after the program.
- A went after the turn: 194.8 s and 187.8 s (Claude, i3, i4), 369.1 s
  (Codex) and 256.7 s (Pi) after B1; the page saw two interjected
  deliveries before it.
- Identity: a replay returned the same operation; the same request id
  through `deck_worker_send` was `REQUEST_ID_CONFLICT`; a stale episode
  `EPISODE_MOVED`; a control character `INVALID_ARGUMENTS`.
- An interjected line that needs the agent's permission (Claude: a
  `python3 -c` program; Codex: a write outside its sandbox): the card said
  `needs-input` (Claude: `PermissionRequest`; Codex: its own hook); a
  further interjected line stayed queued; the dialog was not answered (no
  file). In c1 and i4 the line reached stage `agent` once the gap ran out
  and stayed queued there for 30 s more, the agent still `needs-input`.
- Cleanup: the registry stopped the carrier and its tmux server (exit 0),
  nothing of the root left running, the root moved to the Trash; the Claude
  folder-trust entry the runner adds for the run's own work directory was
  removed (air2's `~/.claude.json` holds no `/tmp` entry afterwards); the
  Codex sign-in copy was removed before the root left; the Pi run's
  borrowed credential entry was gone from its root and air2's own Deck
  showed the same processes and data-file names before and after.

The client's own step records are in `runs/` (`*-client.json`), with the
page verdicts (`*-verdict.txt`) and the Pi runs' summaries
(`pi-p*-result.json`).

Two earlier runs (i1, i2) failed on the client's task, not on Deck: Claude
Code 2.1.332 refuses a foreground `sleep` (“Blocked: standalone sleep”), so
the agent ended its turn and the card fell quiet — A was then eligible and
went before B1, as designed. Hence `phase1.py`.

## 4. Gates (this Mac)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | clean |
| `cargo llvm-cov --workspace --all-targets --all-features --locked --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` (isolated HOME, empty TMPDIR), final code | deck-app 1506 passed, 0 failed, 14 ignored; every other test binary passed; lines 85.59 %, functions 83.73 %; TMPDIR empty; `/tmp/tmux-501` holds only the production `deck` socket |
| `scripts/ui-tests`, final code | 1046/1046, coverage inventory ok, TMPDIR empty |
| `scripts/check-js-syntax.py`, `node app/ui/js/check.mjs` | 236 modules; 99 modules, ok |
| `scripts/extension_catalog.py --check` | current |
| `python3 -m unittest` (the gate's eight script suites) | 202 OK, TMPDIR empty (`check-edr-binary` now pins both observer hand-over calls) |
| mutation: the Pi pass rule reverted (`Some(Hold::PiHandover) => false`) | `a_pi_interjected_line_passes_a_line_that_waits_for_the_pause_between_runs` fails at its first assertion; restored |

## 5. Not run here

- A release carrier, macOS 26, an installed Deck: not run.
- agy inside Deck: not run; `deck_worker_interject` refuses an agy card
  before anything is queued (unit test), on the evidence in
  `agent-probes.md`.
- Codex's shared daemon: worker types must use `--no-daemon`; unchanged.
- Target lifecycle (agent exit, shell in the foreground, re-entry) with an
  interjected line pending: the same guards as every row (unchanged code
  path), no interject-specific native run; Deck restart with a pending
  interjected line: persistence and fence tests only.
- Pi with a draft in its editor or a dialog open while a steering message
  is handed over: not run separately (the hand-over is Pi's API call, not
  keystrokes; the ordinary-line `pi-worker` smoke covers that call).
