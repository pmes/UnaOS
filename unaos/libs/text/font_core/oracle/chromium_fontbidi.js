// FONTBIDI (SR56) Chromium oracle. For every job "id<TAB>fonts<TAB>size<TAB>text" (fonts: comma-separated paths, a
// fallback stack) it renders the paragraph in Chromium with local @font-face rules on the SAME files (`dir=auto`,
// so the paragraph direction comes from the first strong character as UAX #9 P2/P3 does), no hinting, grayscale
// AA; screenshots it (8-bit gray PGM), records canvas.measureText for the same font stack, and the visual order of
// grapheme clusters (Intl.Segmenter) sorted by their Range.getClientRects() x — written as UTF-8 byte offsets.
// Usage: NODE_PATH=/opt/node22/lib/node_modules node chromium_fontbidi.js jobs.tsv outdir
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const { chromium } = require('playwright');

function decodePng(buf) {
  let p = 8, w = 0, h = 0, ct = 0, idat = [];
  while (p < buf.length) {
    const len = buf.readUInt32BE(p), type = buf.toString('ascii', p + 4, p + 8);
    const d = buf.subarray(p + 8, p + 8 + len);
    if (type === 'IHDR') { w = d.readUInt32BE(0); h = d.readUInt32BE(4); ct = d[9]; }
    else if (type === 'IDAT') idat.push(d);
    p += 12 + len;
  }
  const bpp = { 0: 1, 2: 3, 4: 2, 6: 4 }[ct];
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * bpp, out = Buffer.alloc(w * h * bpp);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)], src = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= bpp ? out[y * stride + x - bpp] : 0, b = y > 0 ? out[(y - 1) * stride + x] : 0;
      const c = x >= bpp && y > 0 ? out[(y - 1) * stride + x - bpp] : 0;
      let v = src[x];
      if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) { const pp = a + b - c, pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
      out[y * stride + x] = v & 255;
    }
  }
  const g = Buffer.alloc(w * h);
  for (let i = 0; i < w * h; i++) g[i] = out[i * bpp];
  return { w, h, g };
}

(async () => {
  const [jobsFile, outDir] = process.argv.slice(2);
  fs.mkdirSync(outDir, { recursive: true });
  const jobs = fs.readFileSync(jobsFile, 'utf8').split('\n').filter(l => l && !l.startsWith('#')).map(l => l.split('\t'));
  const browser = await chromium.launch({ args: ['--font-render-hinting=none', '--disable-lcd-text', '--force-color-profile=srgb'] });
  const page = await browser.newPage({ viewport: { width: 2400, height: 200 }, deviceScaleFactor: 1 });
  for (const [id, fonts, size, text] of jobs) {
    const fl = fonts.split(',');
    const faces = fl.map((f, i) => `@font-face { font-family: F${i}; src: url("file://${f}"); }`).join('\n');
    const fam = fl.map((_, i) => `F${i}`).join(', ');
    const html = `<!doctype html><meta charset=utf-8><style>${faces}
      html, body { margin: 0; background: #fff; }
      #t { position: absolute; left: 4px; top: 4px; display: inline-block; font-family: ${fam}; font-size: ${size}px;
           line-height: normal; color: #000; white-space: pre; font-kerning: normal; text-rendering: geometricPrecision;
           -webkit-font-smoothing: antialiased; }
      #b { display: inline-block; width: 0; height: 0; vertical-align: baseline; }
    </style><div id=t dir=auto></div>`;
    const file = path.join(outDir, 'page.html');
    fs.writeFileSync(file, html);
    await page.goto('file://' + file);
    const meta = await page.evaluate(async ([text, size, fam, n]) => {
      const t = document.getElementById('t');
      const tn = document.createTextNode(text);
      t.appendChild(tn);
      await Promise.all([...document.fonts].map(f => f.load()));
      await document.fonts.ready;
      let ok = true;
      for (let i = 0; i < n; i++) ok = ok && document.fonts.check(`${size}px F${i}`);
      // Baseline: a zero-size inline-block at the end of the same line (read, then removed again).
      const b = document.createElement('span'); b.id = 'b'; t.appendChild(b);
      const baseline = b.getBoundingClientRect().top;
      b.remove();
      const r = t.getBoundingClientRect();
      const c = document.createElement('canvas').getContext('2d');
      c.font = `${size}px ${fam}`; c.fontKerning = 'normal'; c.textRendering = 'geometricPrecision';
      const enc = new TextEncoder();
      const seg = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
      const items = [];
      for (const s of seg.segment(text)) {
        const range = document.createRange();
        range.setStart(tn, s.index); range.setEnd(tn, s.index + s.segment.length);
        const rs = range.getClientRects();
        if (!rs.length) continue;
        const x = rs[0].left + rs[0].width / 2;
        items.push([x, enc.encode(text.slice(0, s.index)).length]);
      }
      items.sort((a, b) => a[0] - b[0]);
      return { ok, left: r.left, right: r.right, bottom: r.bottom, baseline, measure: c.measureText(text).width,
               order: items.map(i => i[1]).join(','), dir: getComputedStyle(t).direction };
    }, [text, size, fam, fl.length]);
    if (!meta.ok) throw new Error('font did not load: ' + fonts);
    const W = Math.ceil(meta.right) + 8, H = Math.ceil(meta.bottom) + 4;
    const png = await page.screenshot({ clip: { x: 0, y: 0, width: W, height: H } });
    const img = decodePng(png);
    fs.writeFileSync(path.join(outDir, id + '.chrome.pgm'), Buffer.concat([Buffer.from(`P5\n${img.w} ${img.h}\n255\n`), img.g]));
    if (id.endsWith('_24') || id.endsWith('_48')) fs.writeFileSync(path.join(outDir, id + '.png'), png);
    fs.writeFileSync(path.join(outDir, id + '.meta'),
      `left=${meta.left}\nbaseline=${meta.baseline}\nwidth=${img.w}\nheight=${img.h}\nmeasure=${meta.measure}\norder=${meta.order}\ndir=${meta.dir}\nfonts=${fonts}\nsize=${size}\ntext=${text}\n`);
  }
  console.log(`chromium fontbidi oracle: ${jobs.length} jobs`);
  await browser.close();
})();
