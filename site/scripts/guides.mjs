// Public guides are authored in paired Markdown files and rendered at build time.
// Content is reviewed repository source, never visitor-supplied Markdown.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { Marked } from 'marked';

export const chapters = ['index', 'start', 'attention', 'prompts', 'sessions', 'automations', 'integrations', 'input-and-settings'];
export const escapeHtml = text => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const secureTunnelPath = locale => `${locale === 'zh' ? '/zh' : ''}/docs/integrations/chatgpt-secure-tunnel/`;
const guidePath = (locale, slug) => slug === 'secure-tunnel' ? secureTunnelPath(locale) : `${locale === 'zh' ? '/zh' : ''}/guide/${slug === 'index' ? '' : `${slug}/`}`;

export function renderMarkdown(source) {
  const headings = [];
  const seen = new Map();
  const parser = new Marked({ renderer: {
    heading({ tokens, text, depth }) {
      const base = text.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, '').trim().replace(/\s+/g, '-') || 'section';
      const count = seen.get(base) || 0;
      seen.set(base, count + 1);
      const id = count ? `${base}-${count + 1}` : base;
      if (depth === 2) headings.push({ id, text });
      return `<h${depth} id="${id}">${this.parser.parseInline(tokens)}</h${depth}>\n`;
    },
    table(token) {
      const cell = (item, tag) => `<${tag}>${this.parser.parseInline(item.tokens)}</${tag}>`;
      return `<div class="table-scroll" tabindex="0" role="region" aria-label="${escapeHtml(token.header.map(item => item.text).join(' / '))}"><table><thead><tr>${token.header.map(item => cell(item, 'th')).join('')}</tr></thead><tbody>${token.rows.map(row => `<tr>${row.map(item => cell(item, 'td')).join('')}</tr>`).join('')}</tbody></table></div>`;
    },
  }});
  const tokens = parser.lexer(source);
  const titles = tokens.filter(token => token.type === 'heading' && token.depth === 1);
  if (titles.length !== 1) throw new Error('Each guide needs exactly one H1');
  const title = titles[0].text;
  const description = tokens.find(token => token.type === 'paragraph')?.text;
  if (!description) throw new Error('Each guide needs an introductory paragraph');
  // The chapter opener shows the title and a leading paragraph; the article
  // shows the rest. Render each token once, in order, so heading ids match.
  const slice = list => Object.assign(list, { links: tokens.links });
  const at = tokens.indexOf(titles[0]);
  let next = at + 1;
  while (tokens[next]?.type === 'space') next += 1;
  const leading = at === tokens.findIndex(token => token.type !== 'space') && tokens[next]?.type === 'paragraph';
  const before = parser.parser(slice(tokens.slice(0, at)));
  const head = parser.parser(slice([titles[0]]));
  const lede = leading ? new parser.Parser(parser.defaults).parseInline(tokens[next].tokens) : '';
  const body = parser.parser(slice(tokens.slice(leading ? next + 1 : at + 1)));
  const html = before + head + (leading ? `<p>${lede}</p>\n` : '') + body;
  return { title, description, html, head, lede, body: before + body, headings };
}

export async function buildGuides(root, output, config) {
  const routes = [];
  for (const locale of ['en', 'zh']) {
    const zh = locale === 'zh';
    const home = zh ? '/zh/' : '/';
    const slugs = [...chapters, 'secure-tunnel'];
    const pages = await Promise.all(slugs.map(async slug => {
      let source = await readFile(slug === 'secure-tunnel' ? path.join(root, '..', 'docs', zh ? 'secure-tunnel.zh.md' : 'secure-tunnel.md') : path.join(root, 'content', locale, `${slug}.md`), 'utf8');
      if (slug === 'secure-tunnel') source = source.replace(/\]\((mcp(?:-tunnel-helper)?\.md)(#[^)]+)?\)/g, (_, file, anchor = '') => `](${config.githubUrl}/blob/main/docs/${file}${anchor})`);
      return { slug, ...renderMarkdown(source) };
    }));
    for (const [index, page] of pages.entries()) {
      const route = guidePath(locale, page.slug);
      const en = guidePath('en', page.slug);
      const cn = guidePath('zh', page.slug);
      const nav = pages.filter(item => item.slug !== 'secure-tunnel').map(item => `<a href="${guidePath(locale, item.slug)}"${item.slug === page.slug ? ' aria-current="page"' : ''}>${escapeHtml(item.title)}</a>`).join('');
      const integrationsLabel = zh ? '集成' : 'Integrations';
      const integrations = `<details open><summary>${integrationsLabel}</summary><nav aria-label="${integrationsLabel}"><a href="${secureTunnelPath(locale)}"${page.slug === 'secure-tunnel' ? ' aria-current="page"' : ''}>${zh ? 'ChatGPT 安全隧道' : 'ChatGPT Secure Tunnel'}</a></nav></details>`;
      const toc = page.headings.map(item => `<a href="#${item.id}">${escapeHtml(item.text)}</a>`).join('');
      const previous = pages[index - 1];
      const next = pages[index + 1];
      const board = page.slug === 'attention';
      const fragment = async name => board ? readFile(path.join(root, 'content', locale, `attention.${name}.html`), 'utf8') : '';
      const crumb = page.slug === 'secure-tunnel' ? zh ? '集成 / ChatGPT' : 'Integrations / ChatGPT' : `${zh ? '使用指南' : 'User guide'} / ${config.guideVersion}`;
      const mark = '<span class="logo-mark" aria-hidden="true"><i class="c-run"></i><i class="c-quiet"></i><i class="c-done"></i><i class="c-stop"></i></span>';
      const html = `<!doctype html>
<html lang="${zh ? 'zh-Hans' : 'en'}"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(page.title)} — deck ${zh ? '使用指南' : 'guide'}</title>
<meta name="description" content="${escapeHtml(page.description)}">
<meta name="theme-color" content="#0d1117">
<link rel="canonical" href="${config.siteUrl}${route}">
<link rel="alternate" hreflang="en" href="${config.siteUrl}${en}">
${cn ? `<link rel="alternate" hreflang="zh-Hans" href="${config.siteUrl}${cn}">` : ''}
<link rel="alternate" hreflang="x-default" href="${config.siteUrl}${en}">
<link rel="icon" href="/favicon.ico" sizes="32x32"><link rel="icon" href="/assets/icon.svg" type="image/svg+xml"><link rel="apple-touch-icon" href="/assets/icon-180.png">
<link rel="stylesheet" href="/assets/site.css?v=880f34f06c24">
</head><body class="site">
<a class="skip-link" href="#main">${zh ? '跳到正文' : 'Skip to content'}</a>
<div class="topbar wrap"><nav class="pillnav" aria-label="${zh ? '主导航' : 'Main navigation'}"><a class="logo" href="${home}">${mark}deck</a><a class="push" href="${guidePath(locale, 'index')}">${zh ? '使用指南' : 'Guide'}</a><a class="opt" href="${config.githubUrl}">GitHub</a><a class="language-link" href="${zh ? en : cn}" lang="${zh ? 'en' : 'zh-Hans'}">${zh ? 'EN' : '中文'}</a><a class="btn btn-primary" href="${page.slug === 'secure-tunnel' ? `${config.githubUrl}/releases` : config.downloadUrl}">${page.slug === 'secure-tunnel' ? zh ? 'Deck 发布版本' : 'Deck releases' : `${zh ? '下载' : 'Download'} ${config.guideVersion}`}</a></nav></div>
<main id="main">
<header class="opener under-nav${board ? ' board-opener' : ''}"><div class="wrap opener-in">
<p class="crumb"><a href="${guidePath(locale, 'index')}">${crumb}</a></p>
<div class="opener-head">${page.head}${page.lede ? `<p class="opener-lede">${page.lede}</p>` : ''}</div>
${await fragment('board')}</div></header>
${await fragment('notes')}
<div class="wrap gshell">
<aside class="gside"><details open><summary>${zh ? '使用指南' : 'User guide'}</summary><nav aria-label="${zh ? '指南章节' : 'Guide chapters'}">${nav}</nav></details>${integrations}${page.slug === 'secure-tunnel' ? '' : `<p class="ver">deck ${config.guideVersion} · ${zh ? '稳定版' : 'Stable'}</p>`}<a class="ref" href="${guidePath(locale, 'input-and-settings')}#${zh ? '更新与版本' : 'updates-and-versions'}">${zh ? '更新与版本' : 'Updates and versions'} ↗</a></aside>
<article class="garticle${board ? ' board-chapter' : ''}">${page.body}
${page.slug === 'secure-tunnel' ? '' : `<nav class="pager2" aria-label="${zh ? '前后章节' : 'Previous and next chapters'}">${previous ? `<a href="${guidePath(locale, previous.slug)}"><span>${zh ? '上一章' : 'Previous'}</span>← ${escapeHtml(previous.title)}</a>` : '<span></span>'}${next ? `<a href="${guidePath(locale, next.slug)}"><span>${zh ? '下一章' : 'Next'}</span>${escapeHtml(next.title)} →</a>` : `<a href="${guidePath(locale, 'index')}"><span>${zh ? '指南' : 'Guide'}</span>${zh ? '返回指南' : 'Back to the guide'} →</a>`}</nav>`}
<p class="gsource">${page.slug === 'secure-tunnel' ? `${zh ? '文档来源：' : 'Canonical source: '}<a href="${config.githubUrl}/blob/main/docs/${zh ? 'secure-tunnel.zh.md' : 'secure-tunnel.md'}">docs/secure-tunnel${zh ? '.zh' : ''}.md ↗</a>` : `${zh ? `适用于 deck ${config.guideVersion} 稳定版。技术说明：` : `For deck ${config.guideVersion} Stable. Technical reference: `}<a href="${config.githubUrl}/blob/${config.guideRef}/README.md">README ↗</a>`} · <a href="${config.feedbackUrl}">${zh ? '反馈问题' : 'Report an issue'}</a></p></article>
<aside class="gout"><nav aria-label="${zh ? '本页目录' : 'On this page'}"><p>${zh ? '本页内容' : 'On this page'}</p>${toc}</nav></aside>
</div>
</main>
<footer class="foot2"><div class="wrap"><a class="logo" href="${home}">${mark}deck</a><span>${zh ? '跨任务关注，本地终端照常运行。' : 'Attention across local terminal work.'}</span><span class="links"><a href="${guidePath(locale, 'index')}">${zh ? '使用指南' : 'Guide'}</a><a href="${config.githubUrl}">GitHub</a><a href="${home}privacy/">${zh ? '隐私' : 'Privacy'}</a><a href="${config.feedbackUrl}">${zh ? '反馈' : 'Feedback'}</a></span></div></footer>
</body></html>`;
      const folder = path.join(output, route);
      await mkdir(folder, { recursive: true });
      await writeFile(path.join(folder, 'index.html'), html);
      routes.push({ route, en, cn });
    }
  }
  const sitemapPath = path.join(output, 'sitemap.xml');
  const sitemap = await readFile(sitemapPath, 'utf8');
  await writeFile(sitemapPath, sitemap.replace('</urlset>', routes.map(({ route, en, cn }) => `  <url><loc>${config.siteUrl}${route}</loc><xhtml:link rel="alternate" hreflang="en" href="${config.siteUrl}${en}"/>${cn ? `<xhtml:link rel="alternate" hreflang="zh-Hans" href="${config.siteUrl}${cn}"/>` : ''}</url>`).join('\n') + '\n</urlset>'));
  const redirectsPath = path.join(output, '_redirects');
  await writeFile(redirectsPath, await readFile(redirectsPath, 'utf8') + routes.map(({ route }) => `${route.slice(0, -1)} ${route} 301`).join('\n') + '\n');
}
