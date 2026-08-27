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
