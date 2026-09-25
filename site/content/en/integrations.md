# Optional integrations

Deck works as a local terminal without any integration. Enable only the access you need in Settings, and review what each connection can read or do.

## Agent status

In **Settings → Integrations & automation**, enable the matching **Claude Code** or **Codex** status integration if you want reported input requests and turn endings in **Needs attention**. Deck adds a hook to that CLI's configuration. The hook reports a fixed status and pane identifier, not the prompt or terminal output. [Understand status signals →](/guide/attention/)

## Slack reactions and channel monitoring

The personal Slack connection can start an automation when **you** add a saved reaction to a message. Configure it in **Settings → Integrations & automation**, then make a project automation with a template. The app must be running to start work. [Set up reaction automations →](/guide/automations/#start-from-a-slack-reaction)

**Slack channel monitor** is an optional capability of the same Deck Slack app, with a separate bot token and one shared App token. Update and reinstall an existing Reaction app, paste its new bot token, and add that bot to every public or private channel you monitor; old bot membership does not transfer. A project rule names specific channel IDs, allowed user or bot IDs, and a text match. The first matching message creates a card and stores the message in its scratchpad; later matches add notes, not automatic prompts. The rule's saved directory and bare `claude` or `codex` command determine what starts. Channel text is still untrusted input to an agent, even from an allowed sender. Review the scope and template before enabling a rule. [Read the channel monitor reference](https://github.com/c9r-io/deck/blob/main/docs/channel-monitor.md).

## Card scratchpads

A card's scratchpad holds notes for that task. You can review and edit notes locally, then choose what to queue for the agent. Channel monitor can add matching messages to it; those later messages are not sent by themselves. A paired Phone Connector client can work with eligible agent-card notes and queues. Do not treat a note as an instruction that Deck has already delivered.

## Phone Connector

The optional Phone Connector pairs a separate iOS companion app with Deck over a reachable private network. Enable it in **Settings → Integrations & automation** and review the pairing QR code and device list. A paired phone can see recent output and send prompts to eligible `claude` or `codex` cards, and can start a task from a preset you saved on the Mac. That can cause an agent to run commands with your account's permissions. Disable Connector when you do not want the listener; **revoke** a device to remove its access. The phone does not have a public relay or background delivery guarantee. [Read the Phone Connector reference](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/connector.md).

## Deck MCP and ChatGPT

**MCP terminal control** is off by default. You explicitly authorize a client for a Deck project and directory. Structured list, read and search tools do not start a shell. Creating a managed session is a separate choice; running commands needs a separate timed approval on the Mac. Execution then uses your macOS account and is not a filesystem sandbox. You can revoke a client in Deck Settings. [Read the MCP permission reference](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/mcp.md).

Local STDIO clients can use Deck MCP directly. For hosted ChatGPT, use the optional, separate Deck Tunnel Helper and official OpenAI `tunnel-client`; Deck itself does not hold the OpenAI Runtime API key. [Connect ChatGPT to Deck with Secure Tunnel →](/docs/integrations/chatgpt-secure-tunnel/)

## Before sharing a problem report

Deck keeps its application state locally, while integrations can connect to their named services. Check any exported log, screenshot, queued prompt or scratchpad note for sensitive content before sharing it. [Privacy details →](/privacy/)
