# Automations (自动化): the Slack connection and the two triggers

An automation is a project-level rule (↻ Automations on the Board) with a
trigger — a clock, or a badge you put on a Slack message. When it fires,
deck starts a session: one card in the column the rule names, launched
with the rule's command, then the rule's template with the message filled
in. When the command is Claude or Codex, the first template row is **not**
sent automatically by default: a newly started agent may be showing a startup dialog
(an update offer, folder trust, first-run setup, MCP or hooks review) that
would take the Enter. The row waits at "Waiting for first agent
interaction" until you interact with the agent once — with its Agent
Status integration on, that interaction is what releases it — or you use
**send now**. Unattended delivery to Claude or Codex therefore needs the
Agent Status integration and one real interaction in that agent process;
later rows then follow the usual rules. Cards are never moved automatically and deck never writes anything back
to Slack. This document is the Slack connection (Settings) and the rules
that both triggers share; the app's Automations drawer is where rules live.

## One Slack app

New users create **one Deck Slack app** from Settings → Slack → Create the Slack app. Its unified manifest contains the Reaction user scopes, channel-monitor bot scopes, both event subscriptions, and Socket Mode. Install it to the workspace, then copy the User OAuth Token (`xoxp-…`) for badge triggers, Bot OAuth Token (`xoxb-…`) for channel monitoring, and App-Level Token (`xapp-…`, `connections:write`) for live delivery. Use only the capabilities you want: channel-only users do not need to enter `xoxp`. Tokens are verified with Slack, including installed OAuth scopes, before Keychain storage.

Existing Reaction users keep their current `xoxp` and `xapp`. To enable monitoring, open **Settings → Slack → Enable Channel Monitoring**, copy the unified manifest, and update **your existing Deck Slack app** under App Manifest. Save, then **Reinstall to Workspace** so the new bot scopes are granted. Paste only the new Bot OAuth Token into Deck. Add the Deck bot to **every public or private channel** you intend to monitor; membership from an older standalone monitor app does not transfer. Preserve your saved channel IDs. Slack sends message events only for conversations the App/bot can access. Deck does not request permission to join channels and cannot automatically join private channels.

If Deck detects credentials for an older standalone Channel Monitor app, it leaves them in Keychain but no longer connects to that App. Existing rules, cards, handled history and durably staged inbox entries stay intact; already staged entries continue through the normal Board transaction and acknowledgement flow. The Settings notice explains the required upgrade. A Channel-only user with only old credentials must set up the canonical connection manually; Deck never combines tokens from different Apps.

The App token is optional for Reaction search: without it Deck still searches every 30 seconds, and Slack's index may lag a fresh reaction by about a minute. With it, live reactions arrive through the single Socket Mode connection while search remains the catch-up path after sleep, disconnect or app downtime. Channel monitoring requires the App token for new events and has no history backfill. Valid credentials and a connected socket do not prove that every configured channel is delivering events; verify bot membership and an observed test message.

The runtime checks that user and bot tokens belong to the same workspace when both are present. Slack's authenticated responses used here do not prove that all three tokens belong to the same App; updating the existing App and copying its new bot token is the provenance step.

## Rules

One automation per badge (emoji name as Slack spells it: `deck`, `bug`,
`white_check_mark`), across every project — the dispatcher matches a badge
to one rule. A rule says which column of its project the card goes to, the
working directory, the launch command (`claude` by default), which of that
project's templates to send, and whether to close the card when the run is
done. Save a template from any card's ⏱ panel (📋 → save this list) or in
◈ Templates first.

Template placeholders are source-neutral: `{{msg.text}}`, `{{msg.from}}`,
`{{msg.where}}`, `{{msg.link}}`. A row that starts with a slash command
hands the message to that skill, e.g. `/bug-fix {{msg.text}}`. A row may
span many lines and is sent exactly as written; the message pasted into it
is flattened, so its newlines become spaces and cannot reshape your prompt.

A badge puts someone else's message into your session, so a badge rule
follows the same admission as a channel rule: its command must start with
`claude` or `codex` and may use simple, unquoted arguments such as `--yolo`
or `--dangerously-skip-permissions` (never a bare shell, where the message
would run as a command). Environment prefixes, executable paths and shell
syntax are refused. Every template row must start with your own words rather than
a placeholder. The editor refuses a badge rule that fails this, a stored one
is listed as blocked, and a badge that reaches it is skipped before any card
is created. The rows are queued through the native agent-only gate, so they
are pasted only while that agent is the pane's foreground program. Such a
run's rows are marked external: after the first, each waits for you to send
it from the ⏱ panel — unless you approved the rule for automatic sending
(below). Quiet output alone cannot tell a finished turn from a
permission prompt (the next row's Enter would answer that prompt), and a
reported turn end is not readiness either — the agent may still own
background work and resume on its own. No automatic row of any
list is pasted while the hook reports the agent waiting for input or
permission. Clock rules send only your own text and keep any command.

Badges that already exist when a rule is added are left alone. A message
with several badges makes one card per rule; the same badge on the same
message only ever makes one. Only your own reactions count. A badge rule
has no pause: the backlog it would collect while paused has no honest
reading, so it is deleted instead (cards it already created stay).

### Automatically continue approved follow-up steps

Your reaction on the message already approves the run and its first step.
A Slack badge rule has one more choice: **Automatically continue approved
follow-up steps**. Ticking it approves *this version* of the rule and its
template so that the run's later steps continue once the agent is safely
ready, without a Send now per step. Unticked, each follow-up waits for Send
now, as on Stable. It does not make the agent unattended:

- a newly started Claude or Codex is not typed into on its own — the first
  step still waits for one real interaction with the agent (or your Send
  now, which is your confirmation that the agent is ready, not an approval),
  unless you chose the separate first-send option below;
- an agent asking for input or permission, a Codex process whose status
  hooks cannot be attributed to it, and inspection checkpoints still pause
  the run, and the ⏱ panel says which ("Approved step · …");
- editing anything the approval covers — the badge, directory, command
  (`codex` → `codex --yolo` included), template or its steps, finish or
  inspection mode — turns it off until you tick it again, and unticking it
  (or deleting the rule) also stops the unsent steps of runs already under
  way.

Steps that paste Slack message content (`{{msg.text}}` and friends) need a
second, explicit tick: external messages are untrusted input and may
influence the agent. Without it, only your fixed steps are sent
automatically and the others wait for Send now. Deck does not claim to make
message text safe. See `docs/scheduler-context-safety.md` ("Automation
delivery authority") for the exact rules.

### Send the first step to a newly started agent without waiting for readiness

A separate, unticked-by-default choice on a Slack badge rule, independent of
the one above. It exists because neither Claude Code nor Codex currently
exposes a trustworthy "ready for the first prompt" fact, so by default Deck
waits for one real interaction with a freshly started agent: a Trust,
Update, sign-in, permission or other startup dialog may be showing, and the
text or its Enter would answer that dialog instead (trust the folder, start
an update). Ticking it — after an explicit confirmation — tells Deck to send
the run's **first step** anyway: you accept that risk for this rule, so the
reaction stays the only thing you do. Deck does not claim the agent is
ready; it only stops waiting for proof.

- Only the first step of runs this rule creates. Later steps keep every
  normal check, with or without the approval above: after the first step
  the agent has still not *proven* an interaction to Deck, so an approved
  follow-up waits for its hooks (or your Send now) as before.
- Everything else still applies to that first step: an input or permission
  request the agent reports, a Codex process whose Signal cannot be
  attributed, pause, the list order, its time, the send gap, the exact
  target pane and program, and the paste-mode check.
- Claude, and Codex only when the command includes `--no-daemon`: Codex's
  default shared background service gives Deck no attributable Signal at
  all, so the option does nothing there (the rule's facts say so).
- Starting still types nothing. The scheduler wakes immediately, then the
  eligible per-session worker waits 6 seconds before the first paste. This
  is compatibility grace for a known fresh-Claude input-loss window, not
  proof that the composer is ready or that no startup dialog owns Enter.
  During and after it, cancellation, rule/settings changes, current Agent
  holds and the exact target generation are checked again. The wait spends
  no delivery attempt, and restarting Deck or the Agent begins a new wait.
- Unticking it, deleting the rule or changing its command before the first
  step is sent stops it for runs already waiting; if Deck cannot read its
  settings it does not use it. Send now always works.
- Slack channel monitors, the Connector, clock rules and lists never have
  this option. A delivery sent this way is recorded as such (no text).
- It is a temporary escape hatch: when an agent exposes an official
  readiness fact, Deck should use that instead.
- The 6-second grace extends Deck's historical 2.5-second fresh-start settle
  after a real Claude 2.1.283 prompt was lost at 3.35 seconds from binding
  while a delivery at 5.8 seconds was processed in the same isolated setup.
  It must be checked against each supported Agent version. It cannot guarantee
  a future or unusually slow startup. Deck does not infer a lost prompt
  from a response timeout and does not automatically resend it. If text was
  pasted but Enter was refused, the row stays in the existing
  ambiguous-delivery state until you explicitly resolve it.

## The clock trigger

The clock is the second source. A clock rule has `source: clock` plus a
schedule — every day, chosen weekdays, or chosen days of the month, at a
local time — a name, and the finish mode. At each slot deck creates a fresh
card in the rule's column (title `name · MM-DD`), launches the command and
queues the template, exactly like a badge would; nothing is typed into an
existing session. The command may independently resume its own prior context.

- One run at a time: a slot that comes due while the rule's previous card is
  still on the Board is skipped and recorded as such.
- A slot deck slept through (asleep, updating, not running) still starts a
  run inside the rule's grace — **if missed, still start within** in the
  editor, 15 minutes by default, up to the rest of the day or never — and
  is otherwise recorded as a `skipped (missed)` run without a card, so
  opening deck in the evening does not start the morning's job. Yesterday's
  slots are never considered. Slots older than the rule's last schedule
  change or resume never fire. Slot keys come from `mktime` of local
  midnight, one value for the whole day, so a DST switch never hands a slot
  two keys.
- **close the card** (both triggers): the finish check requires an empty
  queue, no agent state and a shell back in the foreground — the agent
  program exited. This must hold for three consecutive polls, and never
  while a pane shows the card. A reported turn end never closes a run: it
  ends an interaction, and the agent may still run background work that
  closing the session would kill. A live interactive agent therefore keeps
  its card until it exits or you close it, and a clock rule's next slot is
  skipped as `busy` meanwhile. The check cannot establish business success.
  **keep it** leaves the card for you.
- **Inspect every row before continuing** is off by default and affects new
  runs when enabled on a rule. All template rows enter the reviewed list in
  one queue transaction. Each delivered row leaves a durable human checkpoint,
  including the last; automatic closing additionally requires final inspection.
  A queue write failure does not create a partially queued reviewed list or
  count as inspection. Card creation and inbound acknowledgement are still
  separate lifecycle transactions; the acknowledgement follows complete
  queue admission, and a failed admission is retried from the card's frozen
  plan. Existing runs and rules without opt-in
  keep their timing. See [inspection and compatibility](scheduler-context-safety.md#human-inspection-checkpoints-c-v01).
- Deleting a rule leaves its existing cards. A rule whose project no longer
  exists cannot dispatch a new card; project deletion currently does not
  remove the saved rule from settings.

## What deck keeps


- `~/.deck/inbound.json`: which (source, message, badge) triples have been
  handled — identifiers and times only, no text — and the run ledger: rule
  id (a badge name for a Slack trigger), slot or message key, card id,
  start/end instants and a closed outcome word (running / closed / skipped
  with its reason). Capped at 200 runs.
- The expanded template prompts are frozen in the private card document until
  every row is queued. This lets deck retry a partial queue write after a
  restart without creating a second card or sending a row twice. The queue
  also retains each row until delivery or cancellation; deck does not keep a
  separate Slack message archive.
- Tokens: macOS Keychain, service `io.c9r.deck`.
