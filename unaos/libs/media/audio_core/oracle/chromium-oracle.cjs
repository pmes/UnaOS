// AUDIOCODEC (SR30) oracle: Chromium's Web Audio `decodeAudioData` is the reference PCM.
// usage: node chromium-oracle.cjs <jobs.json>   jobs = [{ "in": "<file>", "rate": <Hz>, "out": "<ref.f32>" }, ...]
// Each `out` holds: u32 channels, u32 frames, u32 rate, then planar f32 LE (channel 0 frames, channel 1 ...).
// The OfflineAudioContext runs at the file's own rate so no resampler sits between the codec and the dump.
// Requires: NODE_PATH with playwright, PLAYWRIGHT_BROWSERS_PATH (=/opt/pw-browsers); CHROME overrides the binary.
const { chromium } = require('playwright');
const fs = require('fs');
(async () => {
  const jobs = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
  const browser = await chromium.launch({ executablePath: process.env.CHROME || undefined });
  const page = await browser.newPage();
  await page.route('http://localhost/**', r => r.fulfill({ status: 200, contentType: 'text/html', body: '<html></html>' }));
  await page.goto('http://localhost/oracle.html');
  let fails = 0;
  for (const j of jobs) {
    const b64 = fs.readFileSync(j.in).toString('base64');
    const res = await page.evaluate(async ([b64, rate]) => {
      const bin = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
      const ctx = new OfflineAudioContext(1, 1, rate);
      try {
        const buf = await ctx.decodeAudioData(bin.buffer);
        const parts = [];
        for (let c = 0; c < buf.numberOfChannels; c++) {
          const d = buf.getChannelData(c);
          const u8 = new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
          let s = '';
          for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
          parts.push(btoa(s));
        }
        return { ok: true, ch: buf.numberOfChannels, frames: buf.length, rate: buf.sampleRate, parts };
      } catch (e) { return { ok: false, err: String(e) }; }
    }, [b64, j.rate]);
    if (!res.ok) { console.log(`FAIL ${j.in}: ${res.err}`); fails++; continue; }
    const hdr = Buffer.alloc(12);
    hdr.writeUInt32LE(res.ch, 0); hdr.writeUInt32LE(res.frames, 4); hdr.writeUInt32LE(res.rate, 8);
    fs.writeFileSync(j.out, Buffer.concat([hdr, ...res.parts.map(p => Buffer.from(p, 'base64'))]));
    console.log(`OK ${j.in} ch=${res.ch} frames=${res.frames} rate=${res.rate}`);
  }
  await browser.close();
  process.exit(fails ? 3 : 0);
})();
