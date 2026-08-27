# Third-party notices

deck bundles or vendors the following third-party software. Each component
remains under its own license; copies of the license texts are available at
the upstream links.

## Bundled tmux sidecar

The app ships a statically linked `tmux` binary
(`app/src-tauri/binaries/tmux-aarch64-apple-darwin`, checksum alongside it),
built reproducibly by `app/src-tauri/binaries/build-tmux.sh` from pinned
upstream releases:

| Component | Version | License | Source |
|---|---|---|---|
| tmux | 3.7c | ISC | https://github.com/tmux/tmux |
| libevent | 2.1.12-stable | BSD-3-Clause | https://github.com/libevent/libevent |
| ncurses | 6.5 | X11/MIT-style | https://invisible-island.net/ncurses/ |
| utf8proc | 2.9.0 | MIT + Unicode data license | https://github.com/JuliaStrings/utf8proc |

## Vendored frontend libraries (`app/ui/vendor/`)

| Component | License | Source |
|---|---|---|
| xterm.js (`xterm.js`, `xterm.css`) | MIT | https://github.com/xtermjs/xterm.js |
| @xterm/addon-fit | MIT | https://github.com/xtermjs/xterm.js |
| @xterm/addon-clipboard | MIT | https://github.com/xtermjs/xterm.js |

## Rust dependencies

Both crates (the TUI at the repo root and the app backend in
`app/src-tauri/`) pin every dependency via `Cargo.lock`, committed in the
repository. The dependency tree — the software bill of materials for the
Rust side — is reproducible from a checkout:

```sh
cargo tree --locked                                          # TUI crate
cargo tree --locked --manifest-path app/src-tauri/Cargo.toml # app backend
```

For a machine-readable SBOM, `cargo cyclonedx` (from
[cargo-cyclonedx](https://github.com/CycloneDX/cyclonedx-rust-cargo)) run
against the same lockfiles produces CycloneDX output; the lockfiles are the
source of truth either way. The tmux sidecar's inputs are pinned in
`build-tmux.sh` (versions above) and the produced binary's SHA-256 is
committed next to it.
