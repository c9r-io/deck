# Reuse templates and automations

Save reusable prompts and create a new session at a scheduled time or from your Slack reaction, reducing the work of remembering and restarting routine tasks.

## Choose the right tool

| What you want | Use |
| --- | --- |
| Continue an existing session later | A list on its card |
| Send the same prompts periodically into that session | Repeat on the list |
| Save prompts for reuse | A project template |
| Create a fresh card, launch a command and send a template each time | A project automation |
| Collect messages from selected Slack channels without automatically sending every message | A channel monitor rule and card scratchpad |

Open **Templates…** or **Automations…** from **New session ▾**. Once a project has rules, the **↻ N automation(s)** chip also opens the manager.

## Prepare a template

Create one in **Templates…**, or save an existing list from its **📋** menu in the card's **⏱** panel. Templates belong to a project. Applying one to a card makes a copy, so later edits do not change those existing rows.

An automation references a template and sends it on each new run. Try a run that keeps its card so you can verify the content before choosing automatic closing.

## Start work on a clock

1. Choose new on a clock from **New session ▾**, or create a clock rule in Automations.
2. Set its name, destination group, working directory, launch command and template.
3. Choose every day, selected weekdays or days of the month, and a local time.
4. Choose the grace period for missed slots and whether to keep or close the card afterwards.
5. Save and check the next slot and recent run history.

For example, start a session at 09:00 on weekdays, launch your existing CLI and ask it to review recent changes for questions needing human judgment. Each run creates a new session, but a launch command may resume the agent's earlier context. A new card does not guarantee empty context.

**The app must be running to start work.** If deck is closed or the Mac asleep at the scheduled time, the default grace period is 15 minutes; you can choose the rest of the day. Outside that window, the slot is recorded as missed without creating a card. Yesterday's slots never catch up. Rescheduling or resuming a rule starts from that moment.

A clock rule has one run at a time. If its previous card remains on the Board, the next slot is skipped and recorded. Read the reason in recent runs before assuming a rule is broken.

## Start from a Slack reaction

Open the Slack connection under **Settings → Integrations → Slack**:

1. Click **Create the Slack app…**, then choose a workspace and create the app on Slack's page.
2. Install it into the workspace and copy the User OAuth Token into deck as instructed. Some workspaces require administrator approval.
3. For prompt event delivery, create an App-Level Token with `connections:write` and enter it as the App token.
4. Enable Slack, then create an automation with the emoji name, directory, launch command and template.

Validated tokens are kept in macOS Keychain. The App token is optional. Without it, deck searches about every 30 seconds and Slack's index may lag by about a minute. With it, live events are used while search remains as catch-up.

Templates can use `{{msg.text}}`, `{{msg.from}}`, `{{msg.where}}` and `{{msg.link}}` for the message text, author, location and link. deck does not automatically reply to Slack.

Only your own reactions trigger work. Reactions already present when the rule is added are not replayed. The same reaction on the same message creates a card only once; an emoji name can belong to only one rule across projects. Slack rules have no pause: delete the rule to stop future triggers. Existing cards remain.

For messages posted in a selected channel by allowed users or bots, enable **channel monitoring in the same Deck Slack app**. Update and reinstall an existing app, paste its new bot token, then add the bot to each monitored public or private channel. Its first match can create a card and queue a saved template; later matches add scratchpad notes without sending them to the agent. Channel text remains untrusted even when its sender is allowed. [Compare optional integrations →](/guide/integrations/#slack-reactions-and-channel-monitoring)

## First input and follow-up permission

For a newly started Claude Code or Codex, the first template row normally waits for **one real agent interaction**, observed through the matching Agent status integration, or your **Send now** after checking the terminal. A startup trust, update, sign-in or hook-review dialog may still own Enter. A saved rule, quiet output or a turn-ending report does not prove readiness.

Clock and Slack badge rules offer a separate, off-by-default **Send the first step to a newly started agent without waiting for readiness** choice. Its confirmation accepts that startup-input risk for the first step of future runs. It supports Claude and Codex with literal `--no-daemon`; a short compatibility wait is not evidence that the agent is ready. Input or permission requests, inspection and target checks still apply. Turning it off withdraws the exception from waiting first steps; it does not stop work that can proceed with real interaction evidence.

A Slack reaction expresses the run and first-step intent; it does not approve every later message. Follow-up steps normally wait for **Send now**. **Automatically continue approved follow-up steps** approves a specific version of the rule and template; rows containing `{{msg.*}}` also need the separate external-content acknowledgment. Changes to covered content or targets, or withdrawing approval, stop unsent steps relying on that approval. Frozen queued text is not rewritten. Approval never answers an Agent's permission prompt or completes inspection. Clock follow-ups contain your own text and do not need a Slack content grant.

Slack badge commands must start with `claude` or `codex` and may include simple unquoted arguments; shell syntax, executable paths and environment prefixes are refused. Every template row must begin with your own words, not a message placeholder. Channel monitoring has its own first-step choice and always keeps later steps manual. [Compare the integration boundaries →](/guide/integrations/)

## Finishing and inspection

**Keep it** leaves the card for you to review. **Close the card** requires an empty queue and three consecutive polls with **no agent state and a shell in the foreground of the single-pane session**. The session must still be alive, and no open Deck pane may show the card. **A reported turn ending never closes the run**: an interactive agent can still own background work. These observations do not prove business success.

A retained scratchpad or an active reminder also prevents automatic closure. Ending a reminder does not release a retirement it previously blocked. With per-row human inspection enabled, the last row must be inspected too. Changing a rule's inspection setting affects new runs only; existing runs keep their setting. Inspection does not bypass scheduling or target checks.

Deleting a rule does not delete cards it already created. Closing a run card does not delete a rule that may trigger again.
