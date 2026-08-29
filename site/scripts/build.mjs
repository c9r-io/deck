import { cp, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const source = path.join(root, 'src');
const output = path.join(root, 'dist');
const config = JSON.parse(await readFile(path.join(root, 'site.config.json'), 'utf8'));

const values = {
  SITE_URL: config.siteUrl.replace(/\/$/, ''),
  DOWNLOAD_URL: config.downloadUrl,
  GITHUB_URL: config.githubUrl,
  FEEDBACK_URL: config.feedbackUrl,
  LICENSE_URL: config.licenseUrl,
  RELEASE_LABEL: config.releaseLabel,
  RELEASE_LABEL_ZH: config.releaseLabelZh,
};

const textExtensions = new Set(['.html', '.css', '.js', '.json', '.txt', '.xml', '']);

async function renderDirectory(from, to) {
  await mkdir(to, { recursive: true });
  for (const entry of await readdir(from)) {
    const sourcePath = path.join(from, entry);
    const outputPath = path.join(to, entry);
    const info = await stat(sourcePath);
    if (info.isDirectory()) {
      await renderDirectory(sourcePath, outputPath);
      continue;
    }
    if (!textExtensions.has(path.extname(entry))) {
      await cp(sourcePath, outputPath);
      continue;
    }
    let contents = await readFile(sourcePath, 'utf8');
    for (const [key, value] of Object.entries(values)) {
      contents = contents.replaceAll(`{{${key}}}`, value);
    }
    const unresolved = contents.match(/\{\{[A-Z0-9_]+\}\}/g);
    if (unresolved) throw new Error(`${sourcePath} has unresolved placeholders: ${unresolved.join(', ')}`);
    await writeFile(outputPath, contents);
  }
}

await rm(output, { recursive: true, force: true });
await renderDirectory(source, output);
console.log(`Built deck landing page in ${output}`);
