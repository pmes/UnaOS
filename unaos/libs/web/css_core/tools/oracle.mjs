// CSSCORE (SR47) M3: the Chromium oracle over EYES's 18 Aether cases.
//
//   node tools/oracle.mjs [<git-ref>]     (from unaos/libs/web/css_core; default ref exec-aether-see)
//
// It also captures the STRESS set into tests/oracle-stress/: CSSCORE's own pages (tests/oracle-pages/*.html,
// written to hit what the EYES corpus does not: layers, nesting, @supports, MQ4 ranges, var() cycles, calc
// trees, font-size keyword rules, colors, shorthands, Selectors 4) and WPT's style-based selector tests
// (is-nested, is-specificity, has-specificity, not-specificity, is-where-pseudo-classes; pinned commit).
//
// For each case (tools/eyes/suites/aether: 16 pages, two of them also at 375x812) Chromium loads the page
// at the case's viewport, runs its scripts, and reports (a) the serialized DOM as a JSON element tree,
// (b) the text of every <style> element in document order, and (c) getComputedStyle() of 20 named
// properties for every element. tests/oracle.rs runs css_core's cascade over (a)+(b) and compares with (c),
// so only the CSS side is judged (the DOM is Chromium's own). Pages are read with `git show`, never merged.
import { createRequire } from 'module';
import { execFileSync } from 'child_process';
import { writeFileSync, readFileSync, mkdtempSync } from 'fs';
import { createHash } from 'crypto';
import { tmpdir } from 'os';
import { join } from 'path';
const require = createRequire(import.meta.url);
const { chromium } = require((process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules') + '/playwright');

export const PROPS = ['display', 'position', 'color', 'background-color', 'font-size', 'font-weight', 'font-style',
  'font-family', 'line-height', 'text-align', 'text-decoration-line', 'white-space', 'visibility', 'list-style-type',
  'border-top-width', 'border-top-style', 'border-left-color', 'padding-top', 'margin-top', 'flex-direction'];

const ref = process.argv[2] || 'exec-aether-see';
const sha = execFileSync('git', ['rev-parse', ref]).toString().trim();
const suite = 'tools/eyes/suites/aether';
const cases = [];
const toml = execFileSync('git', ['show', `${ref}:${suite}/cases/corpus.toml`]).toString();
for (const block of toml.split('[[case]]').slice(1)) {
  const name = block.match(/name\s*=\s*"([^"]+)"/)[1];
  const page = block.match(/page\s*=\s*"([^"]+)"/)[1];
  const w = +(block.match(/width\s*=\s*(\d+)/) || [0, 800])[1];
  const h = +(block.match(/height\s*=\s*(\d+)/) || [0, 600])[1];
  cases.push({ name, page, w, h });
}
const tmp = mkdtempSync(join(tmpdir(), 'csscore-oracle-'));
const browser = await chromium.launch({ args: ['--font-render-hinting=none', '--force-device-scale-factor=1'] });

async function capture(src, fileName, w, h, out, meta) {
  const file = join(tmp, fileName);
  writeFileSync(file, src);
  const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
  await page.goto('file://' + file, { waitUntil: 'load' });
  const got = await page.evaluate((PROPS) => {
    function ser(e) {
      const cs = getComputedStyle(e);
      return { n: e.localName, ns: e.namespaceURI || '',
        a: [...e.attributes].map(a => [a.namespaceURI || '', a.localName, a.value]),
        t: [...e.childNodes].some(n => (n.nodeType === 3 || n.nodeType === 4) && n.data.length > 0),
        s: PROPS.map(p => cs.getPropertyValue(p)),
        c: [...e.children].map(ser) };
    }
    return { styles: [...document.querySelectorAll('style')].map(s => s.textContent), dom: ser(document.documentElement) };
  }, PROPS);
  await page.close();
  const fx = { ...meta, page_sha256: createHash('sha256').update(src).digest('hex'), width: w, height: h,
    chromium: browser.version(), props: PROPS, styles: got.styles, dom: got.dom };
  writeFileSync(out, JSON.stringify(fx));
  console.log(`${meta.case}: ${w}x${h}`);
}

for (const c of cases) {
  const src = execFileSync('git', ['show', `${ref}:${suite}/${c.page}`]);
  await capture(src, c.page.split('/').pop(), c.w, c.h, `tests/oracle/${c.name}.json`,
    { case: c.name, page: `${suite}/${c.page}`, ref, commit: sha });
}

// ---- the stress set
const stress = [['s1-cascade', 800, 600], ['s1-cascade', 375, 812], ['s2-values', 800, 600], ['s3-selectors', 800, 600]];
for (const [n, w, h] of stress) {
  const src = readFileSync(`tests/oracle-pages/${n}.html`);
  const name = w === 800 ? n : `${n}@${w}`;
  await capture(src, `${n}.html`, w, h, `tests/oracle-stress/${name}.json`, { case: name, page: `tests/oracle-pages/${n}.html` });
}
const WPT = 'https://raw.githubusercontent.com/web-platform-tests/wpt/564b9b1eb1387ef42456f1cba77d2d38599fad61/css/selectors/';
for (const f of ['is-nested.html', 'is-specificity.html', 'has-specificity.html', 'not-specificity.html', 'is-where-pseudo-classes.html']) {
  const src = execFileSync('curl', ['-sSfL', '--max-time', '120', WPT + f]);
  await capture(src, f, 800, 600, `tests/oracle-stress/wpt-${f.replace('.html', '')}.json`, { case: `wpt-${f.replace('.html', '')}`, page: WPT + f });
}
await browser.close();
