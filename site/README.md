# deck website and user guide

Static source for `https://deck.c9r.io`: a product homepage and task-oriented,
English/Simplified Chinese guide based on deck **0.6.5**.

The site deliberately uses no framework, runtime JavaScript, analytics,
cookies, forms, third-party fonts or remote assets. Common external URLs are in
`site.config.json`; English and Simplified Chinese source routes live under
`src/`. Guide chapters live in paired `content/en/*.md` and `content/zh/*.md`
files; `scripts/guides.mjs` renders shared navigation, page outlines, metadata,
previous/next links and sitemap/redirect entries. `marked` is build-time only.

## Local review

```sh
cd site
npm ci
npm test
npm run build
npm run preview
```

The preview listens on `http://127.0.0.1:4173` by default. Set `PORT` to use a
different port.

## Content maintenance

- `site.config.json` pins the guide version and immutable source reference.
  Document the shipped baseline, not whatever is newest on main or Nightly.
- Update paired chapters together when promoting a release. Check defaults,
  menu labels, signal semantics, failure actions and data compatibility against
  that exact release; older changelog entries are not current specifications.
- Keep the homepage focused on arranging work, checking less and stepping in.
  Detailed timing/target rules belong in the guide. The homepage signal example
  uses fictional tasks and is explicitly labeled as a simplified illustration.
- Keep agent observations, delivery and human inspection distinct. Include what
  the user must do next for uncertain, failed, stale or stopped states.
- Keep privacy text aligned with voice, agent hooks, Slack and local storage.
  Existing social-preview art remains unchanged.
- `npm test` builds and checks all public HTML routes, local links and fragments,
  locale/version metadata, static-site privacy constraints and Markdown headings.
- Before publishing, verify the labeled Stable version is actually published.
  The website is deployed separately from application release promotion.

Do not publish directly from a developer shell. The reviewed external path is
the manual `site-deploy` GitHub Actions workflow after the Cloudflare handoff in
`docs/cloudflare-handoff.md` is complete.
