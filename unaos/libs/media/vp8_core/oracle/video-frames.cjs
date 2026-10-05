// SPDX-License-Identifier: LGPL-3.0-or-later
// VP8CORE video oracle: Chromium plays a WebM through <video> at 0.25x (slow enough that headless
// Chromium presents every frame); on every requestVideoFrameCallback the presented frame is wrapped
// as a WebCodecs VideoFrame and copied out with copyTo — the raw planes Chromium's own VP8 decoder
// produced, no canvas and no colour conversion. Writes <outdir>/<basename>.<mediaTime in µs>.i420
// (Y, U, V cropped and packed) and prints one JSON line per file with the frame list.
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//     node video-frames.cjs <outdir> <file>...
// Score with: play-check <file> --i420-oracle <outdir>
const { chromium } = require('playwright');
const http = require('http'), fs = require('fs'), path = require('path');
const [outdir, ...files] = process.argv.slice(2);
fs.mkdirSync(outdir, { recursive: true });
const srv = http.createServer((q, r) => {
  const p = decodeURIComponent(q.url.slice(1));
  const f = files.find((x) => path.basename(x) === p);
  if (!f) { r.writeHead(200, { 'Content-Type': 'text/html' }); return r.end('<!doctype html>'); }
  const buf = fs.readFileSync(f);
  r.writeHead(200, { 'Content-Type': f.endsWith('.webm') ? 'video/webm' : 'video/mp4', 'Content-Length': buf.length });
  r.end(buf);
});
srv.listen(0, async () => {
  const port = srv.address().port;
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required', '--disable-gpu'] });
  const page = await browser.newPage();
  await page.exposeFunction('saveFrame', (name, us, b64) => {
    fs.writeFileSync(path.join(outdir, `${name}.${us}.i420`), Buffer.from(b64, 'base64'));
  });
  await page.goto(`http://127.0.0.1:${port}/blank`);
  for (const f of files) {
    const name = path.basename(f);
    const res = await page.evaluate(async (name) => {
      const v = document.createElement('video');
      v.muted = true; v.src = '/' + name; document.body.appendChild(v);
      const out = { file: name, frames: [] };
      await new Promise((ok, ko) => { v.onloadedmetadata = ok; v.onerror = () => ko(new Error('media error')); setTimeout(() => ko(new Error('timeout')), 10000); });
      out.width = v.videoWidth; out.height = v.videoHeight;
      const pending = [];
      await new Promise((finish) => {
        const cb = (now, md) => {
          try {
            const vf = new VideoFrame(v, { timestamp: Math.round(md.mediaTime * 1e6) });
            const fmt = vf.format, w = vf.visibleRect.width, h = vf.visibleRect.height;
            const buf = new Uint8Array(vf.allocationSize({ rect: vf.visibleRect }));
            pending.push(vf.copyTo(buf, { rect: vf.visibleRect }).then(async (layout) => {
              vf.close();
              if (fmt !== 'I420') { out.frames.push({ t: md.mediaTime, fmt }); return; }
              const cw = (w + 1) >> 1, ch = (h + 1) >> 1;
              const pk = new Uint8Array(w * h + 2 * cw * ch);
              let d = 0;
              for (const [i, pw, ph] of [[0, w, h], [1, cw, ch], [2, cw, ch]])
                for (let y = 0; y < ph; y++) for (let x = 0; x < pw; x++) pk[d++] = buf[layout[i].offset + y * layout[i].stride + x];
              let bin = ''; for (let i = 0; i < pk.length; i += 0x8000) bin += String.fromCharCode.apply(null, pk.subarray(i, i + 0x8000));
              const us = Math.round(md.mediaTime * 1e6);
              await window.saveFrame(name, us, btoa(bin));
              out.frames.push({ t: md.mediaTime, us, fmt, presented: md.presentedFrames });
            }));
          } catch (e) { out.frames.push({ t: md.mediaTime, err: String(e) }); }
          if (!v.ended) v.requestVideoFrameCallback(cb);
        };
        v.requestVideoFrameCallback(cb);
        v.onended = () => setTimeout(finish, 300);
        v.playbackRate = 0.25;
        v.play();
      });
      await Promise.all(pending);
      out.frames.sort((a, b) => a.t - b.t);
      return out;
    }, name);
    console.log(JSON.stringify({ file: res.file, width: res.width, height: res.height, frames: res.frames.length, formats: [...new Set(res.frames.map((x) => x.fmt || x.err))] }));
  }
  await browser.close();
  srv.close();
});
