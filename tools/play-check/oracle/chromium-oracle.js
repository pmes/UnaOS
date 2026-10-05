// SPDX-License-Identifier: GPL-3.0-or-later
// PLAYBACK (SR26) oracle: what Chromium reports for a media file.
//
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node chromium-oracle.js <dir> <file>...
//
// For each file prints one JSON line: duration, videoWidth/Height, the requestVideoFrameCallback
// mediaTime sequence and presentedFrames while it plays at 0.25x (slow enough that headless
// Chromium presents every frame), and, per presented frame, an FNV-1a hash of the decoded RGBA
// ( mediaTime, drawImage into a canvas) — so two containers carrying the same
// stream can be shown to decode to the same pixels.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright');
const http = require('http'), fs = require('fs'), path = require('path');
const dir = process.argv[2];
const files = process.argv.slice(3);
const srv = http.createServer((q, r) => {
  const p = path.join(dir, decodeURIComponent(q.url.slice(1)));
  if (!fs.existsSync(p)) { r.writeHead(404); return r.end(); }
  const buf = fs.readFileSync(p);
  const ty = p.endsWith('.webm') ? 'video/webm' : p.endsWith('.mp4') ? 'video/mp4' : 'text/html';
  r.writeHead(200, { 'Content-Type': ty, 'Content-Length': buf.length });
  r.end(buf);
});
srv.listen(0, async () => {
  const port = srv.address().port;
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${port}/blank`).catch(() => {});
  for (const f of files) {
    if (process.env.DUMP_FRAME) await page.evaluate((n) => { window.__dump = n; }, +process.env.DUMP_FRAME);
    const res = await page.evaluate(async (url) => {
      const v = document.createElement('video');
      v.muted = true; v.src = url; document.body.appendChild(v);
      const out = { file: url.split('/').pop(), times: [], hashes: [] };
      try {
        await new Promise((ok, ko) => {
          v.onloadedmetadata = ok;
          v.onerror = () => ko(new Error('media error ' + (v.error && v.error.code)));
          setTimeout(() => ko(new Error('timeout')), 10000);
        });
      } catch (e) { out.error = String(e); v.remove(); return out; }
      out.duration = v.duration; out.width = v.videoWidth; out.height = v.videoHeight;
      await new Promise((finish0) => { let done = finish0;
        let live = true; let c = null, g = null;
        const cb = (now, md) => {
          if (!live) return;
          out.times.push(md.mediaTime); out.presented = md.presentedFrames;
          if (!c) { c = document.createElement('canvas'); c.width = md.width; c.height = md.height; g = c.getContext('2d', { willReadFrequently: true }); }
          g.drawImage(v, 0, 0, c.width, c.height);
          const px = g.getImageData(0, 0, c.width, c.height).data;
          let h = 0x811c9dc5;
          for (let i = 0; i < px.length; i++) { h ^= px[i]; h = Math.imul(h, 0x01000193) >>> 0; }
          out.hashes.push(h.toString(16));
          if (window.__dump === out.hashes.length - 1) out.png = c.toDataURL('image/png');
          v.requestVideoFrameCallback(cb);
        };
        { const f = done; done = () => { live = false; f(); }; }
        v.requestVideoFrameCallback(cb);
        v.onended = () => setTimeout(done, 200);
        v.playbackRate = 0.25;
        v.play().catch(e => { out.error = String(e); done(); });
        setTimeout(done, 120000);
      });
      v.remove();
      return out;
    }, `http://127.0.0.1:${port}/${f}`);
    if (res.png && process.env.DUMP_DIR) {
      fs.writeFileSync(path.join(process.env.DUMP_DIR, res.file + '.frame' + process.env.DUMP_FRAME + '.png'), Buffer.from(res.png.split(',')[1], 'base64'));
    }
    delete res.png;
    console.log(JSON.stringify(res));
  }
  await browser.close();
  srv.close();
});
