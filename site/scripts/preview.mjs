import { createReadStream } from 'node:fs';
import { stat } from 'node:fs/promises';
import { createServer } from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..', 'dist');
const port = Number(process.env.PORT || 4173);
const types = new Map([
  ['.css', 'text/css; charset=utf-8'],
  ['.html', 'text/html; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.png', 'image/png'],
  ['.svg', 'image/svg+xml'],
  ['.ico', 'image/x-icon'],
  ['.txt', 'text/plain; charset=utf-8'],
  ['.xml', 'application/xml; charset=utf-8'],
]);

createServer(async (request, response) => {
  try {
    const url = new URL(request.url || '/', 'http://localhost');
    let relative = decodeURIComponent(url.pathname).replace(/^\/+/, '');
    if (!relative || relative.endsWith('/')) relative += 'index.html';
    const requested = path.resolve(root, relative);
    if (requested !== root && !requested.startsWith(root + path.sep)) throw new Error('invalid path');
    const info = await stat(requested);
    if (!info.isFile()) throw new Error('not a file');
    response.writeHead(200, { 'content-type': types.get(path.extname(requested)) || 'application/octet-stream' });
    createReadStream(requested).pipe(response);
  } catch {
    response.writeHead(404, { 'content-type': 'text/html; charset=utf-8' });
    createReadStream(path.join(root, '404.html')).pipe(response);
  }
}).listen(port, '127.0.0.1', () => {
  console.log(`deck landing preview: http://127.0.0.1:${port}`);
});
