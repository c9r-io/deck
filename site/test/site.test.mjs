import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';

const exec = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const dist = path.join(root, 'dist');
const config = JSON.parse(await readFile(path.join(root, 'site.config.json'), 'utf8'));

await exec(process.execPath, ['scripts/build.mjs'], { cwd: root });

const routes = ['index.html', 'zh/index.html', 'privacy/index.html', 'zh/privacy/index.html', '404.html'];

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

test('first version contains no analytics, cookies, remote assets, or executable JavaScript', async () => {
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
  assert.match(chinese, /安静不等于 READY/i);
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

test('every root-relative page link resolves in the static build', async () => {
  for (const route of routes) {
    const html = await readFile(path.join(dist, route), 'utf8');
    for (const [, href] of html.matchAll(/href="([^"]+)"/g)) {
      if (!href.startsWith('/') || href.startsWith('//')) continue;
      const pagePath = href.split('#', 1)[0];
      if (!pagePath || pagePath.startsWith('/assets/')) continue;
      const bare = pagePath.replace(/^\//, '').replace(/\/$/, '');
      const relative = pagePath === '/' ? 'index.html' : path.extname(bare) ? bare : `${bare}/index.html`;
      assert.equal((await stat(path.join(dist, relative))).isFile(), true, `${route} -> ${href}`);
    }
  }
});
