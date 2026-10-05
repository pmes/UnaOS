// JSON: build a nested document, stringify / parse round trips with replacer and reviver.
function makeDoc(n) {
  var items = [];
  for (var i = 0; i < n; i++) items.push({ id: i, name: "item-" + i, price: (i * 1.25) % 97, tags: ["t" + (i % 7), "t" + (i % 11)], dims: { w: i % 13, h: i % 17, d: null }, active: i % 3 === 0 });
  return { version: 3, generated: "2024-01-01T00:00:00Z", items: items, meta: { count: n, nested: [[1, [2, [3, [4]]]], { deep: { deeper: { deepest: "x" } } }] } };
}
var doc = makeDoc(2500), sum = 0, len = 0;
for (var r = 0; r < 6; r++) {
  var s = JSON.stringify(doc);
  len += s.length;
  var back = JSON.parse(s, function (k, v) { return k === "price" ? Math.round(v * 100) / 100 : v; });
  for (var i = 0; i < back.items.length; i++) sum += back.items[i].price + back.items[i].tags.length;
  var pretty = JSON.stringify(back.meta, null, 2);
  len += pretty.length + JSON.stringify(back.items.slice(0, 50), ["id", "name"]).length;
}
print("json " + len + " " + sum.toFixed(2));
