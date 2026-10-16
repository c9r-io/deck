# deck website and user guide

Static source for `https://deck.c9r.io`: a product homepage and task-oriented,
English/Simplified Chinese guide for **0.7.26 Stable**.

The site deliberately uses no framework, runtime JavaScript, analytics,
cookies, forms, third-party fonts or remote assets. Common external URLs are in
`site.config.json`; English and Simplified Chinese source routes live under
`src/`. Guide chapters live in paired `content/en/*.md` and `content/zh/*.md`
files; `scripts/guides.mjs` renders shared navigation, page outlines, metadata,
previous/next links and sitemap/redirect entries. `marked` is build-time only.

The English ChatGPT Secure Tunnel integration page is rendered directly from
`../docs/secure-tunnel.md` at `/docs/integrations/chatgpt-secure-tunnel/`.
There is no copied page body under `site/content`.

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

## Homepage demo

Both homepages embed the same local 26-second recording before the feature
section. The video uses native controls and `preload="none"`; it does not
autoplay or load a third-party player. Its poster and MP4 live in
`src/assets/demo/`. The recording has a Chinese interface and burned-in Chinese
captions, which the English caption identifies. Each page includes a text
summary of the flow and a direct MP4 link.

## Content maintenance

- `site.config.json` pins the guide version and immutable source reference.
  Document the shipped baseline, not whatever is newest on main or Nightly.
- Review the same facts in both languages when promoting a release. Check defaults,
  menu labels, signal semantics, failure actions and data compatibility against
  that exact release; older changelog entries are not current specifications.
  Read the release itself: `git show <tag>:<path>` for code, module headers and
  docs, and its `app/ui/js/i18n/en.js` / `zh-Hans.js` for every quoted label.
  `git log <tag>..main` lists what is not in it yet; leave that off the site.
- Write Chinese from the reader's actual situation, independently of English
  sentence structure and headings. Prefer concrete actions and complete sentences
  over engineering shorthand or slogans that tell the reader how to feel. Keep
  quoted UI labels exact, and preserve behavior, defaults and recovery conditions
  when making wording more natural. Published section anchors should remain usable.
- Lead with cross-work human attention: when several terminal tasks run at once,
  deck helps the user see which work needs them without checking every session.
  Deck is Agent-agnostic by design. Real local terminal/tmux sessions are the
  foundation, not the primary product definition. Agent vendors own each
  Agent's planning, execution, questions, permissions and turn semantics.
  Describe authoritative observations and their coverage limits; do not infer
  task meaning, readiness or success from quiet, turn endings or missing signals.
  Automation reduces repetitive coordination and attention cost; it does not
  make deck the owner of task execution. Avoid Agent-orchestrator, AI-OS,
  workflow-engine and autonomous-platform positioning or completion promises.
  Ordinary shell use needs no Agent integration or automation. Explain that
  deck has no cloud account or remote task-hosting server; distinguish local
  tmux from user-selected CLI and integration network access. Detailed timing
  and target rules belong in the guide. The homepage signal example uses
  fictional tasks and is labeled as a simplified illustration.
- Keep agent observations, delivery and human inspection distinct. Include what
  the user must do next for uncertain, failed, stale or stopped states.
- Keep privacy text aligned with voice, agent hooks, Slack, Phone Connector,
  optional MCP/Tunnel access and local storage.
  Existing social-preview art remains unchanged.
- `npm test` builds and checks all public HTML routes, local links and fragments,
  locale/version metadata, static-site privacy constraints and Markdown headings.
  It also reads the locale files at `guideRef` and requires the status words
  and labels in `PINNED_LABELS` verbatim on the attention chapter and homepage,
  so bumping the pinned release without re-checking the copy fails; and it
  requires every colour in `site.css` to come from the `:root` tokens.
- Before publishing, verify the labeled Stable version is actually published.
  The website is deployed separately from application release promotion.

## Visual design

These rules keep the site consistent with the product it describes.

- **Colour comes from the `:root` tokens in `assets/site.css`** and nowhere else.
  Teal (`--accent`) marks actions and links. Amber (`--amber`) means "look
  here": the product's status colour for an input request or a quiet session,
  and caution notes such as product boundaries and guide callouts. Do not use
  it for buttons or plain decoration. Green, amber, teal and grey are the app
  icon's status colours. A translucent variant derives from its token with
  `color-mix(in srgb, var(--token) N%, transparent)`; a new hue needs a new
  token and a reason.
- **Product pictures show the pinned release as it is.** A Board card has a
  status dot, title, `$ command · directory` and one status word; it never
  shows terminal output. Status words, filters and column headings are the
  release's strings. Example tasks are fictional and labeled as examples.
- **Static and quiet.** No runtime JavaScript, analytics, third-party fonts or
  remote assets. Motion is CSS only, small, and stops under
  `prefers-reduced-motion`.
- **Pictures come from the product's own objects** (the wall of cards, the
  Needs attention list) rather than generic landing-page devices: decorative
  glows, numbered markers that do not encode a real sequence, or an eyebrow
  label on every section. The homepage hero is a wall of status-word cards
  with one lit; guide chapters open on a header, and the Needs attention
  chapter on a board built from the real view (`content/*/attention.*.html`).
- Pages use `<body class="site">`. Fonts are self-hosted in
  `assets/fonts/` with their OFL texts and listed in `THIRD_PARTY_NOTICES.md`;
  Chinese pages use system CJK faces for headings and board cells.

Do not publish directly from a developer shell. The reviewed external path is
the manual `site-deploy` GitHub Actions workflow after the Cloudflare handoff in
`docs/cloudflare-handoff.md` is complete.
