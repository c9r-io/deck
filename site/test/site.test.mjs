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

test('every route contains no analytics, remote assets, or executable JavaScript', async () => {
  for (const route of routes) {
    const html = await readFile(path.join(dist, route), 'utf8');
    assert.doesNotMatch(html, /<script\b/i, route);
    assert.doesNotMatch(html, /google-analytics|googletagmanager|cloudflareinsights|plausible|posthog|segment\.com/i, route);
    assert.doesNotMatch(html, /<link[^>]+(?:font|preconnect)/i, route);
    assert.doesNotMatch(html, /https?:\/\/[^"']+\.(?:js|css|woff2?)/i, route);
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
