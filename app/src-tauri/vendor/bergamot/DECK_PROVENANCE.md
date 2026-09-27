# Deck Bergamot source provenance

Upstream Bergamot: `browsermt/bergamot-translator` commit
`9271618ebbdc5d21ac4dc4df9e72beb7ce644774`.
Marian submodule: `2781d735d4a10dca876d61be587afdab2726293c` (the revision checked out with this Bergamot commit).
The source was copied from the FR-5D/FR-5E calibrated tree with its `.git`,
examples, tests, and unused build outputs omitted. No source is fetched during
a Deck build. The CMake build compiles an in-process static library only.

Deck-specific changes:

- `CMakeLists.txt` adds `deck_bergamot_bridge` and a local link check, and
  removes any configure-time git submodule update.
- `cmake/GetVersionFromFile.cmake` and Marian's `common/git_revision.h` pin
  the above versions without requiring git metadata at build time.
- Marian CMake accepts the pinned, metadata-free source tree.
- `patches/DECK_CLT27.patch` is the exact, reverse-checkable delta for
  Marian stack-array constants, zlib Apple macro compatibility,
  SentencePiece enum initialization, and Marian `TargetArch.cmake`
  Apple `arm64` mapping. `scripts/check-bergamot-patches` fails when the
  vendored files drift from that recorded delta. The SentencePiece
  submodule base is `ae41b7740d7006596bb9257e83340b2620db9d00`.
- `app/src-tauri/native/BergamotBridge.cpp` is Deck's C ABI adaptation. It
  uses the calibrated single-model in-memory configuration, HTML input mode,
  and Accelerate CPU build. No provider code or model is downloaded at build
  or at runtime except the explicit data-pack download in Settings.

The model identity, official HTTPS asset URLs, uncompressed SHA-256 values,
and exact sizes live in `app/src-tauri/src/intelligence/pack.rs`. A change to
any upstream revision or model asset requires new native and quality
certification rather than inheriting the FR-5D/FR-5E evidence.
