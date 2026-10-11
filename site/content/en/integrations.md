# Optional integrations

Deck works as a local terminal without any integration. Enable only the access you need in Settings, and review what each connection can read or do.

## Agent status

In **Settings → Agents & notifications**, enable the matching **Claude Code** or **Codex** status integration for reported input requests and turn endings in **Needs attention**. Hooks report fixed status words, pane identifiers and opaque interaction IDs, not prompts or output. Codex hooks still require review and trust in `/hooks`; unavailable or unattributable signals do not prove that no work needs you. [Understand status signals →](/guide/attention/)

## Slack reactions and channel monitoring

Your personal Slack connection can start an automation when **you** add a saved reaction to a message. Configure it in **Settings → Integrations → Slack**, then make a project automation with a template. Deck must be running to start work; missed reactions can be found by search catch-up. [Set up reaction automations and their permissions →](/guide/automations/#start-from-a-slack-reaction)

**Slack channel monitor** is an optional capability of the same Deck Slack app, with a separate bot token and one shared App token. Update and reinstall an existing Reaction app, paste its new bot token, and add that bot to every public or private channel you monitor; old bot membership does not transfer. Rules name explicit channel IDs, allowed user or bot IDs, and a text match. There is no channel-history backfill; connection gaps can miss events.

The first matching message creates a card, stores the message in its scratchpad and queues the saved template. Later matches add notes, never automatic prompts. The saved directory and command come from your rule, not the message. Commands must start with `claude` or `codex` and may include simple unquoted arguments; executable paths, environment prefixes and shell syntax are refused. Each template row must begin with your own words, not a message placeholder. An allowed sender establishes provenance, not safe content; an Agent may act on that text with its own permissions.

The rule's **Automatically send the first step for future matching channel events** choice is off by default. Saving it grants scoped future runs their first step, including substituted external content, and accepts startup-input risk together. It supports Claude and Codex with literal `--no-daemon`. Deck verifies the Slack connection before saving the permission; a failed verification leaves the rule unsaved for retry. Changed scope, target or effective first step requires updated permission. Turning it off withdraws covered unsent first steps even if Agent interaction has since been observed; a send already committed may finish. Old pending events gain no later permission. Every follow-up still waits for **Send now**, and no Agent tool is approved by this choice. [Read the pinned channel reference](https://github.com/c9r-io/deck/blob/dcc3c064404e9eb4d392cb6228ae0b9f2cf14054/docs/channel-monitor.md).

## Card scratchpads

A card's scratchpad holds task notes. You can edit your own notes and queue selected copies; collected channel entries retain their original text. Notes and queued copies are distinct: a note appearing here is not evidence of delivery. Later channel messages are not sent by themselves. A retained scratchpad prevents automatic card retirement. A paired phone can work only with eligible Agent-card notes and queues, subject to the restrictions below.

## Phone Connector

The optional Phone Connector pairs a separate iOS companion app with Deck over a reachable private network. Enable it in **Settings → Remote access** and review the pairing QR code and device list. Pairing grants ongoing access until you revoke it. Disabling Connector stops the listener; it does not revoke paired devices. **Revoke** a device or reset and re-pair to end access. Connector provides no public relay or background delivery guarantee.

A phone can read recent output and request prompts only on cards whose saved command starts supported `claude` or `codex` with simple arguments and whose current foreground program is that Agent. A shell card does not become eligible merely because you manually launched an Agent inside it. Output can include earlier shell text from the same pane. An MCP-controlled card is excluded until you take control back on the Mac.

Phone messages and queued note copies wait for **Send now** on the Mac; pairing is not automatic-send approval. A phone task preset saved under **Project defaults** may separately authorize fixed follow-up steps and accept first-step startup risk, using the [automation choices](/guide/automations/#first-input-and-follow-up-permission). The phone cannot set those choices. Editing covered preset content or targets invalidates the relevant approval; revoking the device withdraws permissions from its unsent task steps. Phone text cannot be queued onto a channel run that carries first-step permission. An Agent may run commands in response to a prompt with your account's permissions. [Read the pinned Connector reference](https://github.com/c9r-io/deck/blob/dcc3c064404e9eb4d392cb6228ae0b9f2cf14054/docs/connector.md).

## Deck MCP and ChatGPT

**MCP terminal control** is off by default, under **Settings → Remote access**. Explicitly authorize a client for a Deck project and directory. Structured list, read and search tools do not start a shell. Creating a managed session is a separate choice; executing commands requires a separate, timed approval on the Mac. Execution uses your macOS account and is not a filesystem sandbox. Revoke clients in Deck Settings. [Read the pinned MCP permission reference](https://github.com/c9r-io/deck/blob/dcc3c064404e9eb4d392cb6228ae0b9f2cf14054/docs/mcp.md).

Local STDIO clients can use Deck MCP directly. For hosted ChatGPT, use the optional, separate Deck Tunnel Helper and official OpenAI `tunnel-client`; Deck itself does not hold the OpenAI Runtime API key. [Connect ChatGPT to Deck with Secure Tunnel →](/docs/integrations/chatgpt-secure-tunnel/)

## Before sharing a problem report

Deck keeps application state locally, while integrations can connect to their named services. Check exported logs, screenshots, queued prompts and scratchpad notes for sensitive content before sharing. [Privacy details →](/privacy/)
