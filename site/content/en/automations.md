# Reuse templates and automations

Save reusable prompts and create a new session at a scheduled time or from your Slack reaction, reducing the work of remembering and restarting routine tasks.

## Choose the right tool

| What you want | Use |
| --- | --- |
| Continue an existing session later | A list on its card |
| Send the same prompts periodically into that session | Repeat on the list |
| Save prompts for reuse | A project template |
| Create a fresh card, launch a command and send a template each time | A project automation |

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

Open the Slack connection under **Settings → Integrations & automation**:

1. Click **Create the Slack app…**, then choose a workspace and create the app on Slack's page.
2. Install it into the workspace and copy the User OAuth Token into deck as instructed. Some workspaces require administrator approval.
3. For prompt event delivery, create an App-Level Token with `connections:write` and enter it as the App token.
4. Enable Slack, then create an automation with the emoji name, directory, launch command and template.

Validated tokens are kept in macOS Keychain. The App token is optional. Without it, deck searches about every 30 seconds and Slack's index may lag by about a minute. With it, live events are used while search remains as catch-up.

Templates can use `{{msg.text}}`, `{{msg.from}}`, `{{msg.where}}` and `{{msg.link}}` for the message text, author, location and link. deck does not automatically reply to Slack.

Only your own reactions trigger work. Reactions already present when the rule is added are not replayed. The same reaction on the same message creates a card only once; an emoji name can belong to only one rule across projects. Slack rules have no pause: delete the rule to stop future triggers. Existing cards remain.

## Finishing and inspection

**Keep it** leaves the card for you to read. **Close the card** requires an empty queue and three consecutive observations of a reported turn ending (or a foreground shell without hook state), with no pane showing the card. These conditions **cannot prove business success or establish that the report belongs to the last prompt**.

If every-row inspection is enabled, the last row must also be inspected before automatic closing is possible. Changing the rule's inspection setting affects new runs only; existing runs keep their settings. Inspection does not replace timing and target checks.

Deleting a rule leaves its existing cards. Ending one run's card does not delete the rule that will trigger future runs.
