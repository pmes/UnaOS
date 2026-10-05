// SPDX-License-Identifier: LGPL-3.0-or-later
// SVGCORE oracle: Chromium's own rendering of each SVG as an <img> (the way Aether shows <img src=x.svg>),
// W x H CSS px at device scale 1 on a white page, screenshotted. Usage:
//   NODE_PATH=/opt/node22/lib/node_modules PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers \
//   FONTCONFIG_FILE=<fonts.conf> node chromium-svg.cjs <outdir> <listfile>
// <listfile>: lines "relpath<TAB>W<TAB>H<TAB>abs-path". Writes <outdir>/<relpath with / -> __>.png.
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

(async () => {
  const [outdir, list] = process.argv.slice(2);
  fs.mkdirSync(outdir, { recursive: true });
  const browser = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-gpu', '--font-render-hinting=none', '--disable-lcd-text'] });
  const page = await browser.newPage({ deviceScaleFactor: 1, viewport: { width: 200, height: 200 } });
  const lines = fs.readFileSync(list, 'utf8').split('\n').filter((l) => l.trim());
  for (const line of lines) {
    const [rel, w, h, abs] = line.split('\t');
    const W = +w, H = +h;
    const src = 'data:image/svg+xml;base64,' + fs.readFileSync(abs).toString('base64');
    await page.setViewportSize({ width: W, height: H });
    await page.setContent(`<!doctype html><html><body style="margin:0;background:#fff"><img id=i style="display:block" width=${W} height=${H} src="${src}"></body></html>`);
    const ok = await page.evaluate(async () => {
      const i = document.getElementById('i');
      try { await i.decode(); return true; } catch (e) { return false; }
    });
    const out = path.join(outdir, rel.replace(/\//g, '__') + '.png');
    await page.screenshot({ path: out, clip: { x: 0, y: 0, width: W, height: H } });
    console.log(`${rel} ${ok ? 'ok' : 'BROKEN'}`);
  }
  await browser.close();
})();
