// usage: node shot.js <file.avif> <out.png>
// Renders the image 1:1 at the top-left of an 800x600 page in Chromium and
// screenshots exactly the image rectangle (sRGB output, no GPU).
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');
(async () => {
  const f = path.resolve(process.argv[2]);
  const html = path.join(path.dirname(path.resolve(process.argv[3])), 'oracle_page.html');
  fs.writeFileSync(html, `<html><body style="margin:0;background:#000"><img id=i src="file://${f}" style="display:block"></body></html>`);
  const b = await chromium.launch({ args: ['--force-color-profile=srgb', '--disable-gpu', '--allow-file-access-from-files'] });
  const p = await b.newPage({ viewport: { width: 800, height: 600 }, deviceScaleFactor: 1 });
  await p.goto('file://' + html);
  await p.waitForFunction(() => { const i = document.getElementById('i'); return i.complete && i.naturalWidth > 0; }, null, { timeout: 20000 });
  const dims = await p.evaluate(() => [document.getElementById('i').naturalWidth, document.getElementById('i').naturalHeight]);
  await p.screenshot({ path: process.argv[3], clip: { x: 0, y: 0, width: Math.min(800, dims[0]), height: Math.min(600, dims[1]) } });
  console.log(dims.join('x'));
  await b.close();
})();
