# Regression (air2, macOS 27.2.1, WK smoke of this change's debug carrier)

Runner: the registry-based `mini_run_smoke.py` (copied from the 2026-11-27
round, `deck-notify` added to the carried sidecars), isolated roots, every
root trashed, nothing left, air2's own Deck and `~/.deck` unchanged.

| Mode | Result | Notes |
|---|---|---|
| `approval` | PASS 12/12 | badge rule approval box, first-send option |
| `attention` | PASS 58/58 (second run) | first run failed `entry-menu` / `defaults-menu`: the smoke pins the New session ▾ menu's labels and this change adds "New on a local notification…"; the pin was updated |
| `ux-layers` | 35/37 | `ux-spacing` one finding in Settings › MCP (a=-46 b=1): identical in the 2026-11-27 baseline on the same machine, pre-existing |
| `run` | 94/97 | `path-editor`: no editor app on air2, pre-existing; `ime-routing` mask 6/7: see below |

`ime-routing`: a clean build of HEAD `1941820a` (worktree, no changes) on air2 the same hour fails it identically (mask 6/7, with `path-editor`), so it is pre-existing on this machine and HEAD, not from this change (it passed in the 2026-11-27 run before the last two commits).
