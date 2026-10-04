// Reference renderer: Chromium (Playwright, pre-installed) screenshots each job.
// usage: node ref.mjs <jobs.tsv>   (lines: <url>\t<out.png>\t<width>\t<height>)
import { createRequire } from 'module';
import { readFileSync } from 'fs';
const require = createRequire(import.meta.url);
const root = process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules';
const { chromium } = require(root + '/playwright');
const jobs = readFileSync(process.argv[2], 'utf8').trim().split('\n').map(l => l.split('\t'));
const browser = await chromium.launch({
  executablePath: process.env.AETHER_REF_CHROMIUM || undefined,
  // Live URLs go through the session's HTTPS proxy when one is configured.
  proxy: process.env.HTTPS_PROXY ? { server: process.env.HTTPS_PROXY, bypass: 'localhost,127.0.0.1' } : undefined,
  args: ['--font-render-hinting=none', '--disable-lcd-text', '--force-device-scale-factor=1'],
});
let failed = 0;
for (const [url, out, w, h] of jobs) {
  const page = await browser.newPage({ viewport: { width: +w, height: +h }, deviceScaleFactor: 1 });
  try {
    await page.goto(url, { waitUntil: 'load', timeout: 30000 });
    await page.screenshot({ path: out });
  } catch (e) { failed++; console.error(`ref FAIL ${url}: ${e.message.split('\n')[0]}`); }
  await page.close();
}
await browser.close();
process.exit(failed ? 1 : 0);
