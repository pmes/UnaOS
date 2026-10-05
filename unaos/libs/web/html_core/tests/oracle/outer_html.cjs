// HTMLCORE (SR46) oracle: Chromium's parse of each page, serialized by Chromium.
// For every <dir>/*.html (not *.chromium.html) writes <dir>/<name>.chromium.html containing
// document.documentElement.outerHTML, with JavaScript disabled (parser scripting flag off).
// Optional second mode: `node outer_html.cjs --dat <in.json> <out.json>` parses an array of HTML strings with
// DOMParser (a scripting-disabled document) and writes their documentElement.outerHTML (null on throw).
const { chromium } = require('playwright'); // resolved via NODE_PATH=$(npm root -g)
const fs = require('node:fs');
const path = require('node:path');

(async () => {
const browser = await chromium.launch();
const ctx = await browser.newContext({ javaScriptEnabled: false });
const page = await ctx.newPage();
if (process.argv[2] === '--dat') {
  const inputs = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
  await page.setContent('<!DOCTYPE html><title>oracle</title>');
  const outs = await page.evaluate((inputs) => inputs.map((s) => {
    try { return new DOMParser().parseFromString(s, 'text/html').documentElement.outerHTML; } catch (e) { return null; }
  }), inputs);
  fs.writeFileSync(process.argv[4], JSON.stringify(outs));
  console.log(`wrote ${outs.length} serializations to ${process.argv[4]}`);
} else {
  const dir = process.argv[2];
  for (const f of fs.readdirSync(dir).sort()) {
    if (!f.endsWith('.html') || f.endsWith('.chromium.html')) continue;
    const url = 'file://' + path.resolve(dir, f);
    await page.goto(url, { waitUntil: 'load' });
    const html = await page.evaluate(() => document.documentElement.outerHTML);
    fs.writeFileSync(path.join(dir, f.replace(/\.html$/, '.chromium.html')), html);
    console.log(`${f}: ${html.length} bytes`);
  }
}
await browser.close();
})();
