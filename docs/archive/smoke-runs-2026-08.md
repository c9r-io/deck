# Smoke run logs, August–September 2026

Point-in-time evidence moved out of `app/SMOKE.md` on 2026-09-06 so the live
checklist holds only current gates. Nothing here is an open task; open items
were carried into the live checklist when they still applied.

## Settings navigation run — 2026-09-05

Validation on 2026-09-05: the settings-only WKWebView run passed all checks;
103 Node tests (including coverage thresholds), 295 Rust tests, fmt and Clippy
passed. The final full WK run passed settings, font layout and link activation,
but `ime-routing` remained 5/7. The same 5/7 failure reproduced with an untouched
HEAD build in a separate data directory/socket; it is an existing regression,
not a passing release gate.

## v0.4.42 shell recovery and selection cursor — 2026-08-30

- [x] A user tested the isolated `deck smoke` bundle with real agent task-text
      selection and scrolling, and confirmed the cursor no longer detaches or
      remains fixed on an unrelated cell. The instance used a private data
      directory and `deck-smoke*` tmux socket and was removed afterwards.
- [x] The real-tmux contract freezes selection coordinates and bytes, clears
      tmux's mutable highlight, then verifies the former endpoint follows the
      live input edge and reports the cursor hidden after that row leaves the
      viewport. The full Rust suite passed 185 unit, 7 privacy and 36 bundled
      tmux contract tests.
- [x] A fresh real-WKWebView production-module run reported
      `selection-scroll-stable=1/3`, `selection-scroll-cursor=24/24`,
      `selection-overlay=1/1` and final `done=1`. The cursor check derives the
      expected row and visibility from xterm's live cursor before selection,
      then verifies backend status, frontend visibility state and removal of
      any offscreen `.xterm-cursor` marker after the frozen range scrolls.
- [x] The same fresh run exposed and removed an older smoke-only assumption
      that a tmux server and cwd fixture already existed. The harness now opens
      its harmless main pane before delete transactions and uses an existing
      cwd, so an empty machine reaches the terminal regressions deterministically.

## Nightly release channel candidate — 2026-08-29

- [x] Unit/static fixtures cover strict/invalid versions, source mismatch,
      non-incrementing candidates, candidate tag/commit/provenance mismatch,
      missing/tampered assets, malformed signatures/manifests, wrong target/
      URL/version/signature, Stable conflicts, prerelease state, forbidden
      promotion build commands and Stable resolver rejection of Nightly tags.
- [x] Settings default and legacy/unknown/damaged channel migration remain
      Stable. Nightly requires explicit confirmation; persistence failure
      restores the previous channel. Switching to Stable performs only a
      settings write and never requests an implicit downgrade.
- [x] Static/Rust gates prove the webview has no updater permission or endpoint,
      each backend check selects exactly one fixed Stable or Nightly URL, and
      Tauri retains download/signature/install ownership. Version/channel/commit
      identity is bounded to public non-personal fields.
- [x] actionlint validates `test.yml`, strict Stable `release.yml`, protected
      `nightly.yml` and copy-only `promote.yml`. Release tooling fixtures run in
      normal CI and in the Nightly pre-build gate.
- [ ] With explicit release authorization, publish two increasing, signed and
      notarized Nightly candidates. Install the first DMG, verify Gatekeeper and
      real data/tmux/scheduler/terminal/i18n survival, then verify Nightly
      self-update to the second while a Stable client sees neither prerelease.
- [ ] With explicit promotion authorization and production Environment review,
      promote the tested candidate. Record candidate/Stable URLs and workflow
      runs; compare DMG/archive/signature SHA-256 byte-for-byte; update an older
      Stable through `/releases/latest/`. No public Nightly, feed or Stable
      promotion is authorized merely by this checklist.

## Theme system candidate — 2026-08-29

- [x] Static/unit gates cover all four themes × four accent presets, require
      every CSS/xterm/native token, and verify ordinary semantic text/status/
      accent pairs at WCAG AA (≥4.5:1). High Contrast semantic pairs are
      required to reach ≥7:1; the light terminal's red/green/yellow/blue/
      magenta/cyan and bright variants are each checked at ≥4.5:1.
- [x] Deck Dark's base surface/text/status values remain the prior production
      values. Component CSS and dynamic styles contain no independent palette
      literals; xterm, focus, drag/drop, completion, modal, toast, warning and
      frozen-selection colors resolve through `ui/js/theme.js`.
- [x] Node DOM coverage proves a rejected `settings.json` write restores the
      previous theme/accent and selectors. Rust typed settings tests accept
      only the closed theme/accent enums while preserving unknown fields.
- [x] System-mode unit coverage drives light→dark appearance changes and proves
      a fixed theme detaches and ignores even a stale queued media callback.
- [x] Two consecutive isolated real-WKWebView production-module runs completed
      with `done=1`; `theme-switch` passed 7/7 and `theme-rollback` passed 1/1
      in both. The complete companion smoke also passed link classification
      31/31 (logical-line/tokenizer/provider debug mask 127/127), completion,
      nested split, selection, IME routing, scheduler and persistence faults.
- [x] User tested the separately launched isolated app and reported the theme
      functionality OK. The pass covered the interactive Settings switch and
      real application rendering rather than a browser mock; its data and tmux
      server were isolated from `~/.deck`. No screenshot artifact was retained.

## Prompt 05 automated candidate — 2026-08-29

- [x] Baseline and fixed candidates ran in real bundled WKWebView with fresh
      absolute smoke data directories and distinct private tmux sockets.
- [x] Environment recorded: macOS 26.6 (25G72), `@xterm/xterm` 5.5.0
      (vendored SHA-256
      `1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495`),
      tmux 3.7c. The initial capture reported ABC as the current/only selected
      source; the final capture reported Pinyin (`SCIM.ITABC`) as current after
      an external input-source change. The smoke did not drive a system IME
      candidate window, so neither observation completes the physical gate.
- [x] Independent tmux reproduction proved the old scroll command changed a
      completed range from `75:0→78:8` to `75:0→75:0` and removed its snapshot.
      The fixed token lease keeps start/end and bytes unchanged while the
      overlay's public row geometry changes with viewport offset.
- [x] Real-WK smoke verifies provider `activate` (not merely menu item
      construction), a 20+ line native clipboard oracle, immediate copy after
      pointerup, exact single-line Command-C, reverse/repeated selection,
      frozen scroll, overlay movement, split/detach/cancel, and an exact
      resize-to-selection grid race (92/92 characters). Synthetic
      events still do not prove `isTrusted`, a physical system Command-C, or a
      real IME candidate window.
- [x] User physical check in the isolated packaged candidate: real path click,
      mouse selection, multi-line external paste and post-selection scroll were
      reported basically OK. The exact byte/newline/hash oracle remains the
      independently generated WK smoke result above.
- [ ] Enable macOS Simplified Chinese Pinyin and, in the packaged app, enter
      ordinary Chinese plus half/full-width `[ ] ? ( ) { } < >`; verify Enter,
      Escape, Backspace and arrows during preedit, then switch back to ABC and
      repeat shifted punctuation. Record the macOS/input-source/keyboard-layout
      values with the release evidence. Do not mark these two physical gates
      complete from synthetic composition events.
- [x] Initial user Pinyin check found that special punctuation such as `?` was
      absent on the first physical keypress and appeared on the second; the
      follow-up trace and packaged retest below resolved that specific defect.
- [x] Follow-up physical trace isolated the loss to the first chord after every
      Shift keydown. Keeping modifier-only Shift out of xterm's byte path made
      the first chord work across repeated Shift release/press cycles; the
      user confirmed the packaged candidate behaves normally.
- [x] The follow-up link classifier smoke rejects nonexistent log tokens such
      as `memcache.go:265`, preserves one exact long HTTP(S) URL over soft wrap,
      recovers a full-width tmux redraw whose xterm `isWrapped` bit is absent,
      and proves that neither URL's `/api` segment is exposed as a second path
      link (31/31 in the real bundled WKWebView).

## v0.4.32 post-review automated candidate — 2026-08-29

The round-seven automated and WKWebView portions below have been executed.
Exact candidate carrier, selection sizes, fault results and the remaining
physical-input blocker are recorded in `RELEASE_REPORT_v0.4.32.md`.

- [x] The isolated debug review candidate was packaged as
      `deck-v0.4.32-review.app` and `deck_0.4.32_review_aarch64.dmg`
      (13,299,623 bytes, SHA-256
      `941bb8c28ed7791944238498c13f8e779be5be927472849a7c6f2cd903fbd584`).
- [x] Production-module pointer smoke crossed upward 135 logical lines
      (generated markers 2369→2499), then reverse-shrank by 962 characters.
      Its independent generator/hash oracle verified `R7C-0305`→`R7C-0398`
      as 3,854 bytes / 94 newlines. A separate
      downward drag crossed 113 logical lines from an existing history viewport.
- [x] Synthetic production routing proved one owner from pointerdown (31/31),
      blocked compatibility events, and passed light click, double-click word,
      triple-click line, single-screen drag and right-button routing (15/15).
- [x] Live output advanced history while selection remained active; resize,
      Escape/cancel, detach/re-attach and split isolation passed, with the
      sibling xterm/tmux both at 25 rows.
- [x] Completion geometry passed 255/255 and rapid owner switch/move/close
      passed 7/7; old/new pane xterm and tmux rows agreed at 24/24 shown and
      25/25 hidden.
- [x] Board overlap passed 255/255. Board first-save failure recovered with
      memory/disk equality; queue-cancel then Board-save failure retained the
      natural-exit card, cleared its active selection, and retired it once after
      recovery (63/63); ambiguous boot-save
      failure remained actionable and flushed to disk (1/1).
- [x] The isolated log contained no absolute home path, URL, raw generated
      session name, JavaScript error or CSP failure. The app and private tmux
      server were explicitly stopped after the run.
- [x] A user completed the physical gate with a real mouse/trackpad in the
      isolated review app and pasted into TextEdit. Light/double/triple click,
      single- and multi-screen selection, reverse shrink, split isolation,
      right-click and cancellation passed; `R7C-0305`→`R7C-0398` matched 3,854
      bytes, 94 newlines and the independent SHA-256 in the release report.

## Root TUI (only if you ship/run the legacy `deck` binary)
- [ ] `mv ~/.deck ~/.deck.bak && cargo run` → `ls -ld ~/.deck` is 0700 and
      `ls -l ~/.deck/board.json` is 0600 on the very first save
- [ ] `o` on a card opens $EDITOR on a notes file that is already 0600, in a
      0700 `~/.deck/notes/`
- [ ] `chmod 755 ~/.deck; chmod 644 ~/.deck/board.json` → relaunch → both are
      restricted again
