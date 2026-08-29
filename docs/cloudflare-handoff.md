# deck.c9r.io Cloudflare handoff

This is the handoff for an agent with browser access. It does not authorize
publication by itself; obtain explicit approval immediately before the first
external write.

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

- `deck.c9r.io` has no DNS record and does not resolve.
- `c9r.io` uses Cloudflare nameservers.
- The deck GitHub repository has no `CLOUDFLARE_API_TOKEN` or
  `CLOUDFLARE_ACCOUNT_ID` Actions secrets.
- The Orchestrator repository has secrets with those names, but GitHub secrets
  cannot be read or copied. Do not couple deck to that repository or Pages
  project.
- No Cloudflare change or site publication has been performed.

## Browser-authenticated setup

1. Sign in to the correct Cloudflare account in the controlled browser.
2. Confirm that the `c9r.io` zone is active in that account.
3. Under Workers & Pages, create a new Pages project named `deck-site` using
   Direct Upload. Do not attach the Orchestrator repository/project.
4. Perform one reviewed deployment to the generated `deck-site.pages.dev`
   address before binding the custom domain.
5. In the Pages project, add custom domain `deck.c9r.io`. Because the zone is
   already on Cloudflare, confirm the automatically proposed CNAME rather than
   creating an unrelated A/AAAA record.
6. Wait for domain status and TLS certificate to become active.

Cloudflare requires the domain to be associated through the Pages custom-domain
flow. A manually created CNAME alone is not sufficient.

## Least-privilege CI credentials

Create a dedicated API token for deck deployment rather than a global API key.
The token should be restricted to the intended account and have Cloudflare
Pages Edit/Write. Handle the first custom-domain/DNS association manually in
the dashboard so the recurring deployment token does not need DNS Edit.

Add these values directly to the deck GitHub repository Actions secrets; never
paste them into chat, issue text, logs or repository files:

- `CLOUDFLARE_API_TOKEN`
- `CLOUDFLARE_ACCOUNT_ID`

Create or protect the GitHub Environment `website-production`. Require a human
reviewer if the repository/account supports environment reviewers.

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

Check both the `pages.dev` preview and `https://deck.c9r.io`:

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
