# Changelog

## Unreleased

- Away notifications and a Dock badge, off by default. With **Notify me
  when away** on (Settings → Integrations & automation), macOS shows a
  notification when an agent reports needs-input or an unread turn ending
  while the deck window is not in front; the Dock icon counts needs-input
  plus unread endings. The notification carries the card title, the
  project name and one of two fixed phrases, nothing else, and clicking it
  opens the card. The decision is made in the backend as each hook event
  arrives, so App Nap freezing the webview does not delay it. Permission
  is asked once when the switch is turned on; a blocked state is shown,
  never worked around. See `docs/notifications.md`.
- Internal: the `minisign` that verifies and signs updater archives in the
  Nightly and promotion workflows is a fixed official release checked
  against a committed SHA-256 (`scripts/install-minisign`), no longer
  whatever Homebrew served that day.

## 0.7.9 — 2026-09-24 (Nightly)

- Installing an update can no longer ask for an administrator password or
  start a shell. The updater plugin's own installer runs an AppleScript
  "with administrator privileges" when deck was installed by an
  administrator and the current user cannot write to it, and spawns `touch`
  after every install — the process behaviour deck promises never to show.
  deck now checks, without starting any process, that the app and its
  folder are writable by the current user, refuses with a sidebar message
  otherwise (reinstall the DMG), and after the plugin has downloaded and
  verified the archive swaps the bundle itself. Nothing changes for the
  ordinary single-user install.
- An agent status event is now accepted only from inside the pane it names.
  The status socket took any line that named a public source, the tmux
  server pid (readable from `$TMUX` in every pane) and a pane id, so a
  program in one pane could report `turn-done` for a sibling card and have a
  "close the card" automation retire it, or paint another card's attention
  state. deck now reads the connecting process from the kernel and walks
  its parents; the pane's own process must be in that chain (the bundled
  status helper stays connected until deck has read it). A program can still
  misreport its own pane, which is no more than exiting would do.
- A program running in a pane can no longer write or read the macOS
  clipboard through the terminal. deck's tmux server ran with
  `set-clipboard on`, so any output containing an OSC 52 sequence (a file
  shown with `cat`, a commit message, an agent tool's output) silently
  replaced the clipboard, and could read the buffer another card's program
  had pushed. The server now runs `set-clipboard off`; ⌘C is unchanged (it
  never used OSC 52) and the unused xterm clipboard addon is gone.
- Quitting Deck with SIGTERM, SIGINT or SIGHUP now goes through the normal
  quit path. Before, an abrupt exit could leave the Board's tmux query client
  running forever, still attached to the shell service. Deck now also removes
  such a leftover client (left by a crash or force quit) the next time it
  connects.
- A new card whose project has a default command (for example `codex`) failed
  with "could not safely create a session" whenever no terminal pane was open,
  and left an invisible tmux session behind each time. Since 0.7.1 the Board's
  persistent tmux query client attached read-only; while it was the only tmux
  client, tmux refused Deck's `send-keys` ("client is read-only"). The query
  client is no longer read-only, which also fixes list and automation
  deliveries whose text landed but whose Enter was refused in the same state.
  A start that fails after creating its pane now removes that session and logs
  a code-only `[start] failed` line.
- A list row never answers an agent's permission prompt. Chain rows fire on
  quiet, and a permission prompt counted as quiet, so a Slack channel rule
  could stage a message that provoked a dangerous tool request and have the
  next row's paste-and-Enter accept it. The tick now reads the agent hook
  state: no row is sent automatically while the hook reports needs-input,
  and a row admitted from outside (Slack channel or badge rules, Connector)
  waits until the hook positively reports turn-done — without a hook it
  waits for a manual send. The list panel shows the new "agent" stage.
  Cards never move.
- Phone Connector stops listening when the network under its address
  changes. The private-network check ran only at enable; the listener now
  records address, interface and netmask, re-checks them every 5 seconds,
  stops on any change, and drops a connection whose local address is not
  the configured one. Settings shows the existing "enabled, not listening"
  state. 100.64/10 is accepted only on VPN (`utun*`) interfaces and
  169.254/16 only on direct-cable bridges (`bridge*`). Phone input passes
  the MCP terminal-input guard, and phone output refuses any pane under MCP
  control.
- A same-user process can no longer starve the managed MCP runner's control
  slots or hold a control-socket slot by trickling a request: the runner
  closes any connection that is not Deck's before it takes a slot, the
  whole control request must arrive within 500 ms, and a job that hits its
  execution timeout is stopped with the same SIGINT/SIGTERM/SIGKILL
  escalation and reports the signal that ended it. Project reads open the
  root one component at a time without following symlinks and refuse a
  root or ancestor that was swapped for a symlink.
- The optional Deck Tunnel Helper is covered by the EDR-quiet gates and is
  published by the nightly workflow as its own independently notarized
  artifact. Each helper command has one budget below Deck's kill timeout,
  the runtime key file is removed on SIGINT/SIGTERM/SIGHUP, stale runtime
  directories of dead owners are swept at start, and key buffers are
  zeroed.
- Internal: the release gate's coverage number no longer counts inline
  test modules as covered production code (the module splits below moved
  them into `tests.rs`); the honest figure was 73.66% lines / 72.64%
  functions and is 82.22% / 80.60% after a round of behaviour tests over
  the least-covered paths. `mcp.rs` is a directory module with one file per concern; the
  bounded state-document mechanism shared by MCP, Connector and the channel
  inbox is `ledger.rs`; the tmux server restart consults MCP through a
  guard MCP registers at boot; `tests/session_architecture.rs` pins these
  layers. The one visible change: the MCP oversized-state message says
  "bounds" like the other two. The tmux contract suite runs under the
  production client topology, every path that puts non-owner text into a
  queue or pane is pinned (`tests/external_admission.rs`), and coverage
  floors are 75%.

## 0.7.7 — 2026-09-23 (Nightly)

- Optional Secure Tunnel helper. Deck MCP can be reached through ChatGPT's
  Secure Tunnel via a separately installed, signed Deck Tunnel Helper
  (`deck-tunnelctl`). Deck MCP works without it, Deck never holds its OpenAI
  credential, and removing it changes no MCP authorization. Deck runs the
  helper only at its fixed `/Applications` path after checking its signature
  and file identity, through a closed status/start/stop/remove protocol;
  Settings shows "optional helper not installed" when it is absent. See
  `docs/mcp-tunnel-helper.md`.
- MCP reports a session's effective output-sharing state.

## 0.7.6 — 2026-09-22 (Nightly)

- Slack badge rules now follow the channel admission: the command must be
  exactly `claude` or `codex` and no template row may start with a message
  placeholder. A badge rule with an empty (plain shell) command could
  previously paste someone else's Slack message into zsh and run it; such a
  rule is now shown as blocked and its badges are skipped until it is edited.
- Deck now verifies, through kernel peer credentials, that a managed MCP
  runner socket is served by the tmux pane's own process before it sends the
  runner key, so a pane job can no longer move the socket aside and collect
  the key.
- MCP execution now uses one structured executable plus exact argument vector
  (`deck_exec`). Execution approval explicitly permits any program, including
  interpreters and shells, with the logged-in user's permissions; it is not a
  sandbox. The misleading shell-denial boundary and `deck_shell_exec` fallback
  were removed. Legacy state-schema-v6 `allowShell` fields are ignored.
- The MCP control socket is bound only while the feature is enabled and its
  thread idles while disabled; every runner control request is
  authenticated, runner sockets are published atomically, control epochs
  resynchronize after a fence, executables must be absolute paths, and the
  runner holds no timer while idle.
- Phone Connector output requires a live agent in the pane's foreground;
  snapshots and every card route are limited to agent cards; paired device
  names are sanitized; phone rows are admitted through the same channel
  path as Slack rules.
- External messages (Slack rules, phone) that lead with `!` or `/` are
  refused.
- Slack channel monitoring stops re-reading the Keychain while a token is
  missing.
- Fixed cross-card shell recovery after a tmux server restart.

## 0.7.5 — 2026-09-21 (Nightly)

- MCP and Phone Connector requests can no longer be applied twice after Deck
  drops their history. MCP control and session-create calls now carry a
  sequence value (control protocol 4, state schema 5), renewals are journaled
  and never extend a lease on replay, and release/request always fit in a
  full journal. Phone commands carry a device sequence that the host checks on
  every POST (journal format 3); older phone builds are asked to update.
  A phone command saved by an older build and still pending gets its sequence
  only when the host confirms it never arrived; an operation whose outcome the
  host no longer keeps is marked expired and never sent again.
- Revoking an execution window now also stops exec and stdin requests that
  were already past their first check, and remote closes re-check takeover,
  client revocation and disable before admission. Session inspect reports the
  window as revoked from that moment, not only once the revocation is saved.
  Revoking an execution window no longer pauses output sharing: the output of
  jobs that already ran stays readable while sharing is on.
- The phone app's local command journal is now version 3, because it can
  record an operation as expired. Earlier journals open unchanged and are
  upgraded on their next save; an older phone build refuses the new file
  instead of misreading it.
- Return to MCP no longer requires a live execution grant: it persists a new
  epoch first, re-fences if the runner refuses, and reports stable codes
  (busy, runner-stale, client-revoked, feature-disabled). A runner left by
  an earlier Deck answers runner-stale; close still works. Human takeover
  starts no shell: keys go to the active job and Ctrl-C stops it. The
  runner handles INT/HUP/TERM, reaps its own jobs, reports stopped jobs, and
  a bounded stop (INT, TERM, KILL) runs before a card closes. A remote close
  commits after admission and never leaves `closing` stuck; a restart turns
  pending Board operations ambiguous instead of replaying them.

## 0.7.4 — 2026-09-21 (Nightly)

- Fixes for the MCP acceptance blockers found in review.

## 0.7.3 — 2026-09-21 (Nightly)

- Fixes to MCP client authorization management.

## 0.7.2 — 2026-09-21 (Nightly)

- MCP security gates (scheme B) complete.

## 0.7.1 — 2026-09-20 (Nightly)

- Opt-in MCP terminal control (Settings → Integrations). A locally
  authorized MCP client (for example ChatGPT through the `deck-mcp` STDIO
  adapter) can read files of the projects it was authorized for, create
  managed sessions, run one structured executable per job through the
  signed `deck-mcp-runner` in a visible tmux pane under a short-lived
  execution grant, send stdin and read output. Structured reads start no
  shell; execution runs as the logged-in user and is not a sandbox; the
  human can take the keyboard back at any time. See `docs/mcp.md`.
- The Board's tmux query channel is persistent instead of being reopened
  on every poll.

## 0.7.0 — 2026-09-20 (Nightly)

- Card scratchpads: per-card notes and immutable copies of what was
  delivered.
- Slack channel monitoring: a saved channel rule collects scoped Slack
  messages into its collection card, with deterministic grouping and
  durable deduplication.
- Phone Connector host (disabled by default): opt-in HTTPS pairing on a
  private address, device revocation, project task presets and a restricted
  set of remote commands through the existing Board writer and guarded
  scheduler, plus an iOS client with durable command recovery. Remote input
  boundaries are hardened. See `docs/connector.md`.
- The terminal suggests agent resume IDs from pane history.
- Failed session polls preserve running cards and queued prompts instead of
  treating an unavailable listing as exited sessions. Invalid working-directory
  metadata no longer hides a live session. Polling and Slack credential
  verification run off the UI thread, with a deadline on polling tmux calls.
- Credential redaction covers complete quoted passwords and Authorization
  headers. Long-line scanning is linear, and shell recovery omits oversized
  logical lines before scanning instead of retaining partial secret values.
- Voice input keeps each pending slice bound to its original recording and
  pane. Switching or cancelling during preparation drops the slice, and late
  replies cannot reset a newer recording. Voice paste now excludes service
  restart through transport and cleanup.
- Shared session guards and deadlines no longer depend on restart policy.
  Terminal links, clipboard handling and byte codecs have dedicated modules;
  the queue panel receives its board dependencies during initialization.

## 0.6.10 — 2026-09-19 (Nightly)

- Agents are asked to exit gracefully before a bounded shell service
  restart.
- Terminal path links separate Chinese/Markdown labels from ASCII brackets,
  preserving balanced filename brackets. Rejected candidates and URL closing
  punctuation are scanned without repeatedly traversing the same suffix.
- A path click survives an unchanged row repaint between press and release.
  Changed targets, viewport/grid changes and drags cancel the click. Link
  diagnostics record numeric attempt IDs, closed outcomes, action durations
  and rate-limited slow scans; paths and terminal text remain private.
- Held double-click word selections and triple-click line selections now extend
  through a drag without losing the original range or selection granularity.
  Reversing the drag keeps the original selected word/line intact.
- Terminal copy reports when no text is selected, including a selection lost
  while copying, and leaves the clipboard untouched for empty snapshots.
  Repeated copy attempts share a short notification cooldown.
- Copy and selection diagnostics carry numeric run, pane, selection and attempt
  IDs. Pointer cancellation records focus and highlight hit state; empty ranges
  record cell deltas. Duplicate key-capture and copy-labelled drag errors are
  retired. No selected text or session names enter these events.
- rustls is patched (RUSTSEC-2026-0285) and the RustSec audit gate runs
  before signing; the withdrawn 0.6.9 candidate was advanced to 0.6.10.

## 0.6.8 — 2026-09-16 (Nightly)

- Voice input types straight into the session. The header microphone starts
  and stops a recording; text the on-device recognizer has confirmed is typed
  into the focused pane while you speak, like keystrokes, never with Enter,
  and a translucent caption under the button shows what has been heard but
  not yet typed. The draft panel with its placements, Clear, Insert only,
  Send and retry flow is gone; keyboard and voice share one input. Switching
  sessions, leaving the view or a pane's exit ends the recording. Stopping no
  longer needs repeated clicks. `app.log` notes which speech engine ran.

## 0.6.7 — 2026-09-14 (Nightly)

- A finished terminal selection survives switching to another app or hiding
  the window, so ⌘C still copies it on return; only a drag still in progress
  ends. A drag released back in its starting cell (or across one wide
  character, or past the end of a line's text) is an empty range and now ends
  quietly as a click instead of a "selection ended" toast.
- Diagnostics record the life of a native double/triple-click selection
  (`native-select`, `native-end-<reason>`) and `cancel-empty`, so a ⌘C that
  finds nothing to copy can be attributed from a log export.

## 0.6.6 — 2026-09-11 (Nightly)

- Voice drafts and scheduled prompts pass text to tmux through stdin instead
  of process arguments, preventing command-line auditing from collecting it.
  Guarded paste, buffer cleanup and explicit retry behavior are preserved.

## 0.6.5 — 2026-09-11 (Nightly)

- Voice input reports disabled macOS Dictation with directions to enable it in
  System Settings → Keyboard, instead of a generic recognition interruption.
  Wrapped native errors retain this guidance; drafts stay available for retry.
- Failed recording setup opens the relevant macOS microphone, Speech Recognition
  or Dictation settings, with a button to reopen it. Release builds require the
  modern speech toolchain; CI and nightly now select Xcode 26.3 explicitly so a
  locally tested `SpeechAnalyzer` path is not silently omitted from distribution.

## 0.6.4 — 2026-09-11 (Nightly)

- Voice drafts recover from a replaced session on the next explicit Insert/Send
  action, without automatic retransmission. Uncertain deliveries keep their
  original target and must be checked and cleared before using a replacement.
  Recorder and preference UI ownership are split into focused modules; tests
  now exercise production voice controls and shared literal delivery. The UI
  coverage gate rejects eligible modules missing from its report.

- Voice language preferences in Settings → Terminal & sessions: Chinese,
  English and Japanese by default, with configurable enabled languages and
  recognition default. One enabled language hides the draft selector. Settings
  persist across restarts; existing session choices stay if enabled, and removal
  waits for active recording to finish. No automatic language detection.

- Native on-device voice drafts in the session toolbar: Bottom by default,
  with Floating and Right placements sharing the same recorder and editor.
  Drafts, recognition language and target bindings follow each session, so
  switching sessions restores its draft without rebinding. Switching during
  recording releases capture and keeps the original session’s displayed text.
  Placement and unsent drafts last only for this app run. Clear, Insert only
  and Send also work during recording: non-empty text stops capture and acts on
  the visible draft; empty text leaves recording untouched. Review before sending; target changes and uncertain deliveries never auto-retry.
  Manual target confirmation is removed: each send checks the session’s current
  foreground program. Transient refusals retain the binding; a replaced session
  can be rebound on the next explicit action. Uncertain delivery retains its
  original target until the result is resolved or the draft is cleared.
  Swift is linked into the app with microphone purpose strings and entitlement;
  no speech helper process, service registration or cloud fallback. See
  [voice input](docs/voice-input.md) for system requirements and validation.

- Restore session split buttons and shortcuts, and correct cross-screen terminal
  selection so the full dragged range follows the copy-mode snapshot.
- Extract DOM-free scheduler and automation models, confine import cycles to
  the view core, and remove obsolete queue styles and translation keys.

## 0.6.3 — 2026-09-10 (Nightly)

- **Inspect after every row.** A list or an automation rule can explicitly
  ask deck to stop after every delivered row, including the last. The sent
  row stays as a durable checkpoint until you view the result and choose
  **Inspected, allow next…**; a turn-done hook, quiet time, an open pane or
  a permission wait never release it. The confirmation names the next row
  and permits only that revision and target: editing, retrying or removing
  it, a changed target, or toggling the mode revokes unused permission. The
  next row still obeys its schedule, minimum gap and context checks; other
  lists in the session interleave as before. A last-row confirmation is the
  extra condition an opted-in automation needs before its finish=close path.
  Skipping or cancelling omits work and never counts as inspection. Existing
  lists, rules and runs are unchanged; a reviewed template is enqueued in one
  transaction.
- The ⏱ panel shows a read-only execution plan (the backend's selection
  stage, gap or quiet remaining at observation; stale reads as unknown) apart
  from hook observations, and a ledger of deliveries and inspections (ids,
  times, source; 200 each; no prompt text). "Delivery uncertain" and
  "acknowledge as sent" replace the ⚠/✓ wording. Docs no longer claim that
  every row waits for quiet, that automation runs start with an empty
  context, or that three finish polls prove a turn belongs to the last row.
- Data: queue.json and settings.json move to a sticky schema v2 envelope once
  they hold inspection state, so an older deck refuses them untouched instead
  of resending sent rows; deck.json stays v1. Back up before installing this
  candidate if you may return to an older build.

## 0.6.2 — 2026-09-09 (Nightly)

- **Needs attention** in the sidebar: one view across projects of the
  sessions asking for input and the turns that finished unread, with a
  filter row and an Open (or Locate) per row. A turn counts as read only
  once its pane was really attached and displayed; a failed attach, a
  stale snapshot or a stopped session leaves it pending. Boards stay manual
  groups: nothing moves, the view is a runtime projection that marks its
  rows stale when a poll fails or a session is missing, and returning from
  a card lands on the same filter, scroll position and row.
- One persistent Board action. The head keeps only **New session ▾**; its
  menu holds new session now, new on a clock, new on a Slack badge (the
  drawer opens preset to that trigger) and the two managers; a ↻ chip shows
  only while the project has rules. Templates… opens from that menu, a
  list's 📋, the automation editor and the project tab menu, and focus
  returns to the opener. A project without cards shows one starting point
  above its groups instead of per-column attention text.
- Vocabulary: 看板 is the page and 栏目 (Group) the column; session stays
  untranslated in Chinese; a split is 窗格; the automation editor's second
  row is When. Status tooltips no longer claim readiness or waiting for
  input.
- Review fixes for the above: a click on a pane whose shell exited only
  focuses it (it no longer re-attaches a dead session or restarts a card
  mid-retirement); a shell that exits before its attach reply lands leaves
  the pane detached and unseen; a poll requested while one is in flight
  runs again afterwards, so an exit or an Open never acts on a stale
  snapshot; keyboard focus survives an attention-row reorder; dismissing
  New session ▾ by clicking elsewhere or Escape no longer leaves its
  keyboard handler on later menus; the project tab menu returns focus to
  the tab; a tab's done-dot tooltip follows freshness; the split picker's
  new shell lands in the selected group like ⌘N.
- Diagnostics whose findings landed are retired: the paste-chain trace,
  clipboard success lines, the separator trace and per-emit PTY lines.
  app.log carries three lines per attachment instead of five; selection
  forensics stay while that problem is open.

## 0.6.1 — 2026-09-08 (Nightly)

- Two concepts for "later". A card's ⏱ panel now holds **lists**: prompts
  sent in order, each row once the session has been quiet for the row's own
  time. A list has two optional fields instead of a three-way mode picker —
  **not before** a date and time (a past instant is refused, never rolled
  to tomorrow; it is also a repeating list's start, so the separate "from"
  is gone) and **repeat** (every 5 min to 4 h, a daily window, until you
  remove it / N times / an instant). A row is added from the list's own
  footer and joins THAT list (`queue_add` takes the list; before, a
  follow-up always joined the newest group); a repeating list's rows can be
  added, edited and removed (`queue_update { steps }`). 📋 on the panel
  starts a list from a template; 📋 on a list inserts one or saves the list
  as one — the three template buttons became that one menu.
- **Automations** have a trigger: a clock, or a Slack badge. The Slack rules
  that lived in Settings › Auto-respond are the same rules, now edited in
  the project's ↻ Automations drawer beside the clock ones, with the same
  finish mode — a badge-started card is closed by "close the card" too, and
  its runs show in the drawer. Settings keeps only the Slack connection
  (switch, create-app link, tokens); "Auto-respond" is no longer a name in
  the app. A rule whose project is deleted goes with it, whatever its
  trigger. "If missed" offers two choices — still start within 15 minutes,
  or the same day; a saved value outside them stays offered so an edit never
  silently rewrites it.
- deck draws its own dropdowns. Every `<select>` — the list form, the
  automation editor, Settings — opens deck's listbox instead of the macOS
  system popup, styled like the rest of the app and keyboard-complete
  (↑↓ Home End, Escape returns focus and no longer leaves the session). The
  native select stays underneath as the single source of value and events,
  so nothing else changed. The automation editor's Slack badge has its own
  labelled row instead of wrapping under the trigger, and its inputs share
  the dropdown's metrics.
- Vocabulary: list, row, not before, repeat, template, automation, trigger.
  queue / group / step / chain / rule / source / inbound / auto-respond left
  the UI, the docs and the site. `queue.json` and `settings.json` are not
  rewritten: an "at" item shows as a list with "not before", an "every"
  item as a list with "repeat", a Slack rule where it always was.

## 0.6.0 — 2026-09-08 (Nightly)

- Board-level automations. A daily, weekly or monthly job is no longer
  something to pin on one card: **↻ Automations** on the Board head opens a
  drawer for the project's rules — a schedule (every day, chosen weekdays or
  days of the month, at a local time), the column to create in, directory,
  command and template. At each slot deck creates a fresh card, launches the
  command and queues the template, so every run starts with an empty agent
  context; a slot that comes due while the previous run is still on the
  Board is skipped and recorded. With “close the card” the run is retired
  once every prompt is delivered and the agent reports its turn done (or the
  program exits). The drawer shows each rule's next slot and its last runs,
  and a run's card carries a chip pointing back at its rule. Automations are
  auto-respond rules with a clock as their source, so a deleted project takes
  its rules with it and the run ledger keeps identifiers and times only.
- A card's schedule sets its own quiet time: “after previous” waits the
  number of seconds, minutes or hours you choose instead of a fixed three
  minutes. “At a time” takes a full date and time and refuses one that has
  already passed instead of silently rolling to tomorrow. A recurring rule
  can start at a date and time and stop at one. The panel picks the mode
  first (after previous / at a time / repeat) and shows only that mode's
  controls.

## 0.5.18 — 2026-09-07 (Nightly)

- Dragging a selection over many rows is no longer sluggish. tmux repaints the
  whole selected region after every single motion it is given, and deck
  re-placed the copy cursor from the top of the frame on every pointer move:
  one full-screen update pushed about 19 KB down the terminal, 438 KB for a
  drag of sixty moves. Those repaints drew cells that already looked right —
  deck paints its own selection overlay. A drag now leaves tmux holding only a
  cursor and builds the selection once when the button is released; the same
  drag costs 11.7 KB. A pointer move that stays inside one terminal cell sends
  nothing at all.
- The highlight no longer covers text the pointer never crossed. tmux pins a
  selection anchor to the text while its cursor stays on a screen row, so on a
  card that was still printing, the frame scrolled between reading it and
  placing the endpoint and the selection walked onto other lines. Both
  endpoints are now tracked as content rows, and the two ends plus the
  selection are placed in one tmux command list that the server runs in a
  single pass — nothing can move the frame between them. If output does race
  the reading the placement was built from, it is rebuilt rather than trusted.

## 0.5.17 — 2026-09-07 (Nightly)

- Prompt templates and scheduled prompts hold MANY LINES. A step used to be
  flattened to one line on the belief that a newline submits the prompt early;
  delivery pastes inside bracketed-paste marks and presses Enter as a separate
  key, so only a carriage return does that, and a CR is now the one byte
  rewritten. In every prompt field Enter types a newline and ⌘↵ commits. A
  template row and a queue row show the prompt's first line plus a `⏎N` badge,
  and one row at a time opens in place to the whole text, so a long chain
  still fits the panel and stays scannable.
- Auto-respond keeps a template step's own lines. The message pasted into it
  is still flattened to one line, so an inbound post cannot reshape the prompt
  built around it.
- Terminal path links understand Chinese and Japanese prose. CJK writes
  sentences without spaces, so a line offered the tokenizer no boundary at
  all: `已修改 src/main.rs。` linkified the full stop, and
  `请看 app/ui/js/pure.js，然后运行测试。` linkified the rest of the sentence with
  it. Punctuation is now matched by Unicode range rather than by a list — the
  first enumeration still missed ——, →, “”, ～ and ※ — and a CJK character
  against an ASCII letter bounds a path the way a space bounds an English one.
  Marks that are word characters (々, ・, ー) are deliberately not separators,
  so `佐々木.txt` and `データ・ベース.txt` stay whole.
- A path link whose start had to be guessed retries with the wider reading
  when the first one does not exist, so a name that genuinely runs CJK into
  letters (`报告v2.pdf`) still opens. Hover is unchanged: it answers from the
  text alone, with no round trip and no flicker.
- Internal: the `link-classify` WKWebView check asserted the path-existence
  filter that 0.5.16 deliberately removed from the link provider, and had been
  failing since. It now asserts what shipped.

## 0.5.16 — 2026-09-06 (Nightly)

- Card memory now reports physical footprint (`ri_phys_footprint`, the
  number Activity Monitor shows) instead of summed RSS. Summing resident
  size over a pane's process tree counted the dyld shared cache and every
  shared binary's text once per process, so an agent session with its
  daemon, pre-warmed workers and mcp servers showed ~3 GB for ~1.3 GB of
  real memory.
- Terminal path links no longer flicker under a repainting TUI: the link
  provider answers synchronously from the text alone and the link actions
  validate the path when it is opened.
- Security: deck executes only the signed tmux sidecar next to its own
  binary — no Homebrew/MacPorts/PATH fallback (`/usr/local/bin` is
  user-writable on many Macs). A build without its sidecar reports
  `TmuxMissing` instead of borrowing a foreign tmux. CI runs `cargo audit`
  against the committed lockfile.
- Internal: one `PaneRow` format and parser in `tmux.rs` serves every pane
  probe (a tab in a directory name no longer truncates the poll's cwd); one
  locale resolver (the `defaults read` spawn is gone); one shell-name list;
  unused `.lproj` resources and duplicated module-header contracts removed.

## 0.5.15 — 2026-09-06 (Nightly)

- Faster new shells under endpoint security: `start_session` no longer
  re-applies the ten server defaults per session (`-f tmux.conf` covers
  every server this build spawns; the once-per-boot reconcile still covers a
  server left by an older build), cutting ~16 tmux execs to ~6. app.log now
  records per-phase milliseconds for each created session (`[start]`) and
  attach-to-first-byte (`[pty] first emit … after Nms`), so a slow "new
  shell" can be attributed to deck or to the login shell's rc files.
- Build: `tauri build` works again on a clean checkout — the CLI requires
  `frontendDist` to exist before cargo runs, so `beforeBuildCommand` creates
  the `ui-dist` directory that build.rs then stages (regression from 0.5.14's
  ui-dist staging; only `app/run.sh` had been exercised).
- Internal (tech-debt round, 2026-09-06): `SMOKE.md` is a live checklist
  again (run logs archived) and the `ime-routing` regression is fixed —
  composing Command chords reach only the zoom actions; release bundles no
  longer ship `ui/test` (build.rs stages `ui-dist`); one cargo workspace
  with the status helper under `src-tauri/`, toolchain pinned by
  `rust-toolchain.toml`; `storage.rs` split into `datadir`, `applog`,
  `redact`, `instance_lock`, `launch_args` with no module cycle; one shared
  Board fixture pins the persisted schema in JS and Rust; selection and
  terminal-input logic that needs no DOM moved to `pure.js` with tests; one
  `DeckError` type with a closed `ErrorKind` replaces `Result<_, String>`;
  there is no `From<String>`, so every message names its kind at the call
  site and only foreign text (tmux stderr, library errors) is classified.

## 0.5.14 — 2026-09-06 (Nightly)

- Internal: split `commands.rs` and `scheduler.rs` into focused modules; move
  each subsystem's contract from CLAUDE.md into its module header; replace
  source-grepping tests with behavioural seams (`restore_start_args`,
  `tmux_conf_text`, the frontend event-label contract, `tests/edr_quiet.rs`);
  frontend runtime slots become an explicit `ctx` object and DOM wiring runs
  at boot; the stylesheet moves to `ui/style.css`; poisoned locks are
  recovered instead of cascading; the legacy v0.1 TUI, the stale GUI mock and
  the unused `queue_clear_session` command are removed; one-off documents are
  archived under `docs/archive/`.

## 0.5.13 — 2026-09-05 (Nightly)

- Prevent Force Touch on buttons from opening the macOS Look Up popover.
- Suppress WebKit's browser menus (Reload, Inspect, Look Up) on app-surface
  Control-click/right-click while retaining Deck's own context actions and
  text-field editing menus.
- Organize Settings into six searchable categories with fixed navigation and
  footer, independently scrolling content, and expandable detailed explanations.
- Add diagnostic log size, export and confirmed reset under Data & logs. Reset
  clears only the active log, serializes with background logging, and preserves
  exports, history, shell recovery data and running sessions.
- Fix copying Chinese and other non-ASCII terminal text by giving the native
  clipboard writer an explicit UTF-8 locale.

## 0.5.4 — 2026-08-30

- Separate Stable and Nightly updater trust roots. Nightly signing now runs in
  a read-only secret-bearing job and hands verified artifacts to a no-secret
  publisher; promotion keeps tested DMG/archive bytes and creates a new Stable
  detached signature inside the protected production environment. v0.5.4 is
  the single legacy-key bootstrap candidate for existing clients.
- Pin every GitHub Action to a full commit SHA. Scope Cloudflare deployment
  credentials to `website-production`, remove its GitHub token, and keep the
  site job read-only toward the repository.
- Make shell recovery opt-in, redact common credential values and private-key
  blocks, expire snapshots after seven days, and stop creating transcript
  backups. Existing explicit preferences are preserved and disabling remains
  a race-safe wipe of all recovery files.
- Bind destructive tmux restart confirmation to a backend-generated token over
  the exact server socket, session identities, pane IDs/PIDs and foreground
  commands, so equal counts cannot authorize a replacement session.
- Clarify that shell commands, CLIs and agents launched or restored by deck
  inherit its macOS Local Network permission.

## 0.5.2 — 2026-08-30

- Fix restored shell sessions losing access to local-network destinations
  after a machine or tmux-server restart. The old recovery path made the
  signed `deck-app` executable the pane bootstrap, so macOS Local Network
  Privacy could keep attributing the replacement login shell and commands such
  as `kubectl` to Deck even though a fresh session on the same server worked.
- Preserve the same bounded history display without putting Deck in the pane
  process chain: recovery now streams sanitized output through a one-use
  private tmux buffer, a system shell writes it only to pane stdout, deletes
  the buffer, and execs the user's login shell. No history is replayed, exposed
  in argv, or written to a new temporary payload file.
- Add a real-tmux regression contract that starts from an empty private server
  and proves restored text becomes scrollback, command-shaped text stays
  inert, the buffer is deleted, and `pane_start_command` contains no
  `deck-app` executable.

## 0.5.1 — 2026-08-30

- Keep sidebar navigation stable: Boards remain grouped in Board order, while
  sessions retain durable card order instead of moving whenever polling flips
  them between live, quiet and stopped states. Focus and status changes now
  update the existing sidebar entries in place.
- Remove verbose diagnostics from user Settings. Maintainers can launch with
  `--debug-logging`; structured event allowlists, redaction, private log files
  and ordinary always-on diagnostics remain unchanged.

## 0.5.0 — 2026-08-30

- Stop restored, history-heavy panes from flashing through tmux's intermediate
  copy-mode frames while dragging a selection. tmux remains the exact byte and
  history-coordinate authority, while deck paints only each settled range in
  one stable overlay and hides the internal drag cursor.
- Keep Settings inside the window at every supported size: the dialog is wider
  when space allows, vertically centered, bounded by the viewport, and scrolls
  its own contents when the full localized form is taller than the window.

## 0.4.42 — 2026-08-30

- Restore bounded shell output directly into the new pane's real tmux
  scrollback instead of blocking the live terminal with a read-only overlay.
  A one-use private bootstrap writes only to pane stdout, marks the restart
  boundary, then execs the login shell; command-shaped text never reaches
  stdin, and ordinary tmux scrolling, selection and copy work immediately.
- Keep the terminal cursor attached to the agent's live input row while a
  frozen text selection is scrolled. Once that row leaves the viewport the
  cursor is hidden, instead of remaining fixed on an unrelated selected cell.

## 0.4.41 — 2026-08-30

- Add an upgrade-aware lifecycle for deck's private tmux server. Quitting,
  crashing, and reopening the same build still reuse the existing PID and
  processes; a different release build now detects the old helper before
  attaching indefinitely.
- Store versioned creator/build/helper/protocol/source metadata in the tmux
  server, classify current/different/legacy/corrupt states centrally, and keep
  Stable/Nightly, development, and smoke sockets from accidentally crossing.
- Automatically replace only an empty incompatible server. An occupied old or
  legacy server stays usable until the user reviews the affected session/pane
  counts and explicitly confirms that restarting the background shell service
  ends its commands and agents. “Later” is durable and remains discoverable in
  the sidebar and Settings without repeated prompts.
- Make replacement a serialized, recoverable transaction with a fresh impact
  check, PTY detach, orderly stop, bounded wait, validated stale-socket cleanup,
  current-helper start, metadata read-back, PID/identity verification and
  content-free diagnostics. Cards and bounded shell snapshots remain, but
  running Unix processes are never described as migrated.
- Prevent the updater's relocated old process and release apps running from
  transient/DMG paths from creating a long-lived server. Add an accurate Local
  Network usage description for user-chosen services reached by terminal tools.
- Add isolated real-tmux lifecycle contracts plus manual signed updater,
  responsible-code and Local Network Privacy smoke steps.
- Keep terminal wheel work armed on every display frame while preserving the
  single in-flight tmux mutation. Real bundled WKWebView verification improves
  sustained scroll updates from about 40 Hz to 60 Hz without losing fractional
  trackpad deltas, inertia tails, direction reversals or tmux scroll authority.
- Synchronize frozen terminal selections across either tmux-status/xterm-frame
  ordering, promote drags by terminal-cell movement rather than a CSS-pixel
  threshold, and recheck the pointerup cell. On the first wheel frame, native
  xterm word/line selections are adopted into the same immutable tmux range
  and overlay used by multi-row drags. Even a one-cell horizontal selection
  now follows its text while its coordinates and clipboard bytes stay frozen.
- Let Codex/Claude history recall yield to editing a visible multi-row prompt:
  after recalling a long entry, Up moves into its preceding visual row first;
  ordinary single-line history, modifiers, shells, editors and other TUIs keep
  their existing keys. The check reads only five public xterm cells per row.
- Fix terminal drag selection inside full-screen agent panes (Claude Code,
  Codex), where the highlight and the copied bytes landed on rows the pointer
  never touched while the identical drag in a shell pane was correct.
- Place copy-mode endpoints with `top-line`/`cursor-down`/`cursor-right` only:
  `start-of-line`, `end-of-line` and `back-to-indentation` walk to the ends of
  the wrapped logical line and `cursor-left` lands on a wide grapheme's
  trailing column, so all four leave the visible row. `cursor-down` snaps the
  column to a line end until the walk first steps off a non-empty line, which
  is why a frame that opens with blank rows selected the wrong rows.
- Measure a row's end the way tmux does, so a pointer past a row's trailing
  blanks clamps to the line end instead of wrapping onto the next row, and read
  the frame once per placement instead of once per endpoint.
- Cover the fix with real-tmux contract tests over alternate-screen frames
  (blank top rows, wide characters, wrapped rows, styled blanks, scrolled
  viewports), asserting endpoints and copied bytes; the contract suite now
  drives the production placement instead of its own copy of the rules.
## 0.4.38 — Nightly candidate, 2026-08-29

- Publish a Nightly-only version bump for validating the Stable-to-Nightly
  updater path after `v0.4.37`; application behavior is otherwise unchanged.

## 0.4.37 — 2026-08-29

- Add opt-in Stable/Nightly update channels with Stable-safe settings
  migration, one-time Nightly risk confirmation, fixed backend-owned endpoints,
  Tauri-native signature verification/install, and visible version/channel/
  commit identity.
- Add deterministic numeric version tooling, candidate manifest/hash/provenance
  validation, and fixture coverage for version, tag, asset, signature,
  manifest, Release-state and copy-only promotion failures.
- Add a protected Nightly candidate workflow with the full test/sign/notarize/
  staple/Gatekeeper gate and a last-step rolling feed update that preserves the
  prior verified pointer on ordinary failures.
- Add a production-approved Stable promotion workflow that copies the exact
  candidate DMG/updater/signature, re-verifies every byte and Apple/minisign
  identity, creates the Stable tag at the same commit, and publishes Stable
  `latest.json` last without rebuilding the application.
- Tighten the legacy Stable resolver to strict tags, per-version concurrency,
  draft-until-complete publication and non-destructive incomplete-Release
  handling; promoted candidate commits cannot trigger a duplicate source build.

## 0.4.36 — 2026-08-29

- Add one closed theme registry for application CSS, native window chrome,
  xterm cursor/selection and ANSI colors, and tmux copy-mode highlighting.
  Deck Dark remains the default; Light, live system appearance, High Contrast,
  and reviewed teal/blue/purple/orange accents are available in Settings.
- Apply the persisted theme before revealing the hidden window, update every
  existing terminal pane in place, make new splits inherit it, and roll the UI
  back with an explicit message when settings persistence fails.
- Add typed Rust and frontend validation/migration, complete-token and
  WCAG-contrast checks for every theme/accent pair, fixed-vs-system listener
  tests, DOM rollback coverage, and real-WK smoke stages for switch/rollback.
- Make native xterm word/line selections use the same prevented, case-insensitive
  Command-C clipboard path as tmux drag selections.
- Freeze a drag at the pointer cell without one final edge-scroll step, and
  synchronize/validate xterm and tmux dimensions so resize reflow cannot shift
  selection endpoints by characters or rows.
- Stabilize the real-WK link-classifier smoke by waiting for the synchronized
  terminal grid and selecting fixture output rather than the shell's echoed
  `printf` command. Two consecutive isolated production-module runs pass.

## 0.4.35 — 2026-08-29

- Add complete English and Simplified Chinese UI localization with immediate
  runtime switching, system-locale detection, native menu localization and
  strict dictionary/static coverage.
- Automatically bind scheduled work to its card and full tmux
  server/session/window/pane/pid identity, with an optional foreground
  executable derived from the launch command or live non-shell pane.
- Remove readiness hooks and user-configured safety policies: an exact pane is
  always required, an automatically captured process must match, and items
  without one retain same-pane compatibility delivery.
- Replace the fixed boot sleep with bounded cancellable target polling and
  atomically guard both identity and optional process at literal paste time.
  Context waiting consumes no attempts; process mismatch has a one-shot
  pointer-confirmed send, while identity replacement requires explicit rebind.

- Route printable macOS IME `keyCode=229` events through the final native
  `InputEvent` path, avoiding xterm's deferred keydown fallback that could
  drop the first Pinyin punctuation press in WKWebView.
- Keep modifier-only Shift keydowns out of xterm's byte path so its transient
  keydown flag cannot suppress the first Pinyin InputEvent of each Shift chord.
- Replace the overlapping terminal-link regex with a single-pass tokenizer:
  complete HTTP(S) URLs own their interval across soft wraps and full-width
  tmux redraw rows, while path-like tokens only become links after the backend
  resolves a real local target in the pane cwd. Menus visibly wrap the complete
  value; IPv4 addresses, nonexistent log filenames and URL `/api` fragments no
  longer open path menus.
- Keep real pointer clicks on xterm's trusted link state machine, remove
  synthetic click replay, prevent the compatibility click after mouseup from
  immediately closing the path menu, and recognize wrapped/Unicode/quoted,
  absolute, relative and `file:line[:column]` paths with public buffer APIs.
- Freeze every completed drag into a generation-bound tmux snapshot and native
  clipboard route. Immediate Command-C waits for the final pointer update;
  stale tokens and disappeared selections fail with closed diagnostic stages.
- Decouple completed selection endpoints from tmux's viewport cursor. A
  public-geometry overlay follows immutable content rows while scrolling, and
  clipboard bytes remain identical before and after the scroll.
- Remove pointer-time `disableStdin`, let composition/Process/Dead/Compose
  events bypass Deck shortcuts, and leave macOS Option/dead-key processing to
  the input method (`macOptionIsMeta: false`).
- Add exact path grammar, overlay geometry, IME routing, frozen-scroll tmux
  contract and real-WKWebView provider/clipboard/scroll smoke coverage.
## 0.4.34 — 2026-08-29

- Make repeated terminal drags replace the prior tmux selection reliably,
  accept the valid zero-cell anchor state, and invalidate late gesture replies.
- Coalesce each burst of PTY repaint bytes before handing it to xterm so tmux
  selection and scroll redraws cannot expose a partially painted frame.
- Drive terminal wheel input at display-frame cadence, preserve fractional
  trackpad deltas, serialize requests, and execute each tmux scroll as one
  server command list instead of several subprocess round trips.
- Add real-WKWebView release regressions for immediate repeated selection and
  fractional pixel/line-mode wheel routing.

## 0.4.33 — 2026-08-29

- Snapshot terminal selections through tmux's native copy buffer so ongoing
  pane output cannot drift stale `capture-pane` coordinates and silently copy
  the wrong rows.
- Add a deterministic contract that grows history before every legacy capture
  step and verifies the production snapshot byte-for-byte against tmux's
  selection, including buffer cleanup and disappeared-selection handling.

## 0.4.32 — 2026-08-29

- Add direct, tmux-owned terminal drag selection that continuously crosses
  screens in either direction, supports reverse shrinking and split isolation,
  and copies exact logical text with explicit 50,000-row/64 MiB boundaries.
- Give every primary-pointer gesture one owner from pointerdown, replay only
  sub-threshold clicks to xterm, and honor tmux's exclusive end column and
  Unicode cell/grapheme boundaries without an extra copied character.
- Remove the auxiliary long-scrollback copying surface and all of its entry
  points, shortcuts, backend capture protocol, styles, listeners and tests.
- Make completion ownership transitions refit both the old and new split panes,
  cancel stale animation frames, and keep xterm and PTY rows synchronized.
- Give natural-exit retirement per-session destructive ownership so overlapping
  polls or manual close operations cannot duplicate close/success callbacks,
  while unrelated sessions continue progressing.
- Add debug-only, isolated WKWebView fault hooks and production-path smoke gates
  for Board persistence recovery, ambiguous delivery repair, natural-exit
  failures, completion ownership, and direct terminal selection.
- Raise tmux history to 50,000 rows and add deterministic Unicode/wrap selection
  contracts at 2,500 and beyond 20,000 rows.

## 0.4.31 — 2026-08-28

- Serialize every Board mutation through one persist-before-commit transaction
  queue, including debounced edits, destructive closes, project deletion and
  natural shell exit; failed writes remain visible and retryable without
  dropping later mutations.
- Keep crash-recovered scheduler deliveries visibly ambiguous in memory even
  when the recovery write fails, and retry that dirty snapshot without ever
  making it schedulable again.
- Make inline rename IME-safe and single-commit across Enter, Escape and blur,
  with durable rollback and immediate updates in Board, sidebar and pane titles.
- Improve edge scrolling in the then-existing auxiliary scrollback view
  (the auxiliary view was retired in 0.4.32).
- Expand file-path actions with safe parent-directory resolution, opening the
  parent in the configured editor and creating a session there without a shell.
- Reserve real pane layout space for completion candidates, refit xterm and PTY
  rows, and keep adjacent split panes unaffected.
- Add production-module DOM regressions and an isolated, in-app WKWebView smoke
  harness covering concurrent Board mutations, rename, copy, paths and layout.

## 0.4.30 — 2026-08-28

- Replace silent assumed-sent crash recovery with an explicit ambiguous
  delivery state and idempotent acknowledge/retry decisions.
- Keep dirty queue persistence retries alive after the last once item leaves
  the in-memory queue.
- Make card/project deletion contingent on successful tmux shutdown and a
  durable Board save; failures keep sessions visible and retryable.
- Fix sidebar Rename for non-active sessions and group sidebar sessions by
  Board, with Board order, counts, and stable waiting/running/stopped order.
- Improve native clipboard and scrollback-capture exactness for the
  then-existing auxiliary view (retired in 0.4.32).
- Redact assignment, JSON, quoted and ANSI-wrapped paths, URLs and credentials
  throughout app logs, migrations and exports.
- Refuse to overwrite invalid JSON, malformed envelopes, wrong typed structure,
  or unreadable existing data; never poison a valid backup with damaged main
  bytes.

## 0.4.29 — 2026-08-28

- Added an auxiliary scrollback view (retired in 0.4.32), strict schema-envelope checks,
  durable queue mutations, completion-bar placement fixes, and expanded log
  privacy protections.
