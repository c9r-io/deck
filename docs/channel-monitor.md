# Slack channel monitor

Channel monitoring is an optional capability of **one Deck Slack app**. It uses a separate bot OAuth token (`xoxb`) from badge automation's user OAuth token (`xoxp`), while both consume one Socket Mode connection through the canonical App token (`xapp`). Deck never writes to Slack.

For an existing Reaction setup, use **Settings → Slack → Enable Channel Monitoring**. Update the manifest of your **existing** Deck Slack app, save and reinstall it to the workspace, then paste its new Bot OAuth Token. Add the Deck bot to every public or private channel you intend to monitor; an older standalone monitor bot's channel membership does not transfer. Slack sends these events only for conversations the App/bot can access. Deck does not join channels automatically. Existing channel IDs and rules remain saved.

For a new Channel-only setup, create one Deck Slack app from the unified manifest, install it, store `xoxb` and `xapp`, and add its bot to each monitored channel. `xoxp` is needed only for badge automations. Older standalone Channel Monitor credentials remain in Keychain but are not used for new events. Already staged inbox entries still drain through the normal Board transaction and `channel_ack`; no credential migration silently merges Apps.

After completing the Unified Slack upgrade, older standalone Channel Monitor users can explicitly choose **Remove legacy Slack credentials** in Settings. Removing legacy credentials does not remove Channel rules, staged messages, cards, or history.

Create rules from a project's **Automations** drawer. Every rule requires explicit channel IDs and at least one allowed user or bot ID. Matching is deterministic: a substring, a keyword list, or a regular expression. A named regular-expression capture can provide an incident key. Cards are grouped only by connection, workspace, channel, rule, and that optional captured value; deck does not infer incidents with AI. Thread replies can be included or excluded.

The first matching event creates a card and stores the original message in its scratchpad before the event is acknowledged. The configured template is expanded and frozen into a durable initial queue plan. Restarts retry the same operation IDs, so an accepted step is not enqueued twice. Later matching events add immutable scratchpad entries and are never sent automatically. The directory and command always come from the saved rule, never from message text.

## What a rule may launch

A channel rule's command must start with `claude` or `codex`. Simple, unquoted arguments are allowed, including `--yolo`, `--dangerously-skip-permissions` and `-c approval_policy=never`. Environment prefixes, executable paths, quoting and shell syntax are refused. These flags can let an agent act without approval, including on untrusted Slack text. Every line of the rule's template must begin with your own words, not a message placeholder, so a message that starts with `!` or `/` never becomes the first thing the agent reads. A rule saved by an older deck that breaks either condition still loads: the Automations drawer shows it as blocked, it matches nothing, and messages already staged for it stay pending until you edit the rule.

Queued channel text is pasted only while the expected agent is the pane's foreground program and has bracketed paste enabled, checked atomically with the paste and again with Enter; **Send now** applies the same check and has no bypass. See `docs/scheduler-context-safety.md` for the one residual window.

## Trust: what an allowlist does and does not mean

Channel text is untrusted input to an agent. Allowing a user or bot means trusting where the text comes from, not what it says. An allowed **bot** admits whatever that bot posts: incoming-webhook callers, Workflow Builder form fillers, alert and issue titles, forwarded email. Sending text as a prompt instead of a shell command does not prevent code execution: the agent may run commands the text asks for, within whatever its own configuration permits. deck cannot see that configuration (`~/.codex/config.toml`, Claude Code settings, a repository's `.claude/` or `.codex/` files), and it can enable approval-free modes even when the rule's command is a bare `claude` or `codex`. Bidi controls, zero-width characters and Unicode tag characters are removed from staged message text so hidden instructions cannot sit invisibly in a note you inspect.

The default collection window is 30 idle minutes, measured from the last event deck successfully saved to the card. An older Slack event arriving from the durable inbox therefore joins the current collection without shortening its window. A saved value of `0` collects until **Stop collecting** is pressed in the scratchpad. Expiry or manual stop keeps every note and only closes that collection group; a later match creates a new card.

Each card scratchpad is limited to 256 entries, 32 KiB per entry, 256 immutable queued copies, 1 MiB of aggregate text, and 2 MiB of serialized data including metadata and JSON escaping. A full scratchpad leaves the Slack event pending and shows an error. The native inbox holds at most 1,000 pending events and 5,000 pending plus handled identities; handled identities expire after 45 days.

Messages that can never be staged (an envelope over 256 KiB, a body over 16 KiB, or an event time more than five minutes ahead of this Mac's clock) are counted as rejected and acknowledged without dropping the connection; a Slack retry would carry the same bytes.

There is no history backfill. A disconnect records an explicit unresolved gap. Slack retries are finite, so an acknowledged transport envelope means the event reached deck's durable inbox, not that every message during a disconnect can later be recovered. Message references retain workspace, channel, event, timestamp, thread, and sender IDs; deck does not invent permalinks it cannot verify.
