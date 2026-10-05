// FONTCORE Chromium oracle (SR48 M2/M3). For every job line "id<TAB>font<TAB>size<TAB>text" it renders the
// text in Chromium with a local @font-face pointing at the SAME font file, `font-kerning: normal`,
// `text-rendering: geometricPrecision`, `-webkit-font-smoothing: antialiased`, launched with
// `--font-render-hinting=none` (no hinting, like FONTCORE), screenshots it, decodes the PNG (node zlib) and
// writes <out>/<id>.chrome.pgm (8-bit gray) and <out>/<id>.meta (key=value: left, baseline, width, height,
// measure = canvas.measureText width with fontKerning normal).
// Usage: NODE_PATH=/opt/node22/lib/node_modules node chromium_oracle.js jobs.tsv outdir
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');
const { chromium } = require('playwright');

function decodePng(buf) {
  let p = 8, w = 0, h = 0, ct = 0, idat = [];
  while (p < buf.length) {
    const len = buf.readUInt32BE(p), type = buf.toString('ascii', p + 4, p + 8);
    const d = buf.subarray(p + 8, p + 8 + len);
    if (type === 'IHDR') { w = d.readUInt32BE(0); h = d.readUInt32BE(4); ct = d[9]; if (d[8] !== 8) throw 'bit depth'; }
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
  for (let i = 0; i < w * h; i++) g[i] = out[i * bpp]; // red channel (text is black on white, gray AA)
  return { w, h, g, rgb: out, bpp };
}

(async () => {
  const [jobsFile, outDir] = process.argv.slice(2);
  fs.mkdirSync(outDir, { recursive: true });
  const jobs = fs.readFileSync(jobsFile, 'utf8').split('\n').filter(l => l && !l.startsWith('#')).map(l => l.split('\t'));
  const browser = await chromium.launch({ args: ['--font-render-hinting=none', '--disable-lcd-text', '--force-color-profile=srgb'] });
  const page = await browser.newPage({ viewport: { width: 2400, height: 200 }, deviceScaleFactor: 1 });
  let colorFringe = 0;
  for (const [id, font, size, text] of jobs) {
    const html = `<!doctype html><meta charset=utf-8><style>
      @font-face { font-family: T; src: url("file://${font}"); }
      html, body { margin: 0; background: #fff; }
      #t { position: absolute; left: 4px; top: 4px; font-family: T; font-size: ${size}px; line-height: normal;
           color: #000; white-space: pre; font-kerning: normal; text-rendering: geometricPrecision;
           -webkit-font-smoothing: antialiased; font-variant-ligatures: common-ligatures; }
      #b { display: inline-block; width: 0; height: 0; vertical-align: baseline; }
    </style><div id=t></div>`;
    const file = path.join(outDir, 'page.html');
    fs.writeFileSync(file, html);
    await page.goto('file://' + file);
    const meta = await page.evaluate(async ([text, size]) => {
      const t = document.getElementById('t');
      t.textContent = text;
      const b = document.createElement('span'); b.id = 'b'; t.appendChild(b);
      await document.fonts.ready;
      const ok = document.fonts.check(`${size}px T`);
      const r = t.getBoundingClientRect(), br = b.getBoundingClientRect();
      const c = document.createElement('canvas').getContext('2d');
      c.font = `${size}px T`; c.fontKerning = 'normal'; c.textRendering = 'geometricPrecision';
      return { ok, left: r.left, baseline: br.top, right: r.right, bottom: r.bottom, measure: c.measureText(text).width };
    }, [text, size]);
    if (!meta.ok) throw new Error('font did not load: ' + font);
    const W = Math.ceil(meta.right) + 8, H = Math.ceil(meta.bottom) + 4;
    const png = await page.screenshot({ clip: { x: 0, y: 0, width: W, height: H } });
    const img = decodePng(png);
    for (let i = 0; i < img.w * img.h; i++) {
      const r = img.rgb[i * img.bpp], g = img.rgb[i * img.bpp + 1], bl = img.rgb[i * img.bpp + 2];
      if (Math.abs(r - g) > 2 || Math.abs(g - bl) > 2) colorFringe++;
    }
    fs.writeFileSync(path.join(outDir, id + '.chrome.pgm'), Buffer.concat([Buffer.from(`P5\n${img.w} ${img.h}\n255\n`), img.g]));
    fs.writeFileSync(path.join(outDir, id + '.meta'),
      `left=${meta.left}\nbaseline=${meta.baseline}\nwidth=${img.w}\nheight=${img.h}\nmeasure=${meta.measure}\nfont=${font}\nsize=${size}\ntext=${text}\n`);
    if (id.endsWith('_eyes')) fs.writeFileSync(path.join(outDir, id + '.png'), png);
  }
  console.log(`chromium oracle: ${jobs.length} jobs, ${colorFringe} colour-fringed pixels (0 = grayscale AA)`);
  await browser.close();
})();
