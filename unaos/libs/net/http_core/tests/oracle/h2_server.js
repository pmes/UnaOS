// HTTPCORE h2 oracle: Node's built-in http2 (nghttp2 underneath) over TLS (Node's OpenSSL) with ALPN h2 only,
// on 127.0.0.1. NOT ours. usage: node h2_server.js <certdir> — prints the port, serves until killed.
// Routes: /hello, /big (1 MiB, exercises our WINDOW_UPDATEs), /gzip (content-encoding gzip), /echo (any method:
// JSON with method, path, body length, sha256, and the request headers), /redirect (302 -> /hello),
// /trailers (a response with trailers), /stats (sessions and streams seen).
const http2 = require('http2'), fs = require('fs'), zlib = require('zlib'), crypto = require('crypto');
const d = process.argv[2];
let sessions = 0, streams = 0;
const big = Buffer.alloc(1 << 20);
for (let i = 0; i < big.length; i++) big[i] = (i * 31 + (i >> 8)) & 0xff;
const server = http2.createSecureServer({
  key: fs.readFileSync(`${d}/p256.key`), cert: fs.readFileSync(`${d}/p256_chain.pem`),
  ALPNProtocols: ['h2'], minVersion: 'TLSv1.3', allowHTTP1: false,
  settings: { initialWindowSize: 65535, maxFrameSize: 16384 },
});
server.on('session', () => sessions++);
server.on('stream', (stream, headers) => {
  streams++;
  const path = headers[':path'];
  const chunks = [];
  stream.on('data', c => chunks.push(c));
  stream.on('end', () => {
    const body = Buffer.concat(chunks);
    if (path === '/hello') { stream.respond({ ':status': 200, 'content-type': 'text/plain' }); stream.end('hello over h2\n'); }
    else if (path === '/big') { stream.respond({ ':status': 200, 'content-length': String(big.length) }); stream.end(big); }
    else if (path === '/gzip') { stream.respond({ ':status': 200, 'content-encoding': 'gzip' }); stream.end(zlib.gzipSync(big.subarray(0, 100000))); }
    else if (path === '/redirect') { stream.respond({ ':status': 302, 'location': '/hello' }); stream.end(); }
    else if (path === '/trailers') { stream.respond({ ':status': 200 }, { waitForTrailers: true }); stream.on('wantTrailers', () => stream.sendTrailers({ 'x-done': 'yes' })); stream.end('body'); }
    else if (path === '/stats') { stream.respond({ ':status': 200, 'content-type': 'application/json' }); stream.end(JSON.stringify({ sessions, streams })); }
    else if (path.startsWith('/echo')) {
      const h = {}; for (const k of Object.keys(headers)) h[k] = String(headers[k]);
      stream.respond({ ':status': 200, 'content-type': 'application/json' });
      stream.end(JSON.stringify({ method: headers[':method'], path, len: body.length, sha256: crypto.createHash('sha256').update(body).digest('hex'), headers: h }));
    } else { stream.respond({ ':status': 404 }); stream.end(); }
  });
});
server.listen(0, '127.0.0.1', () => console.log(server.address().port));
