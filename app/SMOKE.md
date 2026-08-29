# deck release smoke checklist

`cargo test` covers the tmux contracts (scroll model, clear-history, literal
injection, poll formats). The items below are **WKWebView/xterm integration
behaviors that cannot be tested headless** — Chromium-based harnesses pass
while the real webview fails (that is how every regression in this list
originally shipped). Run through them in the `app/run.sh` build before
tagging a release; 3 minutes total.

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

## Input & rename
- [ ] New session → type `ls` → characters echo, Enter runs it (TSM/IMK alive)
- [ ] Chinese IME: type 中文, composition window appears, Enter commits
- [ ] With Simplified Chinese Pinyin and ABC in turn, type `[ ] ? ( ) { } < >`,
      shifted variants and full-width punctuation; no key silently disappears
- [ ] ⌘V pastes into the shell; ⌘C copies a selection out
- [ ] Rename a non-active session from the sidebar: Enter removes the editor
      immediately, persists exactly once, and updates Board/sidebar/open-pane
      titles. Reopen the app and confirm the title remains.
- [ ] Rename again: Escape restores without a write; click away commits once;
      Chinese IME Enter commits composition first and does not end editing.

## Scrolling & selection
- [ ] Fresh shell: trackpad scroll does nothing (no pull-down, no copy-mode badge)
- [ ] `seq 200` → scroll up reaches history, scroll to bottom auto-returns live
- [ ] Compare slow trackpad movement, fast swipes, inertia tails and direction
      reversal with Terminal.app/Warp: updates follow display frames without
      the old 50ms stepping, and sub-line input is not dropped.
- [ ] Scroll up and STOP: an accent "⤓ scrollback" chip appears in the pane
      header within the gesture (view is frozen history — an agent TUI must
      never look silently hung); clicking the chip OR typing returns live
      and the chip disappears
- [ ] Inside `claude`: long output scrollable; typing still reaches the agent
      (typing while scrolled first leaves copy-mode, so keys are never eaten
      as copy-mode commands)
- [ ] Drag-select multiple lines → ⌘C → paste elsewhere matches
- [ ] Without cancelling that selection first, immediately drag-select a
      different multi-line range. The new range replaces it without a
      "session changed" error or leaving the pane in copy-mode.
- [ ] Produce at least 2,500 deterministic rows containing Chinese, emoji,
      combining/ZWJ characters, blank lines, fenced-code markers, tabs,
      trailing spaces and a line wider than the pane. Drag directly on xterm
      cells into the top edge and hold: highlight and anchor remain continuous
      while tmux crosses multiple screens. Repeat downward from history.
- [ ] After crossing multiple screens, reverse direction within the same drag:
      the selection shrinks without duplicating, dropping or reversing rows.
      ⌘C copies only that logical selection; with no selection, the existing
      clipboard remains byte-identical.
- [ ] Paste into an external text target and verify start/end markers, order,
      hard blank lines, joined soft wraps, Unicode byte count and summary.
- [ ] Repeat in horizontal and vertical splits. Only the gesture's pane may
      scroll or highlight; its sibling keeps focus, viewport and xterm/PTY rows.
- [ ] While holding the selection, generate live output and resize the window,
      sidebar and divider. Then test Escape, pointer cancel, app blur, pane
      switch and detach: each stops edge work immediately and restores input.
- [ ] Select beyond 20,000 rows and at the 50,000-row history boundary. The UI
      announces the reachable limit; clipboard requests over 64 MiB fail
      explicitly and never return a truncated highlighted range.

## Board & cards
- [ ] Drag a card between boards (native drop must not swallow HTML5 DnD)
- [ ] Double-click board title renames (no render() mid-dblclick regression)
- [ ] Card ✕ closes instantly; in-session Close shows the custom confirm
      (window.confirm is a silent no-op in WKWebView — never use it)
- [ ] With delayed persistence, overlap two card closes; close+rename/move;
      project delete+unrelated create/rename; and a failed first write followed
      by a successful second mutation. Reload `deck.json`: it must exactly equal
      the final visible Board, with no resurrection or lost unrelated change.
- [ ] Ctrl+D/natural exit with queue-cancel or Board-save failure keeps the
      stopped card and pane visible, toasts only once, and retries. After durable
      success it closes the pane and retires once without repeated toasts.

## Completion & separators
- [ ] Second command typed shows gray ghost; Tab applies remainder only
- [ ] Test a fresh shell prompt, a scrolled-history prompt, a prompt on the
      last visible row, a long wrapped command, rapid input, pane resize, and
      horizontal/vertical/nested splits. The candidates occupy real reserved
      space and never cover any terminal row; only the focused pane shrinks.
- [ ] While candidates show, compare xterm rows and `tmux display -p
      '#{pane_width} #{pane_height}'`; they agree. Hide candidates and confirm
      both grow back, the prompt/cursor remains visible, and no extra jump or
      blank row is introduced.
- [ ] Separator lines appear between shell commands, none inside `claude`

## File drop & image paste (Warp-style path insertion)
- [ ] Take a screenshot (⌘⇧4) → drag its floating thumbnail onto a terminal
      pane → the pane outlines in accent, and on drop a quoted path under
      `~/.deck/drops/` is typed at the cursor (no Enter); the agent/shell can
      read that file
- [ ] Drag a file from Finder onto a pane → same path insertion; dragging a
      CARD between boards still works (file drags must not break card DnD)
- [ ] ⌃⌘⇧4 (screenshot to clipboard) → ⌘V in a pane → same: file saved,
      path typed; plain TEXT ⌘V still pastes as text
- [ ] `ls -l ~/.deck/drops` → files 0600, dir 0700; relaunch after 7 days
      (or backdate with touch) → old drops pruned

## Scheduler deletion (release gate — orphan sessions)
- [ ] Card with a recurring rule ("every 1 min") → close the card → the queue
      panel loses its rows, and after several minutes NO tmux session comes
      back: `tmux -L deck ls` shows nothing for it and `~/.deck/queue.json`
      lists the session under `cancelled`
- [ ] Same, but close the card in the second the prompt fires (rule due, hit
      ✕): the send may still land, `deliveries` records it, and still nothing
      re-arms or restarts
- [ ] Delete a whole project holding 2–3 scheduled cards → every one of their
      queue rows is gone at once, other projects untouched
- [ ] Ctrl+D a shell that has queued prompts → card retires itself and its
      queue rows go with it
- [ ] `chmod 400 ~/.deck/queue.json` → close a card → an explicit toast, the
      card STAYS on the board (never a silent delete with a live schedule);
      `chmod 600` back → closing works
- [ ] Start from a persisted `firing` item and force the boot repair save to
      fail. The UI still exposes acknowledge/retry immediately; the item stays
      ambiguous and cannot fire. Restore writes: the exact in-memory snapshot
      is flushed, remains ambiguous after restart, and the dirty flag clears.

## File-path menu

- [ ] Print relative and absolute paths containing spaces, Chinese and emoji,
      plus `:line[:column]`. Keyboard-open the menu: URL entries remain only
      Open/Copy; file entries include Open, Reveal, Copy, Open parent folder in
      editor, and New session in parent folder. Arrow/Home/End/Escape navigation
      and focus restoration work.
- [ ] Print a nonexistent log token (`memcache.go:265`), an IPv4 address with
      port, and a long HTTP(S) URL that soft-wraps through its `/api` segment.
      The first two have no link; every wrapped URL row resolves to one exact
      URL value and never exposes `/api` as a file path.
- [ ] Open parent uses the configured editor with the directory as an argument;
      New session starts in the canonical parent and follows the normal project/
      Board placement rules. Repeated clicks create at most one session. Missing,
      unreadable or stale paths show a safe error and create no ghost card/session.

## Splits
- [ ] ⌘D split; typing goes to the FOCUSED pane; no reflow jitter from the
      completion bar; divider drags

## PTY flow control
- [ ] `seq 1 500000` (or `yes | head -2000000`) → output streams smoothly to
      the end, scrollback intact at the tail (ACK window at work: no dropped
      or reordered bytes, no beachball)
- [ ] While it streams, close the pane mid-flood → no hang, no crash
      (detach closes the AckGate and releases the emitter); reopen the card
      → terminal repaints correctly (fresh generation, stale tail dropped)
- [ ] After heavy output, `grep "ack stall" ~/.deck/app.log` — a stall line
      is fine (it means the window did its job); the app must have stayed
      responsive throughout

## Update & settings
- [ ] Settings → editor list shows installed editors; file link opens there
- [ ] deck menu → Check for Updates… reports up-to-date (or offers install)

## Scheduler queue & templates
- [ ] Add an `at` prompt 1 min out on a harmless shell card whose launch command
      is empty → deck automatically binds the exact pane and fires once in
      compatibility mode; its row disappears (there is no "fired" UI; the send is
      recorded in queue.json's `deliveries` audit list, capped at 200 entries)
- [ ] Start a Codex/Claude/OpenCode card with an explicit launch command, queue
      a prompt, then put another program in the foreground → the row says it is
      waiting for the expected executable and attempts remain 0. Return that
      executable to the foreground → exactly one send lands. Do not configure
      any tmux pane hook for this test.
- [ ] Queue while a manually started agent is already foregrounded on a card
      with an empty launch command → deck captures that executable and waits if
      it later changes. Queue before the agent starts on the same kind of card
      → no process is captured; the prompt still sends to the exact same pane.
- [ ] Kill a scheduled session before it is due. On the next tick deck starts
      it and polls pane/process metadata rather than sleeping a fixed 2.5s. The
      expected executable appearing succeeds; a mismatch reaches the bounded
      timeout and remains blocked.
      While polling, separately pause, edit and delete items; each stops without
      a firing intent, delivery attempt, or ambiguous record.
- [ ] With one session blocked in boot readiness, a due prompt on a second
      session still advances independently.
- [ ] Add a chain of 2 prompts → they fire in order, second only after the
      first target went quiet (~3 min; "quiet" ≠ "done" — the UI must say
      quiet) and ≥60s after the first send (per-session min gap)
- [ ] Two prompts due at once on the SAME session → they arrive one per
      20s-tick, a minute apart — never both in one tick
- [ ] Schedule onto a stopped card whose directory was deleted → the row shows
      context unavailable/waiting, attempts do not increase, no firing intent
      is created, and a queued follow-up does not run past it. Separately force
      a real post-intent tmux refusal to retain the existing backoff/gave-up,
      retry ↻, skip ⏭ and group-blocking behavior.
- [ ] For a process-mismatch row choose keep waiting and cancel in turn.
      “Send now…” shows the expected and current processes; Enter and blur
      cannot accept its high-risk dialog, while an explicit click performs one
      process-only override without changing the saved expected executable.
- [ ] Replace a scheduled session/pane under the same name → immediate send is
      refused even after the danger confirmation path. Explicitly rebind to the
      new pane (or reschedule/cancel), then verify the new identity is persisted.
- [ ] Save a template from the queue group header → re-add it on another card
- [ ] Pause a recurring rule → skipped while paused; resume → fires again

## Data durability
- [ ] Quit deck → corrupt `~/.deck/deck.json` (truncate mid-JSON) → relaunch:
      board restores from `.bak`, a toast explains, the corrupt file is set
      aside as `.corrupt-<ts>` — NEVER silently replaced with an empty board
- [ ] Valid-JSON corruption too: replace deck.json's contents with `{"x":1}`
      → same recovery path (typed validation, not just a JSON parse)
- [ ] Delete `.bak` as well → relaunch shows a hard "could not be loaded"
      toast; deck runs with an in-memory board and does NOT write a default
      file until you actually change something
- [ ] Set `"schema_version": 99` in deck.json → toast says update deck; the
      file is left byte-identical (no .corrupt, no overwrite on save)
- [ ] Set `"schema_version": "1"` (a STRING) → treated as damage: recovery
      from `.bak` + `.corrupt-<ts>` kept, never read as a legacy file
- [ ] Delete the `data` key but keep `schema_version` → same recovery path
- [ ] Same truncate drill for `queue.json`
- [ ] Launch a second deck instance → alert "deck is already running", no
      data raced

## Privacy (release gate)
- [ ] `rm ~/.deck/app.log`, then: type a command with a distinctive marker
      string into a session, schedule a prompt containing the marker, export
      logs. `grep <marker> ~/.deck/app.log ~/.deck/exports/*` → ZERO hits.
      Bytes/counts/session names in logs are fine; user content is not.
- [ ] With Settings → debug logging ON, repeat — including ⌘V-pasting the
      marker into the shell (bracketed paste) and typing it through the IME:
      marker still absent (debug adds volume, never content; the frontend
      can only emit whitelisted event codes, per-code closed detail values
      and numbers)
- [ ] `grep -E '/Users/|file://' ~/.deck/app.log ~/.deck/exports/*` → zero
      hits (errors are logged as category codes; the tmux binary is logged
      as sidecar/homebrew/…, never as a path; storage recovery logs name
      files, never absolute paths)
- [ ] `grep -E 'deck-[a-z0-9]+-[a-z0-9-]+' ~/.deck/app.log ~/.deck/exports/*`
      → zero hits: sessions appear as `sess-xxxxx` tags, never by name
- [ ] Migration of what an OLDER deck left: append a fake legacy line
      (`echo "1 [pty] attached deck-my-card-ab12 /Users/$USER/secret" >>
      ~/.deck/app.log`), relaunch deck → the line is still there structurally
      but the name and path read `<redacted>`, the file is still 0600, and no
      `.bak` copy of the raw line exists anywhere in `~/.deck`
- [ ] Permissions: `ls -ld ~/.deck ~/.deck/exports` → `drwx------` (0700);
      `ls -l ~/.deck/*.json ~/.deck/*.json.bak ~/.deck/*.corrupt-* \
      ~/.deck/app.log ~/.deck/exports/*` → everything `-rw-------` (0600),
      including deck.json, queue.json, settings.json, history.json, every
      `.bak`, every quarantined `.corrupt-*` and every export
- [ ] `chmod 644 ~/.deck/deck.json; chmod 755 ~/.deck` → relaunch deck →
      both are back to 0600/0700 (boot-time migration)

## Security baseline
- [ ] `app.log` contains no `CSP` violation lines after a full session of use
      (the securitypolicyviolation listener logs any)
- [ ] A `file:///…` or non-http link printed in a terminal does NOT open on
      click (only http/https leave the app)

## Root TUI (only if you ship/run the legacy `deck` binary)
- [ ] `mv ~/.deck ~/.deck.bak && cargo run` → `ls -ld ~/.deck` is 0700 and
      `ls -l ~/.deck/board.json` is 0600 on the very first save
- [ ] `o` on a card opens $EDITOR on a notes file that is already 0600, in a
      0700 `~/.deck/notes/`
- [ ] `chmod 755 ~/.deck; chmod 644 ~/.deck/board.json` → relaunch → both are
      restricted again
