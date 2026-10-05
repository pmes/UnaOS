// usage: node vshot.cjs <video.mp4|webm> <outprefix> <fps> <frame>[,<frame>...]
// Chromium plays the file in a <video> at 1:1; for each frame N it seeks to the middle of that
// frame's display interval, waits until requestVideoFrameCallback reports the frame presented
// (its mediaTime is printed for verification) and screenshots exactly the video rectangle.
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');
(async () => {
  const f = path.resolve(process.argv[2]);
  const pre = process.argv[3];
  const fps = parseFloat(process.argv[4]);
  const frames = process.argv[5].split(',').map(Number);
  const html = path.join(path.dirname(path.resolve(pre)), 'oracle_video.html');
  fs.writeFileSync(html, `<html><body style="margin:0;background:#000"><video id=v src="file://${f}" muted preload=auto style="display:block"></video></body></html>`);
  const b = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-gpu', '--allow-file-access-from-files', '--autoplay-policy=no-user-gesture-required'] });
  const p = await b.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 });
  await p.goto('file://' + html);
  await p.waitForFunction(() => document.getElementById('v').readyState >= 2, null, { timeout: 20000 });
  const dims = await p.evaluate(() => [document.getElementById('v').videoWidth, document.getElementById('v').videoHeight]);
  for (const n of frames) {
    const t = (n + 0.5) / fps;
    const mt = await p.evaluate((t) => new Promise((res) => {
      const v = document.getElementById('v');
      v.requestVideoFrameCallback((now, md) => res(md.mediaTime));
      v.currentTime = t;
    }), t);
    // one more frame callback round so the compositor has painted the new frame
    await p.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    const out = `${pre}${String(n).padStart(3, '0')}.png`;
    await p.screenshot({ path: out, clip: { x: 0, y: 0, width: dims[0], height: dims[1] } });
    console.log(JSON.stringify({ frame: n, seek: t, mediaTime: mt, frameFromMediaTime: Math.round(mt * fps), out }));
  }
  await b.close();
})();
