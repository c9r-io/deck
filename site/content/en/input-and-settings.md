# Input, settings and troubleshooting

Use on-device voice input and shortcuts in your terminals, adjust your workspace in Settings, and troubleshoot delivery or recording problems.

## Type into a terminal by voice

1. Open the intended session and pane, check the foreground program, then click the microphone in that pane's header.
2. Speak. Confirmed words are typed into the pane as they arrive. A faint caption shows words still being recognized; that unfinished text has not been typed.
3. Click the microphone again to stop. Deck finishes recognition and types any remaining confirmed words. Edit the text in the terminal, then press Enter yourself when ready.

Deck **never sends Enter for voice input**. There is no separate voice draft, Insert, or Send button. Text already typed stays in the pane if you stop or switch away. On supported macOS 26 devices, confirmed segments may arrive during speech; the older on-device recognizer may type the utterance only when it ends. Each recording is limited to five minutes, and some system recognizers may stop earlier.

Switching sessions or panes, leaving the session, or hiding the window stops recording. Words not yet delivered can be dropped. Deck does not automatically retry an uncertain paste; check the terminal before speaking again. Recording does not prove that the foreground agent is ready—be especially careful at a permission menu, editor, or shell prompt.

Recognition uses your Mac; Deck does not save audio or transcripts to disk. Text typed into a terminal may still be handled by the program running there under that program's own rules.

## When voice cannot record or type

Recording requires a supported macOS 12+ on-device recognition configuration. Compatible macOS 26 devices and languages use the newer local engine. First use may need Apple language assets; follow setup guidance when they are unavailable. There is no cloud speech fallback.

| Situation | What to do |
| --- | --- |
| Microphone access denied | Follow the System Settings link, allow deck microphone access and explicitly start recording again |
| Speech Recognition permission needed | Allow Speech Recognition in System Settings, then retry |
| System Dictation disabled | Enable System Settings → Keyboard → Dictation, then return to deck and start recording |
| Unsupported language or assets not ready | Choose a supported language and follow system guidance to prepare its assets |
| Target pane or foreground program changed | Check the original terminal and start a new recording in the intended pane |
| Typed text is uncertain | Check the terminal before trying again; Deck does not automatically resend it |

Permission and Dictation setup errors offer the corresponding System Settings entry. Returning to deck never records automatically.

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

This guide covers **deck 0.7.8 Stable**. Stable is the default channel; Nightly is an opt-in candidate channel whose features and stability may differ. Settings shows the version, channel and short commit.

Stable and Nightly replace the same app and share local data and tmux sessions. They cannot run side by side. Switching to Stable changes future update checks; it does not automatically downgrade a newer build. When the same Nightly version is promoted to Stable, the app archive is unchanged; no reinstall is needed just for the channel name. If you have a higher version installed, check data compatibility first.

**Disabling human inspection does not undo its data upgrade.** First use moves relevant queue and settings data to v2; older builds without support refuse to read it. Do not edit version numbers or formats to bypass that check. Before downgrading, back up important data and verify the target version's compatibility. A data backup does not restore running processes.

For the complete maintenance reference, see the [release-channel documentation](https://github.com/c9r-io/deck/blob/692438f9310f079743700a10bda7cf80f77b06c6/docs/release-channels.md). See [privacy](/privacy/) for application data and integrations.
