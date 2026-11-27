# Local notifications — evidence (2026-11-28)

A generic local notification entry: programs of the same macOS user hand
Deck a notification with the bundled `deck-notify`; Deck stores it, shows it
under Notifications, and routes it through local notification rules into the
ordinary card and queue paths. Contract: `app/src-tauri/src/local_notify.rs`,
`docs/local-notifications.md`. Every row names where it ran and what it ran
against; a row that says fixture or unit is not real-app evidence.

All of it ran on uncommitted changes over `1941820a` (the commit that holds
this directory is the change). Closed facts only: step names, exit codes,
status words, counts. No agent text, session id or credential is kept here.

## 1. Gates (this Mac, final code)

See `gates.md` (filled at the end of the round).

## 2. Real app, real CLI, real Claude Code (air2, macOS 27.2.1)

The checkout's debug carrier on an isolated root (`/private/tmp/deck-verify-*`,
own tmux socket directory, experiment registry cleanup), air2's installed
Deck and `~/.deck` recorded before and after and never touched. The
operator ran the carrier's own `Contents/MacOS/deck-notify` and the
reference adapter `tools/local-notify-example/ci_watch.py` over SSH (same
uid); the page used the real Automations editor, the real Notifications
page and its Route menu. Cards ran Claude Code 2.1.332 (air2's own sign-in;
the run's work directory trusted for the run and the entry removed after).
Runner: `~/c9r-io/deck-local-notify-20261128/run_e2e.py`.

- `runs/r1-summary.json` — first run (page script before the `local-frozen`
  test fix): 14 of the then 16 page checks positive; `local-frozen` failed on
  the test's own read (sent rows leave the queue), fixed to read the card's
  frozen plan. CLI table, socket/inbox modes, inbox state and Claude markers
  inside.
- `runs/r2-summary.json` — first recovery run: the real `NO_RECEIPT` path
  (exit 6 → retry `duplicate` → `status` found) and a real adapter defect
  (its `occurred_at` was "now", so a retry a second later was a `CONFLICT`;
  adapter and docs fixed). The page threw after `local-revoked` (a test bug:
  a smoke fault count above its cap of 8), so the relaunch phase could not
  pass. r3 located it (stage 9), superseded.
- `runs/r4-summary.json` — **PASS**, final code: phase 1 19/19, relaunch 5/5.

### r4, item by item (the brief's matrix)

| # | Item | Where it is shown |
|---|---|---|
| 1 | first publish accepted; unmatched one listed in the real page | `local-unmatched` (row in `#notices-list`), CLI `accepted` exit 0 |
| 2 | same event again: no second notification, card or row | CLI `duplicate` ×2 (`duplicate`, `ciFail1Retry`); one auto card (`local-auto-card`) |
| 3 | same id, other content | CLI `CONFLICT` exit 4 |
| 4 | stored, receipt lost (fault `local-no-receipt` after the write) | `lostReceipt` exit 6 `NO_RECEIPT` → `lostRetry` `duplicate` → `lostStatus` `found` |
| 5 | update in place, late lower revision, resolve, next occurrence | `ciFail2` applied; `late` `stale-revision`; `ciPass3` resolve; `ciFail4` occurrence 2 (`local-frozen` revision 41, occurrence 2) |
| 6 | click and rule race; two clicks at once | `local-taken`: route of the auto-taken one `taken`; two simultaneous routes → one `ok`, one `taken`, one card |
| 7 | several rules, stable choice | the first rule in order took build-1842 both times (inbox claims `auto` on rule 1); unit `rules_match_in_settings_order_and_by_pattern` |
| 8 | restart windows | real hard stop + relaunch: ack refused after the card existed (`local-unacked` → `relaunch-acked`, still one card); claim stored, never offered (`local-held` → `relaunch-claimed`, one card); stored and unrouted stays open (`relaunch-settled`) |
| 9 | closing a card revives nothing | `local-deleted` (40 s past a poll: still routed/done, no new card) |
| 10 | updates never rewrite a queued run | inbox: claim 1 frozen `attempt 1`, the notification now `attempt 4`; `local-frozen`; second occurrence = new card titled `attempt 4` |
| 11 | revocation | `local-revoked`: approval and first-step option withdrawn → both unsent rows stripped, nothing sent in 45 s; Claude marker for attempt 4 absent |
| 12 | off, invalid, oversized, protocol | `disabled` exit 5 `UNAVAILABLE`, socket absent; `INVALID_INPUT`/`UNSUPPORTED_VERSION`/`TOO_LARGE` exit 3; protocol mismatch and capacity: unit tests |
| 13 | user and socket boundary; no impersonation; no remote surface | socket 0600 in a 0700 dir; `source: "slack"` refused; EDR census (one reviewed Unix listener added, no TCP); not in the MCP tool table |
| 14 | schema | unit: sticky v14, older reader refuses; backup not authority unchanged (existing tests) |
| 15 | Slack / MCP / phone | unchanged code paths; Rust suite (mentions, MCP mentions, connector projection) and UI `slack-mentions.test.mjs` green; WK regression modes in §3 |
| 16 | packaged CLI | carrier `Contents/MacOS/deck-notify` used for every call; `sidecar_provenance.py` (6 files), `tauri.conf.json` `externalBin`, `check-edr-binary`, `package-compat-candidate` |

Real Agent: Claude Code 2.1.332 answered step 1 (`LOCAL-NOTIFY-STEP-1-OK 1`)
and the approved step 2 after the 180 s chain quiet, with no click. Every
other card's head waited at the first-interaction gate (manual rule) or was
stripped (revocation). Cleanup: registry stop of every registered process,
root moved to Trash, trust entry removed, air2's own `~/.deck`, Deck and
tmux server identical before and after.

## 3. Regression (air2, WK smoke, same carrier)

See `regression.md`.

## Not run

- Release/notarized bundle (only the debug carrier; packaging scripts and
  provenance checked statically).
- A real Slack workspace (`mentions-live`); Slack is covered by unit and WK
  regression modes only.
- Sandboxed apps calling the CLI (documented as unsupported, not tried).
- Codex as the card agent (Claude only).
