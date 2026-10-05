// SPDX-License-Identifier: LGPL-3.0-or-later
// ANIMWEBP frame oracle: Chromium's WebCodecs ImageDecoder decodes EVERY frame of an animated WebP /
// APNG / GIF (fully composited by Blink's own decoders), and each VideoFrame is read out with
// VideoFrame.copyTo — the raw straight RGBA Blink produced (premultiplyAlpha 'none',
// colorSpaceConversion 'none'), with NO canvas in between, so translucent pixels do not take a
// premultiplied round trip (chromium-frames.cjs draws through a canvas; this one does not).
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//     node chromium-anim-raw.cjs <outdir> <file>...
// Writes <outdir>/<basename>.fNNN.rgba (raw w*h*4, the shape pixel-check --compare-raw reads) and
// prints "<file> WxH frames=N loop=R durations_ms=[..] formats=[..] [failed_at=K]" — a frame Blink
// fails ends the list (frames 0..K-1 are written), as the browser keeps showing the frames before it.
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
    const type = { png: 'image/png', apng: 'image/apng', gif: 'image/gif', webp: 'image/webp' }[ext];
    const b64 = fs.readFileSync(f).toString('base64');
    const r = await page.evaluate(async ([b64, type]) => {
      try {
        const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
        const dec = new ImageDecoder({ data: bytes, type, colorSpaceConversion: 'none', premultiplyAlpha: 'none' });
        await dec.tracks.ready;
        try { await dec.completed; } catch (e) { /* a frame that fails is reported below */ }
        const t = dec.tracks.selectedTrack;
        const frames = [], durs = [], fmts = [];
        let w = 0, h = 0, failedAt = null;
        for (let i = 0; i < t.frameCount; i++) {
          let image;
          // A frame Blink fails to decode ends the list: the frames before it are what it shows.
          try { ({ image } = await dec.decode({ frameIndex: i, completeFramesOnly: true })); } catch (e) { failedAt = i; break; }
          w = image.displayWidth; h = image.displayHeight;
          const fmt = image.format;
          fmts.push(fmt);
          const out = new Uint8Array(w * h * 4);
          if (['RGBA', 'RGBX', 'BGRA', 'BGRX'].includes(fmt)) {
            const buf = new Uint8Array(image.allocationSize());
            const layout = await image.copyTo(buf);
            const stride = layout[0].stride, off = layout[0].offset;
            const bgr = fmt[0] === 'B', opaque = fmt[3] === 'X';
            for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
              const s = off + y * stride + x * 4, d = (y * w + x) * 4;
              out[d] = buf[s + (bgr ? 2 : 0)]; out[d + 1] = buf[s + 1]; out[d + 2] = buf[s + (bgr ? 0 : 2)];
              out[d + 3] = opaque ? 255 : buf[s + 3];
            }
          } else {
            // A YUV frame (an opaque lossy still): let the browser convert, through a canvas.
            const c = new OffscreenCanvas(w, h);
            const ctx = c.getContext('2d');
            ctx.drawImage(image, 0, 0);
            out.set(ctx.getImageData(0, 0, w, h).data);
          }
          let s = '';
          for (let k = 0; k < out.length; k += 0x8000) s += String.fromCharCode.apply(null, out.subarray(k, k + 0x8000));
          frames.push(btoa(s));
          durs.push(image.duration == null ? null : image.duration / 1000);
          image.close();
        }
        return { w, h, frames, durs, fmts, failedAt, loop: t.repetitionCount };
      } catch (e) { return { err: String(e) }; }
    }, [b64, type]);
    if (r.err) { console.log(`${f} CHROMIUM-REFUSED ${r.err}`); continue; }
    r.frames.forEach((fr, i) => fs.writeFileSync(path.join(outdir, `${path.basename(f)}.f${String(i).padStart(3, '0')}.rgba`), Buffer.from(fr, 'base64')));
    console.log(`${f} ${r.w}x${r.h} frames=${r.frames.length} loop=${r.loop} durations_ms=${JSON.stringify(r.durs)} formats=${JSON.stringify([...new Set(r.fmts)])}${r.failedAt === null ? '' : ' failed_at=' + r.failedAt}`);
  }
  await browser.close();
})();
