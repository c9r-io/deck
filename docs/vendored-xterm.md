# Vendored xterm.js

`app/ui/vendor/xterm.js` is the published `@xterm/xterm` 5.5.0 build
(`lib/xterm.js`) with **one** change taken from upstream. `xterm.css` and
`addon-fit.js` are the published files, unchanged. xterm.js is MIT licensed
(Copyright (c) 2017-2019 The xterm.js authors, 2014-2016 SourceLair Private
Company, 2012-2013 Christopher Jeffrey); the licence and source are listed in
`THIRD_PARTY_NOTICES.md`.

## Identity

| | |
|---|---|
| Package | `@xterm/xterm` 5.5.0, npm `gitHead` `9ba6c00a195c95fcf8292a2b9084d91450e5daae` (tag `5.5.0`) |
| Published `lib/xterm.js` SHA-256 | `1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495` |
| Upstream change | commit `52e8a75e9f3b0f12cdba71d8c3cfe3a5f4958885`, "Fix duplicate input for some IMEs" (fixes xtermjs/xterm.js#5023), the direct child of the 5.5.0 tag commit; it touches only `src/browser/input/CompositionHelper.ts` (+3 −2) |
| Shipped `app/ui/vendor/xterm.js` SHA-256 | `c365e94f10448c3b6cedf260b42632162c35b77e84d6ad0eb7054fc34b2a0b78` |

## The change

`CompositionHelper` sends a finished composition from a zero-delay timer. If
the next composition has already started when that timer runs, 5.5.0 cut the
committed text at the *previous* composition's remembered end, which can be
stale, so letters of the next preedit were sent with the committed text. The
upstream commit cuts at the start of the new composition instead:

```diff
         if (this._isComposing) {
-          // Use the end position to get the string if a new composition has started.
-          input = this._textarea.value.substring(currentCompositionPosition.start, currentCompositionPosition.end);
+          // Use the start position of the new composition to get the string
+          // if a new composition has started.
+          input = this._textarea.value.substring(currentCompositionPosition.start, this._compositionPosition.start);
         } else {
```

In the published minified build this is the single expression recorded as
`before` / `after` in `app/ui/test/fixtures/xterm-patch.json`. Nothing else
in the composition code is changed: later upstream work on composition (the
selection-based start, the suffix handling) is **not** included, and the
handling of blur during a composition is as in 5.5.0.

## Rebuild and verify

```sh
npm pack @xterm/xterm@5.5.0            # or fetch the registry tarball
tar -xzf xterm-xterm-5.5.0.tgz package/lib/xterm.js
node scripts/patch-vendored-xterm.mjs package/lib/xterm.js app/ui/vendor/xterm.js
node scripts/patch-vendored-xterm.mjs --verify
```

The script refuses an input that is not the pinned published build, a
pattern that does not match exactly once, and a result with another digest.
`ui/test/static.test.mjs` pins the same facts on every test run: the vendor
file list, both digests, the upstream commit, and that undoing exactly this
change gives back the published build.

This is a single, explicitly authorized exception. A further vendor change
needs its own authorization and its own pin; moving to a newer xterm release
replaces this file and this document rather than adding to them.
