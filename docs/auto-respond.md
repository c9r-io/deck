# Auto-respond (自动响应)

A badge you put on a message in Slack starts a session in deck: one card in
the column a rule names, launched with the rule's command, then the rule's
template prompts with the message filled in. Cards are never moved
automatically and deck never writes anything back to Slack.

## One-time Slack setup

Every Slack token comes from an app you create and install for yourself;
there is no token without that step. Bot users and channel invites are not
needed.

1. https://api.slack.com/apps → **Create New App → From scratch**, any name,
   your workspace.
2. **OAuth & Permissions → User Token Scopes**, add:
   `search:read reactions:read channels:history groups:history im:history
   mpim:history users:read channels:read groups:read im:read mpim:read`.
3. **Socket Mode → Enable**, and generate an **app-level token** with
   `connections:write` (starts with `xapp-`).
4. **Event Subscriptions → Enable Events → Subscribe to events on behalf of
   users**, add `reaction_added`. Save.
5. **Install App → Install to Workspace** (some workspaces route this through
   an admin approval). Copy the **User OAuth Token** (starts with `xoxp-`).
6. In deck → Settings → Auto-respond: paste both tokens (they go to your
   macOS Keychain, never under `~/.deck`), tick **Slack**, add rules.

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
hands the message to that skill, e.g. `/bug-fix {{msg.text}}`. Prompts are
one line: newlines in the message become spaces.

Badges that already exist when a rule is added are left alone. A message
with several badges makes one card per rule; the same badge on the same
message only ever makes one. Only your own reactions count.

## What deck keeps

- `~/.deck/inbound.json`: which (source, message, badge) triples have been
  handled — identifiers and times only, no text.
- The message text exists once, inside the queued prompt of the card it
  created, exactly like a prompt you typed.
- Tokens: macOS Keychain, service `io.c9r.deck`.
