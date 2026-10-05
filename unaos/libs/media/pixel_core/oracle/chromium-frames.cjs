// SPDX-License-Identifier: LGPL-3.0-or-later
// PIXELCORE frame oracle: Chromium's WebCodecs ImageDecoder decodes every frame of a file (fully
// composited, as Blink's image decoders produce them); each frame is drawn to a canvas and read back
// with getImageData, giving Chromium's RGBA directly — no screenshot, no background. Exact for opaque
// and binary-alpha content (canvas storage is premultiplied, so partial alpha may round).
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//     node chromium-frames.cjs <outdir> <file>...
// Writes <outdir>/<basename>.fNNN.rgba (raw w*h*4) and prints "<file> WxH frames=N loop=R durations_ms=[..]".
// Score with: pixel-check --compare-raw <file> <outdir>
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
    const ext = path.extname(f).slice(1).toLowerCase();
    const type = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', bmp: 'image/bmp', webp: 'image/webp' }[ext];
    const b64 = fs.readFileSync(f).toString('base64');
    const r = await page.evaluate(async ([b64, type]) => {
      try {
        const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
        const dec = new ImageDecoder({ data: bytes, type, colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        await dec.tracks.ready;
        await dec.completed;
        const t = dec.tracks.selectedTrack;
        const frames = [];
        const durs = [];
        let w = 0, h = 0;
        for (let i = 0; i < t.frameCount; i++) {
          const { image } = await dec.decode({ frameIndex: i, completeFramesOnly: true });
          w = image.displayWidth; h = image.displayHeight;
          const c = new OffscreenCanvas(w, h);
          const ctx = c.getContext('2d');
          ctx.drawImage(image, 0, 0);
          const d = ctx.getImageData(0, 0, w, h).data;
          let s = '';
          for (let k = 0; k < d.length; k += 0x8000) s += String.fromCharCode.apply(null, d.subarray(k, k + 0x8000));
          frames.push(btoa(s));
          durs.push(image.duration == null ? null : image.duration / 1000);
          image.close();
        }
        return { w, h, frames, durs, loop: t.repetitionCount };
      } catch (e) { return { err: String(e) }; }
    }, [b64, type]);
    if (r.err) { console.log(`${f} CHROMIUM-REFUSED ${r.err}`); continue; }
    r.frames.forEach((fr, i) => fs.writeFileSync(path.join(outdir, `${path.basename(f)}.f${String(i).padStart(3, '0')}.rgba`), Buffer.from(fr, 'base64')));
    console.log(`${f} ${r.w}x${r.h} frames=${r.frames.length} loop=${r.loop} durations_ms=${JSON.stringify(r.durs)}`);
  }
  await browser.close();
})();
