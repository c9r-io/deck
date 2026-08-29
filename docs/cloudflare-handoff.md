# deck.c9r.io Cloudflare handoff

This records how deck's website hosting is set up and what is still owed. It
does not authorize publication by itself; obtain explicit approval immediately
before any further external write.

## Prepared locally

- Independent static site under `site/`; no Orchestrator code, content, visual
  identity or product links.
- English root, Simplified Chinese `/zh/`, paired privacy routes and 404 page.
- No runtime JavaScript, analytics, cookies, forms, remote fonts or external
  assets.
- Central URLs in `site/site.config.json`.
- Build and privacy tests in `site/test/`.
- Manual deployment workflow in `.github/workflows/site-deploy.yml`.
- Continuous site checks in `.github/workflows/site-check.yml`.
- Intended independent Cloudflare Pages project: `deck-site`.
- Intended custom domain: `deck.c9r.io`.

## Current external state (checked 2026-08-30)

The account-level setup below was performed with the owner's explicit
authorization during the 2026-08-30 session.

- Cloudflare account `Gpgkd906@gmail.com's Account`
  (`1d89dd5127009d5f489abfcf33c57674`); the `c9r.io` zone is active in it.
- Independent Pages project `deck-site` exists, production branch `main`. It
  shares nothing with `orchestrator-docs` beyond the account.
- A production deployment is live at `https://deck-site.pages.dev` and passed
  the online verification below.
- `deck.c9r.io` is a Pages custom domain
  (`a76170bd-a356-476a-ab98-79545c06243b`), **active**, with a Google-issued
  certificate. The zone holds `CNAME deck -> deck-site.pages.dev`, proxied,
  matching how `docs` is configured.
- A Configuration Rule named `deck.c9r.io — no Web Analytics RUM injection`
  matches `http.host eq "deck.c9r.io"` and disables RUM. See "Zone-inherited
  analytics" below — this rule is load-bearing for deck's privacy claims.
- GitHub Actions secrets `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_API_TOKEN`
  are both set. The token is `deck-site pages deploy`, scoped to
  Account -> Cloudflare Pages -> Edit on that one account only: no DNS edit,
  no zone access, no other account.
- GitHub Environment `website-production` exists with a required human
  reviewer.
- No app Release, tag, updater feed or community post was touched.

Every later deployment now goes through the manual `site-deploy` workflow
rather than a developer shell.

## Zone-inherited analytics

`c9r.io` has an account-level Web Analytics site (created for the Orchestrator
docs) whose configured hostname is the apex `c9r.io` with automatic JS
injection. Cloudflare therefore injected
`static.cloudflareinsights.com/beacon.min.js` into `deck.c9r.io` responses the
moment the hostname went live, purely by zone inheritance. deck's CSP blocked
the script from executing, but the served HTML still contained a third-party
analytics tag, which contradicts the privacy page.

That Web Analytics site offers no per-hostname exclusion: every option there
is zone-wide and would change what the Orchestrator collects. The fix is
therefore a Configuration Rule scoped to `deck.c9r.io` alone, which disables
RUM for that hostname and leaves `docs.c9r.io` untouched. Verified after
deployment: `deck.c9r.io` serves zero script tags, `docs.c9r.io` still serves
its beacon.

If deck ever moves to another zone or another zone-wide feature is enabled,
re-check the served HTML for injected third-party scripts. A passing CSP is
not sufficient evidence — request the page with a browser `Accept` and
`User-Agent`, because Cloudflare only injects for browser-shaped requests.

## Account setup (completed 2026-08-30)

All steps are done; they are kept here because a rebuild or a second
environment has to repeat them.

1. Sign in to the correct Cloudflare account. (`wrangler login` is enough; it
   grants `pages:write` but not DNS or token administration.)
2. Confirm that the `c9r.io` zone is active in that account.
3. Create a Pages project named `deck-site` using Direct Upload. Do not attach
   the Orchestrator repository/project.
4. Perform one reviewed deployment to `deck-site.pages.dev` before binding the
   custom domain.
5. Register custom domain `deck.c9r.io` on the Pages project.
6. Add `CNAME deck -> deck-site.pages.dev` (proxied) in the zone and wait for
   the domain status and TLS certificate to become active.
7. Add the Configuration Rule that disables RUM for the hostname, then confirm
   no third-party script is injected.

Cloudflare requires the domain to be associated through the Pages custom-domain
flow, and that association separately requires the zone record. Neither half is
sufficient alone.

## Least-privilege CI credentials

Create a dedicated API token for deck deployment rather than a global API key.
The token should be restricted to the intended account and have Cloudflare
Pages Edit/Write. Handle the first custom-domain/DNS association manually in
the dashboard so the recurring deployment token does not need DNS Edit.

Add the token directly to the deck GitHub repository Actions secrets; never
paste it into chat, issue text, logs or repository files:

- `CLOUDFLARE_API_TOKEN` — set, and verified `active` against
  `/user/tokens/verify` before storing.
- `CLOUDFLARE_ACCOUNT_ID` — set. This is an account identifier rather than a
  credential, but it is kept as a secret so the workflow reads both the same
  way.

The GitHub Environment `website-production` exists and requires a human
reviewer, so a `site-deploy` run waits for an approval before it can reach
Cloudflare.

## First deployment

Before running the workflow:

1. Review `git diff` and confirm no real terminal content or identifying paths
   are present in the site or social preview image.
2. Run `npm test` and `npm run build` in `site/`.
3. Confirm the current Stable Release URL in `site/site.config.json`.
4. Confirm the feedback issue template exists on the target branch.
5. Confirm analytics remain disabled.
6. Confirm explicit approval to publish.

Run the `site-deploy` workflow manually with confirmation text:

```text
deploy-deck.c9r.io
```

The workflow deploys `site/dist` to `deck-site` on the production branch. It
does not create or modify the app Stable/Nightly releases.

## Online verification

Verified on `https://deck.c9r.io` on 2026-08-30: all four routes returned 200,
an unknown path returned the deck 404 page, the three trailing-slash redirects
returned 301 to their canonical paths, HTTP redirected to HTTPS on a valid
certificate, `favicon.ico`, `icon.svg`, `icon-180.png`, `og.png`, `sitemap.xml`
and `robots.txt` served with correct content types, every security header was
present, the browser console was clean, and the only network requests were the
same-origin document, stylesheet and icon. Desktop 1440x900 and mobile 390x844
both render without horizontal overflow.

Still to check: how a specific social platform renders the Open Graph card.

Repeat this list after any hosting change:

- `/`, `/zh/`, `/privacy/`, `/zh/privacy/` return 200;
- unknown paths show the deck 404 page;
- certificate is valid and HTTP redirects to HTTPS;
- English/Chinese language links and canonical/hreflang values are correct;
- latest Stable, GitHub, feedback and MIT links reach the expected c9r-io/deck
  destinations;
- the download path shows a published non-prerelease, not Nightly;
- security headers are present;
- no requests go to analytics, advertising or font providers;
- desktop and narrow mobile layouts are usable with keyboard focus visible;
- Open Graph preview uses `/assets/og.png` and contains no invented product
  claims.

Record the Pages deployment ID, review date and reviewer in the progress
summary, but never record credentials.

## Rollback

If the custom domain fails but the Pages deployment is healthy, remove or pause
the custom-domain association in the dashboard; do not change unrelated DNS.

If the deployed content is defective, use Cloudflare Pages deployment history
to roll back to the prior reviewed deployment, then fix the repository and run
the manual workflow again. Do not overwrite app Release assets or updater feeds
as part of a website rollback.

If the first deployment is not acceptable, leave `deck.c9r.io` unbound and use
the private review process on the `pages.dev` deployment until approved.
