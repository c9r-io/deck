# deck v0.4.32 release report

Date: 2026-08-29 (Asia/Tokyo)

## Scope and authority model

The governed data flow is:

`tmux history/copy-mode → PTY repaint → xterm viewport → pointer coordinator → native clipboard`

tmux copy-mode is the only selection authority. The frontend takes ownership
of every physical primary-pointer gesture at `pointerdown`, before xterm can
start its mouse service. A drag is promoted to tmux; a sub-threshold click is
replayed once to xterm with its single/double/triple click count. Physical
compatibility mouse events are blocked through the end of the gesture, so
xterm and tmux cannot own the same drag.

tmux reports directional, half-open cell endpoints. Production extraction
normalizes forward/reverse endpoints, converts cell columns to grapheme
boundaries, and uses `capture-pane -J` to preserve hard newlines and real
trailing spaces while joining soft wraps. Mouse cell columns are separately
converted to tmux grapheme-step counts, preventing every endpoint after a wide
character from shifting right.

The reachable history limit is 50,000 terminal rows. Clipboard extraction has
an explicit 64 MiB rejection limit and never truncates a highlighted range.

## Post-review candidate carrier

- App: `/tmp/deck-r7-review.C01Yz2/deck-v0.4.32-review.app`
- DMG: `/tmp/deck-r7-review.C01Yz2/deck_0.4.32_review_aarch64.dmg`
- Carrier: debug arm64 review bundle in a compressed DMG, using only an
  isolated temporary data directory and a unique tmux socket
- DMG size: 13,299,623 bytes
- DMG SHA-256: `941bb8c28ed7791944238498c13f8e779be5be927472849a7c6f2cd903fbd584`
- Both `CFBundleShortVersionString` and `CFBundleVersion`: `0.4.32`
- Signature: ad-hoc debug signature; this is not a formal release asset

## Executed WKWebView evidence

The packaged candidate loaded the real production modules in WKWebView and
reported each scenario through a closed diagnostic code:

- Direct upward terminal selection crossed 135 logical lines in the
  2,500-row mixed Unicode fixture; the visible generated marker range was
  `R7-2369` through `R7-2499`, and the selected JavaScript length was 4,439.
- Reverse shrink removed 962 characters and left 3,477, without changing the
  anchor or reversing row order. Downward selection from an existing history
  viewport crossed 113 logical lines / 2,884 characters.
- The synthetic audit of the physical-event ownership route passed 31/31:
  pointerdown was owned
  before xterm, an in-gesture compatibility mousedown and a late mouseup were
  both blocked, the drag was promoted exactly once, no click was replayed for
  the drag, and xterm retained no native selection.
- Supplemental real-WK synthetic routing passed 15/15 for a light tap, double
  click word selection, triple click line selection, ordinary single-screen
  drag and an untouched right-button pointer path. This supplements but does
  not replace the physical-input gate below.
- The production Command-C path was checked without calling
  `copyTerminalSelection()` to construct expected output. The independent
  generator selected the half-open literal range `R7C-0305` through
  `R7C-0398`, including its final hard newline: 3,854 UTF-8 bytes, 94
  newlines, FNV-1a-64 `d21c209269ac4699`, and SHA-256
  `a7f11990dad9893150f4a0675a6ee75d359c8fc699cea34f699626a64cba4d8a`.
- Live output advanced while selection stayed active. The PTY stream continued
  through its bounded four-batch ACK gate and the full smoke reached `done=1`.
- Resize, Escape/cancel, detach/re-attach and split isolation passed. Only the
  gesture-origin pane entered copy-mode; its sibling xterm/tmux rows agreed at
  25/25.
- Completion geometry passed 255/255. Rapid A→B→A, pane move and owner close
  passed 7/7; xterm/tmux rows agreed at 24/24 with reserved space and 25/25
  after hiding.
- Board persistence fault recovery passed: the injected first save failed,
  the next mutation committed, and memory matched disk.
- Natural-exit fault recovery passed 63/63: queue-cancel and Board-save
  failures retained the pane/card, recovery retired it once, and a late poll
  was a no-op.
- Ambiguous boot repair passed 1/1 after a forced first queue-save failure;
  the decision remained actionable and the recovered state flushed cleanly.
- No home path, URL, raw generated session name, JavaScript error or CSP
  failure appeared in the isolated log. Data files were 0600 and their
  directory was 0700. The candidate and private tmux server were terminated.

## Production extraction contract

The integration contract establishes a selection in one isolated tmux pane,
then compares tmux's own `copy-selection-no-clear` bytes with the exact module
called by production `terminal_selection_copy()` using `capture-pane -J`.

All byte comparisons pass for single-line and multi-line forward/reverse
selection, one ASCII character, line start/end, an empty line, hard and soft
wrap boundaries, real trailing spaces, a Chinese wide character requested
through its second cell, a combining grapheme, and a ZWJ emoji. The literal
column 3→7 case is `DEFG`, proving `selection_end_x` is exclusive and no
additional cell is copied.

## Removed auxiliary copying surface

Removed from production: the pane-header entry, card/session menu route,
dedicated shortcut, document DOM/CSS, retry and whole-document controls,
request guard, auto-scroll timers/listeners/pointer capture, state slot,
frontend helpers, backend scrollback-capture command/registration and obsolete
smoke/documentation. A static gate scans production sources so these entry
points cannot silently return.

## Deterministic gates

- Node: 33 tests pass, including the 2,500-row Unicode payload, selection
  model, stale replies, retirement concurrency, independent WK clipboard
  oracle and production static wiring.
- App Rust: 131 unit tests, 6 log-privacy tests and 14 tmux contracts pass.
- Root Rust: 10 tests pass.
- Both Rust workspaces pass fmt and Clippy with warnings denied; every frontend
  module passes `node --check`, the identifier checker passes, and
  `git diff --check` passes.
- Retirement overlapping drains call close/success exactly once per sid;
  blocked A does not block B, and joiner/no-op and disposed late completion do
  not emit success.
- Fault hooks remain debug-only, require explicit WKWebView smoke mode plus an
  absolute isolated data root, accept only three closed enum values with
  counts 0 through 8, and are inert for normal/release launches.

## Physical input and external paste gate

The user executed the release-blocking gate on 2026-08-29 in a separately
launched review bundle with a fresh temporary data directory and independent
tmux socket. Using a real mouse/trackpad in the terminal, the user confirmed
light click, double-click word, triple-click line, ordinary selection,
upward/downward multi-screen selection, reverse shrink, split isolation,
right-click, cancellation and resize/blur recovery.

Command-C was pasted into an independent TextEdit plain-text target. The
user confirmed the requested deterministic half-open range `R7C-0305` through
`R7C-0398`, including 3,854 UTF-8 bytes, 94 newlines and SHA-256
`a7f11990dad9893150f4a0675a6ee75d359c8fc699cea34f699626a64cba4d8a`.
This is the physical evidence; the synthetic routing and `pbpaste` checks above
remain supplemental automation only.

The isolated candidate process and private tmux server were stopped after the
manual gate.

## Formal release evidence

- Release: <https://github.com/c9r-io/deck/releases/tag/v0.4.32>; published,
  not a draft and not a prerelease.
- Annotated tag object: `58b2d633fdb7bba5ad278d28fc027b483ffb8f6a`.
- Dereferenced release commit:
  `f048182077624bbf43ce59683ec5ff33884e4fc2`.
- GitHub `test` workflow: [run 33186635596](https://github.com/c9r-io/deck/actions/runs/33186635596),
  completed successfully for the release commit.
- GitHub `release` workflow: [run 33186638877](https://github.com/c9r-io/deck/actions/runs/33186638877),
  completed successfully for the release commit.

The published release contains all four required assets:

- `deck_0.4.32_aarch64.dmg`: 4,769,183 bytes; SHA-256
  `51ae7e0357e4480ec20c1cb383ea7a0c857cc454fcdf57188d8d146ed514b98e`.
- `deck_aarch64.app.tar.gz`: 4,698,495 bytes; SHA-256
  `8d2997e5a1c1f4b518001c1da7076ab09ca1925fe4114acaaac36fdad19c4e55`.
- `deck_aarch64.app.tar.gz.sig`: 400 bytes; SHA-256
  `cbedcdf6b3c06bbe724cd98e91c108ded0c132e9a83955c20d1d307f9535140d`.
- `latest.json`: 1,316 bytes; SHA-256
  `ba420db0c2baa4566a92192006045d895fcf922878741426ee9018ab52abb49f`.

Each independently downloaded file's SHA-256 exactly matched the digest
reported by its GitHub Release asset. The official DMG mounted with all image
CRC checks verified. Its `deck.app` has both version fields set to `0.4.32`.
`codesign --verify --deep --strict` passed and reported identifier
`io.c9r.deck`, arm64 hardened runtime, Developer ID Application
`han chen (Y8ZG3D692W)`, Team ID `Y8ZG3D692W`, and CDHash
`188595acd2cf3298cea875402c6521404f7cdad3`. The application contains a
stapled notarization ticket; `xcrun stapler validate` passed and Gatekeeper
returned `accepted`, source `Notarized Developer ID`.

The downloaded `latest.json` reports version `0.4.32`. Both
`darwin-aarch64` platform entries point to this release's
`deck_aarch64.app.tar.gz`, and their metadata signature exactly equals the
published `.sig` asset (whose Base64 payload decodes to the expected Tauri
minisign envelope).

## Known remaining risk

GitHub emitted only a non-blocking maintenance annotation: `actions/checkout@v4`
and `actions/cache@v4` still declare Node.js 20 and GitHub forced them onto
Node.js 24. It did not affect either successful workflow or any release asset.
Physical pointer behavior still depends on macOS/WKWebView input delivery, so
the deterministic ownership tests and the completed real-device gate should
remain mandatory for future changes to selection routing.
