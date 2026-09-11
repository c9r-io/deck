// Public guides are authored in paired Markdown files and rendered at build time.
// Content is reviewed repository source, never visitor-supplied Markdown.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { Marked } from 'marked';

export const chapters = ['index', 'start', 'attention', 'prompts', 'sessions', 'automations', 'input-and-settings'];
export const escapeHtml = text => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const guidePath = (locale, slug) => `${locale === 'zh' ? '/zh' : ''}/guide/${slug === 'index' ? '' : `${slug}/`}`;

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
  return { title, description, html: parser.parser(tokens), headings };
}

export async function buildGuides(root, output, config) {
  const routes = [];
  for (const locale of ['en', 'zh']) {
    const zh = locale === 'zh';
    const home = zh ? '/zh/' : '/';
    const pages = await Promise.all(chapters.map(async slug => ({ slug, ...renderMarkdown(await readFile(path.join(root, 'content', locale, `${slug}.md`), 'utf8')) })));
    for (const [index, page] of pages.entries()) {
      const route = guidePath(locale, page.slug);
      const en = guidePath('en', page.slug);
      const cn = guidePath('zh', page.slug);
      const nav = pages.map(item => `<a href="${guidePath(locale, item.slug)}"${item.slug === page.slug ? ' aria-current="page"' : ''}>${escapeHtml(item.title)}</a>`).join('');
      const toc = page.headings.map(item => `<a href="#${item.id}">${escapeHtml(item.text)}</a>`).join('');
      const previous = pages[index - 1];
      const next = pages[index + 1];
      const html = `<!doctype html>
<html lang="${zh ? 'zh-Hans' : 'en'}"><head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(page.title)} — deck ${zh ? '使用指南' : 'guide'}</title>
<meta name="description" content="${escapeHtml(page.description)}">
<meta name="theme-color" content="#0d1117">
<link rel="canonical" href="${config.siteUrl}${route}">
<link rel="alternate" hreflang="en" href="${config.siteUrl}${en}">
<link rel="alternate" hreflang="zh-Hans" href="${config.siteUrl}${cn}">
<link rel="alternate" hreflang="x-default" href="${config.siteUrl}${en}">
<link rel="icon" href="/favicon.ico" sizes="32x32"><link rel="icon" href="/assets/icon.svg" type="image/svg+xml"><link rel="apple-touch-icon" href="/assets/icon-180.png">
<link rel="stylesheet" href="/assets/site.css">
</head><body>
<a class="skip-link" href="#main">${zh ? '跳到正文' : 'Skip to content'}</a>
<header class="site-header"><nav class="shell nav" aria-label="${zh ? '主导航' : 'Main navigation'}"><a class="brand" href="${home}"><span class="brand-mark" aria-hidden="true">▦</span>deck</a><div class="nav-links"><a class="guide-link" href="${guidePath(locale, 'index')}">${zh ? '使用指南' : 'Guide'}</a><a href="${config.githubUrl}">GitHub</a><a class="language-link" href="${zh ? en : cn}" lang="${zh ? 'en' : 'zh-Hans'}">${zh ? 'EN' : '中文'}</a><a class="button primary" href="${config.downloadUrl}">${zh ? '下载' : 'Download'} ${config.guideVersion}</a></div></nav></header>
<div class="shell guide-layout">
<aside class="guide-sidebar"><details open><summary>${zh ? '使用指南' : 'User guide'}</summary><nav aria-label="${zh ? '指南章节' : 'Guide chapters'}">${nav}</nav></details><p class="guide-version">deck ${config.guideVersion} · Stable</p><a class="reference-link" href="${guidePath(locale, 'input-and-settings')}#${zh ? '更新与版本' : 'updates-and-versions'}">${zh ? '更新与版本' : 'Updates and versions'} ↗</a></aside>
<main id="main" class="guide-article"><p class="eyebrow">${zh ? '使用指南' : 'User guide'} / ${config.guideVersion}</p>${page.html}
<nav class="chapter-pagination" aria-label="${zh ? '前后章节' : 'Previous and next chapters'}">${previous ? `<a href="${guidePath(locale, previous.slug)}"><span>${zh ? '上一章' : 'Previous'}</span>← ${escapeHtml(previous.title)}</a>` : '<span></span>'}${next ? `<a href="${guidePath(locale, next.slug)}"><span>${zh ? '下一章' : 'Next'}</span>${escapeHtml(next.title)} →</a>` : `<a href="${guidePath(locale, 'index')}">${zh ? '返回指南' : 'Back to the guide'} →</a>`}</nav>
<p class="guide-source">${zh ? '适用于 deck 0.6.6，操作方式与 deck 0.6.5 相同。更多技术说明：' : 'Based on deck 0.6.5 functionality; also applies to 0.6.6. Technical reference: '}<a href="${config.githubUrl}/blob/${config.guideRef}/README.md">README ↗</a> · <a href="${config.feedbackUrl}">${zh ? '反馈问题' : 'Report an issue'}</a></p></main>
<aside class="guide-toc"><nav aria-label="${zh ? '本页目录' : 'On this page'}"><p>${zh ? '本页内容' : 'On this page'}</p>${toc}</nav></aside>
</div><footer class="site-footer"><div class="shell footer"><a class="brand" href="${home}">▦ deck</a><span>${zh ? '集中管理 CLI session，减少来回查看。' : 'Less checking. Know when to step in.'}</span><div class="footer-links"><a href="${home}privacy/">${zh ? '隐私' : 'Privacy'}</a><a href="${config.githubUrl}">GitHub</a></div></div></footer>
</body></html>`;
      const folder = path.join(output, route);
      await mkdir(folder, { recursive: true });
      await writeFile(path.join(folder, 'index.html'), html);
      routes.push({ route, en, cn });
    }
  }
  const sitemapPath = path.join(output, 'sitemap.xml');
  const sitemap = await readFile(sitemapPath, 'utf8');
  await writeFile(sitemapPath, sitemap.replace('</urlset>', routes.map(({ route, en, cn }) => `  <url><loc>${config.siteUrl}${route}</loc><xhtml:link rel="alternate" hreflang="en" href="${config.siteUrl}${en}"/><xhtml:link rel="alternate" hreflang="zh-Hans" href="${config.siteUrl}${cn}"/></url>`).join('\n') + '\n</urlset>'));
  const redirectsPath = path.join(output, '_redirects');
  await writeFile(redirectsPath, await readFile(redirectsPath, 'utf8') + routes.map(({ route }) => `${route.slice(0, -1)} ${route} 301`).join('\n') + '\n');
}
