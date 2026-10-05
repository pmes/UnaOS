// CSSCORE (SR47) M2: re-express WPT's selector-matching tests over a JSON test DOM.
//
//   node tools/wpt-fixtures.mjs            (from unaos/libs/web/css_core; writes tests/wpt/*.json)
//
// Chromium (Playwright, pre-installed) is used ONLY as an HTML parser: it turns each WPT document into a
// JSON element tree (scripts stripped, so the tree is the markup's). The EXPECTED results are WPT's own,
// read from the test files (selectors.js arrays; the test_selector*(...) calls; the attribute-case arrays)
// — never Chromium's answers. Sources are pinned to one WPT commit; their sha256 go into the fixture.
import { createRequire } from 'module';
import { execFileSync } from 'child_process';
import { writeFileSync, mkdtempSync } from 'fs';
import { createHash } from 'crypto';
import { tmpdir } from 'os';
import { join } from 'path';
import vm from 'vm';
const require = createRequire(import.meta.url);
const { chromium } = require((process.env.NODE_GLOBAL_ROOT || '/opt/node22/lib/node_modules') + '/playwright');

const WPT_COMMIT = '564b9b1eb1387ef42456f1cba77d2d38599fad61';
const RAW = `https://raw.githubusercontent.com/web-platform-tests/wpt/${WPT_COMMIT}/`;
const sources = [];
function get(path) {
  const buf = execFileSync('curl', ['-sSfL', '--max-time', '120', RAW + path]);
  sources.push({ url: RAW + path, sha256: createHash('sha256').update(buf).digest('hex') });
  return buf.toString('utf8');
}
const tmp = mkdtempSync(join(tmpdir(), 'csscore-wpt-'));
const browser = await chromium.launch();

const SER = `(() => {
  function ser(e) {
    return { n: e.localName, ns: e.namespaceURI || '',
      a: [...e.attributes].map(a => [a.namespaceURI || '', a.localName, a.value]),
      t: [...e.childNodes].some(c => (c.nodeType === 3 || c.nodeType === 4) && c.data.length > 0),
      c: [...e.children].map(ser) };
  }
  return ser;
})()`;

async function domOf(html, name, hash = '', setup = null) {
  const file = join(tmp, name);
  writeFileSync(file, html.replace(/<script[\s\S]*?<\/script>/gi, ''));
  const page = await browser.newPage();
  await page.goto('file://' + file + hash);
  const out = await page.evaluate(([SER, setup]) => {
    const ser = eval(SER);
    const res = {};
    if (setup) { eval(setup); }
    res.doc = ser(document.documentElement);
    return res;
  }, [SER, setup]);
  await page.close();
  return out;
}

// ---------------------------------------------------------------- dom/nodes Selectors-API suite
{
  const selJs = get('dom/nodes/selectors.js');
  const content = get('dom/nodes/ParentNode-querySelector-All-content.html');
  const harness = get('dom/nodes/ParentNode-querySelector-All.js');
  const setupSrc = harness.match(/function setupSpecialElements[\s\S]*?\n}\n/)[0];
  const setup = setupSrc + '\nsetupSpecialElements(document, document.getElementById("root"));';
  const ctx = {};
  vm.createContext(ctx);
  vm.runInContext(selJs + '\nthis.invalidSelectors = invalidSelectors; this.validSelectors = validSelectors;', ctx);
  const doms = await domOf(content, 'qsa-content.html', '#target', setup);
  const valid = [];
  for (const s of ctx.validSelectors) {
    if (!(s.testType & 0x01)) continue;               // TEST_QSA only (the HTML document run)
    // The In-document Element context (#root, after a data-clone copy of #root is appended to <body>)
    // and the Detached Element context (a clone of #root) are built from dom_document by the harness.
    const ctxs = ['document', 'element', 'detached'].filter(c => !(s.exclude && (s.exclude.includes(c) || s.exclude.includes('html'))));
    if (ctxs.length) valid.push({ name: s.name, selector: s.selector, expect: s.expect, ctx: ctxs });
  }
  writeFileSync('tests/wpt/selectors-api.json', JSON.stringify({
    note: 'WPT dom/nodes/ParentNode-querySelector-All (HTML): selectors.js expectations over the content document (target fragment #target)',
    wpt_commit: WPT_COMMIT, sources: sources.splice(0), dom_document: doms.doc,
    invalid: ctx.invalidSelectors.map(s => s.selector), valid }));
  console.log(`selectors-api: ${valid.reduce((n, v) => n + v.ctx.length, 0)} valid cases, ${ctx.invalidSelectors.length} invalid`);
}

// ---------------------------------------------------------------- css/selectors self-describing tests
{
  const files = ['has-basic.html', 'has-relative-argument.html', 'is-where-basic.html', 'is-where-not.html', 'not-complex.html'];
  const suites = [];
  for (const f of files) {
    const src = get('css/selectors/' + f);
    const dom = (await domOf(src, f)).doc;
    const cases = [];
    const lst = s => s.split(',').map(x => x.trim()).filter(Boolean);
    for (const m of src.matchAll(/test_selector(_all)?\(\s*'([^']*)'\s*,\s*(\[[^\]]*\]|\w+)\s*\)/g)) {
      const exp = m[3].startsWith('[') ? lst(m[3].slice(1, -1)) : [m[3]];
      cases.push({ kind: m[1] ? 'all' : (m[3].startsWith('[') ? 'all' : 'first'), selector: m[2], expect: exp });
    }
    for (const m of src.matchAll(/test_closest\(\s*(\w+)\s*,\s*'([^']*)'\s*,\s*(\w+)\s*\)/g))
      cases.push({ kind: 'closest', node: m[1], selector: m[2], expect: [m[3]] });
    for (const m of src.matchAll(/test_matches\(\s*(\w+)\s*,\s*'([^']*)'\s*,\s*(true|false)\s*\)/g))
      cases.push({ kind: 'matches', node: m[1], selector: m[2], expect: m[3] === 'true' ? ['true'] : [] });
    suites.push({ file: f, scope: 'main', dom, cases });
    console.log(`${f}: ${cases.length} cases`);
  }
  // attribute-case/semantics.html: the match / matchHTMLOnly / nomatch arrays, standards-mode HTML.
  const sem = get('css/selectors/attribute-selectors/attribute-case/semantics.html');
  const arrays = sem.slice(sem.indexOf('var match = ['), sem.indexOf('var mode ='));
  const ctx = {};
  vm.createContext(ctx);
  vm.runInContext(arrays + '\nthis.m = match; this.h = matchHTMLOnly; this.n = nomatch;', ctx);
  const attrCases = [];
  for (const [list, want] of [[ctx.m, true], [ctx.h, true], [ctx.n, false]])
    for (const arr of list) attrCases.push({ selector: arr[0], attrs: arr.slice(1), expect: want });
  console.log(`attribute-case/semantics: ${attrCases.length} cases`);
  writeFileSync('tests/wpt/css-selectors.json', JSON.stringify({
    note: 'WPT css/selectors matching tests: expectations from the test files; DOM parsed by Chromium with scripts stripped',
    wpt_commit: WPT_COMMIT, sources: sources.splice(0), suites, attribute_case: attrCases }));
}
await browser.close();
