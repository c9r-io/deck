# Voice input

The session header's microphone starts a recording for the focused pane; the
same button stops it. Speech is recognized on the Mac. Text the recognizer has
confirmed is typed into that pane while you are still speaking, exactly as if
it came from the keyboard: no Enter is ever sent, so you correct it with the
keyboard and press Enter yourself. There is no draft, panel, clear, insert or
send. Keyboard and voice share one input.

While recording, the button shows the elapsed time; during preparation, model
download or finalization it shows an ellipsis. A translucent caption under the
button shows what has been heard but not yet typed (the recognizer's volatile
tail); it is display only, never typed, and disappears when the recording ends. **Stop** finishes recognition and
types the remaining confirmed words (finalization is bounded to 15 seconds).
Each recording is limited to five minutes; older system recognition may end an
utterance earlier, which ends the recording the same way as Stop.

Only confirmed text is typed. Volatile partial results reach the caption and
nothing else: not the terminal, not the log. With the modern recognizer, confirmed segments arrive during the
recording; with the legacy recognizer, the whole utterance is confirmed only
when the recording ends, so it is typed then. Within a recording, a word
separator is typed together with the word that follows it, and the last blank
of an utterance is dropped. Line breaks in recognized text become spaces, so a
recording can never execute a shell command.

The recording is bound to the pane (session, window, pane and process
generation) captured when it started, and every typed slice re-checks that
identity and the pane's current foreground program atomically with the paste.
Switching sessions or panes, leaving the session view, the pane's exit, a
hidden window or page unload ends the recording; words already typed stay,
words not yet submitted for delivery are dropped. A delivery already submitted
keeps its original target; a late reply cannot alter a newer recording. A replaced session, a lost pane, a
program change during the paste, another message being delivered to the same
session, or text the program cannot take end the recording with a toast; a
transient refusal is retried briefly first. If a paste could not be confirmed,
it is counted as typed, a toast says so, and nothing is retransmitted. There
is no automatic retry and no automatic Enter. These checks do not establish
that an agent is ready for input: review the terminal, especially a permission
menu, shell or editor.

Text reaches tmux through a stdin pipe, never process arguments, environment
variables or temporary files. The literal-paste implementation and per-session
busy exclusion are shared with scheduled prompts. Both hold the session activity
guard throughout transport and cleanup, excluding service restart, but voice input has no
schedule, quiet wait or rate limit, and never sends Enter. Voice text is never
evaluated by a Deck shell command.

**Settings → Terminal & sessions → Voice input** chooses the available
recognition languages and the default. **Follow system language** selects
among the chosen languages; it does not detect the spoken language. The
default in force when a recording starts is its language for that recording.
Changing preferences never requests microphone permission or downloads
models; assets are requested only when a recording needs them. Nothing about
a recording survives an app restart.

## System integration

- Swift code is compiled and statically linked into the signed application.
  There is no runtime Swift/Python process, helper executable, launch service
  registration, global keyboard listener or Accessibility permission.
- With a macOS 26 SDK build on a compatible macOS 26+ device/locale,
  `SpeechAnalyzer` / `SpeechTranscriber` provide local transcription and
  deliver confirmed segments progressively. Continuous speech without pauses
  finalizes late and lands at Stop; the caption shows it meanwhile. The
  `fastResults` reporting option was tried and rejected: it finalized sooner
  but typed noticeably worse text. Apple manages language assets; the first
  recording may download a model. `app.log` records which engine ran as
  `[voice] engine=modern` or `engine=legacy`, nothing else about a recording.
- Otherwise, macOS 12+ uses `SFSpeechRecognizer` only when
  `supportsOnDeviceRecognition` is true, with `requiresOnDeviceRecognition`
  explicitly set. Its text is confirmed only at the end. Unsupported
  configurations show an error, never a cloud fallback. macOS 11 can still run
  Deck, but recording is unavailable (the native bridge needs the system Swift
  concurrency runtime).
- The legacy recognizer also depends on macOS Dictation being enabled. If it
  returns `kLSRErrorDomain / 201`, enable **System Settings → Keyboard →
  Dictation**, then explicitly start recording again. This system requirement
  applies equally to shell and agent sessions and is separate from microphone
  and Speech Recognition app permissions. Deck shows specific setup guidance
  for that error, including when wrapped by another framework error.
- Release builds require Swift 6.2+ and the macOS 26 SDK, and fail compilation
  instead of silently omitting the modern path. CI and nightly builds select
  Xcode 26.3 on macOS 15 explicitly (`DECK_REQUIRE_MODERN_SPEECH=1` also enforces
  this in debug tests). Older developer toolchains can still build legacy debug
  apps. A new OS alone cannot enable a modern path omitted at build time.
- Mic permission is requested on explicit recording only. The legacy recognizer
  also requests Speech Recognition permission. Denials have settings guidance.
  Release and development bundles carry purpose strings and Audio Input
  entitlement; no hardened-runtime exception for unsigned code or JIT is added.
- After an explicit recording attempt fails because microphone access, Speech
  Recognition access or system Dictation is disabled, Deck releases capture,
  shows the guidance as a toast and opens the corresponding System Settings
  pane once. If the system cannot open the pane, the guidance still names the
  manual path. Permission toggles and Dictation enablement are completed by the
  user; returning to Deck does not automatically record.
- There are no external speech API credentials or audio uploads. Audio buffers
  and transcripts are transient; no recordings or transcripts are written to
  disk or logs. A transcript reaches the selected CLI only by being typed into
  it; that CLI's own data handling still applies.

This narrows the new process/network behavior; it is not a guarantee that every
enterprise EDR policy will permit microphone access. Test the signed distribution
in the target environment.

## Validation

`scripts/test-speech-bridge` compiles the production Swift bridge and exercises
direct/wrapped Dictation-disabled errors, unrelated errors and bounded error
traversal without requesting microphone access. It runs in CI and the nightly gate.

`scripts/ui-tests` covers the slicing rule, progressive and end-only typing,
stop/cancel/switch lifecycle, late callbacks, permission and recognition
failures, delivery failures and bounded retry, unconfirmed pastes, the header
button's states and toasts. Its coverage inventory fails if any non-excluded
production JS module is absent from the report; smoke carriers are not counted
as Node tests. `cargo test --workspace -- --test-threads=1` includes the native
bridge EDR tripwires, recording lifecycle and stale-snapshot checks, input
validation, byte-literal typing during capture, scheduler exclusion and
generation expiry and restart exclusion. Frontend regressions cover cancellation
during pane preparation and late success/failure after a new recording starts.
Shared delivery tests execute the production implementation
through its transport boundary, including real guarded pastes against an
isolated bundled tmux server. Swift/Apple speech device behavior is not
measured by the Rust or Node coverage gates; the device checks below remain
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
utterances typed progressively into an agent prompt and into a bare shell,
mixed technical terms, stopping during preparation, device removal, window
hide/minimize, and switching sessions mid-recording. Speech accuracy and
enterprise EDR behavior require the actual device/policy.
