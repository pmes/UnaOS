// SPDX-License-Identifier: LGPL-3.0-or-later
// PIXELCORE oracle: Chromium's own decode of a file, as a 1:1 screenshot of an <img> on a known
// background. Usage:
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//     node chromium-oracle.cjs <outdir> <r,g,b> <file>...
// Writes <outdir>/<basename>.shot.png per input. PNG inputs are first copied with their colour-
// management chunks (gAMA, cHRM, sRGB, iCCP) removed, because pixel_core returns stored samples and
// does not colour-manage (documented); everything else is fed as is. Then score with
//   pixel-check --compare <file> <outdir>/<basename>.shot.png <r,g,b> [--orient]
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

function stripPng(buf) {
  if (buf.length < 8 || buf.readUInt32BE(0) !== 0x89504e47) return buf;
  const out = [buf.subarray(0, 8)];
  let p = 8;
  while (p + 12 <= buf.length) {
    const len = buf.readUInt32BE(p);
    const type = buf.subarray(p + 4, p + 8).toString('latin1');
    const end = p + 12 + len;
    if (!['gAMA', 'cHRM', 'sRGB', 'iCCP'].includes(type)) out.push(buf.subarray(p, end));
    p = end;
  }
  return Buffer.concat(out);
}

(async () => {
  const [outdir, bg, ...files] = process.argv.slice(2);
  fs.mkdirSync(outdir, { recursive: true });
  const browser = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-gpu'] });
  const page = await browser.newPage({ deviceScaleFactor: 1, viewport: { width: 64, height: 64 } });
  for (const f of files) {
    let bytes = fs.readFileSync(f);
    if (/\.png$/i.test(f)) bytes = stripPng(bytes);
    const ext = path.extname(f).slice(1).toLowerCase();
    const mime = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', bmp: 'image/bmp', webp: 'image/webp', qoi: 'application/octet-stream' }[ext] || 'application/octet-stream';
    const src = `data:${mime};base64,${bytes.toString('base64')}`;
    await page.setContent(`<!doctype html><html><body style="margin:0;background:rgb(${bg})"><img id=i style="display:block" src="${src}"></body></html>`);
    const dims = await page.evaluate(async () => {
      const i = document.getElementById('i');
      if (!i.complete) await new Promise((r) => { i.onload = r; i.onerror = r; });
      try { await i.decode(); } catch (e) { return null; }
      return [i.naturalWidth, i.naturalHeight];
    });
    if (!dims || !dims[0]) { console.log(`${f} CHROMIUM-REFUSED`); continue; }
    await page.setViewportSize({ width: Math.max(dims[0], 16), height: Math.max(dims[1], 16) });
    const shot = path.join(outdir, path.basename(f) + '.shot.png');
    await page.screenshot({ path: shot, clip: { x: 0, y: 0, width: dims[0], height: dims[1] } });
    console.log(`${f} ${dims[0]}x${dims[1]} -> ${shot}`);
  }
  await browser.close();
})();
