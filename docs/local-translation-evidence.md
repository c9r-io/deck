# Local Translation Phase 1 — decision evidence

This is the durable summary of the isolated September 2026 calibration. All
text used by the machine probes was synthetic. A maintainer pasted one real
Agent answer into the anonymous quality demo; only byte length and ratings
were retained. The source and translations were not saved.

## Workload and product bound

The current structured Agent session exposed five assistant answers: median
7,845 UTF-8 bytes, maximum 9,804 bytes. This small, report-heavy sample is
not a population percentile. It supported testing 4 KiB Live viewports and
8/16 KiB explicit snapshots. The earlier 256 KiB limit was an engineering
ceiling without workload or native throughput evidence and is not a Phase 1
promise.

## Why the single provider is Bergamot

Apple TranslationSession was authority-safe but had poor interactive latency
and cancellation on the tested Mac. SystemLanguageModel streamed quickly but
hostile source text sometimes acquired instruction authority. The dedicated
MT comparison used pinned Bergamot `9271618ebbdc5d21ac4dc4df9e72beb7ce644774`
and Mozilla en→zh base-memory data. Candle Marian was slower and less faithful
to developer literals. An official M2M100-418M INT8 model improved one
maintainer-rated Chinese sample but took a warm median of about 14.9 seconds
for 2.2 KiB and 24.7 seconds for 4.1 KiB through the protected path. The
NLLB-600M comparison was a non-commercial quality oracle, not a candidate for
Deck distribution. The final product decision prioritizes fast, private
everyday reading over professional translation quality and retains Copy Source
for the user to verify important content externally.

For the pinned Bergamot path, ten warm repetitions yielded medians of about
110 ms at 751 B, 300 ms at 2,158 B, and 563 ms at 4,100 B in the isolated
bake-off. In a synthetic 8.6 KiB document all 60 segments and 100 protected
anchors completed in 1.37 seconds; at 16.8 KiB all 117 segments and 195
anchors completed in 2.57 seconds. These figures characterize that machine,
model and corpus, not a user-facing latency guarantee.

## Protected content and Live presentation

The FR-5E protected-span calibration translated natural language through
Bergamot HTML mode while Deck held developer literals in an in-memory marker
map. Restoration rejected missing, duplicate, unknown and malformed markers.
The 8/16 KiB synthetic documents restored 140/140 and 280/280 protected
anchors with all segments present; ten hostile BANANA trials remained
translation content. This supports exact protected literals, not exact
Markdown whitespace or arbitrary prose fidelity.

At 4 KiB with source updates every 300 ms, the one-running/one-replaceable-
pending simulation produced successive readable completed snapshots and
caught up after the stream stopped. The presentation marks a displayed
translation as Updating whenever its revision trails the latest viewport.
Copy Source binds to the displayed revision's original snapshot.

## Unload and allocator calibration

Three separate native test processes exercised the final static provider and
synthetic 1–16 KiB requests. Physical footprint was about 4 MiB before load,
69 MiB immediately after load, and 805 MiB after repeated translation. It
fell by only about 96 KiB when the provider destroyed its model and service.
The public `malloc_zone_pressure_relief(NULL, 0)` API returned 0 bytes in all
three trials, took less than 1 ms, and did not change footprint or RSS after
three seconds idle. It is therefore **not** part of the production unload path.

A separate resource-count run showed two additional Bergamot worker threads
while loaded and a return to the baseline thread count after unload (2 → 4 →
2); open file descriptors stayed at 4. The provider handle reported unloaded.
Post-unload `vmmap` attributed about 684 MiB to *Malloc Large (empty)*, which
distinguishes allocator-retained freed pages from a live model allocation.
Immediate physical-footprint recovery is not claimed.

## Reproducibility and limits of this evidence

The production pack manifest in `app/src-tauri/src/intelligence/pack.rs` pins
the four asset URLs, exact byte sizes and SHA-256 values. The vendored source
provenance and compatibility patch live in
`app/src-tauri/vendor/bergamot/DECK_PROVENANCE.md`. The executable synthetic
regressions live in `intelligence/protected.rs`, `intelligence/provider.rs`,
`translation-lens-model.test.mjs`, and the WKWebView translation smoke.

The source calibration had one Apple Silicon Mac and a small set of synthetic
technical fixtures. Human quality review remains subjective. English to
Simplified Chinese is the only certified pair, and the final app/older-macOS
runtime evidence must be reported separately from these calibration results.

## Later Lens behaviour (2026-09-29)

The calibration above is dated September 2026 and keeps that boundary. The
Lens interaction was later changed without changing the provider, the pack or
the bounds: Live starts on open for a static viewport, a user wheel gesture is
debounced (250 ms) separately from the output throttle, reading no longer
pauses Live, the explicit "use current clipboard" read was removed in favour
of the Copied-text tab, and *Updating* is shown only while work is pending.
Its acceptance evidence is produced by `scripts/translation-lens-verify.py`
(isolated runs, machine report) and is not a re-certification of the numbers
above.

## Clipboard safety follow-up (2026-09-29, afternoon)

During the first debug run of the Lens fix, the smoke's old pasteboard guard
refused to restore after a text-equality claim failed, and the user's
original general-pasteboard content from before that run was lost. It was not
recovered and is recorded here as an incident. The follow-up replaced
text-based claims and text-based self-copy exclusion with version checks
before every test write and writer receipts (see `docs/translation-lens.md`).
