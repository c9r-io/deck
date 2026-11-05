# Leave, close and recover

Leaving a terminal view, quitting the app and ending a session are different actions. Decide whether you want work to continue before you close anything.

## What keeps running

| Action | Effect on the session | Effect on lists and automations |
| --- | --- | --- |
| Return to the Board or close a split pane | Leaves the view; does not directly end the session | The app remains running, so deliveries can continue |
| Close the app window (red button or ⌘W) | Hides the window; sessions continue | The app remains running, so deliveries can continue |
| Quit deck (⌘Q) | Sessions in private tmux keep running | No deliveries or new automation starts while the app is closed |
| Close a card | Ends that session and removes the card | Cancels that card's pending lists |
| Restart the Mac or background shell service | Existing processes end | Local lists remain saved. After a Mac restart they are rechecked when deck runs again; a shell-service restart initiated in Deck pauses affected lists until you resume them |

> For an automation configured to close its card, leaving the pane may allow automatic finishing. Closing a pane does not itself terminate a session, but it does not promise to keep such a run forever. [Automatic finish conditions →](/guide/automations/#finishing-and-inspection)

## End a session deliberately

Closing a card affects its running shell, agent and processes. The card corner × and right-click close act immediately only when neither a reminder nor a retained scratchpad requires confirmation; the in-session close action asks first. Closing a card with scratchpad entries also asks before deleting those notes. An active reminder requires explicit **Cancel reminder and close**; a remote or automatic close cannot silently cancel it. Check unsaved work and pending lists first.

Ctrl+D in a terminal is interpreted by the foreground program. It exits the shell only in applicable contexts, such as at a shell prompt. It is not a universal shortcut for closing deck.

An agent exiting usually returns to the shell. It does not necessarily end the card's entire session.

When Deck verifies a normal exit of an ordinary card's owning shell, it can retire that card and cancel its scheduled prompts. A missing session, service loss or signal termination is not that proof: the card stays stopped. MCP-origin cards, retained scratchpads and reminders have additional retention protection. A reminder protects the card, not a process; ending it does not release an automatic retirement it previously blocked.

## When the app is upgraded

Reopening the same build reuses the existing background service and processes. After installing a different build, deck may need to restart the background shell service. An empty service can be replaced automatically; an occupied service lists the effects and asks you to confirm.

Choose **Later** to keep using existing sessions. The pending restart stays in the sidebar and Settings; Deck creates no new session until the service is restarted. Confirming the restart ends all processes in that service; running agents cannot be moved into the replacement.

Reopening a stopped card normally starts a shell in its directory and **does not replay the launch command from card creation**. A card whose first command was never delivered is the exception. Look for an agent's resume hint and decide whether to restore it yourself. A list's delivery startup path may use the command its program needs; inspect the list separately.

## Reopen an Agent after a planned restart

Enable the matching session re-entry observation under **Settings → Agents & notifications** before relying on this option. Claude Code and Codex with `--no-daemon` can be eligible; Codex also needs its session hooks trusted in `/hooks`. Review the cards under **Reopen Agent sessions**, select the ones you intend to reopen, and confirm the restart. Deck ends the old clients, then makes one exact-session resume request for each eligible selected card. It sends no task prompt and does not keep the old process alive.

The default policy adds no bypass flag; for a bypass-launched Codex, it requests an ordinary profile because bare resume could retain bypass. Claude's native restored state is still its own. Reapplying bypass is a separate explicit choice for this restart only, limited to selected, verified bypass launches. Read that warning if permissions were lowered while an Agent was running.

**Affected lists stay paused even after re-entry.** Inspect the terminal and resume a list only when you want it to continue. Codex may report only “started”, not confirmed entry or task progress. An Agent with native queued work may continue on its own. If a result is unconfirmed, inspect the client that already started; Deck does not retry it. The result can offer **Insert resume command** for a card that was not reopened or started; this inserts text without submitting it.

## Recover after the shell service is lost

With Claude session re-entry observation enabled, Deck keeps a local record of the Claude session attached to each observed card. If both that shell service and client are gone, a separate **Recover interrupted Agent sessions** dialog can offer those cards. Review the selected cards and confirm before anything is launched. Each receives one plain exact-session resume request, with no bypass flag and no task prompt. This does not restore the old process or prove the task has resumed successfully.

Confirming or declining consumes the offered records; a confirmed recovery deletes them before launching, so it is not offered or retried again. Check any card that was not recovered manually. This recovery does not cover Codex or Claude launched with `CLAUDE_CONFIG_DIR`. It is separate from the shell-output snapshots below. [Recovery contract for this release](https://github.com/c9r-io/deck/blob/7f14b9b1e42a2f72230b421078b3a93cd8a02963/app/src-tauri/src/agent_reentry/recovery.rs).

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
