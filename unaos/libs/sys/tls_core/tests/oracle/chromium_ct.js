// CTCORE oracle: Chromium's own CT verdict for public sites, and the exact chain it was served.
//   NODE_PATH=$(npm root -g) node chromium_ct.js OUTDIR host...
// For each host: navigate (through the session's HTTPS proxy, if any), read the main document's
// securityDetails (certificateTransparencyCompliance, signedCertificateTimestampList, issuer) over CDP,
// and save the certificate chain Chromium verified (Network.getCertificate) as OUTDIR/<host>.chain (base64 DER
// per line) for tls_core to judge the SAME bytes. Prints one JSON line per host.
const { chromium } = require('playwright');
const fs = require('fs');
(async () => {
  const [out, ...hosts] = process.argv.slice(2);
  const proxy = process.env.HTTPS_PROXY || process.env.https_proxy;
  const browser = await chromium.launch({
    executablePath: process.env.CHROMIUM || undefined,
    proxy: proxy ? { server: proxy } : undefined,
  });
  for (const host of hosts) {
    const ctx = await browser.newContext();
    const page = await ctx.newPage();
    const cdp = await ctx.newCDPSession(page);
    await cdp.send('Network.enable');
    let sec = null;
    cdp.on('Network.responseReceived', (e) => {
      if (e.type === 'Document' && !sec && e.response.securityDetails) sec = e.response.securityDetails;
    });
    const r = { host };
    try {
      await page.goto(`https://${host}/`, { waitUntil: 'domcontentloaded', timeout: 30000 });
      if (sec) {
        r.ct = sec.certificateTransparencyCompliance;
        r.scts = (sec.signedCertificateTimestampList || []).map((s) => `${s.logDescription}|${s.origin}|${s.status}`);
        r.issuer = sec.issuer;
        r.protocol = sec.protocol;
        r.keyExchangeGroup = sec.keyExchangeGroup;
        const c = await cdp.send('Network.getCertificate', { origin: `https://${host}` });
        fs.writeFileSync(`${out}/${host}.chain`, c.tableNames.join('\n') + '\n');
        r.chain = c.tableNames.length;
      } else {
        r.error = 'no securityDetails';
      }
    } catch (e) {
      r.error = String(e.message || e).split('\n')[0];
    }
    console.log(JSON.stringify(r));
    await ctx.close();
  }
  await browser.close();
})();
