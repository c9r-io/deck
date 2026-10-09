import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { readFile, readdir, stat } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { renderMarkdown } from '../scripts/guides.mjs';

const exec = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const dist = path.join(root, 'dist');
const config = JSON.parse(await readFile(path.join(root, 'site.config.json'), 'utf8'));

await exec(process.execPath, ['scripts/build.mjs'], { cwd: root });

async function htmlRoutes(folder, prefix = '') {
  const result = [];
  for (const entry of await readdir(folder, { withFileTypes: true })) {
    const relative = path.join(prefix, entry.name);
    if (entry.isDirectory()) result.push(...await htmlRoutes(path.join(folder, entry.name), relative));
    else if (entry.name.endsWith('.html')) result.push(relative);
  }
  return result;
}
const routes = await htmlRoutes(dist);

test('build emits every public route and infrastructure file', async () => {
  for (const file of [...routes, 'assets/site.css', 'assets/og.png', 'assets/icon.svg', 'assets/icon-180.png', 'favicon.ico', 'robots.txt', 'sitemap.xml', '_headers', '_redirects']) {
    assert.equal((await stat(path.join(dist, file))).isFile(), true, file);
  }
});

test('pages have language, metadata, navigation, and resolved config', async () => {
  const english = await readFile(path.join(dist, 'index.html'), 'utf8');
  const chinese = await readFile(path.join(dist, 'zh/index.html'), 'utf8');
  assert.match(english, /<html lang="en">/);
  assert.match(chinese, /<html lang="zh-Hans">/);
  for (const html of [english, chinese]) {
    assert.doesNotMatch(html, /\{\{[A-Z0-9_]+\}\}/);
    assert.ok(html.includes(config.downloadUrl));
    assert.ok(html.includes(config.githubUrl));
    assert.ok(html.includes(config.feedbackUrl));
    assert.match(html, /rel="canonical"/);
    assert.match(html, /hreflang="en"/);
    assert.match(html, /hreflang="zh-Hans"/);
    assert.ok(html.includes(`${config.siteUrl}/assets/og.png`));
  }
});

function assertStaticPrivacy(html, route) {
  assert.doesNotMatch(html, /<script\b/i, route);
  assert.doesNotMatch(html, /<form\b|\son[a-z]+\s*=|javascript:/i, route);
  assert.doesNotMatch(html, /\b(?:src|poster)\s*=\s*["'](?:https?:)?\/\//i, route);
  assert.doesNotMatch(html, /google-analytics|googletagmanager|cloudflareinsights|plausible|posthog|segment\.com/i, route);
  assert.doesNotMatch(html, /<link[^>]+(?:font|preconnect)/i, route);
  // Canonical/alternate links and source-reference anchors do not load assets.
  for (const [tag] of html.matchAll(/<link\b[^>]*>/gi)) {
    if (/\brel\s*=\s*["'](?:stylesheet|icon|apple-touch-icon|preload|modulepreload|prefetch|dns-prefetch)["']/i.test(tag)) {
      assert.doesNotMatch(tag, /\bhref\s*=\s*["'](?:https?:)?\/\//i, route);
    }
  }
  assert.doesNotMatch(html, /<(?:link|script|img|source|video|audio|iframe|embed|object)\b[^>]+https?:\/\/[^"']+\.(?:js|css|woff2?)/i, route);
}

test('every route contains no analytics, remote assets, or executable JavaScript', async () => {
  for (const route of routes) assertStaticPrivacy(await readFile(path.join(dist, route), 'utf8'), route);
});

test('privacy guard accepts source links but rejects active content and remote resources', () => {
  assert.doesNotThrow(() => assertStaticPrivacy('<a href="https://github.com/c9r-io/deck/blob/main/app/ui/js/attention-model.js">Source</a><link rel="stylesheet" href="/assets/site.css">', 'source reference'));
  const forbidden = {
    'inline script': '<script>alert(1)</script>',
    'local script': '<script src="/assets/site.js"></script>',
    'remote script': '<script src="https://example.com/site.js"></script>',
    'remote stylesheet': '<link rel="stylesheet" href="https://example.com/site.css">',
    'stylesheet without extension': '<link href="https://example.com/styles" rel="stylesheet">',
    'protocol-relative stylesheet': '<link rel="stylesheet" href="//example.com/site.css">',
    'remote image': '<img src="https://example.com/image.png" alt="Example">',
    'remote video': '<source src="//example.com/demo.mp4">',
    'remote poster': '<video poster="https://example.com/poster.jpg"></video>',
    'remote icon': '<link rel="icon" href="https://example.com/favicon.ico">',
    form: '<form action="/subscribe"><input name="email"></form>',
    'inline event': '<a href="/" onclick="alert(1)">Home</a>',
    'script URL': '<a href="javascript:alert(1)">Run</a>',
  };
  for (const [name, html] of Object.entries(forbidden)) {
    assert.throws(() => assertStaticPrivacy(html, name), { code: 'ERR_ASSERTION' }, name);
  }
});

test('marketing copy keeps status and scheduler boundaries explicit', async () => {
  const english = await readFile(path.join(dist, 'index.html'), 'utf8');
  const chinese = await readFile(path.join(dist, 'zh/index.html'), 'utf8');
  assert.match(english, /quiet is not ready/i);
  assert.match(english, /app must be running/i);
  assert.match(english, /amber means the session has been quiet/i);
  assert.match(chinese, /安静不代表程序已准备好/i);
  assert.match(chinese, /应用必须运行/i);
  assert.match(chinese, /琥珀色只说明 session/i);
});

test('every page declares locally hosted icons', async () => {
  for (const route of routes) {
    const html = await readFile(path.join(dist, route), 'utf8');
    assert.match(html, /<link rel="icon" href="\/favicon\.ico"/, route);
    assert.match(html, /<link rel="icon" href="\/assets\/icon\.svg"/, route);
    assert.match(html, /<link rel="apple-touch-icon" href="\/assets\/icon-180\.png"/, route);
  }
});

test('security headers prohibit telemetry connections', async () => {
  const headers = await readFile(path.join(dist, '_headers'), 'utf8');
  assert.match(headers, /connect-src 'none'/);
  assert.match(headers, /frame-ancestors 'none'/);
  assert.match(headers, /form-action 'none'/);
});

test('every local page link and fragment resolves in the static build', async () => {
  for (const route of routes) {
    const html = await readFile(path.join(dist, route), 'utf8');
    for (const [, href] of html.matchAll(/href="([^"]+)"/g)) {
      if ((!href.startsWith('/') && !href.startsWith('#')) || href.startsWith('//')) continue;
      const [pagePath, fragment] = href.split('#');
      if (pagePath.startsWith('/assets/')) continue;
      const bare = pagePath.replace(/^\//, '').replace(/\/$/, '');
      const relative = !pagePath ? route : pagePath === '/' ? 'index.html' : path.extname(bare) ? bare : `${bare}/index.html`;
      assert.equal((await stat(path.join(dist, relative))).isFile(), true, `${route} -> ${href}`);
      if (fragment) {
        const target = await readFile(path.join(dist, relative), 'utf8');
        assert.ok(target.includes(`id="${decodeURIComponent(fragment)}"`), `${route} -> missing fragment ${href}`);
      }
    }
  }
});

test('both languages publish all guide topics, overview, metadata and version-scoped references', async () => {
  const topics = ['', 'start/', 'attention/', 'prompts/', 'sessions/', 'automations/', 'integrations/', 'input-and-settings/'];
  const sitemap = await readFile(path.join(dist, 'sitemap.xml'), 'utf8');
  const redirects = await readFile(path.join(dist, '_redirects'), 'utf8');
  for (const topic of topics) {
    for (const locale of ['', 'zh/']) {
      const route = `${locale}guide/${topic}`;
      const html = await readFile(path.join(dist, route, 'index.html'), 'utf8');
      assert.ok(html.includes(`<html lang="${locale ? 'zh-Hans' : 'en'}">`));
      assert.equal([...html.matchAll(/<h1\b/g)].length, 1);
      assert.match(html, /aria-current="page"/);
      assert.match(html, /deck 0\.7\.8/);
      assert.ok(html.includes(`<summary>${locale ? '集成' : 'Integrations'}</summary>`));
      assert.ok(html.includes(`/blob/${config.guideRef}/README.md`));
      assert.ok(html.includes(`rel="canonical" href="${config.siteUrl}/${route}"`));
      assert.ok(html.includes(`hreflang="${locale ? 'en' : 'zh-Hans'}"`));
      assert.ok(sitemap.includes(`<loc>${config.siteUrl}/${route}</loc>`));
      assert.ok(redirects.includes(`/${route.slice(0, -1)} /${route} 301`));
    }
  }
  // The Slack placeholders are user-authored template syntax, not unresolved site config.
  for (const route of ['guide/automations/', 'zh/guide/automations/']) {
    assert.match(await readFile(path.join(dist, route, 'index.html'), 'utf8'), /\{\{msg\.text\}\}/);
  }
});

test('both languages describe current voice and integration behavior', async () => {
  const english = await readFile(path.join(dist, 'guide/input-and-settings/index.html'), 'utf8');
  const chinese = await readFile(path.join(dist, 'zh/guide/input-and-settings/index.html'), 'utf8');
  for (const html of [english, chinese]) {
    assert.doesNotMatch(html, /An empty draft starts recording|仅插入把文字放进终端/);
    assert.match(html, /0\.7\.8/);
  }
  assert.match(english, /never sends Enter for voice input/);
  assert.match(chinese, /语音输入不会替你按回车/);
  for (const route of ['guide/integrations/index.html', 'zh/guide/integrations/index.html']) {
    const html = await readFile(path.join(dist, route), 'utf8');
    assert.match(html, /Phone Connector|手机 Connector/);
    assert.match(html, /Secure Tunnel/);
  }
});

test('Markdown supports stable Unicode anchors, duplicate headings and rejects missing structure', () => {
  const page = renderMarkdown('# Guide\n\nLead.\n\n## 下一步\n\nOne.\n\n## 下一步\n\nTwo.');
  assert.deepEqual(page.headings.map(item => item.id), ['下一步', '下一步-2']);
  assert.match(page.html, /id="下一步-2"/);
  assert.throws(() => renderMarkdown('No title.'), /exactly one H1/);
  assert.throws(() => renderMarkdown('# Only a title'), /introductory paragraph/);
});

test('parallel-session scenario is paired, discoverable and indexed without changing the guide version', async () => {
  const sitemap = await readFile(path.join(dist, 'sitemap.xml'), 'utf8');
  const redirects = await readFile(path.join(dist, '_redirects'), 'utf8');
  for (const locale of ['', 'zh/']) {
    const route = `/${locale}scenarios/parallel-sessions/`;
    const other = `/${locale ? '' : 'zh/'}scenarios/parallel-sessions/`;
    const html = await readFile(path.join(dist, locale, 'scenarios/parallel-sessions/index.html'), 'utf8');
    const home = await readFile(path.join(dist, locale, 'index.html'), 'utf8');
    const guide = await readFile(path.join(dist, locale, 'guide/index.html'), 'utf8');
    assert.ok(html.includes(`<html lang="${locale ? 'zh-Hans' : 'en'}">`));
    assert.equal([...html.matchAll(/<h1\b/g)].length, 1);
    assert.ok(html.includes(`rel="canonical" href="${config.siteUrl}${route}"`));
    assert.ok(html.includes(`hreflang="${locale ? 'en' : 'zh-Hans'}" href="${config.siteUrl}${other}"`));
    assert.ok(html.includes(`class="language-link" href="${other}"`));
    assert.ok(html.includes(`property="og:image" content="${config.siteUrl}/assets/og.png"`));
    assert.match(html, /<meta name="description" content="[^"]+"/);
    assert.match(html, /<th scope="col">/);
    assert.ok(home.includes(`href="${route}"`));
    assert.ok(guide.includes(`href="${route}"`));
    assert.ok(sitemap.includes(`<loc>${config.siteUrl}${route}</loc>`));
    assert.ok(redirects.includes(`${route.slice(0, -1)} ${route} 301`));
    assert.ok(guide.includes(`deck ${config.guideVersion}`));
  }
});

test('parallel-session scenario keeps coverage, outcome and restart limits explicit in both languages', async () => {
  const english = await readFile(path.join(dist, 'scenarios/parallel-sessions/index.html'), 'utf8');
  const chinese = await readFile(path.join(dist, 'zh/scenarios/parallel-sessions/index.html'), 'utf8');
  for (const html of [english, chinese]) {
    assert.match(html, /Codex/);
    assert.match(html, /\/hooks/);
    assert.match(html, /dcc3c064404e9eb4d392cb6228ae0b9f2cf14054/);
  }
  assert.match(english, /turn ending is not task completion or success/);
  assert.match(english, /Quiet is not readiness/);
  assert.match(english, /Input-request hooks do not cover every question/);
  assert.match(english, /Unread history and missed-event replay are not guaranteed/);
  assert.match(english, /live status never moves cards/);
  assert.match(chinese, /本轮结束不等于任务完成或成功/);
  assert.match(chinese, /安静不代表已准备好接收输入/);
  assert.match(chinese, /输入请求 hook 也不能覆盖每一种提问/);
  assert.match(chinese, /不保证保留未读历史，也不保证补回漏掉的事件/);
  assert.match(chinese, /状态变化不会自动移动卡片/);
});

test('ChatGPT Secure Tunnel pages render both canonical repo guides with paired navigation', async () => {
  const source = await readFile(path.join(root, '..', 'docs', 'secure-tunnel.md'), 'utf8');
  const chineseSource = await readFile(path.join(root, '..', 'docs', 'secure-tunnel.zh.md'), 'utf8');
  const page = await readFile(path.join(dist, 'docs/integrations/chatgpt-secure-tunnel/index.html'), 'utf8');
  const chinesePage = await readFile(path.join(dist, 'zh/docs/integrations/chatgpt-secure-tunnel/index.html'), 'utf8');
  const overview = await readFile(path.join(dist, 'guide/index.html'), 'utf8');
  const chineseOverview = await readFile(path.join(dist, 'zh/guide/index.html'), 'utf8');
  const sitemap = await readFile(path.join(dist, 'sitemap.xml'), 'utf8');
  assert.match(source, /^# Connect ChatGPT to Deck with Secure Tunnel$/m);
  assert.match(chineseSource, /^# 使用 Secure Tunnel 将 ChatGPT 连接到 Deck$/m);
  assert.match(page, /<html lang="en">/);
  assert.match(chinesePage, /<html lang="zh-Hans">/);
  assert.match(page, /<title>Connect ChatGPT to Deck with Secure Tunnel/);
  assert.match(chinesePage, /<title>使用 Secure Tunnel 将 ChatGPT 连接到 Deck/);
  assert.match(page, /<meta name="description" content="Use ChatGPT to access a Deck project/);
  assert.match(chinesePage, /<meta name="description" content="让 ChatGPT 访问你明确授权的 Deck 项目/);
  assert.match(page, /aria-current="page">ChatGPT Secure Tunnel/);
  assert.match(chinesePage, /aria-current="page">ChatGPT 安全隧道/);
  assert.match(page, /<h2 id="2-create-a-runtime-api-key">/);
  assert.match(chinesePage, /<h2 id="2-创建-runtime-api-key">/);
  assert.match(page, /Restricted/);
  assert.match(chinesePage, /Restricted/);
  assert.match(page, /docs\/secure-tunnel\.md/);
  assert.match(chinesePage, /docs\/secure-tunnel\.zh\.md/);
  assert.match(page, /class="language-link" href="\/zh\/docs\/integrations\/chatgpt-secure-tunnel\/"/);
  assert.match(chinesePage, /class="language-link" href="\/docs\/integrations\/chatgpt-secure-tunnel\/"/);
  assert.match(page, /hreflang="zh-Hans" href="https:\/\/deck\.c9r\.io\/zh\/docs\/integrations\/chatgpt-secure-tunnel\/"/);
  assert.match(overview, /href="\/docs\/integrations\/chatgpt-secure-tunnel\/"/);
  assert.match(chineseOverview, /href="\/zh\/docs\/integrations\/chatgpt-secure-tunnel\/"/);
  assert.match(sitemap, /\/docs\/integrations\/chatgpt-secure-tunnel\//);
  assert.match(sitemap, /\/zh\/docs\/integrations\/chatgpt-secure-tunnel\//);
});
