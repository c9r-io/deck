# deck release smoke checklist

`cargo test` covers the tmux contracts (scroll model, clear-history, literal
injection, poll formats). The items below are **WKWebView/xterm integration
behaviors that cannot be tested headless** — Chromium-based harnesses pass
while the real webview fails (that is how every regression in this list
originally shipped). Run through them in the `app/run.sh` build before
tagging a release; 3 minutes total.

## Input
- [ ] New session → type `ls` → characters echo, Enter runs it (TSM/IMK alive)
- [ ] Chinese IME: type 中文, composition window appears, Enter commits
- [ ] ⌘V pastes into the shell; ⌘C copies a selection out

## Scrolling & selection
- [ ] Fresh shell: trackpad scroll does nothing (no pull-down, no copy-mode badge)
- [ ] `seq 200` → scroll up reaches history, scroll to bottom auto-returns live
- [ ] Inside `claude`: long output scrollable; typing still reaches the agent
- [ ] Drag-select multiple lines → ⌘C → paste elsewhere matches

## Board & cards
- [ ] Drag a card between boards (native drop must not swallow HTML5 DnD)
- [ ] Double-click board title renames (no render() mid-dblclick regression)
- [ ] Card ✕ closes instantly; in-session Close shows the custom confirm
      (window.confirm is a silent no-op in WKWebView — never use it)

## Completion & separators
- [ ] Second command typed shows gray ghost; Tab applies remainder only
- [ ] Separator lines appear between shell commands, none inside `claude`

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
