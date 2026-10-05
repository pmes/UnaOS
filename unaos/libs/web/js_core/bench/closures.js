// Closures and functional style: higher-order pipelines, generators, Map/Set, spread and destructuring.
function range(n) { var a = []; for (var i = 0; i < n; i++) a.push(i); return a; }
function compose() { var fs = Array.prototype.slice.call(arguments); return function (x) { return fs.reduceRight(function (acc, f) { return f(acc); }, x); }; }
function memo(f) { var cache = new Map(); return function (n) { if (cache.has(n)) return cache.get(n); var v = f(n); cache.set(n, v); return v; }; }
var fib = memo(function (n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); });
function* primes() { var seen = []; for (var n = 2; ; n++) { if (seen.every(p => n % p !== 0)) { seen.push(n); yield n; } } }
var total = 0;
for (var round = 0; round < 30; round++) {
  var f = compose(x => x * 2, x => x + 1, x => x * x);
  var xs = range(2000).map(f).filter(x => x % 3 === 1);
  var { a, b, ...rest } = { a: xs.length, b: xs[0], c: 1, d: 2 };
  var counts = new Map();
  for (var x of xs) { var k = x % 10; counts.set(k, (counts.get(k) || 0) + 1); }
  var uniq = new Set(xs.map(x => x % 101));
  var ps = []; for (var p of primes()) { if (p > 300) break; ps.push(p); }
  var pairs = Object.entries(Object.fromEntries([...counts].map(([k, v]) => ["k" + k, v])));
  total += a + b + Object.keys(rest).length + uniq.size + ps.length + pairs.length + fib(70) % 1000 + [...xs.slice(0, 5), ...ps.slice(-3)].reduce((s, v) => s + v, 0) % 7;
}
print("closures " + total);
