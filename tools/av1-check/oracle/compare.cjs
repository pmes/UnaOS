// usage: node compare.cjs <a.png> <b.png>   -> PSNR (RGB, per channel) and max abs diff
// PNG decode with node's built-in zlib (no packages): 8-bit RGB/RGBA, non-interlaced, all 5 filters.
const fs = require('fs');
const zlib = require('zlib');
function readPng(path) {
  const d = fs.readFileSync(path);
  let o = 8, w, h, ct, idat = [];
  while (o < d.length) {
    const len = d.readUInt32BE(o), typ = d.toString('latin1', o + 4, o + 8);
    const body = d.subarray(o + 8, o + 8 + len);
    if (typ === 'IHDR') { w = body.readUInt32BE(0); h = body.readUInt32BE(4); ct = body[9]; if (body[8] !== 8 || body[12] !== 0) throw 'unsupported png'; }
    if (typ === 'IDAT') idat.push(body);
    o += 12 + len;
  }
  const bpp = ct === 6 ? 4 : ct === 2 ? 3 : (() => { throw 'colour type ' + ct; })();
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = w * bpp, out = Buffer.alloc(w * h * 4);
  let prev = Buffer.alloc(stride), p = 0;
  for (let y = 0; y < h; y++) {
    const f = raw[p++], line = Buffer.from(raw.subarray(p, p + stride)); p += stride;
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? line[i - bpp] : 0, b = prev[i], c = i >= bpp ? prev[i - bpp] : 0;
      let v = line[i];
      if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) { const pp = a + b - c, pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c); v += (pa <= pb && pa <= pc) ? a : (pb <= pc ? b : c); }
      line[i] = v & 255;
    }
    for (let x = 0; x < w; x++) { for (let k = 0; k < 3; k++) out[(y * w + x) * 4 + k] = line[x * bpp + k]; out[(y * w + x) * 4 + 3] = 255; }
    prev = line;
  }
  return { w, h, px: out };
}
const A = readPng(process.argv[2]), B = readPng(process.argv[3]);
// The screenshot is the top-left min(800,w) x min(600,h) of the page: compare that window.
const W = Math.min(A.w, B.w), H = Math.min(A.h, B.h);
let se = [0, 0, 0], maxd = 0, hist = new Array(256).fill(0);
for (let y = 0; y < H; y++) for (let x = 0; x < W; x++) for (let k = 0; k < 3; k++) {
  const d = A.px[(y * A.w + x) * 4 + k] - B.px[(y * B.w + x) * 4 + k]; se[k] += d * d; const ad = Math.abs(d); hist[ad]++; if (ad > maxd) maxd = ad;
}
const n = W * H, psnr = (s) => s === 0 ? Infinity : 10 * Math.log10(255 * 255 / (s / n));
const all = 10 * Math.log10(255 * 255 / ((se[0] + se[1] + se[2]) / (3 * n)));
const within1 = (hist[0] + hist[1]) / (3 * n) * 100;
console.log(JSON.stringify({ size: `${A.w}x${A.h}`, window: `${W}x${H}`, psnr_rgb: +all.toFixed(2), psnr_r: +psnr(se[0]).toFixed(2), psnr_g: +psnr(se[1]).toFixed(2), psnr_b: +psnr(se[2]).toFixed(2), max_abs_diff: maxd, pct_within_1: +within1.toFixed(3), exact_pct: +(hist[0] / (3 * n) * 100).toFixed(3) }));
