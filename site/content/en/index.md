# User guide

deck is an interface around local terminals. Use it for ordinary commands, or organize multiple task sessions by project. This guide covers the basics and optional features.

## Start with one task

[Start your first task →](/guide/start/)

Creating a session opens a shell where you can run your usual commands. Lists, automations and agent integrations can be configured later, if you want them. deck includes tmux; install and configure agent CLIs such as Claude Code or Codex separately.

deck has no cloud service, cloud account or remote server hosting your tasks. Shells, programs and sessions run on your Mac. A CLI that uses a model service still needs its own setup and sign-in.

## Find the answer you need

| Your question | Where to go |
| --- | --- |
| What needs me now? | [Decide what needs attention](/guide/attention/) |
| Can I arrange the next prompts in advance? | [Arrange follow-up input](/guide/prompts/) |
| Why has the next row not been sent? | [Read delivery and inspection states](/guide/prompts/#when-a-list-does-not-continue) |
| What happens when I close the window? | [Leave, close and recover](/guide/sessions/) |
| How do I start work daily or from a Slack reaction? | [Reuse templates and automations](/guide/automations/) |
| How do I dictate a prompt, change settings or fix a problem? | [Input, settings and troubleshooting](/guide/input-and-settings/) |

## Keep three distinctions in mind

- **You own the Board's groups.** Group names, card positions and order do not change with live status. The sidebar's Needs attention view separately gathers signals across projects.
- **Delivery and task results are separate.** Sent means input was delivered. Quiet, a turn ending and human inspection each describe different facts.
- **A session and the app have separate lifetimes.** Sessions normally keep running when you quit deck. Lists and automations need the app running to deliver input or start work.

## Version scope

This guide uses the **deck 0.6.5** feature baseline and also applies to **0.6.6 Stable**, including Needs attention, lists, human inspection, automations and on-device voice drafts. Check your installed version in Settings; earlier versions may have different labels and entry points. Version 0.6.6 hardens prompt-delivery privacy without adding new user flows. Later Nightly changes are outside this guide's baseline.

See [updates and versions](/guide/input-and-settings/#updates-and-versions) for upgrades, channel switching and older data compatibility.
