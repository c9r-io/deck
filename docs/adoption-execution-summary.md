# Adoption validation execution summary

Last updated: 2026-08-30.

## Implemented locally

- Reframed the work from commercialization to product positioning, adoption
  and promotion validation.
- Corrected the README claim that deck knows where attention is needed.
- Added an independent English/Chinese static landing page for the future
  `deck.c9r.io` domain.
- Added honest status and scheduler boundaries throughout the landing copy.
- Added English/Chinese privacy pages with no analytics or third-party scripts.
- Added a content-safe public feedback issue form.
- Added a dedicated social preview asset.
- Added static build, local preview, route, metadata, privacy and security
  tests.
- Added continuous site checks and a manual, confirmation-gated Cloudflare
  deployment workflow.
- Added adoption metrics, cohort protocol, decision thresholds, promotion copy,
  interview guide, demonstration script and Cloudflare handoff.
- Added `site/scripts/make-icons.mjs`, which generates `favicon.ico`,
  `icon.svg` and the 180px apple-touch icon from the stylesheet's own tokens
  using only node builtins. Browser QA had found the page requesting a
  nonexistent `/favicon.ico`.

## Intentionally not implemented

- Pricing, payments, preorders, licenses, accounts or feature gating.
- Outbound product telemetry or stable install identifiers.
- Website analytics, cookies, forms or tracking scripts.
- Timed in-app feedback prompts.
- A third update channel.

## External work performed with authorization

The owner authorized Cloudflare setup and a GitHub branch/PR on 2026-08-30 and
personally completed the Cloudflare browser sign-in.

- Pushed branch `site/deck-landing-page` and opened
  <https://github.com/c9r-io/deck/pull/1>. Nothing was merged to `main`.
- Created the independent Cloudflare Pages project `deck-site` (production
  branch `main`), unrelated to `orchestrator-docs`.
- Deployed and verified the site at <https://deck-site.pages.dev>.
- Registered `deck.c9r.io` as a Pages custom domain. It is `pending` until the
  zone CNAME exists.
- Set the `CLOUDFLARE_ACCOUNT_ID` Actions secret and created the
  `website-production` GitHub Environment with a required human reviewer.

No app Release, tag, updater feed, community post, email or survey was created.

## Pending external work

- Add the `c9r.io` CNAME for `deck` and create the least-privilege
  `CLOUDFLARE_API_TOKEN` secret; both need dashboard access the deployment
  session does not have. See `docs/cloudflare-handoff.md`.
- Verify TLS, the HTTP-to-HTTPS redirect and the social link preview on
  `deck.c9r.io` once it resolves.
- Capture privacy-safe product screenshots and record the 90-second
  demonstration, then enable its CTA. Attempted on 2026-08-30 with an isolated
  synthetic board (separate data directory and tmux socket, 8 cards across 2
  projects, 6 live sessions); the capture itself could not be automated because
  `screencapture` has no Screen Recording permission in this environment, and
  the isolated instance was torn down afterwards.
- Approve and execute the first promotion cohort.

## Local validation results

Completed on 2026-08-30:

- site tests: 7 passed;
- frontend tests: 65 passed;
- root Rust tests: 10 passed;
- app Rust unit/storage/scheduler tests: 155 passed;
- log privacy tests: 7 passed;
- tmux contract tests: 21 passed;
- release tooling tests: 11 passed;
- Rust formatting and clippy checks passed for both crates;
- frontend syntax and identifier checks passed;
- workflow validation passed;
- English, Chinese, privacy and 404 local routes returned their expected HTTP
  status codes;
- GitHub, latest Stable Release, MIT License and generic issue endpoints
  returned HTTP 200.

Browser QA completed on 2026-08-30 against the local preview and the live
`pages.dev` deployment:

- desktop 1440x900 and mobile 390x844 for the English and Chinese routes, with
  no horizontal overflow at either width;
- keyboard traversal reaches the skip link and every navigation control with a
  visible focus outline;
- the browser console is clean and the only network requests are the
  same-origin document, stylesheet and icon — no analytics, advertising or font
  providers;
- privacy and 404 routes render correctly.

The site ships no JavaScript at all, so no-JavaScript usability is structural
rather than a fallback. Open Graph tags and the preview image were reviewed
locally; how a specific social platform renders the card can only be confirmed
once `deck.c9r.io` resolves.
