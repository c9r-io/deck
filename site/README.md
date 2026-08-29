# deck landing page

Independent static source for the future `https://deck.c9r.io` site.

The site deliberately uses no framework, runtime JavaScript, analytics,
cookies, forms, third-party fonts or remote assets. Common external URLs are in
`site.config.json`; English and Simplified Chinese source routes live under
`src/`.

## Local review

```sh
cd site
npm test
npm run build
npm run preview
```

The preview listens on `http://127.0.0.1:4173` by default. Set `PORT` to use a
different port.

Do not publish directly from a developer shell. The reviewed external path is
the manual `site-deploy` GitHub Actions workflow after the Cloudflare handoff in
`docs/cloudflare-handoff.md` is complete.
