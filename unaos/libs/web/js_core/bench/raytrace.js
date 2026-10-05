// Ray tracer: classes for vectors, spheres, planes and lights; diffuse + specular shading with shadows.
class V { constructor(x, y, z) { this.x = x; this.y = y; this.z = z; }
  add(o) { return new V(this.x + o.x, this.y + o.y, this.z + o.z); } sub(o) { return new V(this.x - o.x, this.y - o.y, this.z - o.z); }
  mul(k) { return new V(this.x * k, this.y * k, this.z * k); } dot(o) { return this.x * o.x + this.y * o.y + this.z * o.z; }
  norm() { var l = Math.sqrt(this.dot(this)); return new V(this.x / l, this.y / l, this.z / l); } }
class Sphere { constructor(c, r, col) { this.c = c; this.r2 = r * r; this.col = col; }
  hit(o, d) { var oc = o.sub(this.c), b = oc.dot(d), c = oc.dot(oc) - this.r2, disc = b * b - c; if (disc < 0) return -1; var t = -b - Math.sqrt(disc); return t > 1e-4 ? t : -1; }
  normal(p) { return p.sub(this.c).norm(); } }
class Plane { constructor(n, off, col) { this.n = n; this.off = off; this.col = col; }
  hit(o, d) { var den = this.n.dot(d); if (Math.abs(den) < 1e-9) return -1; var t = -(this.n.dot(o) + this.off) / den; return t > 1e-4 ? t : -1; }
  normal() { return this.n; } }
var scene = [new Sphere(new V(0, 1, 5), 1, [1, 0.2, 0.2]), new Sphere(new V(-2, 0.6, 6), 0.6, [0.2, 1, 0.2]), new Sphere(new V(2, 0.8, 4.5), 0.8, [0.3, 0.3, 1]), new Plane(new V(0, 1, 0), 0, [0.8, 0.8, 0.8])];
var lights = [new V(-5, 8, -3), new V(6, 5, 0)];
function trace(o, d, depth) {
  var best = Infinity, obj = null;
  for (var i = 0; i < scene.length; i++) { var t = scene[i].hit(o, d); if (t > 0 && t < best) { best = t; obj = scene[i]; } }
  if (obj === null) return [0.1, 0.1, 0.15];
  var p = o.add(d.mul(best)), n = obj.normal(p), col = [0.05, 0.05, 0.05];
  for (var l = 0; l < lights.length; l++) {
    var ld = lights[l].sub(p).norm(), shadow = false;
    for (var s = 0; s < scene.length && !shadow; s++) if (scene[s].hit(p, ld) > 0) shadow = true;
    if (shadow) continue;
    var diff = Math.max(0, n.dot(ld)), refl = ld.sub(n.mul(2 * n.dot(ld))), spec = Math.pow(Math.max(0, refl.dot(d)), 20);
    for (var c = 0; c < 3; c++) col[c] += obj.col[c] * diff * 0.7 + spec * 0.3;
  }
  if (depth < 2) { var r = d.sub(n.mul(2 * n.dot(d))).norm(), rc = trace(p, r, depth + 1); for (var k = 0; k < 3; k++) col[k] += rc[k] * 0.2; }
  return col;
}
var W = 96, H = 72, acc = 0, eye = new V(0, 1, -2);
for (var y = 0; y < H; y++) for (var x = 0; x < W; x++) {
  var dir = new V((x - W / 2) / H, -(y - H / 2) / H + 0.2, 1).norm(), c = trace(eye, dir, 0);
  acc += Math.min(255, (c[0] * 255) | 0) + Math.min(255, (c[1] * 255) | 0) * 3 + Math.min(255, (c[2] * 255) | 0) * 7;
}
print("raytrace " + acc);
