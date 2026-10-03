#!/usr/bin/env node
// One editable SVG master supplies dashboard, homepage, favicons and previews.
// node scripts/build_logo.mjs static/logo.svg static /path/to/sharp
import { createRequire } from 'node:module';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
const require = createRequire(import.meta.url);
const [input = 'static/logo.svg', out = 'static', modulePath = 'sharp', homepage = 'homepage'] = process.argv.slice(2);
if (!input.endsWith('.svg')) throw new Error('Usage: build_logo.mjs INPUT.svg [OUTPUT_DIR] [SHARP_MODULE] [HOMEPAGE_DIR]');
const sharp = require(modulePath);
const source = await readFile(resolve(input));
const paper = '#f5f4ee';
await mkdir(out, { recursive: true });
await mkdir(resolve(homepage, 'assets'), { recursive: true });
await writeFile(resolve(homepage, 'assets/logo.svg'), source);
const master = await sharp(source, { density: 384 }).resize(512, 512)
  .flatten({ background: paper }).png().toBuffer();
const files = new Map([['logo.png', master]]);
for (const [name, size] of [['favicon.png', 32], ['apple-touch-icon.png', 180]]) {
  files.set(name, await sharp(source, { density: 384 }).resize(size, size)
    .flatten({ background: paper }).png().toBuffer());
}
const sizes = [16, 32, 48, 64, 128, 256];
const bitmaps = await Promise.all(sizes.map(size => sharp(source, { density: 384 }).resize(size, size)
  .flatten({ background: paper }).png().toBuffer()));
const header = Buffer.alloc(6 + sizes.length * 16);
header.writeUInt16LE(1, 2); header.writeUInt16LE(sizes.length, 4);
let offset = header.length;
for (let i = 0; i < sizes.length; i++) {
  const index = 6 + i * 16;
  header[index] = header[index + 1] = sizes[i] === 256 ? 0 : sizes[i];
  header.writeUInt16LE(1, index + 4); header.writeUInt16LE(32, index + 6);
  header.writeUInt32LE(bitmaps[i].length, index + 8); header.writeUInt32LE(offset, index + 12);
  offset += bitmaps[i].length;
}
files.set('favicon.ico', Buffer.concat([header, ...bitmaps]));
for (const [name, bytes] of files) {
  await writeFile(resolve(out, name), bytes);
  await writeFile(resolve(homepage, name === 'favicon.ico' ? name : `assets/${name}`), bytes);
}
// A share card uses the same mark, with text kept in the SVG layout source.
const card = Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="630" viewBox="0 0 1200 630">
<rect width="1200" height="630" fill="${paper}"/>
<path d="M64 112H1136M64 536H1136" stroke="#c8cec5"/>
<svg x="56" y="54" width="72" height="72" viewBox="0 0 128 128">${source.toString().match(/<g id="praxis-mark">[\s\S]*?<\/g>/)[0]}</svg>
<text x="144" y="103" fill="#142724" font-family="DejaVu Sans, sans-serif" font-size="30" font-weight="600">Praxis</text>
<text x="72" y="272" fill="#142724" font-family="DejaVu Serif, serif" font-size="68">From a decision</text>
<text x="72" y="360" fill="#142724" font-family="DejaVu Serif, serif" font-size="68">to a verified result.</text>
<text x="76" y="444" fill="#23594d" font-family="DejaVu Sans, sans-serif" font-size="26">A self-hosted runtime for AI agent workflows.</text>
<text x="76" y="584" fill="#23594d" font-family="DejaVu Sans, sans-serif" font-size="22">getpraxis.boo</text>
<svg x="908" y="260" width="200" height="200" viewBox="0 0 128 128">${source.toString().match(/<g id="praxis-mark">[\s\S]*?<\/g>/)[0]}</svg>
</svg>`);
await sharp(card).png().toFile(resolve(homepage, 'assets/social-card.png'));
console.log('Updated dashboard and homepage assets from the Praxis SVG master.');
