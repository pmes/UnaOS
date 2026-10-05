// SPDX-License-Identifier: LGPL-3.0-or-later
// AETHERVIDEO (SR39) fixture: Chromium's WebCodecs VP9 encoder turns the test-pattern frames
// (`aethervideo-check make` → frames.rgba + meta.json) into chunks.bin, every frame a keyframe at
// quantizer 0 (VP9 lossless in YUV 4:2:0). Chromium is the oracle's toolchain, not linked.
//
//   PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers node encode.cjs <dir>
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || '/opt/node22/lib/node_modules/playwright');
const http = require('http'), fs = require('fs'), path = require('path');
const dir = process.argv[2];
const meta = JSON.parse(fs.readFileSync(path.join(dir, 'meta.json'), 'utf8'));
const raw = fs.readFileSync(path.join(dir, 'frames.rgba'));
// WebCodecs needs a secure context: serve a blank page from 127.0.0.1.
const srv = http.createServer((q, r) => { r.writeHead(200, { 'Content-Type': 'text/html' }); r.end('<!doctype html><title>enc</title>'); });
srv.listen(0, async () => {
  const browser = await chromium.launch();
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${srv.address().port}/`);
    const chunks = await page.evaluate(async ({ b64, meta }) => {
      const bytes = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
      const { width, height, frames, fps } = meta;
      const out = [];
      let err = null;
      const enc = new VideoEncoder({
        output: (chunk) => { const d = new Uint8Array(chunk.byteLength); chunk.copyTo(d); out.push({ key: chunk.type === 'key', ts: chunk.timestamp, data: btoa(String.fromCharCode(...d)) }); },
        error: (e) => { err = String(e); },
      });
      const cfg = { codec: 'vp09.00.10.08', width, height, bitrateMode: 'quantizer', framerate: fps, latencyMode: 'quality' };
      const sup = await VideoEncoder.isConfigSupported(cfg);
      if (!sup.supported) return { error: 'vp9 quantizer mode unsupported' };
      enc.configure(cfg);
      const size = width * height * 4;
      for (let i = 0; i < frames; i++) {
        const f = new VideoFrame(bytes.subarray(i * size, (i + 1) * size), { format: 'RGBA', codedWidth: width, codedHeight: height, timestamp: Math.round(i * 1e6 / fps), duration: Math.round(1e6 / fps) });
        enc.encode(f, { keyFrame: true, vp9: { quantizer: 0 } });
        f.close();
      }
      await enc.flush();
      enc.close();
      return err ? { error: err } : { chunks: out };
    }, { b64: raw.toString('base64'), meta });
    if (chunks.error) { console.error(chunks.error); process.exitCode = 1; return; }
    const parts = [];
    for (const c of chunks.chunks) {
      const d = Buffer.from(c.data, 'base64');
      const h = Buffer.alloc(5); h.writeUInt32LE(d.length, 0); h[4] = c.key ? 1 : 0;
      parts.push(h, d);
    }
    fs.writeFileSync(path.join(dir, 'chunks.bin'), Buffer.concat(parts));
    console.log(JSON.stringify({ chunks: chunks.chunks.length, bytes: parts.reduce((n, p) => n + p.length, 0), keyframes: chunks.chunks.filter(c => c.key).length }));
  } finally { await browser.close(); srv.close(); }
});
