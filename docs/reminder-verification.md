# Reminder ownership and unattended verification

`persistence.js` is the single Board writer. `card.reminder` contains a random
32-hex identity, revision, UTC milliseconds, original zone, bounded note,
in-app-only intent and a monotonic due latch. There is at most one reminder per
card and no content-authority/execution grant. Frontend transactions check exact
revision claims before destructive effects. `documents::save_board` validates
these against the committed Board again; ordinary callers cannot silently omit
protection. Ending/edits use that same writer, including native responses.

Board Reminder or blocked-retirement fields upgrade `deck.json` to sticky v5.
Old readers (maximum v4) refuse the actual envelope before decoding and cannot
save it. Reminder-free legacy Boards retain their ordinary schema. Ending all
reminders does not downgrade the envelope. Unknown schema, corruption and save
errors preserve the existing storage refusal/recovery policy.

The native projector only reads a validated committed snapshot supplied by the
authoritative Board load/save door. It never consumes storage recovery ahead
of that loader. The save fence peeks without quarantine; a previously
quarantined Board recovers from its typed backup across reload/restart, or
refuses empty defaults when no valid backup exists until the user explicitly
restores a kept copy (its reminders are projected as they were) or starts a
new Board (an empty projection withdraws the old requests). The native
projector only projects committed Board intent. Request identifiers
are `deck.reminder.<hex UTF-8 cardId>.<reminderId>.<revision>`; Agent session
identifiers keep their old closed alphabet. The Swift serial queue reconciles
pending/delivered requests only within that namespace, uses one non-repeating
UTC calendar trigger, and reports real asynchronous add completion. Synchronous
desired-version fencing and late-callback removal stop obsolete registrations
from overriding current intent. Past/due recovery never adds another request.
Authorization failure preserves saved intent. Neither title nor project changes
are identity. Notification notes are excluded from the projection entirely.

Responses are closed Open/Snooze actions with an actual response timestamp.
A bounded, typed, private durable `reminder-actions.json` inbox survives the
native-to-WebView startup gap. The loaded Board transaction rechecks card,
identity and revision; old/duplicate snoozes cannot change current time or
resurrect a removed reminder. Commit precedes acknowledgment, so crash replay
is a no-op against the newer revision. Inbox write failure is surfaced and never
reported as a successful snooze. A Snooze that reaches the delegate before the
WebView has asked about its window makes the launch response-only: the window
stays unrevealed, the app is hidden so the application in use keeps the
keyboard, the automatic work `main.rs` deferred does not start, and the
process ends once the inbox is empty and the system has confirmed the request
of every Snooze that launch transacted. Anything else (the Dock, an Open, a
failed save or registration, six seconds without a finish) turns it into an
ordinary visible launch; no path leaves Deck running without a window. Actual
OS callback ordering remains a required platform acceptance item. No response
creates a shell or grants input.

Blocked retirement identities persist as a bounded card field: exact native
server PID/start/session generation for a normal shell exit, or the frozen
source/event/rule tuple for an automation run. They are neither inferred exit
facts nor permanent bans on future generations. Ending Reminder leaves them
intact. Existing buffers, review rules, queue cancellation and MCP admission
remain separate. A failed close may already have stopped a process; committed
card/reminder protection remains and the existing close error reports failure.

Reproduce autonomous checks in a Codex session with the native CUA tool:

```sh
python3 scripts/reminder-verify.py --target auto
```

`--target local|macmini|auto` selects the execution resource. Auto checks the
live local GUI and whether a separate test installation is writable; it does
not infer notification permission or system-action access from that fact.
Those are separately measured in the signed carrier. The remote own-window
path remains available and cannot certify Notification Center actions.

The local path owns `/Applications/Deck Reminder Smoke Acceptance.app` only
when its signed resource owner matches the run manifest. It refuses an occupied
installation. A dedicated `io.c9r.deck.reminder.smoke.local` identity may retain
its legitimate notification permission; every run still queries the actual
status. Its signed Resources configuration retains a new private `/tmp` Board,
process HOME/Agent directories and unique bundled-tmux socket even on a system
notification cold start. No production installation, data or credentials are
used. The application version remains unchanged. After cargo returns, the local
verifier freezes the compiled executable under a uniquely owned private `/tmp`
build root. All carriers copy that frozen file, so concurrent cargo gates cannot
replace later trials' payloads. Cross-trial compiled digests and actual launched
signed digests are checked; per-carrier signatures may differ because their
signed Resources contain distinct private roots. The frozen build root is
removed and verified during finally cleanup.

System UI requests are a bounded agent-driver coordination seam:
`ui-request.json` names the exact test title, app and request identity. The
connected agent uses `cua_repl` to observe/click that actual system object,
then records its fresh observation in the named response file. These requests
are never assigned to a human, and missing access is bounded BLOCKED. The
verifier checks each receipt against a real delegate callback, current Board
revision, actual UN inventory and process start/exit. A receipt alone cannot
pass a native chain. A plain unattended shell without the connected driver
cannot certify system clicks; it must not be presented as a standalone OS
click implementation. Own-window UI controls remain driven by the existing
AppKit smoke seam, not global input or fabricated response handlers.

Foreground setup uses a directed CUA click on the owned window before arming.
Actual pre-arrival and delivered inventory separately measure frontmost state;
unestablished or externally changed foreground control remains BLOCKED while
other native tracks continue. A separately signed, owned AppKit sentinel records
its frontmost samples during another real arrival. No unrelated application is
selected or modified for that control. Live inventory also checks the private
HOME/Claude/Codex environment, including on notification cold starts.

Actual foreground due delivery, running Snooze, normal quit followed by system
delivery, default-click cold start and cold Snooze run three times with new
run IDs/Boards/reminder IDs/sockets. UI minute input uses a legal short future
time. Snooze saves exactly action time plus one hour; the native calendar
trigger has one-second resolution, recorded separately. The inventory-only
LaunchServices observer runs before Board initialization and never projects
requests; its timestamps distinguish delivery while quit from observation
later. Actual delegate evidence survives ACK in the private debug evidence
file, never in a release log. Due rendering precedes Open target focus so a
cold-start render cannot immediately replace the focused card DOM.

A public IORegistry console-lock preflight stores only lock/on-console facts.
An explicitly locked desktop blocks GUI authentication-dependent acceptance; the
verifier never unlocks it or requests user takeover. Unknown lock status does
not prove unavailable access: actual driver probes still run and must succeed. `--inventory-only` attempts
real registration, normal quit, delivery and non-rearming withdrawal independently.
Its configuration clicks are explicitly own-DOM controls inside the real WKWebView;
they use the ordinary Reminder editor/save transaction and are not evidence of
OS UI operability. After delivery it also exercises real editor replacement,
a failed End save, only-in-app withdrawal, End and restart retirement protection.
No callback, UN request or Board file is inserted by the harness.
It retains all required UI scopes as unverified and cannot yield complete PASS.
The test bundle explicitly declares GUI application metadata and registers only
its exact dedicated path with LaunchServices; no global registry reset occurs.

Every trial has bounded observation deadlines. `--fault-mode assert|timeout|cancel`
arms a real private system reminder before exercising finally cleanup. Cleanup
stops exact owned generations/descendants, then launches the same test identity
in a non-rearming withdrawal mode, checks actual pending/delivered inventory,
and removes only its verified installation, socket and private root. Full
path matching covers spaces, `/tmp` normalization and prefix collisions.
History and failures remain in separate evidence directories. Dedicated macOS
permission/LaunchServices records are not deleted through private databases.

Verdicts derive from per-item evidence: FAIL outranks BLOCKED; unknown/unrun
required items, partial readiness, identity changes or failed cleanup cannot
become PASS. A1–E6 parent scopes remain intact when a native subtrack passes.
Physical sleep/wake is separately blocked on a shared local desktop without
exclusive safe recovery authorization; it is not used to mark unrelated native
checks blocked.

Logic/contract gates remain separate:

```sh
scripts/ui-tests
node app/ui/js/check.mjs
python3 scripts/test_reminder_verify.py
cargo fmt --manifest-path app/src-tauri/Cargo.toml --check
cargo clippy --manifest-path app/src-tauri/Cargo.toml --workspace -- -D warnings
cargo test --manifest-path app/src-tauri/Cargo.toml --workspace
```

Negative controls reject stale-action overwrite, deferred retirement after
ending a reminder, and absent/duplicate/pre-exit native delivery evidence.
Fixture success is never promoted to real OS acceptance. The fixed A1–E6
matrix includes every requested item; PARTIAL PASS/BLOCKED/NOT RUN records
cannot constitute complete product PASS. Dedicated notification authorization
records, if created by macOS, are not deleted through a privacy database.

A real cold Snooze is a response-only launch: after its callback the verifier
requires the owned process to end by itself, the committed time to be exact,
and the new request to be the one pending, which it reads through the
inventory-only observer because no Deck is running. The next setup starts the
owned app again as an ordinary launch. This track has not been run since the
change; it needs the connected system-UI driver.

Before subsequent editor and normal-quit commands, the own-window driver
requires actual AppKit active/key-window bits. AX Raise alone does not establish
input routing. These setup actions occur after the preceding observation and
response assertions; they cannot rescue a missing native notification response.

An unavailable public UI observation is retained as a BLOCKED N1 subtrack,
without aborting independent N2–N4 system-action tests. Native delivery does
not certify visual presentation. Required visibility and all original matrix
items remain required; partial evidence can never produce overall PASS.

A console that becomes explicitly locked between tracks blocks subsequent
native UI setup without attempting authentication. Inventory-only execution
keeps its existing own-DOM editor and own-window normal-quit transport and
does not require global foreground activation. A prior failed active-window
assertion under a lock remains in historical evidence, never overwritten.

### Background reconciliation (2026-10-03)

The webview's reconcile is what sets the due latch, refreshes the Dock at a
due instant, tries a failed registration again and transacts a notification
response; the native callback only stores the response. It runs at boot, on
every return to the window, and every 2 seconds while the Board has a
reminder. A Board without one makes no periodic call: the `reminder` WK mode
counts the webview's own `reminder_status` / `reminder_actions` requests for
five seconds after the last reminder ended and requires none.

Whether that tick keeps running in the background was measured on the
isolated Mac mini with a temporary, uncommitted probe in a debug carrier
hidden through the own-window driver for 23.8 minutes (process priority 4,
another application frontmost):

- a 2-second interval fired every 3.0 seconds (median; maximum 3.4), twenty
  times in every minute;
- one-shot timers aimed 1 to 16 minutes ahead were 61 to 202 ms late;
- an event emitted from Rust reached its listener in 3 ms (median; maximum
  8), and a Board transaction started from it committed in 30 ms (maximum
  113), 286 of 286 and 23 of 23;
- an in-app-only reminder due after three hidden minutes showed in the Dock
  one second after its instant and had its latch committed within two.

So in that state (a window that was shown and then hidden) timers are slowed,
not frozen: the tick that transacts a background response keeps running every
three seconds. The probe ran on to 38 minutes with the same cadence (twenty
ticks in every minute, 457 of 457 events, 38 of 38 transactions). A window
that was never shown is a different state; see the by-hand check below.

#### Follow-up tests owed

Test 1 ran by hand on 2026-10-04 and passed (next section). Tests 2 and 3 did
not run; each stays open until the named evidence exists.

1. **A response answered while Deck is in the background** (end to end). No
   authorized channel clicked a notification, so this is BLOCKED; only the
   tick behind it was measured. By hand, in an isolated carrier with
   notification permission: set a reminder one minute ahead, hide the app
   (Command-H), choose **Remind in 1 hour** on the banner without bringing the
   app forward and wait ten seconds. Pass: the carrier's `deck.json` has the
   reminder at revision plus one, `dueAt` equal to the click time plus one
   hour and `due` false, and the system holds a pending request for that new
   revision. Done by hand on 2026-10-04: passed, the Board was written 1.2
   seconds after the answer while the app was hidden.
2. **Deeper background states**: a locked screen, display sleep, and hours
   hidden. Repeat the cadence measurement (or test 1) in each. Pass: the tick
   keeps running, or a response is transacted within a stated bound.
3. **A withdrawal the notification center did not answer.** When the three
   reads in `deckReminderProject` do not return within 5 seconds, the
   projector returns without removing obsolete requests. While the Board has
   a reminder the 2-second tick asks again. When the request that should go
   belonged to the LAST reminder there is no tick: nothing asks again until
   the next window focus, visibility change, Board save, wake, clock change
   or restart, and the ended reminder can still be delivered in between. This
   cannot be reproduced unattended today; it needs a debug-only fault that
   makes one projection's reads time out. Owed, with that fault: end the last
   reminder, assert through the carrier inventory that its request is still
   pending, then assert it is gone after (a) a window focus and (b) a Board
   save. Decide from the result whether the projector should try again on
   its own.

### By-hand notification check (2026-10-04)

No agent UI channel can see Notification Center, so a person answered real
notifications. The carrier was the dedicated local one described above (same
path, identity and signed launch file), built from `6ce8ed5`. A scratch
driver steered only Deck's own window through the `reminder-native` smoke
commands and read the Board file, the carrier's notification inventory and
the native delegate's evidence file; it clicked no system object and injected
nothing. An ad-hoc signed carrier cannot do this: the notification center
refuses its permission request (`usernoted` fails to validate the client).
This is evidence from a person. The verifier's matrix is defined for
unattended agent clicks and is unchanged by it.

Each answer below was a real click on a real banner:

- running, window visible, not the frontmost application, **Remind in 1
  hour**: revision plus one, `dueAt` equal to the answer plus one hour, due
  latch cleared, the old request replaced by the new one; the Board was
  written 1.3 seconds after the answer;
- running, hidden with Command-H, **Remind in 1 hour**: the same, 1.2
  seconds (owed test 1 above);
- running, the frontmost application when the banner arrived, **Remind in 1
  hour**: the same, 0.15 seconds;
- not running, **Remind in 1 hour**: the system started the carrier without
  arguments, it stayed in its private environment; the same, 0.2 seconds;
- not running, a click on the banner itself: cold start, the stopped due card
  focused on the Board, the reminder unchanged, no session started.

Not covered: sleep and wake, a locked screen, hours in the background.

What a cold Snooze leaves behind, from four of them:

- about 0.1 seconds after launch the system makes Deck the frontmost
  application (four of four), while the background-launch guard keeps its
  window unrevealed. The application the person was using stops receiving
  keystrokes until they click one of its windows (confirmed by typing). A
  Snooze answered while Deck is already running does not change the
  frontmost application;
- about 7.5 seconds after launch the system suspends the webview's content
  process (the three that were then left alone). No interval runs while it
  is suspended;
- a later Snooze, for a second card two minutes after the first, was still
  transacted 0.37 seconds after the answer: the content process was resumed
  when the response arrived and the overdue interval ran. That is one
  observation, with Deck still the frontmost application.

These observations describe the build that was tested. Since then a cold
Snooze is a response-only launch that ends (see the Responses paragraph
above), so Deck no longer stays in that state; the change itself has unit
tests and still needs a click on a real banner to be confirmed.

### Evidence evaluator closure (2026-09-30)

`aggregate` evaluates the closed Reminder subassertion list in
`scripts/reminder-verify.py`. Every A1–E6 parent retains its permutations and
lists `remainingAssertions`; N1–N6 are views of the same assertions, not extra
executed tests. No parent or native view has a fixed PASS/PARTIAL verdict.
Named WK checks cover their stated subset. Gate totals cannot certify an
individual assertion. The legacy adapter deliberately leaves permutations
without a precise named result unverified, even when an earlier parent summary
said PASS. Raw earlier summaries, successful observations and failures remain
unchanged and referenced separately.

Receipts bind an assertion, required layer, source digest, environment and
run ID to hashed evidence files. Runtime receipts also require the recorded
compiled and actually launched signed identity. Missing files, changed hashes,
unexecuted PASS claims, other candidates and wrong layers are excluded with a
reason. Duplicate run IDs cannot fill the three required repetitions. Complete
critical chains require the intersection of three independent run IDs in one
environment; partial WK and native runs cannot be stitched into complete chains.
A valid failure outranks unavailable coverage, including success for the same
run. Original execution failures are also retained by offline evaluation.
Intentional negative controls retain their raw CLI FAIL and certify only their
explicitly checked cleanup assertions.

The report separates four scopes:

- `core`: A1–E4 product assertions, including actual system visibility, default
  click/cold start, exact system Snooze, authoritative Board handling and no
  additional shell/Agent generation. Unknown isolation blocks certification.
- `platform`: E5 physical sleep/wake. `safeSleep` is a platform prerequisite
  observation, excluded from core readiness. An actual sleep/wake receipt is
  required for platform PASS. E5 is never deleted, waived or made inapplicable.
- `cleanup`: E6 normal/assertion/timeout/cancel/SIGINT owned-resource checks.
- `sharedSafety`: E6 ownership/action audit and the separate final observation
  of any OS-generated background-running notice. App-owned empty UN inventory
  cannot establish the disappearance of that system notice.

Each scope has its own PASS/FAIL/BLOCKED verdict. Full PASS requires all four;
core PASS remains reachable when all core evidence is sufficient, even if the
separate platform verdict is BLOCKED. A prerequisite flag does not replace a
product assertion. `isolationVerified`, `cleanupCompleted` and
`sharedResourcesSafe` are independently evidenced true/false/null values.
Null means UNKNOWN, not a safety failure and not verified safety. Isolation
proof needs the per-run private paths/socket manifest, actual launched identity
and actual private HOME/Claude/Codex runtime checks. Creating a carrier is
insufficient. Cleanup proof covers only its recorded resources and observation
time. Shared safety needs its own action/resource evidence and cannot be inferred
from successful cleanup.

Read existing evidence without launching applications or performing system UI:

```sh
python3 scripts/reminder-verify.py \
  --evaluate-saved /absolute/path/to/original/unattended-acceptance.json \
  --evidence /absolute/path/to/new-evaluation-directory
python3 scripts/test_reminder_verify.py
```

Offline output must use a different, previously unused report directory. The
reader supports the existing local report, remote WK report and the saved local
closure layout (`locked-native-final`, `locked-wk-final`, named ac00 gates and
negative cleanup controls). It stores original verdicts/matrices, raw trial
references and historical failures. `testedCandidateDiffDigest` identifies the
old runtime candidate; `evaluatorDiffDigest` identifies the current tool/docs
revision. Re-evaluation is evidence analysis, not a fresh runtime test of the
changed toolchain. Unit fixtures prove evaluator behavior only and never enter
product acceptance receipts.

The production candidate actually tested before this closure had source digest
`ac00e592e4d465ce2e4aa10f6928c53b702ae1ac78d0b7e61fcfc8e90dedfbee`.
Its three real quit/delivery inventory runs and three remote WK runs remain
valid within their recorded scopes. None establishes three complete final
system-click/Snooze closures. Full acceptance remains BLOCKED. This evaluator
closure changes no production functionality and performs no system UI reruns.
The historical lock blocked foreground/input setup and notification inspection;
the separate tool policy rejected access to `com.apple.UserNotificationCenter`.
Neither warrants unlocking, changing lock settings, user takeover or using an
alternate entry point to evade the rejected operation. Future supplementation
requires an already lawful, operable public UI channel and a usable isolated
GUI resource; E5 additionally requires exclusive safe sleep/wake recovery.
No future run is scheduled by this closure.
