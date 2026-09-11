# Input, settings and troubleshooting

Use voice drafts and shortcuts to reduce repeated typing, adjust your workspace in Settings, and keep your text while resolving a problem.

## Prepare a prompt by voice

1. Open the target session and click the header microphone. An empty draft starts recording.
2. Select the recognition language and speak. Stop to edit, or act directly on the text currently displayed while recording.
3. **Insert only** puts text in the terminal without Enter. **Send** inserts text and sends Enter. Acting on non-empty text stops recording first; an empty draft does nothing and leaves recording running.
4. Check the foreground program before sending, especially a permission menu, editor or shell. Send means delivery, not that the agent has accepted the task.

Enter adds a newline in the draft; ⌘Enter sends it. Clear discards the current text, and a successful send clears that session's draft. Each recording is limited to five minutes; some system recognizers may stop earlier.

Drafts, recognition language and target bindings belong to each session. Switching sessions while recording stops capture and keeps the original session's displayed text. It never starts recording in the new session. Bottom, Floating and Right share one editor; changing placement does not restart recording.

**Unsent drafts and placement last only for this app run and are lost when the app quits.** Copy text into your own document if you need to retain it.

## When voice cannot record or send

Recording requires a supported macOS 12+ on-device recognition configuration. Compatible macOS 26 devices and languages use the newer local engine. First use may need Apple language assets; follow setup guidance when they are unavailable. There is no cloud speech fallback.

| Situation | What to do |
| --- | --- |
| Microphone access denied | Follow the System Settings link, allow deck microphone access and explicitly start recording again |
| Speech Recognition permission needed | Allow Speech Recognition in System Settings, then retry |
| System Dictation disabled | Enable System Settings → Keyboard → Dictation, then return to deck and start recording |
| Unsupported language or assets not ready | Choose a supported language and follow system guidance to prepare its assets |
| Target session changed | Check the terminal; the next explicit Insert or Send rebinds and attempts delivery once |
| Delivery uncertain | Check the original terminal, then choose Retry after checking terminal or clear the draft. No automatic resend; if the original session was replaced, check and clear before composing again |

In 0.6.5, permission and Dictation setup errors offer the corresponding System Settings entry. Returning to deck never records or sends automatically. Recording failures keep available text, which may be incomplete.

Use **Settings → Terminal & sessions → Voice input** to change enabled languages and the recognition default. Chinese, English and Japanese are initially available. One enabled language hides the selector. **Follow system language** does not detect your spoken language. Preferences persist across restarts; an existing session retains a still-enabled language choice.

## Everyday controls

| Action | Control |
| --- | --- |
| New session | ＋ New session / ⌘N |
| Split right or down | ⌘D / ⌘⇧D, or header split buttons |
| Place an existing session in a split | Drag its sidebar card onto a pane edge |
| Collapse the sidebar | ⌘B |
| Copy and paste terminal text | Drag to select, then ⌘C / ⌘V |
| Select across screens | Hold a drag at the top or bottom terminal edge to extend through history |
| Accept command completion | Tab or → accepts the gray suggestion; only shell commands are recorded, not agent prompts |
| Open paths or websites | Click existing local paths or complete HTTP(S) links in terminal output |

## Settings and logs

Settings has six searchable categories: General & appearance, Shortcuts, Terminal & sessions, Integrations & automation, Data & logs, and About & updates.

Appearance offers Dark, Light, Follow System and High Contrast, with preset accents. Changes also update open terminal panes. Interface language can follow the system or use English or Simplified Chinese.

**Data & logs** exports or resets diagnostic logs. Reset asks for confirmation and clears only the current log. Command history, shell recovery data, exported logs and running sessions remain. History and recovery each have separate clear actions.

When [reporting an issue on GitHub](https://github.com/c9r-io/deck/issues/new?template=feedback.yml), include the version, channel, reproduction steps and expected behavior. Reports are usually public; inspect screenshots and exports before sharing and remove tokens, private paths and real task content.

## Updates and versions

This guide is based on **0.6.5**. Stable is the default channel; Nightly is an opt-in candidate channel whose features and stability may differ. Settings shows the version, channel and short commit.

Stable and Nightly replace the same app and share local data and tmux sessions. They cannot run side by side. Switching to Stable changes future update checks; it does not automatically downgrade a newer build. For example, switching an installed 0.6.6 Nightly to Stable does not install 0.6.5.

**Disabling human inspection does not undo its data upgrade.** First use moves relevant queue and settings data to v2; older builds without support refuse to read it. Do not edit version numbers or formats to bypass that check. Before downgrading, back up important data and verify the target version's compatibility. A data backup does not restore running processes.

For the complete maintenance reference, see the [0.6.5 release-channel documentation](https://github.com/c9r-io/deck/blob/caa944275d3313655df2585f4f552cecd9f10649/docs/release-channels.md). See [privacy](/privacy/) for application data and integrations.
