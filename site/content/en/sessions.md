# Leave, close and recover

Leaving a terminal view, quitting the app and ending a session are different actions. Decide whether you want work to continue before you close anything.

## What keeps running

| Action | Effect on the session | Effect on lists and automations |
| --- | --- | --- |
| Return to the Board or close a split pane | Leaves the view; does not directly end the session | The app remains running, so deliveries can continue |
| Close the app window (red button or ⌘W) | Hides the window; sessions continue | The app remains running, so deliveries can continue |
| Quit deck (⌘Q) | Sessions in private tmux keep running | No deliveries or new automation starts while the app is closed |
| Close a card | Ends that session and removes the card | Cancels that card's pending lists |
| Restart the Mac or background shell service | Existing processes end | Local lists remain saved and are rechecked when deck runs again |

> For an automation configured to close its card, leaving the pane may allow automatic finishing. Closing a pane does not itself terminate a session, but it does not promise to keep such a run forever. [Automatic finish conditions →](/guide/automations/#finishing-and-inspection)

## End a session deliberately

Closing a card affects its running shell, agent and processes. In 0.6.5, the card corner × and right-click close act immediately; the in-session close action asks for confirmation. Check unsaved work and pending lists first.

Ctrl+D in a terminal is interpreted by the foreground program. It exits the shell only in applicable contexts, such as at a shell prompt. It is not a universal shortcut for closing deck.

An agent exiting usually returns to the shell. It does not necessarily end the card's entire session.

## When the app is upgraded

Reopening the same build reuses the existing background service and processes. After installing a different build, deck may need to restart the background shell service. An empty service can be replaced automatically; an occupied service lists the effects and asks you to confirm.

Choose **Later** to keep using existing sessions. The pending restart stays in the sidebar and Settings. Confirming the restart ends all processes in that service; running agents cannot be moved into the replacement.

Reopening a stopped card normally starts a shell in its directory and **does not replay the launch command from card creation**. A card whose first command was never delivered is the exception. Look for an agent's resume hint and decide whether to restore it yourself. A list's delivery startup path may use the command its program needs; inspect the list separately.

## Optional shell recovery

Enable shell recovery in **Settings → Terminal & sessions**. It is **off by default**. Once enabled, deck saves the current directory and up to 256 KB of recent plain output only while the pane is back at a shell prompt, with best-effort redaction of common credential shapes.

After a machine or tmux restart, opening the card starts a new shell in the saved directory and places old text in that pane's scrollback. A clear restart boundary separates the history from the new live prompt.

- Old text is written only as output, never replayed as commands.
- Old processes, jobs, environment variables and agent terminal interfaces are not restored.
- Snapshots expire after seven days. Disabling recovery clears them; Settings also offers a separate clear action.
- Redaction cannot guarantee removal of every sensitive value. Consider your terminal content before enabling it.

## When reopening does not resume work

Confirm deck is running, then inspect the card's terminal and **⏱** panel. It may be waiting for the original program, inspection, a quiet condition or resolution of an uncertain delivery. Avoid repeatedly sending prompts just to “recover”.

If the original process has ended, use the CLI's own context-restoration method. Old output and a retained card do not restore the process.
