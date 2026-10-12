// AETHERSELECT (SR69) innerText oracle, Chromium side: for each page in tests/data/innertext/*.html, print
// Chromium's `document.body.innerText` (and, for comparison, `getSelection().toString()` after
// `selectAllChildren(document.body)`). Output: tests/data/innertext/chromium.json, which
// tests/innertext.rs reads (its sha-256 is pinned there).
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node handlers/aether/tests/innertext/chromium.mjs > handlers/aether/tests/data/innertext/chromium.json
import { createRequire } from 'module';
import { readdirSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
const require = createRequire(import.meta.url);
const root = process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules';
const { chromium } = require(root + '/playwright');
const dir = join(dirname(fileURLToPath(import.meta.url)), '..', 'data', 'innertext');
const pages = readdirSync(dir).filter(f => f.endsWith('.html')).sort();
const browser = await chromium.launch({ args: ['--font-render-hinting=none', '--disable-lcd-text', '--force-device-scale-factor=1'] });
const out = {};
for (const f of pages) {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 }, deviceScaleFactor: 1 });
  await page.goto('file://' + join(dir, f), { waitUntil: 'load' });
  out[f] = await page.evaluate(() => {
    const innerText = document.body.innerText;
    getSelection().selectAllChildren(document.body);
    return { innerText, selection: getSelection().toString() };
  });
  await page.close();
}
await browser.close();
process.stdout.write(JSON.stringify(out, null, 1) + '\n');
