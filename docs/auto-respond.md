# Automations (自动化): the Slack connection and the two triggers

An automation is a project-level rule (↻ Automations on the Board) with a
trigger — a clock, or a badge you put on a Slack message. When it fires,
deck starts a session: one card in the column the rule names, launched
with the rule's command, then the rule's template with the message filled
in. When the command is Claude or Codex, the first template row is **not**
sent automatically: a newly started agent may be showing a startup dialog
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
it from the ⏱ panel. Quiet output alone cannot tell a finished turn from a
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
