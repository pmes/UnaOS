// SPDX-License-Identifier: LGPL-3.0-or-later
// VP8CORE oracle for lossy WebP: Chromium's own decode of each file through WebCodecs ImageDecoder
// (premultiplyAlpha 'none', colorSpaceConversion 'none'), read out with VideoFrame.copyTo — the raw
// RGBA Blink's WebP decoder produced, with no canvas (so no premultiplied round trip) and no
// screenshot background. An opaque lossy file comes back as I420 — libwebp's decoded planes
// themselves — and is written as <outdir>/<basename>.i420 (Y, U, V cropped and packed); anything with
// alpha comes back as RGBA/BGRA and is written as <outdir>/<basename>.rgba. Prints "<file> WxH format".
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//     node webp-raw.cjs <outdir> <file>...
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

(async () => {
  const [outdir, ...files] = process.argv.slice(2);
  fs.mkdirSync(outdir, { recursive: true });
  const html = path.join(outdir, 'oracle.html');
  fs.writeFileSync(html, '<!doctype html><title>oracle</title>');
  const browser = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-gpu'] });
  const page = await browser.newPage();
  await page.goto('file://' + path.resolve(html));
  for (const f of files) {
    const b64 = fs.readFileSync(f).toString('base64');
    const r = await page.evaluate(async (b64) => {
      try {
        const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
        const dec = new ImageDecoder({ data: bytes, type: 'image/webp', colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        const { image } = await dec.decode({ frameIndex: 0 });
        const w = image.displayWidth, h = image.displayHeight, fmt = image.format;
        const buf = new Uint8Array(image.allocationSize());
        const layout = await image.copyTo(buf);
        image.close();
        if (fmt === 'I420') {
          const cw = (w + 1) >> 1, ch = (h + 1) >> 1;
          const out = new Uint8Array(w * h + 2 * cw * ch);
          let d = 0;
          const planes = [[0, w, h], [1, cw, ch], [2, cw, ch]];
          for (const [i, pw, ph] of planes) for (let y = 0; y < ph; y++) for (let x = 0; x < pw; x++) out[d++] = buf[layout[i].offset + y * layout[i].stride + x];
          let bin = ''; for (let i = 0; i < out.length; i += 0x8000) bin += String.fromCharCode.apply(null, out.subarray(i, i + 0x8000));
          return { w, h, fmt, b64: btoa(bin) };
        }
        // Normalise to packed RGBA rows.
        const out = new Uint8Array(w * h * 4);
        const stride = layout[0].stride, off = layout[0].offset;
        const bgr = fmt === 'BGRA' || fmt === 'BGRX';
        const opaque = fmt === 'RGBX' || fmt === 'BGRX';
        if (!['RGBA', 'RGBX', 'BGRA', 'BGRX'].includes(fmt)) return { err: 'format ' + fmt };
        for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
          const s = off + y * stride + x * 4, d = (y * w + x) * 4;
          out[d] = buf[s + (bgr ? 2 : 0)]; out[d + 1] = buf[s + 1]; out[d + 2] = buf[s + (bgr ? 0 : 2)];
          out[d + 3] = opaque ? 255 : buf[s + 3];
        }
        let bin = ''; for (let i = 0; i < out.length; i += 0x8000) bin += String.fromCharCode.apply(null, out.subarray(i, i + 0x8000));
        return { w, h, fmt, b64: btoa(bin) };
      } catch (e) { return { err: String(e) }; }
    }, b64);
    if (r.err) { console.log(`${f} CHROMIUM-REFUSED ${r.err}`); continue; }
    const ext = r.fmt === 'I420' ? '.i420' : '.rgba';
    fs.writeFileSync(path.join(outdir, path.basename(f) + ext), Buffer.from(r.b64, 'base64'));
    console.log(`${f} ${r.w}x${r.h} ${r.fmt}`);
  }
  await browser.close();
})();
