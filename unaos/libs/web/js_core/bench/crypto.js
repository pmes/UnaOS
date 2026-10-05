// Crypto: RSA-style modular exponentiation on arbitrary-precision integers stored as arrays of 26-bit limbs
// (schoolbook multiply + Montgomery-free long division), cross-checked against BigInt.
var BITS = 26, BASE = 1 << BITS, MASK = BASE - 1;
function fromBig(b) { var a = []; while (b > 0n) { a.push(Number(b & BigInt(MASK))); b >>= BigInt(BITS); } return a; }
function toBig(a) { var b = 0n; for (var i = a.length - 1; i >= 0; i--) b = (b << BigInt(BITS)) + BigInt(a[i]); return b; }
function trim(a) { while (a.length && a[a.length - 1] === 0) a.pop(); return a; }
function mul(a, b) {
  var r = new Array(a.length + b.length).fill(0);
  for (var i = 0; i < a.length; i++) {
    var carry = 0, ai = a[i];
    for (var j = 0; j < b.length; j++) {
      var lo = (ai & 0x1fff) * b[j], hi = (ai >>> 13) * b[j];
      var t = r[i + j] + carry + (lo % BASE) + (hi % 8192) * 8192;
      carry = Math.floor(t / BASE) + Math.floor(lo / BASE) + Math.floor(hi / 8192);
      r[i + j] = t % BASE;
    }
    var k = i + b.length;
    while (carry) { var t2 = r[k] + carry; r[k] = t2 % BASE; carry = Math.floor(t2 / BASE); k++; }
  }
  return trim(r);
}
function cmp(a, b) { if (a.length !== b.length) return a.length - b.length; for (var i = a.length - 1; i >= 0; i--) if (a[i] !== b[i]) return a[i] - b[i]; return 0; }
function sub(a, b) { var r = a.slice(), borrow = 0; for (var i = 0; i < r.length; i++) { var t = r[i] - (b[i] || 0) - borrow; borrow = t < 0 ? 1 : 0; r[i] = t < 0 ? t + BASE : t; } return trim(r); }
function shl1(a) { var r = [], c = 0; for (var i = 0; i < a.length; i++) { var t = a[i] * 2 + c; r.push(t & MASK); c = t >> BITS; } if (c) r.push(c); return r; }
function mod(a, m) {
  // binary long division on limbs: r = r*2 + bit
  var r = [];
  for (var i = a.length - 1; i >= 0; i--) for (var bit = BITS - 1; bit >= 0; bit--) {
    r = shl1(r); if ((a[i] >> bit) & 1) { if (r.length === 0) r.push(1); else r[0] |= 1; }
    if (cmp(r, m) >= 0) r = sub(r, m);
  }
  return r;
}
function modpow(b, e, m) { var r = [1]; b = mod(b, m); for (var i = e.length - 1; i >= 0; i--) for (var bit = BITS - 1; bit >= 0; bit--) { r = mod(mul(r, r), m); if ((e[i] >> bit) & 1) r = mod(mul(r, b), m); } return r; }
var n = (2n ** 127n - 1n) * (2n ** 61n - 1n), e = 65537n, ok = 0, last = 0n;
for (var i = 0; i < 6; i++) {
  var msg = 123456789123456789n * BigInt(i + 1) + 987654321n;
  var c = toBig(modpow(fromBig(msg), fromBig(e), fromBig(n)));
  var ref = 1n, bb = msg % n, ee = e; while (ee > 0n) { if (ee & 1n) ref = ref * bb % n; bb = bb * bb % n; ee >>= 1n; }
  if (c === ref) ok++; last = c;
}
print("crypto " + ok + " " + (last % 1000000007n));
