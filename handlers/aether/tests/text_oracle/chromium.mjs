// AETHERFONT text oracle, Chromium side: for each job <page.html>\t<out.png>\t<w>\t<h>, screenshot the page and
// print one JSON line per job with every `.row` div's laid-out text width (Range.getBoundingClientRect) and top.
// Chromium flags as EYES uses them: no hinting, grayscale AA, device scale 1.
import { createRequire } from 'module';
import { readFileSync } from 'fs';
const require = createRequire(import.meta.url);
const root = process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules';
const { chromium } = require(root + '/playwright');
const jobs = readFileSync(process.argv[2], 'utf8').trim().split('\n').map(l => l.split('\t'));
const browser = await chromium.launch({ args: ['--font-render-hinting=none', '--disable-lcd-text', '--force-device-scale-factor=1'] });
for (const [page_path, out, w, h] of jobs) {
  const page = await browser.newPage({ viewport: { width: +w, height: +h }, deviceScaleFactor: 1 });
  await page.goto('file://' + page_path, { waitUntil: 'load' });
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: out });
  const rows = await page.evaluate(() => [...document.querySelectorAll('.row')].map(d => {
    const r = document.createRange(); r.selectNodeContents(d);
    const b = r.getBoundingClientRect();
    return { w: b.width, top: d.getBoundingClientRect().top, h: d.getBoundingClientRect().height };
  }));
  console.log(JSON.stringify({ page: page_path, rows }));
  await page.close();
}
await browser.close();
