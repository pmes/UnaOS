// The Chromium oracle for tests/forms_oracle.rs: serves each fixture at http://oracle.test/<name>.html,
// clicks its #go submit button, and records the submission request Chromium sends (method, URL,
// Content-Type, body) as <name>.expect, the multipart boundary replaced by <B>.
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node oracle.mjs   (run in this directory)
import { createRequire } from 'module';
import { readFileSync, writeFileSync, readdirSync } from 'fs';
const require = createRequire(import.meta.url);
const root = process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules';
const { chromium } = require(root + '/playwright');
const dir = new URL('.', import.meta.url).pathname;
const browser = await chromium.launch();
for (const f of readdirSync(dir).filter(f => f.endsWith('.html')).sort()) {
  const name = f.replace(/\.html$/, '');
  const page = await browser.newPage();
  let got = null;
  await page.route('http://oracle.test/**', async (route) => {
    const req = route.request();
    if (req.url().endsWith('/' + f)) {
      return route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: readFileSync(dir + f) });
    }
    got = { method: req.method(), url: req.url(), ct: req.headers()['content-type'] || '', body: req.postDataBuffer() || Buffer.alloc(0) };
    return route.fulfill({ status: 200, contentType: 'text/html', body: '<p>ok</p>' });
  });
  await page.goto('http://oracle.test/' + f);
  await page.click('#go');
  for (let i = 0; i < 100 && !got; i++) await page.waitForTimeout(20);
  if (!got) throw new Error(name + ': no submission');
  const m = /boundary=(.*)$/.exec(got.ct);
  let body = got.body.toString('latin1');
  let ct = got.ct;
  if (m) { body = body.split(m[1]).join('<B>'); ct = ct.replace(m[1], '<B>'); }
  const out = `METHOD ${got.method}\nURL ${got.url}\nCONTENT-TYPE ${ct}\nBODY-BASE64 ${Buffer.from(body, 'latin1').toString('base64')}\n`;
  writeFileSync(dir + name + '.expect', out);
  console.log(name, got.method, got.url, ct, JSON.stringify(body));
  await page.close();
}
await browser.close();
