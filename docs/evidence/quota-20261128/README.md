# Usage quota per agent account — evidence (2026-11-28)

Design: `docs/design/quota-projection-20261128.md` (local, untracked).
Implementation: `e7728f51` (feat(quota)), the smoke's `/bin/sh` fix in the
evidence commit.

## Gates (this Mac, final code)

| Item | Result |
|---|---|
| `cargo fmt --check`, clippy (CI form, `-D warnings`) | pass |
| CI-form `cargo llvm-cov --workspace --all-targets --all-features --locked … --test-threads=1`, empty TMPDIR | exit 0; deck-app 1571 passed, 0 failed; lines 85.05 %, functions 82.66 %; `scripts/check-test-tmpdir` 0 |
| `deck-status-helper` (unit 20, CLI 10: the tee passes the payload byte for byte, ends stdout before Deck answers, `--quiet` prints nothing, an unknown form still passes its input on) | pass |
| `scripts/ui-tests` | 1062/1062 |
| `python3 scripts/extension_catalog.py --check`, `check-js-syntax.py`, `node ui/js/check.mjs` | pass |

## Real Deck on air2 (debug carrier, isolated data root and tmux socket, experiment registry)

Runner: `run_e2e.py` here (copy of `~/c9r-io/deck-quota-20261128/run_e2e.py`).

| Mode | Run | What it ran | Result |
|---|---|---|---|
| `quota-switch` | `v1dcf6e72` | Carrier at `<run home>/Applications/deck.app`, `HOME` = the run's own home holding a Claude `settings.json` with a statusLine of "the user's" (`~/.claude/line.sh "$1"`, `padding: 1`) and an agy `settings.json` without one. The page clicked the Settings box: decline, accept, reopen, off. The operator copied both files after each checkpoint. | **PASS** 6/6. Files: unchanged after clean and decline; on = Claude `'<helper>' --quota claude-code \| ~/.claude/line.sh "$1"` with `padding` and every other key and byte kept, agy `'<helper>' --quota agy --quiet` with `stack_with_default: true`; off = both files byte for byte as before. |
| `quota-live` | `v53831458` | air2's own signed-in Claude Code 2.1.332 (two cards, same account) with `--settings` holding Deck's statusLine tee into the fixture's user command (prints `QUOTA-USER-LINE`), and Codex 0.169.0 (`--no-daemon`) under a Codex home of the run's own (copied sign-in, removed after). | **PASS** 9/9 in 69 s: clean start (no account, area hidden); one Claude entry with 2 windows (5 h, 7 d) and both cards; one Codex entry from its rollout with 2 windows, linked to its card; the sidebar area 2 rows = 2 accounts; the card chip; the detail listing 2 cards. Both Claude panes show `QUOTA-USER-LINE` in the status line (the user's command still runs behind the tee). |

`vf5657904` (`l1`) failed before any product step: the card scripts lived
under Deck's data root, whose files Deck keeps 0600, so the shell refused
to execute them. The smoke now runs them through `/bin/sh`; the run was
cleaned up by the registry like the others.

Each run: registry cleanup stopped exactly the carrier and its tmux server,
nothing left under the root, root moved to the Trash, the Claude folder-trust
entry for the run's work dir removed, the copied Codex sign-in deleted;
air2's own `~/.deck`, installed Deck and `deck` tmux server unchanged
(`machineBefore` = `machineAfter`).

## Not run

- `deck_worker_list` `quota` through a real MCP client and orchestrator card
  (unit tests only: the projection, its card filter, no folder paths).
- agy with a real session (air2 has agy 1.5.16; only the switch's file
  edits were run). agy's account key (`email`) is from its documentation.
- The switch from the release bundle on a person's real home, and boot
  migration after a moved bundle (unit tests only).
- The collapsed-sidebar ring and the stale/expired styling in a real window
  (unit tests on the fake document only).
