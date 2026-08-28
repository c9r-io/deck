# deck release smoke checklist

`cargo test` covers the tmux contracts (scroll model, clear-history, literal
injection, poll formats). The items below are **WKWebView/xterm integration
behaviors that cannot be tested headless** — Chromium-based harnesses pass
while the real webview fails (that is how every regression in this list
originally shipped). Run through them in the `app/run.sh` build before
tagging a release; 3 minutes total.

## v0.4.31 executed release gate — 2026-08-28

The round-six blockers were executed, not merely added to the checklist:

- [x] A debug arm64 `.app` was placed in a 0.4.31 DMG, mounted read-only, and
      run in its real WKWebView with a fresh private data directory and unique
      tmux socket. The 13,928 KiB candidate DMG had SHA-256
      `d3bcf7e484b8a02f231022032cb0af283db138d020b0b3c61a1966a1f8893888`.
- [x] Board overlap cases completed with mask 255 and disk JSON equal to the
      final in-memory Board. The scheduler boot-save-failure and natural-exit
      retry cases passed their deterministic Rust/Node regressions.
- [x] Sidebar Enter rename ended editing and updated all four surfaces once;
      the mounted app was killed and relaunched against the same isolated data,
      then reported the renamed title in both loaded UI state and disk.
- [x] A deterministic 2,204-row / 31,657-JavaScript-character Unicode capture
      auto-scrolled down and up. Native selection and Copy all produced distinct
      clipboard payloads; external paste measurement was 108 selection lines /
      2,280 bytes versus 2,203 full-output newlines / 47,071 bytes.
- [x] The real file menu retained two URL actions and exposed five path actions;
      editor-parent plus relative and absolute Unicode/space parent-session
      actions all completed without a ghost card.
- [x] Completion geometry passed mask 255 across bottom-follow, tmux scrollback,
      split pane, sidebar resize, rapid show/hide and a long full-width/emoji
      prefix. The terminal viewport was 617 px high, the reserved bar 38 px,
      their gap was 0 px without intersection, and the sibling gap was 5 px.
      xterm/tmux rows agreed at 24 while shown and 25 after hiding.
- [x] Smoke logs contained zero absolute-path/URL hits and zero raw generated
      session-name hits. macOS denied programmatic window capture without Screen
      Recording permission, so the retained evidence is the closed numeric DOM
      rectangles, overlap masks and PTY dimensions above rather than a screenshot.

The commit/tag SHA and signed release-DMG digest are recorded in the release
report after CI produces the notarized artifact; the debug digest above is only
the pre-tag functional carrier.

## Input & rename
- [ ] New session → type `ls` → characters echo, Enter runs it (TSM/IMK alive)
- [ ] Chinese IME: type 中文, composition window appears, Enter commits
- [ ] ⌘V pastes into the shell; ⌘C copies a selection out
- [ ] Rename a non-active session from the sidebar: Enter removes the editor
      immediately, persists exactly once, and updates Board/sidebar/open-pane
      titles. Reopen the app and confirm the title remains.
- [ ] Rename again: Escape restores without a write; click away commits once;
      Chinese IME Enter commits composition first and does not end editing.

## Scrolling & selection
- [ ] Fresh shell: trackpad scroll does nothing (no pull-down, no copy-mode badge)
- [ ] `seq 200` → scroll up reaches history, scroll to bottom auto-returns live
- [ ] Scroll up and STOP: an accent "⤓ scrollback" chip appears in the pane
      header within the gesture (view is frozen history — an agent TUI must
      never look silently hung); clicking the chip OR typing returns live
      and the chip disappears
- [ ] Inside `claude`: long output scrollable; typing still reaches the agent
      (typing while scrolled first leaves copy-mode, so keys are never eaten
      as copy-mode commands)
- [ ] Drag-select multiple lines → ⌘C → paste elsewhere matches
- [ ] Drag a selection UP to the top edge of the pane: the view does not
      scroll (tmux owns the history — this is the known limit) and deck
      toasts the way out ONCE
- [ ] ⧉ in the pane header (also ⌘⇧C, also right-click card → Copy output…)
      opens the copy panel: use at least 2,000 lines containing Chinese,
      emoji, blank lines, code-block markers and one line wider than the pane.
      Dragging at the bottom edge must continuously scroll down; dragging at
      the top must continuously scroll up. ⌘C copies only that native selection,
      while "Copy all" pastes the full capture byte-for-byte — including lines
      long since scrolled out of view and the unwrapped long line. Pointer-up,
      cancel, Escape, backdrop close and window blur must stop auto-scroll.

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
- [ ] Add an `at` prompt 1 min out on a harmless card (`while true; do date;
      sleep 1; done`) → fires once and its row disappears (there is no
      "fired" UI; the send is recorded in queue.json's `deliveries` audit
      list, capped at 200 entries)
- [ ] Add a chain of 2 prompts → they fire in order, second only after the
      first target went quiet (~3 min; "quiet" ≠ "done" — the UI must say
      quiet) and ≥60s after the first send (per-session min gap)
- [ ] Two prompts due at once on the SAME session → they arrive one per
      20s-tick, a minute apart — never both in one tick
- [ ] Schedule onto a card whose directory was deleted → the row shows
      "⚠ send failed, retrying"; after retries exhaust it shows "gave up —
      blocks later steps" with ↻ retry and ⏭ skip; a queued follow-up in
      the same group shows "⏸ waiting" and does NOT fire until skip/retry
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
