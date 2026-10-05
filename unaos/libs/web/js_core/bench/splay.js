// Splay tree: top-down splaying with inserts, finds and removals of string-payload nodes.
function Node(key, value) { this.key = key; this.value = value; this.left = null; this.right = null; }
function SplayTree() { this.root = null; }
SplayTree.prototype.splay = function (key) {
  if (this.root === null) return;
  var dummy = new Node(null, null), left = dummy, right = dummy, cur = this.root;
  for (;;) {
    if (key < cur.key) {
      if (cur.left === null) break;
      if (key < cur.left.key) { var t = cur.left; cur.left = t.right; t.right = cur; cur = t; if (cur.left === null) break; }
      right.left = cur; right = cur; cur = cur.left;
    } else if (key > cur.key) {
      if (cur.right === null) break;
      if (key > cur.right.key) { var t2 = cur.right; cur.right = t2.left; t2.left = cur; cur = t2; if (cur.right === null) break; }
      left.right = cur; left = cur; cur = cur.right;
    } else break;
  }
  left.right = cur.left; right.left = cur.right; cur.left = dummy.right; cur.right = dummy.left; this.root = cur;
};
SplayTree.prototype.insert = function (key, value) {
  if (this.root === null) { this.root = new Node(key, value); return; }
  this.splay(key);
  if (this.root.key === key) return;
  var n = new Node(key, value);
  if (key > this.root.key) { n.left = this.root; n.right = this.root.right; this.root.right = null; }
  else { n.right = this.root; n.left = this.root.left; this.root.left = null; }
  this.root = n;
};
SplayTree.prototype.remove = function (key) {
  this.splay(key);
  if (this.root === null || this.root.key !== key) return null;
  var removed = this.root;
  if (this.root.left === null) this.root = this.root.right;
  else { var right = this.root.right; this.root = this.root.left; this.splay(key); this.root.right = right; }
  return removed;
};
SplayTree.prototype.find = function (key) { this.splay(key); return this.root !== null && this.root.key === key ? this.root : null; };
var seed = 49734321;
function rnd() { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648; }
var tree = new SplayTree(), keys = [];
for (var i = 0; i < 8000; i++) { var k = rnd(); keys.push(k); tree.insert(k, { id: i, label: "node" + i, payload: [i, i * 2, String(k).slice(0, 8)] }); }
var found = 0;
for (var round = 0; round < 25; round++) {
  for (var j = 0; j < 1000; j++) {
    var idx = Math.floor(rnd() * keys.length);
    if (tree.find(keys[idx]) !== null) found++;
    var r = tree.remove(keys[idx]);
    var nk = rnd(); keys[idx] = nk; tree.insert(nk, { id: j, label: "re" + j, payload: r ? r.value.payload : [] });
  }
}
print("splay " + found);
