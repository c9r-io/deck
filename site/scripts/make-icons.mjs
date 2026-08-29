// Regenerates the deck site icons from the same tokens the stylesheet uses.
// Pure node builtins on purpose: the site has no build dependencies.
//
//   node scripts/make-icons.mjs
//
// Writes src/assets/icon.svg, src/assets/icon-180.png and src/favicon.ico.
import { deflateSync } from 'node:zlib';
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const assets = path.resolve(here, '..', 'src', 'assets');
const src = path.resolve(here, '..', 'src');

const BG = [13, 17, 23, 255]; // --bg
const EDGE = [40, 60, 62, 255]; // --accent at low alpha over --bg
const TILES = [
  { x: 6, y: 6, color: [69, 211, 146, 255] }, // --green
  { x: 17, y: 6, color: [239, 185, 95, 255] }, // --amber
  { x: 6, y: 17, color: [82, 214, 190, 255] }, // --accent
  { x: 17, y: 17, color: [109, 120, 137, 255] }, // --stopped
];
const TILE = 9;
const TILE_R = 2;
const FRAME_R = 7;
const GRID = 32; // design grid every geometry value above is expressed in

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32" role="img" aria-label="deck">
  <rect width="32" height="32" rx="${FRAME_R}" fill="#0d1117"/>
  <rect x="0.5" y="0.5" width="31" height="31" rx="${FRAME_R - 0.5}" fill="none" stroke="#52d6be" stroke-opacity="0.5"/>
${TILES.map(
  (t) =>
    `  <rect x="${t.x}" y="${t.y}" width="${TILE}" height="${TILE}" rx="${TILE_R}" fill="${rgbHex(t.color)}"/>`,
).join('\n')}
</svg>
`;

function rgbHex([r, g, b]) {
  return `#${[r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('')}`;
}

// Signed distance to a rounded rectangle, in design-grid units.
function roundedRectDistance(px, py, x, y, w, h, r) {
  const cx = x + w / 2;
  const cy = y + h / 2;
  const qx = Math.abs(px - cx) - (w / 2 - r);
  const qy = Math.abs(py - cy) - (h / 2 - r);
  const outside = Math.hypot(Math.max(qx, 0), Math.max(qy, 0));
  return outside + Math.min(Math.max(qx, qy), 0) - r;
}

function over(dst, srcColor, alpha) {
  for (let i = 0; i < 3; i += 1) dst[i] = Math.round(dst[i] * (1 - alpha) + srcColor[i] * alpha);
  dst[3] = Math.round(dst[3] * (1 - alpha) + srcColor[3] * alpha);
}

// 4x4 supersampled coverage keeps the 32px icon readable without a rasterizer
// dependency.
function coverage(px, py, unit, shape) {
  let hits = 0;
  for (let sy = 0; sy < 4; sy += 1) {
    for (let sx = 0; sx < 4; sx += 1) {
      const gx = ((px + (sx + 0.5) / 4) * GRID) / unit;
      const gy = ((py + (sy + 0.5) / 4) * GRID) / unit;
      if (shape(gx, gy) <= 0) hits += 1;
    }
  }
  return hits / 16;
}

function render(size) {
  const pixels = new Uint8Array(size * size * 4);
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const pixel = [0, 0, 0, 0];
      const frame = (gx, gy) => roundedRectDistance(gx, gy, 0, 0, 32, 32, FRAME_R);
      over(pixel, BG, coverage(x, y, size, frame));
      const border = (gx, gy) =>
        Math.max(frame(gx, gy), -(roundedRectDistance(gx, gy, 1, 1, 30, 30, FRAME_R - 1)));
      over(pixel, EDGE, coverage(x, y, size, border));
      for (const tile of TILES) {
        const shape = (gx, gy) =>
          roundedRectDistance(gx, gy, tile.x, tile.y, TILE, TILE, TILE_R);
        over(pixel, tile.color, coverage(x, y, size, shape));
      }
      pixels.set(pixel, (y * size + x) * 4);
    }
  }
  return pixels;
}

function chunk(type, body) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(body.length);
  const typed = Buffer.concat([Buffer.from(type, 'latin1'), body]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(typed));
  return Buffer.concat([length, typed, crc]);
}

let crcTable = null;
function crc32(buffer) {
  if (!crcTable) {
    crcTable = new Int32Array(256);
    for (let n = 0; n < 256; n += 1) {
      let c = n;
      for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crcTable[n] = c;
    }
  }
  let c = -1;
  for (const byte of buffer) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function png(size) {
  const pixels = render(size);
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y += 1) {
    raw[y * (size * 4 + 1)] = 0; // no per-scanline filter
    Buffer.from(pixels.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // bit depth
  header[9] = 6; // truecolour with alpha
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// ICO may carry PNG payloads, so browsers requesting /favicon.ico directly get
// the same artwork without a second rasterization path.
function ico(sizes) {
  const images = sizes.map((size) => ({ size, data: png(size) }));
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(images.length, 4);
  let offset = 6 + images.length * 16;
  const entries = images.map((image) => {
    const entry = Buffer.alloc(16);
    entry[0] = image.size >= 256 ? 0 : image.size;
    entry[1] = image.size >= 256 ? 0 : image.size;
    entry.writeUInt16LE(1, 4); // colour planes
    entry.writeUInt16LE(32, 6); // bits per pixel
    entry.writeUInt32BE(0, 8);
    entry.writeUInt32LE(image.data.length, 8);
    entry.writeUInt32LE(offset, 12);
    offset += image.data.length;
    return entry;
  });
  return Buffer.concat([header, ...entries, ...images.map((image) => image.data)]);
}

await mkdir(assets, { recursive: true });
await writeFile(path.join(assets, 'icon.svg'), svg);
await writeFile(path.join(assets, 'icon-180.png'), png(180));
await writeFile(path.join(src, 'favicon.ico'), ico([16, 32, 48]));
console.log('Wrote icon.svg, icon-180.png and favicon.ico');
