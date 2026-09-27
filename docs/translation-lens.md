# Local Translation — Phase 1

Local Translation is an optional Local Intelligence capability. It defaults OFF. The Settings switch does not silently download a model: a missing English → Simplified Chinese offline pack requires an explicit download confirmation. Turning the feature off closes the Translation Lens, disarms clipboard observation, invalidates requests, and unloads the model. The verified pack remains installed until the user explicitly deletes it.

The Lens is a session companion dock on a wide workspace and an overlay on a narrow one. It is not a card, layout leaf, terminal, shell, or Agent parser. Live captures only the focused xterm viewport. Selection translates an explicit Deck token or xterm selection snapshot. Clipboard mode accepts only a post-baseline text change while Deck is focused, with a separate focused **Use current clipboard** action. Native AppKit focus checks enforce the clipboard boundary. Copy Translation and Copy Source disarm and rebaseline automatic clipboard observation so Deck's own writes do not feed back into translation.

## Provider and data pack

The sole Phase 1 provider is in-process Bergamot, pinned to commit `9271618ebbdc5d21ac4dc4df9e72beb7ce644774`. Its Mozilla en→zh base-memory data pack is fixed by four asset filenames, HTTPS URLs, byte sizes, and SHA-256 values in `app/src-tauri/src/intelligence/pack.rs`. The pack is data only and installed under Deck's private data directory. Downloads stage atomically, verify exact size and digest, and activate only after every asset passes. Installed assets are verified before provider load. Deletion removes only the four manifest-owned files. The model is loaded lazily on the first translation request, one instance at a time. There is no translation network request after installation.

`TranslationProvider` is a narrow typed seam with `capabilities`, `load`, `translate`, and `unload`. There is one implementation and no plugin loader, model picker, prompt API, or cloud fallback. A future dedicated MT provider could use this seam without changing Lens, clipboard authority, segmentation, or protected content. This is an architectural boundary, not a plugin system.

Unloading destroys the single model/service instance. In the macOS 27 native certification, process physical footprint stayed high immediately afterward because the system allocator retained freed large pages; model lifecycle and immediate OS memory reclamation are separate observations.

## Certified inputs and result semantics

Only English → Simplified Chinese is certified. Live uses a fixed 4 KiB UTF-8 bound; selection uses 16 KiB; Clipboard accepts the Settings choice of 8 or 16 KiB. Oversize input is refused, never truncated. The older 256 KiB Apple Translation ceiling is not a Phase 1 promise.

The lexical protected-content layer shields fenced and inline code, URLs, paths, explicit commands, structured literals, and high-confidence identifiers. All source HTML is escaped before Deck-owned carrier markers are inserted. Restoration requires every marker exactly once and rejects missing, duplicate, unknown, or malformed markers. Segmentation prefers paragraph, line, sentence, then Unicode grapheme boundaries; joined source segments are byte-identical to the input. Natural prose is translated by Bergamot. Code and protected literals are exact; Markdown whitespace and wrapping are not promised to be byte-identical.

Live keeps one running request and one replaceable latest pending snapshot. A completed translation stays readable even when the viewport has moved; the Lens marks it **Updating** until the displayed and source revisions match. Copy Source copies the source snapshot paired with the displayed translation, not the current terminal viewport. Interaction with the result pauses Live; Resume captures the current viewport. Cancellation invalidates result authority; the in-process native segment finishes before the next begins.

Source, translation, and protected map stay in memory and never enter Deck logs, settings, diagnostics, terminal output, or Deck network transport. Model download traffic transfers only the fixed static pack assets. Local Translation is for fast everyday reading. Important content should be checked through a professional translation service chosen by the user after an explicit Copy Source action.

The deployment target remains macOS 11. Bergamot is statically linked into Deck; it is not a downloaded executable or helper process. An older-runtime GUI launch remains a separate certification claim and must not be inferred from a successful build or source availability guards.
