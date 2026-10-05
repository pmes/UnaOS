// SPDX-License-Identifier: GPL-3.0-or-later
// PLAYBACK (SR26) oracle for play-check's PNG writer: Chromium decodes each PNG in an <img>,
// draws it into a canvas, and prints one JSON line per file with its size and the FNV-1a hash
// of the RGBA — the same hash play-check prints as `frame_fnv` for the pixels it encoded.
//
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node png-oracle.js <file.png>...
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright');
const fs = require('fs');
(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  for (const f of process.argv.slice(2)) {
    const b64 = fs.readFileSync(f).toString('base64');
    const res = await page.evaluate(async (src) => {
      const img = new Image();
      img.src = src;
      await img.decode();
      const c = document.createElement('canvas');
      c.width = img.naturalWidth; c.height = img.naturalHeight;
      const g = c.getContext('2d');
      g.drawImage(img, 0, 0);
      const px = g.getImageData(0, 0, c.width, c.height).data;
      let h = 0x811c9dc5;
      for (let i = 0; i < px.length; i++) { h ^= px[i]; h = Math.imul(h, 0x01000193) >>> 0; }
      return { width: c.width, height: c.height, fnv: h.toString(16) };
    }, 'data:image/png;base64,' + b64);
    console.log(JSON.stringify({ file: f.split('/').pop(), ...res }));
  }
  await browser.close();
})();
