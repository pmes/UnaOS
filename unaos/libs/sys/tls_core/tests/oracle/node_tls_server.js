// CTCORE oracle: a TLS 1.3 server on Node's OpenSSL (3.5.x here — ML-KEM capable; the host's own OpenSSL is 3.0.13).
//   node node_tls_server.js DIR LEAF GROUPS        e.g. GROUPS = X25519MLKEM768 | X25519 | P-256 | X25519MLKEM768:X25519
// Prints the port, then per connection one JSON line {protocol, cipher, bytes} and answers "ok <n>\n" to the
// client's request line. GROUPS restricts the server's key exchange (OpenSSL's group list), so a completed
// handshake with GROUPS=X25519MLKEM768 can only have used the hybrid group.
const tls = require('tls');
const fs = require('fs');
const [dir, leaf, groups] = process.argv.slice(2);
const srv = tls.createServer({
  key: fs.readFileSync(`${dir}/${leaf}.key`),
  cert: fs.readFileSync(`${dir}/${leaf}_chain.pem`),
  ecdhCurve: groups,
  minVersion: 'TLSv1.3',
}, (s) => {
  let n = 0;
  s.on('data', (d) => {
    n += d.length;
    if (d.includes(10)) {
      console.log(JSON.stringify({ protocol: s.getProtocol(), cipher: s.getCipher().name, bytes: n, openssl: process.versions.openssl }));
      s.end(`ok ${n}\n`);
    }
  });
  s.on('error', () => {});
});
srv.on('tlsClientError', (e) => console.log(JSON.stringify({ error: e.message })));
srv.listen(0, '127.0.0.1', () => console.log(srv.address().port));
