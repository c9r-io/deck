# Local Translation Quality Lab

This isolated HTML/WKWebView window lets a maintainer paste an English Agent answer and review the exact Phase 1 production translation core: the repository-owned `TranslationProvider` implementation, protected-content scanner, segmentation, and fail-closed restoration. The host launches a short-lived Rust example binary for each manual request. No source or translation is saved or sent to a server.

```sh
scripts/translation-quality-demo/run
```

Normal launch verifies the fixed Mozilla en→zh base-memory model in `/tmp/deck-translation-quality-demo/model`, compiles the repository-owned static Bergamot source if needed, and opens the two-pane window. It performs no download. A different already prepared directory may be supplied with `--model-dir /path/to/model`.

If the model is absent, explicitly fetch the pinned data pack with:

```sh
scripts/translation-quality-demo/run --prepare
```

`--prepare` downloads model data only. It does not clone or patch engine source. The production C++ engine is already vendored in the repository. CMake is required at build time; set `CMAKE=/path/to/cmake` if it is not on `PATH`. The model must match all four production SHA-256 values.

Paste source on the left, click **Translate**, read the selectable result on the right, then use **Copy Translation**, **Clear**, synthetic samples, or **Show protected spans**. The Demo's 16 KiB input ceiling matches the certified Phase 1 document limit. It shows metadata outside the translated text and never persists the source or output. The window host uses a local child process only for this isolated manual Demo; Deck production translation remains in-process.
