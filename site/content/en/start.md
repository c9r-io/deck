# Start your first task

Begin work in a real terminal, then return to the Board. Each card represents a persistent session.

## Install and prepare

1. [Download Stable](https://github.com/c9r-io/deck/releases/latest), open the DMG and drag deck to Applications. Releases are signed and Apple-notarized; Apple Silicon Macs are currently supported.
2. Open deck. No deck account is needed, and tmux is included.
3. Prepare the CLI you want to run. deck provides the terminal and controls; installation, sign-in, model access and charges belong to the CLI you choose. You can also start with a plain shell.

## Create and run

1. Select a project and click **＋ New session**. Without project defaults, the shell opens in your home directory.
2. Change to your working directory in the terminal and run an installed CLI, such as `claude` or `codex`.
3. Enter a task as you would in any terminal. Double-click the title to rename it, for example “Review changes”.
4. Use the back button to return to the Board. The agent keeps running in its session. Click the card to return to it.

Board groups are manual. Drag cards to place them, and click a group's empty area to choose where new sessions go. Live status never moves a card.

## Make the next start shorter

Open **New session ▾ → Project defaults…**, also available by right-clicking the project tab. Set an optional directory and launch command. **＋** and ⌘N then use these defaults and send the command once on the first start.

**New shell only** uses the project directory without sending its command. Creating a session from a path menu or a card's “in this directory” action uses that location and always starts a shell only.

If the directory no longer exists, deck offers cancel, edit defaults or a shell in your home directory. It does not save a new card for a failed start.

## Know when to return

Work on something else, then check **Needs attention** in the sidebar for input requests and unread turn endings across projects. Explicit agent states require the matching integration; without it, deck can only observe output activity. [Set up and understand signals →](/guide/attention/)

> An empty Needs attention view does not mean all work is complete. Sessions without a valid status report may still need a look.

## Before you leave

Returning to the Board or closing a split pane leaves the terminal view. **Closing a card ends its session** and cancels its pending lists; it is not a way to hide a window.

Sessions survive quitting the app, but deck stops delivering lists while it is closed. Machine restarts and upgrade service restarts have different effects. See [leave, close and recover](/guide/sessions/).
