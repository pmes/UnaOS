// EYES / facet — the Chromium oracle for the golden PNGs: screenshot every pages/<case>.html at
// its declared viewport (Playwright, the pre-installed Chromium; never `playwright install`).
// usage: node oracle.mjs <outdir>        (writes <outdir>/<case>.chromium.png)
import { createRequire } from 'module';
import { readdirSync, readFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
const require = createRequire(import.meta.url);
const { chromium } = require((process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules') + '/playwright');
const here = dirname(fileURLToPath(import.meta.url));
const out = process.argv[2] || join(here, 'out');
mkdirSync(out, { recursive: true });
const browser = await chromium.launch({ args: ['--force-device-scale-factor=1', '--force-color-profile=srgb'] });
for (const f of readdirSync(join(here, 'pages')).filter(f => f.endsWith('.html')).sort()) {
  const html = readFileSync(join(here, 'pages', f), 'utf8');
  const [, w, h] = html.match(/width:(\d+)px;height:(\d+)px/);
  const page = await browser.newPage({ viewport: { width: +w, height: +h }, deviceScaleFactor: 1 });
  await page.goto('file://' + join(here, 'pages', f), { waitUntil: 'load' });
  await page.screenshot({ path: join(out, f.replace('.html', '.chromium.png')) });
  await page.close();
}
await browser.close();
