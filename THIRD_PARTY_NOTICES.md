# Third-party notices

deck bundles or vendors the following third-party software. Each component
remains under its own license; copies of the license texts are available at
the upstream links.

## Bundled tmux sidecar

The app ships a statically linked `tmux` binary
(`app/src-tauri/binaries/tmux-aarch64-apple-darwin`), built by
`app/src-tauri/binaries/build-tmux.sh` from the pinned upstream releases
below; the script refuses any source tarball whose SHA-256 differs from the
one it records. The committed binary's own SHA-256 is pinned in
`tmux-aarch64-apple-darwin.sha256` beside it and checked on every test run.
The build is not bit-for-bit reproducible across Xcode/macOS SDK versions:
the same inputs on a newer toolchain give a different hash, so a rebuilt
binary is committed together with its new pin.

| Component | Version | License | Source |
|---|---|---|---|
| tmux | 3.7c | ISC | https://github.com/tmux/tmux |
| libevent | 2.1.12-stable | BSD-3-Clause | https://github.com/libevent/libevent |
| ncurses | 6.5 | X11/MIT-style | https://invisible-island.net/ncurses/ |
| utf8proc | 2.9.0 | MIT + Unicode data license | https://github.com/JuliaStrings/utf8proc |

## Vendored frontend libraries (`app/ui/vendor/`)

| Component | License | Source |
|---|---|---|
| xterm.js 5.5.0 (`xterm.js`, `xterm.css`); `xterm.js` carries one upstream change, see `docs/vendored-xterm.md` | MIT | https://github.com/xtermjs/xterm.js |
| @xterm/addon-fit | MIT | https://github.com/xtermjs/xterm.js |

## Local Translation (optional, offline model pack)

The app statically compiles the repository-owned Bergamot source under
`app/src-tauri/vendor/bergamot/`, pinned to commit
`9271618ebbdc5d21ac4dc4df9e72beb7ce644774`. Its Marian submodule is
pinned to `2781d735`. Build patches and the exact engine delta are described
in `app/src-tauri/vendor/bergamot/DECK_PROVENANCE.md`. The offline
English → Simplified Chinese base-memory model is downloaded only after the
user explicitly approves it. Its fixed asset URLs, sizes, and SHA-256 values
are in `app/src-tauri/src/intelligence/pack.rs`; Deck verifies every asset
before activation and use.

| Component | License | Source |
|---|---|---|
| Bergamot translator | MPL-2.0 | https://github.com/browsermt/bergamot-translator |
| Marian NMT | MIT | https://github.com/marian-nmt/marian-dev |
| Mozilla en→zh base-memory data pack | MPL-2.0 | https://storage.googleapis.com/moz-fx-translations-data--303e-prod-translations-data/db/models.json |
| ssplit-cpp | Apache-2.0 for C++ and build files; model/data licenses as noted upstream | https://github.com/mediacloud/ssplit-cpp |
| SentencePiece | Apache-2.0 | https://github.com/google/sentencepiece |
| yaml-cpp | MIT | https://github.com/jbeder/yaml-cpp |
| intgemm | MIT | https://github.com/marian-nmt/intgemm |
| ruy | Apache-2.0 | https://github.com/google/ruy |
| spdlog | MIT | https://github.com/gabime/spdlog |
| zlib | zlib | https://zlib.net/ |
| PCRE2 (system library) | BSD-3-Clause | https://www.pcre.org/ |

## Rust dependencies

Both crates (the TUI at the repo root and the app backend in
`app/src-tauri/`) pin every dependency via `Cargo.lock`, committed in the
repository. `cargo tree` prints the human-readable dependency tree (it is
NOT a machine-readable SBOM):

```sh
cargo tree --locked                                          # TUI crate
cargo tree --locked --manifest-path app/src-tauri/Cargo.toml # app backend
```

An actual machine-readable SBOM (CycloneDX JSON) is produced from the same
lockfiles with [cargo-cyclonedx](https://github.com/CycloneDX/cyclonedx-rust-cargo):

```sh
cargo install cargo-cyclonedx
cargo cyclonedx --format json                                          # → deck.cdx.json
cargo cyclonedx --format json --manifest-path app/src-tauri/Cargo.toml # → deck-app.cdx.json
```

The tmux sidecar's inputs are pinned in `build-tmux.sh` (versions above) and
the produced binary's SHA-256 is committed next to it.

## Dependency vulnerability checks

Run [cargo-audit](https://github.com/rustsec/rustsec) against both lockfiles
before tagging a release:

```sh
cargo install cargo-audit
cargo audit                                             # TUI crate
(cd app/src-tauri && cargo audit)                       # app backend
```

This is a manual release-gate step rather than a CI job for now: CI has no
network-isolation guarantees for the advisory DB fetch, and an advisory
against a transitive dev-dependency should not block unrelated pushes — a
human triages the report instead. Revisit if the project gains more
maintainers.
