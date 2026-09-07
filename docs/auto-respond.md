# Auto-respond (自动响应)

A badge you put on a message in Slack starts a session in deck: one card in
the column a rule names, launched with the rule's command, then the rule's
template prompts with the message filled in. Cards are never moved
automatically and deck never writes anything back to Slack.

## One-time Slack setup

Every Slack token comes from an app you create and install for yourself;
there is no token without that step (Slack allows no OAuth redirect to a
local app and has no API that mints app-level tokens). Bot users and
channel invites are not needed. deck does everything it can:

1. Settings → Auto-respond → **Create the Slack app…** opens Slack's
   "Create an app" page with the manifest prefilled: name, the user scopes,
   Socket Mode, the `reaction_added` user event. Pick a workspace, **Create**.
2. **Install App → Install to Workspace → Allow** (some workspaces route
   this through an admin approval). Copy the **User OAuth Token**
   (`xoxp-…`) into deck's *User token* field.
3. **Basic Information → App-Level Tokens → Generate Token and Scopes**,
   add `connections:write`, generate, copy the `xapp-…` token into deck's
   *App token* field.

deck checks each token with Slack before storing it in your macOS Keychain
(never under `~/.deck`). Then tick **Slack** and add rules.

The app token is optional: without it deck only searches every 30 seconds,
and Slack's search index lags a fresh reaction by about a minute. With it,
new reactions arrive within a second; the search stays on as the catch-up
for anything missed while deck was closed or the Mac was asleep.

## Rules

One rule per badge (emoji name as Slack spells it: `deck`, `bug`,
`white_check_mark`): which project and column the card goes to, the working
directory, the launch command (`claude` by default) and which of that
project's templates to send. Save a template from any card's queue panel
first.

Template placeholders are source-neutral: `{{msg.text}}`, `{{msg.from}}`,
`{{msg.where}}`, `{{msg.link}}`. A step that starts with a slash command
hands the message to that skill, e.g. `/bug-fix {{msg.text}}`. A step may
span many lines and is sent exactly as written; the message pasted into it
is flattened, so its newlines become spaces and cannot reshape your prompt.

Badges that already exist when a rule is added are left alone. A message
with several badges makes one card per rule; the same badge on the same
message only ever makes one. Only your own reactions count.

## Scheduled automations (自动化)

The clock is a second source. An automation is a rule of the same shape with
`source: clock` plus a schedule — every day, chosen weekdays, or chosen days
of the month, at a local time — a name, and a finish mode. It lives in the
project's **↻ Automations** drawer on the Board, not in Settings. At each
slot deck creates a fresh card in the rule's column (title `name · MM-DD`),
launches the command and queues the template, exactly like a badge would;
nothing is typed into an existing session, so every run starts with an
empty agent context.

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
- **close the card**: once every prompt is delivered and the agent reported
  its turn done (agent hooks) or the program left the foreground, the card
  is retired through the same path as an explicit close — after the reading
  held for three consecutive polls, so the instant between a step's delivery
  and the agent's next `working` hook can never close a run early, and never
  while a pane shows the card: a run you are reading or talking to is yours
  until you leave it. **keep it** leaves the card for you.
- Deleting a project deletes its automations; cards a rule already created
  stay when the rule is deleted.

## What deck keeps


- `~/.deck/inbound.json`: which (source, message, badge) triples have been
  handled — identifiers and times only, no text — and, for automations, the
  run ledger: rule id, slot, card id, start/end instants and a closed outcome
  word (running / closed / skipped with its reason). Capped at 200 runs.
- The message text exists once, inside the queued prompt of the card it
  created, exactly like a prompt you typed.
- Tokens: macOS Keychain, service `io.c9r.deck`.
