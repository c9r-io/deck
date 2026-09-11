# Voice input

The session header's microphone opens Deck's voice draft and begins recording
when the draft is empty. Audio is recognized on the Mac. **Clear**, **Insert
only**, and **Send** (or Command-Enter) also work during recording, preparation,
and finalization. With non-blank text, they stop capture and act on exactly the
text displayed when clicked; late transcription cannot alter that text or refill
a cleared draft. With empty or whitespace-only text, they do nothing and leave
recording running. **Stop** remains available to finish the transcript before
editing it. Delivery still requires a valid target and prevents duplicate clicks.
Enter in the editor inserts a newline; IME confirmation never submits. Continue
recording appends to the existing draft. Each recording is limited to five
minutes; older system recognition may end an utterance earlier.

**Bottom** is the default. **Floating** is an overlay inside the same window;
**Right** docks beside the terminal. These positions share one mounted editor,
recording and target. Switching preserves text, selection and recording.
Placement is remembered only for this app run. Right reduces terminal width;
Bottom reduces height; Floating obscures part of the terminal.

Draft text, recognition language and target binding belong to each session.
Selecting another session (including a split pane) restores its own draft and
automatically selects its target; returning does not require rebinding. Switching
while recording releases the microphone and keeps the last displayed partial
transcript in the original session. It never starts recording in the new session.
Successful sends clear only that session's draft and retain its target binding.

The language menu defaults to Chinese (Simplified), English and Japanese.
**Settings → Terminal & sessions → Voice input** lets you choose the available
languages and default recognition language. At least one language must remain;
with only one enabled, the draft hides its language selector. **Follow system
language** selects from enabled languages; it does not detect the spoken language.
These preferences persist across app restarts. Existing sessions keep an enabled
language when the default changes. Removing their language applies the default,
deferred until recording finishes if capture is active. Changing preferences
does not request microphone permission or download models; assets are requested
only when recording needs them.

The session/pane identity is captured before recording. There is no manual
**Confirm current target** step. Each explicit send uses the session's current
foreground program, then checks it and the tmux pane generation at paste and
again at Enter. A change during that operation blocks delivery; replacing the
session/pane also blocks that delivery and reports an expired binding. The draft
is preserved; after checking the terminal, the next explicit Insert/Send action
binds the replacement and attempts delivery once. Merely switching sessions or
reopening the panel never resends. A refusal before any text is pasted (such as
unsupported multiline paste) keeps the session binding, so correcting the text
and sending again requires no rebinding. These checks do not establish that an
agent is ready for a prompt: review the terminal, especially a permission menu,
shell or editor. Multi-line interactive input requires the target program to
enable bracketed paste, including for Insert only.

The exact literal-paste implementation and per-session busy exclusion are
shared with scheduled prompts, but interactive voice input has no schedule,
quiet wait or one-minute rate limit. Prompt text is never evaluated by a Deck
shell command. Send injects text and a separate Enter; it does not establish
that the agent accepted or completed the task. Transport ambiguity or an
unconfirmed Enter preserves the draft and requires checking the terminal and
**Retry after checking terminal** before repeating the operation. This retries
the original Insert only or Send action against the same session, without a
rebinding step. Ordinary Send/Insert clicks remain disabled until that retry
or clearing the uncertain draft. If that original session has been replaced,
retry cannot move the uncertain text to the replacement: check the terminal,
then clear the draft before composing a new message. Switching sessions does not authorize a retry. No automatic retransmission.

Closing the panel, leaving the session view, closing its target pane, or closing
the app window cancels capture and preserves the last displayed draft in frontend
memory. Hiding or minimizing the app stops native capture and finalizes it.
Per-session drafts, language choices, bindings and placement do not survive app restart. Recording errors preserve
available text, which may be incomplete. A finalization timeout is 15 seconds.

## System integration

- Swift code is compiled and statically linked into the signed application.
  There is no runtime Swift/Python process, helper executable, launch service
  registration, global keyboard listener or Accessibility permission.
- With a macOS 26 SDK build on a compatible macOS 26+ device/locale,
  `SpeechAnalyzer` / `SpeechTranscriber` provide local transcription. Apple
  manages language assets; the first recording may download a model.
- Otherwise, macOS 12+ uses `SFSpeechRecognizer` only when
  `supportsOnDeviceRecognition` is true, with `requiresOnDeviceRecognition`
  explicitly set. Unsupported configurations show an error, never a cloud
  fallback. macOS 11 can still run Deck and edit drafts, but recording is
  unavailable (the native bridge needs the system Swift concurrency runtime).
- The legacy recognizer also depends on macOS Dictation being enabled. If it
  returns `kLSRErrorDomain / 201`, enable **System Settings → Keyboard →
  Dictation**, then explicitly start recording again. This system requirement
  applies equally to shell and agent sessions and is separate from microphone
  and Speech Recognition app permissions. Deck now shows specific setup guidance
  for that error, including when wrapped by another framework error.
- Release builds require Swift 6.2+ and the macOS 26 SDK, and fail compilation
  instead of silently omitting the modern path. CI and nightly builds select
  Xcode 26.3 on macOS 15 explicitly (`DECK_REQUIRE_MODERN_SPEECH=1` also enforces
  this in debug tests). Older developer toolchains can still build legacy debug
  apps. A new OS alone cannot enable a modern path omitted at build time:
  the installed 0.6.4 investigated here used SDK 14.5, while the working local
  isolated bundle used SDK 26.5 and included `SpeechAnalyzer`.
- Mic permission is requested on explicit recording only. The legacy recognizer
  also requests Speech Recognition permission. Denials have settings guidance.
  Release and development bundles carry purpose strings and Audio Input
  entitlement; no hardened-runtime exception for unsigned code or JIT is added.
- After an explicit recording attempt fails because microphone access, Speech
  Recognition access or system Dictation is disabled, Deck releases capture and
  opens the corresponding System Settings pane once. The error also offers
  **Open System Settings** for reopening it. Returning to a draft never opens
  settings by itself. If the system cannot open the pane, the manual path remains
  visible. Permission toggles and Dictation enablement are completed by the user;
  returning to Deck does not automatically record or send.
- There are no external speech API credentials or audio uploads. Audio buffers
  and transcripts are transient; no recordings or drafts are written to disk or
  logs. A transcript reaches the selected CLI only when the user sends/inserts
  it; that CLI's own data handling still applies.

This narrows the new process/network behavior; it is not a guarantee that every
enterprise EDR policy will permit microphone access. Test the signed distribution
in the target environment.

## Validation

`scripts/test-speech-bridge` compiles the production Swift bridge and exercises
direct/wrapped Dictation-disabled errors, unrelated errors and bounded error
traversal without requesting microphone access. It runs in CI and the nightly gate.

`scripts/ui-tests` covers recorder/target lifecycle, layout state preservation,
session switching during recording and delivery, late callbacks, cancellation,
errors and duplicate-delivery prevention, plus production voice DOM handlers
and binding recovery. Its coverage inventory fails if any non-excluded production
JS module is absent from the report; smoke carriers are not counted as Node tests.
`cargo test --workspace -- --test-threads=1` includes the native bridge EDR
tripwires, recording lifecycle and stale-snapshot checks, input validation,
scheduler exclusion and generation expiry. Shared delivery tests execute the
production implementation through its transport boundary, including real guarded
pastes against an isolated bundled tmux server. Swift/Apple speech device behavior
is not measured by the Rust or Node coverage gates; the device checks below remain
required for recognition changes.

Use a **fresh** isolated directory for the real WebView + IPC + terminal check:

```sh
DECK_SMOKE_DATA_DIR=/tmp/deck-voice-smoke-unique \
DECK_SMOKE_TMUX_SOCKET=deck-smoke-voice-unique \
DECK_SMOKE_WKWEBVIEW=voice app/run.sh
```

That deterministic smoke never activates the microphone. In a signed build,
also check microphone/legacy Speech authorization, denial, missing models,
offline recognition with installed assets, short Chinese/English/Japanese
utterances, mixed technical terms, stopping during preparation, device removal,
window hide/minimize, and recording while switching all three placements.
Speech accuracy and enterprise EDR behavior require the actual device/policy.
