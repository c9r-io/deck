# deck promotion kit

All material in this file is a reviewable draft. Do not publish it without an
explicit external-write approval.

## Recommended positioning

English:

> A native macOS command center for persistent terminal agent sessions.

Simplified Chinese:

> 面向持久终端 agent session 的原生 macOS 控制台。

The supporting line is intentionally more explicit:

> See which CLI sessions have recent output, are quiet, or have stopped—then
> decide where to look next.

It must not be shortened to “know which agent needs attention.”

## Message variants to test

Change only one variant per cohort.

1. **Overview first** — “Keep every CLI agent session in view.”
2. **Survival first** — “Quit the app, not the session.”
3. **Switching first** — “Run more CLI agents without living in terminal tabs.”

The current landing page uses variant 1.

## 90-second demonstration

Use a synthetic workspace and generic session titles. Do not record real paths,
repository names, prompts, commands, credentials, notifications, menu-bar
identity or unrelated desktop content.

| Time | Visual | Narration |
| --- | --- | --- |
| 0–8s | Six synthetic sessions on one Board; green, amber and gray dots visible | “When several CLI agents run at once, the hard part is remembering where to look.” |
| 8–20s | Point to the three status states without opening terminal content | “deck shows recent output, quiet sessions and stopped sessions together. Quiet is only a signal—you decide what it means.” |
| 20–34s | Open one session, return to Board, switch project tab | “Every card is a real terminal session, organized by project and by the attention layout you control.” |
| 34–47s | Create a right split, then a lower split | “Open several sessions side by side. Closing a pane never kills the session.” |
| 47–60s | Quit deck, show a neutral title card, reopen with sessions intact | “The sessions run on deck’s private tmux server, so they survive when the app closes.” |
| 60–76s | Open scheduler with synthetic placeholder text; show context status and pause | “You can queue prompts for later. deck rechecks the exact pane and process before sending, and makes uncertain delivery visible.” |
| 76–84s | Show boundary title card | “Quiet does not mean ready, and deck must be running for scheduled delivery.” |
| 84–90s | Landing hero and download CTA | “deck is MIT licensed, local-first, and available as a signed Apple Silicon DMG.” |

Do not put a “Watch demo” link on the website until the final recording has been
privacy-reviewed and the real URL is configured in `site/site.config.json`.

## GitHub README conversion copy

The README opening should say what the signals actually prove and show a quiet
state rather than “waiting for your input.” This repository now contains that
change. Do not add the unpublished `deck.c9r.io` link until DNS and TLS are live.

## Release post template

```markdown
# deck — persistent local CLI sessions on macOS

I built deck for the point where Claude Code, Codex, OpenCode and other CLI
sessions stop fitting comfortably into terminal tabs.

Each card is a real tmux-backed terminal session. deck shows recent output,
quiet sessions and stopped sessions in one Board; sessions survive when the app
closes; split view lets you inspect several terminals together.

Important boundary: quiet does not mean an agent is finished or waiting for
input. deck gives you a simple signal and leaves the decision to you.

- Apple Silicon macOS
- signed and notarized DMG
- MIT licensed
- no account
- terminal content stays local

Download: [approved deck.c9r.io or latest Stable Release URL]
Source: https://github.com/c9r-io/deck
```

## Show HN template

Title:

> Show HN: deck – a native macOS board for persistent CLI agent sessions

Body:

```text
I use several long-running coding-agent CLIs at once and wanted a thin control
surface around real terminals rather than another agent runtime.

deck maps each card to a tmux session. It shows recent output / quiet / stopped,
keeps sessions alive when the app closes, groups work by project, and supports
nested split terminals. It works with any CLI and terminal content stays local.

One deliberate limitation: quiet is only “no output for 15 seconds,” not a
claim that an agent is waiting or finished.

It is MIT licensed and the current signed/notarized DMG is Apple Silicon only.
[approved URL]
```

## Community post template

```text
If you regularly have 5+ Claude Code, Codex, OpenCode or other CLI sessions
running on a Mac, I would value feedback on deck.

It is a native Board around real persistent tmux sessions: recent-output / quiet
/ stopped signals, per-project organization, nested splits and optional
scheduled prompts. No account; terminal content stays local; MIT licensed.

I am testing whether the overview and session-survival workflow is genuinely
useful—not selling a license. [approved URL]
```

## D3 voluntary follow-up

> You have had a few days with deck. If you choose to share feedback, which was
> most useful: session survival, activity/quiet/stopped overview, Boards, split
> view, or scheduled prompts? Please describe the workflow at a high level and
> do not include terminal content, paths, session/project/repository names or
> secrets.

## D10 voluntary follow-up

> Are you still using deck for real session work? If yes, what operation did you
> last perform and what became easier? If no, what got in the way? Please keep
> the answer free of terminal content and private project details.

These messages are for an approved opt-in follow-up channel. Do not add timed
in-app prompts in the first release.

## Interview guide

Opening:

1. Confirm that participation is voluntary and notes exclude terminal content
   and identifying project details.
2. Ask for the session-count bucket and prior workflow category.
3. Ask the participant to describe the problem they expected deck to solve.

Core questions:

1. Tell me about the last time you had several CLI sessions running.
2. Before deck, how did you find the session you wanted to inspect?
3. What did you believe green, amber and gray meant?
4. Did you rely on a session surviving a deck restart? What changed as a
   result?
5. Did Boards replace anything, or add another organizational layer?
6. When did split view help, if at all?
7. Did you try scheduled prompts? What boundary was unclear?
8. What nearly made you uninstall or stop using deck?
9. What would you miss if deck disappeared tomorrow?

Close with the closed milestones in `docs/adoption-validation.md`. Do not ask
participants to share their screen, terminal history, logs or local data files.

## FAQ source of truth

- **Does deck upload code or terminal content?** No. It has no account or cloud
  workspace; prompts and output stay on the Mac.
- **How do sessions survive?** deck detaches from a private tmux server when the
  UI closes. Closing a card or exiting its shell ends the session.
- **Does amber mean waiting?** No. It means no output for at least 15 seconds.
- **Why direct DMG?** Direct Developer ID distribution preserves the terminal
  and tmux integration without an App Store sandbox redesign. The DMG is signed
  and notarized.
- **Which CLIs?** Any interactive CLI that runs in a macOS terminal.
- **Which Macs?** Apple Silicon only today.
- **Is it paid?** No commercial offer is being tested in this phase. The source
  is MIT licensed.

## Promotion channel order

1. Existing peers who match the five-session ICP — highest context and best
   interviews, but avoid friends-only positive bias.
2. GitHub Release and repository visitors — low-friction distribution signal.
3. Focused macOS / terminal / coding-agent communities — one community per
   cohort, follow each community’s self-promotion rules.
4. Show HN only after installation, privacy copy and the 90-second demo have
   been independently reviewed.

Do not launch several channels on the same day; that prevents learning which
message and audience produced the cohort.
