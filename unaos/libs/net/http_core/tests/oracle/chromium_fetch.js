// HTTPCORE Chromium oracle: Chromium (Playwright, the pre-installed browser) fetches each URL as a page
// navigation and writes the body Chromium decoded (gzip/deflate, chunked) to <outdir>/<n>.bin, plus the
// Accept-Encoding it sent. usage: node chromium_fetch.js <outdir> <url>...
const { chromium } = require('playwright');
const fs = require('fs');
(async () => {
  const [outdir, ...urls] = process.argv.slice(2);
  const browser = await chromium.launch({ args: ['--no-proxy-server'] });
  const page = await browser.newPage();
  for (let i = 0; i < urls.length; i++) {
    const resp = await page.goto(urls[i]);
    const body = await resp.body();
    fs.writeFileSync(`${outdir}/${i}.bin`, body);
    const req = resp.request();
    const ae = (await req.allHeaders())['accept-encoding'] || '';
    console.log(`${i} status=${resp.status()} bytes=${body.length} accept-encoding=${ae}`);
  }
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
