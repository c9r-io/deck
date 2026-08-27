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

## Update & settings
- [ ] Settings → editor list shows installed editors; file link opens there
- [ ] deck menu → Check for Updates… reports up-to-date (or offers install)

## Scheduler queue & templates
- [ ] Add an `at` prompt 1 min out on a harmless card (`while true; do date;
      sleep 1; done`) → fires once, queue row moves to fired
- [ ] Add a chain of 2 prompts → they fire in order, second only after the
      first target went quiet (~3 min; "quiet" ≠ "done" — the UI must say
      quiet)
- [ ] Save a template from the queue group header → re-add it on another card
- [ ] Pause a recurring rule → skipped while paused; resume → fires again

## Data durability
- [ ] Quit deck → corrupt `~/.deck/deck.json` (truncate mid-JSON) → relaunch:
      board restores from `.bak`, a toast explains, the corrupt file is set
      aside as `.corrupt-<ts>` — NEVER silently replaced with an empty board
- [ ] Same drill for `queue.json`
- [ ] Launch a second deck instance → alert "deck is already running", no
      data raced

## Privacy (release gate)
- [ ] `rm ~/.deck/app.log`, then: type a command with a distinctive marker
      string into a session, schedule a prompt containing the marker, export
      logs. `grep <marker> ~/.deck/app.log ~/.deck/exports/*` → ZERO hits.
      Bytes/counts/session names in logs are fine; user content is not.
- [ ] With Settings → debug logging ON, repeat: marker still absent (debug
      adds volume, never content)

## Security baseline
- [ ] `app.log` contains no `CSP` violation lines after a full session of use
      (the securitypolicyviolation listener logs any)
- [ ] A `file:///…` or non-http link printed in a terminal does NOT open on
      click (only http/https leave the app)
