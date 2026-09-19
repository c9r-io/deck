# Slack channel monitor

The channel monitor is a separate, read-only Slack connection for explicitly scoped project automations. It does not reuse the personal reaction connection or its credentials, and it never writes to Slack.

Create the Slack app from **Settings → Integrations & automation → Slack channel monitor**, install it in the chosen workspace, then store its bot token and Socket Mode app token. The credentials stay in the macOS Keychain. Enabling the connection without both credentials leaves it visibly disconnected.

Create rules from a project's **Automations** drawer. Every rule requires explicit channel IDs and at least one allowed user or bot ID. Matching is deterministic: a substring, a keyword list, or a regular expression. A named regular-expression capture can provide an incident key. Cards are grouped only by connection, workspace, channel, rule, and that optional captured value; deck does not infer incidents with AI. Thread replies can be included or excluded.

The first matching event creates a card and stores the original message in its scratchpad before the event is acknowledged. The configured template is expanded and frozen into a durable initial queue plan. Restarts retry the same operation IDs, so an accepted step is not enqueued twice. Later matching events add immutable scratchpad entries and are never sent automatically. The directory and command always come from the saved rule, never from message text.

The default collection window is 30 idle minutes, measured from the last event deck successfully saved to the card. An older Slack event arriving from the durable inbox therefore joins the current collection without shortening its window. A saved value of `0` collects until **Stop collecting** is pressed in the scratchpad. Expiry or manual stop keeps every note and only closes that collection group; a later match creates a new card.

Each card scratchpad is limited to 256 entries, 32 KiB per entry, 256 immutable queued copies, 1 MiB of aggregate text, and 2 MiB of serialized data including metadata and JSON escaping. A full scratchpad leaves the Slack event pending and shows an error. The native inbox holds at most 1,000 pending events and 5,000 pending plus handled identities; handled identities expire after 45 days.

There is no history backfill. A disconnect records an explicit unresolved gap. Slack retries are finite, so an acknowledged transport envelope means the event reached deck's durable inbox, not that every message during a disconnect can later be recovered. Message references retain workspace, channel, event, timestamp, thread, and sender IDs; deck does not invent permalinks it cannot verify.
