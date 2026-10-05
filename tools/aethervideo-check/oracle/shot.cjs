// SPDX-License-Identifier: LGPL-3.0-or-later
// AETHERVIDEO (SR39) oracle: Chromium's screenshot of a page with <video> at frame N.
//
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node shot.cjs <page.html> <N> <fps> <out.png> [w h]
//
// Loads the page from file:// at a w×h viewport, waits for every <video>'s and <audio>'s
// metadata, seeks each video to the MIDDLE of frame N ((N + 0.5) / fps — the same target
// Aether's render seeks to), waits for `seeked` and a presented frame, screenshots the
// viewport, and prints one JSON line per media element in document order: its box
// (getBoundingClientRect), currentSrc, videoWidth/Height (videos), duration, currentTime.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright');
const path = require('path');
const [page_, n, fps, out, w, h] = process.argv.slice(2);
(async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage({ viewport: { width: +(w || 640), height: +(h || 400) }, deviceScaleFactor: 1 });
    await page.goto('file://' + path.resolve(page_));
    const info = await page.evaluate(async ({ n, fps }) => {
      const vids = [...document.querySelectorAll('video')];
      const all = [...document.querySelectorAll('video, audio')];
      await Promise.all(all.map(v => v.readyState >= 1 ? 0 : new Promise((ok, ko) => {
        v.addEventListener('loadedmetadata', ok, { once: true });
        v.addEventListener('error', () => ko(new Error('media error ' + (v.error && v.error.code) + ' ' + v.currentSrc)), { once: true });
        setTimeout(() => ko(new Error('metadata timeout')), 10000);
      })));
      await Promise.all(vids.map(v => new Promise((ok) => {
        // A paused element's seek does not always run requestVideoFrameCallback in headless
        // Chromium; `seeked` plus a bounded wait for the callback is the presented-frame gate.
        v.addEventListener('seeked', () => {
          let done = false; const fin = () => { if (!done) { done = true; ok(); } };
          if (v.requestVideoFrameCallback) v.requestVideoFrameCallback(fin);
          setTimeout(fin, 500);
        }, { once: true });
        v.currentTime = (n + 0.5) / fps;
      })));
      await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
      return all.map(v => { const r = v.getBoundingClientRect(); return { tag: v.tagName.toLowerCase(), src: v.currentSrc.split('/').pop(), box: [r.x, r.y, r.width, r.height], video: [v.videoWidth || 0, v.videoHeight || 0], duration: v.duration, t: v.currentTime }; });
    }, { n: +n, fps: +fps });
    await page.screenshot({ path: out });
    for (const i of info) console.log(JSON.stringify(i));
  } finally { await browser.close(); }
})().catch(e => { console.error(String(e)); process.exit(1); });
