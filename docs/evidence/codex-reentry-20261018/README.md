# Codex session re-entry — evidence (2026-10-18)

What this records: the first implementation of re-entering an embedded
(`--no-daemon`) Codex client's session after a shell-service restart
(`app/src-tauri/src/agent_reentry/codex.rs`, the multi-adapter core in
`agent_reentry/mod.rs`, the Codex lifecycle hooks in `agent_status/hooks.rs`,
the lifecycle helper's second source word), and the three isolated real-Codex
transactions that were run against it on this Mac. The design brief and the
lead's acceptance stay outside git (`docs/design/`).

Every row below says where it ran and on what. A green gate closes only what
its jobs executed; the limits are listed at the end.

## The product behaviour under test

| Question | Answer | Where it is pinned |
|---|---|---|
| Which clients | A direct client whose running code is OpenAI's signed `codex`, started with `--no-daemon` (required), `--yolo` or not, `resume` alone or with one id, `-m/--model`, `--add-dir`. A daemon client, prompt, `exec`, approval or profile flag, `-c`, hook-trust bypass: unsupported | `codex.rs` `GRAMMAR`; `procinfo.rs` `a_required_flag_and_a_bare_resume_word_classify_as_the_grammar_says` |
| The final session | Receipts crossed with the exit hint: the ids the kernel-attributed `SessionEnd` receipts name, and what the pane printed between the freeze capture and the seal capture (`resume::printed`, difference by multiplicity). A printed id counts only among the reported ones; nothing printed and exactly one end names it; else the card is skipped | `codex.rs` `final_session`; `mod.rs` `printed`, `newly_printed`; tests `the_final_session_is_the_printed_id_among_the_reported_ends`, `newly_printed_is_the_difference_by_multiplicity_in_order`, `codex_like_cards_are_started_under_policy_a…` (cards c, d, e) |
| Confirmation | No entry receipt before the first turn → `started` (launch image, directory, environment and foreground generation checked; no receipt contradicts); a `resume` receipt inside the share → `confirmed` as for Claude; a wrong one → `unconfirmed` | `mod.rs` `reopen`; tests `a_codex_like_entry_receipt_confirms_and_a_wrong_one_does_not` |
| The request | `'<frozen exe>' resume --no-daemon [--yolo \| --sandbox workspace-write --ask-for-approval on-request] <id>`; Policy B re-sends `--yolo` on a bypass launch; Policy A on a bypass launch asks for the ordinary profile (`ordinary-profile-requested`); an ordinary launch is a bare resume under either | `codex.rs` `request`; `mod.rs` `Grant`; tests `codex_like_cards_are_started_under_policy_a…` / `…policy_b…` |
| Hooks | Two Lifecycle entries in `~/.codex/hooks.json`, `ShellBrief` (one simple command, timeout 1, no async), installed by the Codex opt-in; Codex asks the user to trust them once | `hooks.rs` `CODEX_LIFECYCLE_HOOKS`, `lifecycle_style`; `hooks/tests.rs` `claude_and_codex_declare_a_reentry_capability_and_lifecycle_hooks` |
| UI | Codex rows labelled `Codex ·`, selectable only with Codex's own observation on; the Codex note said once; `started` counted as started, never as reopened; "Insert resume command" on an entry that was neither reopened nor started; `--no-daemon` in the completion grammar | `reentry-model.test.mjs` (two new tests), `restart-reentry-dom.test.mjs`, `resume-model.test.mjs` |

## Isolated real-Codex transactions (this Mac, 2026-10-18, 22:31–22:45 JST)

Driver `cxctl.sh` (+ `run-campaign.sh`, `hookstate.py`, `ids.py`), one
campaign per root under `/private/tmp/deck-cx-<campaign>`: an isolated HOME
with its own `config.toml` (loopback stub provider from the 2026-10-17
experiment, no auth, update checks off, both working directories trusted),
Deck's two lifecycle hooks in `hooks.json` exactly as production writes them,
their trust state pre-written with the replica of Codex's hash (calibrated
3/3 against the 2026-10-17 experiment's stored hashes), a private tmux
server (the bundled sidecar copied into the root so the registry may stop
it), and the production transaction run by the test controller
`agent_reentry::tests::native_m1_transact` (`DECK_M1_CLIENT` = the real
`codex-cli 0.163.0`, admitted under `codex::CAPABILITY` by its signature).
No model request was made: no turn was sent to any client. The clients were
started by typing the one start line each, exactly as a user would; the only
operator input afterwards was `/status` in each re-entered client.

Card `a` was started `codex --no-daemon --yolo` (bypass launch), card `b`
`codex --no-daemon` (ordinary). The thread each client held was read from
the writer lock it keeps open (its own kernel descriptor table), digested
with a campaign salt for the controller's "the request names the session the
client exited in" check; raw ids are in no file of this directory
(`<id-a>`, `<id-b>` in the copied records).

| Campaign | Policy | Controller | a | b | `/status` after (operator read) |
|---|---|---|---|---|---|
| `a1` | A | 4 of 24 checks FAILED, all in the controller itself (see below); the transaction's own facts as in `a2` | `started`, `ordinary-profile-requested` | `started` | a: Session = the exited id, Permissions **Workspace (Ask for approval)**; b: Session = the exited id, Workspace (Ask for approval) |
| `a2` | A | exit 0, failed-checks=0 (24 checks) | `started`, `ordinary-profile-requested`; request `resume --no-daemon --sandbox workspace-write --ask-for-approval on-request <id-a>` | `started`; request `resume --no-daemon <id-b>` | a: Session = the exited id, **Workspace (Ask for approval)**; b: same, Workspace |
| `b1` | B | exit 0, failed-checks=0 (24 checks) | `started`, `bypass-reauthorized`; request `resume --no-daemon --yolo <id-a>` | `started`; request `resume --no-daemon <id-b>` | a: Session = the exited id, **Full Access / YOLO mode**; b: Session = the exited id, Workspace (Ask for approval) |

In every campaign: both old clients exited on Deck's one Ctrl+D and printed
their exit hint; both old generations were gone before the kill; the queue
row stayed paused on disk through the kill and after; one request and one
Enter per entry; the started clients were the panes' foreground, children of
the new shells, with exactly the request's words in their launch image; and
after the re-entry each working directory still held exactly one rollout
(the resume wrote into the exited thread, no new session was created).

`a1`'s four failed checks were the controller's, not the product's: (1)
`preview-carries-no-native-identity` scanned the preview for the client's
file name, and for Codex that name is also the adapter's source word the row
is labelled with; (2) the request parser tried the plain shape first, which
is a prefix of the lowered shape, so card a's request was dropped, and (3)
and (4) followed from it. Both were fixed in the controller before `a2` and
`b1` (`tests.rs`, `native_m1_transact`); `a1`'s record is kept as it was.
Each campaign was ended through the experiment registry
(`scripts/edr_runtime.py experiment cleanup`), verified; two driver defects
met on the way (a backgrounded `cd && nohup … &` list that held the caller's
pipe, and my editing of the running driver script, which broke a step's
parse after its transaction had already ended with exit 0) are recorded in
each campaign's `log.txt` and were corrected in `cxctl.sh`.

Per campaign (`a1/`, `a2/`, `b1/`): `log.txt`, `transact.out` /
`transact.stderr` (the controller's full output), `screen-start-*.txt`
(the TUI at the start, no trust notice), `client-*.txt` (the old client's
process line), `newclient-*.txt` (the started client's arguments),
`screen-after-*.txt`, `status-raw-*.txt`, `rollouts-after.txt`,
`tools.sha256` (the Codex binary, the helper, the tmux sidecar and its copy,
the test binary, the stub source and binary), `codex-version.txt`,
`server-env.txt`.

## Gates on the final tree

All run on this Mac on 2026-10-18 on the tree of the commit this directory
lands in, each after the last code change:

| Gate | Result |
|---|---|
| `scripts/ui-tests` (node:test + coverage thresholds) | 869 passed, 0 failed; every eligible module in coverage |
| `python3 scripts/check-js-syntax.py` · `node app/ui/js/check.mjs` | 181 modules pass; 74 modules resolve |
| `python3 scripts/extension_catalog.py --check` | current |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | clean |
| `cargo llvm-cov --workspace --all-targets --all-features --locked --offline --fail-under-lines 75 --fail-under-functions 75 -- --test-threads=1` (isolated HOME) | every test set ok (deck-app unit tests 1282 passed, 13 ignored); TOTAL lines 85.69 %, functions 83.91 % |

Two earlier runs of the coverage step on this tree found what the unit runs
had not: the Signal census needed the two new `.agent` sites pinned as agent
CLI names (`resume.rs` `printed`, `terminal.js` `insertResumeHint`), and the
lifecycle helper's process-level test listed `codex session-start` among the
argv the helper refuses. Both were test changes; the third run is the one in
the table.

## Not shown

- The real application (its window, IPC, the Settings toggle writing
  `~/.codex/hooks.json`, Codex's own trust prompt for the two entries, the
  dialog's Codex rows and the `started` result in WKWebView): the transaction
  ran through the test controller, as Claude's P3 part 1 did. The dialog's
  rendering is covered by the Node DOM tests only.
- The user's own Codex home and hooks: the campaigns used an isolated home
  with the trust state pre-written; the trust prompt a user meets was not
  exercised.
- A shared-daemon client (refused by the grammar, by design), a client under
  a shell other than zsh (the helper's direct-parent property was measured on
  zsh only), `/new` or `/resume` inside a real client before the restart (the
  stand-in covers the two-ends case; no real client switched threads here),
  a real turn before or after the re-entry, and a session with an active goal
  or queued input.
- macOS other than this machine's, the release carrier, Developer ID and
  notarization.
