// SPDX-License-Identifier: LGPL-3.0-or-later
// AETHERVIDEO (SR39) oracle: Chromium's screenshot of a page with <video> at frame N.
//
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node shot.cjs <page.html> <N> <fps> <out.png> [w h]
//
// Loads the page from file:// at a w×h viewport, waits for every <video>'s metadata, seeks each
// to the MIDDLE of frame N ((N + 0.5) / fps — the same target Aether's render seeks to), waits
// for `seeked` and a presented frame, screenshots the viewport, and prints one JSON line per
// video: its box (getBoundingClientRect), currentSrc, videoWidth/Height, currentTime.
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
      await Promise.all(vids.map(v => v.readyState >= 1 ? 0 : new Promise((ok, ko) => {
        v.addEventListener('loadedmetadata', ok, { once: true });
        v.addEventListener('error', () => ko(new Error('media error ' + (v.error && v.error.code) + ' ' + v.currentSrc)), { once: true });
        setTimeout(() => ko(new Error('metadata timeout')), 10000);
      })));
      await Promise.all(vids.map(v => new Promise((ok) => {
        v.addEventListener('seeked', () => (v.requestVideoFrameCallback ? v.requestVideoFrameCallback(() => ok()) : ok()), { once: true });
        v.currentTime = (n + 0.5) / fps;
      })));
      await new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)));
      return vids.map(v => { const r = v.getBoundingClientRect(); return { src: v.currentSrc.split('/').pop(), box: [r.x, r.y, r.width, r.height], video: [v.videoWidth, v.videoHeight], t: v.currentTime }; });
    }, { n: +n, fps: +fps });
    await page.screenshot({ path: out });
    for (const i of info) console.log(JSON.stringify(i));
  } finally { await browser.close(); }
})().catch(e => { console.error(String(e)); process.exit(1); });
