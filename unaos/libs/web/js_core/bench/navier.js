// Navier-Stokes style 2D fluid solver kernels (diffuse / project / advect) on Float64Array grids.
var N = 64, size = (N + 2) * (N + 2);
function IX(i, j) { return i + (N + 2) * j; }
function setBnd(b, x) {
  for (var i = 1; i <= N; i++) {
    x[IX(0, i)] = b === 1 ? -x[IX(1, i)] : x[IX(1, i)]; x[IX(N + 1, i)] = b === 1 ? -x[IX(N, i)] : x[IX(N, i)];
    x[IX(i, 0)] = b === 2 ? -x[IX(i, 1)] : x[IX(i, 1)]; x[IX(i, N + 1)] = b === 2 ? -x[IX(i, N)] : x[IX(i, N)];
  }
  x[IX(0, 0)] = 0.5 * (x[IX(1, 0)] + x[IX(0, 1)]); x[IX(0, N + 1)] = 0.5 * (x[IX(1, N + 1)] + x[IX(0, N)]);
  x[IX(N + 1, 0)] = 0.5 * (x[IX(N, 0)] + x[IX(N + 1, 1)]); x[IX(N + 1, N + 1)] = 0.5 * (x[IX(N, N + 1)] + x[IX(N + 1, N)]);
}
function linSolve(b, x, x0, a, c) {
  for (var k = 0; k < 20; k++) {
    for (var j = 1; j <= N; j++) for (var i = 1; i <= N; i++)
      x[IX(i, j)] = (x0[IX(i, j)] + a * (x[IX(i - 1, j)] + x[IX(i + 1, j)] + x[IX(i, j - 1)] + x[IX(i, j + 1)])) / c;
    setBnd(b, x);
  }
}
function diffuse(b, x, x0, diff, dt) { var a = dt * diff * N * N; linSolve(b, x, x0, a, 1 + 4 * a); }
function advect(b, d, d0, u, v, dt) {
  var dt0 = dt * N;
  for (var j = 1; j <= N; j++) for (var i = 1; i <= N; i++) {
    var x = i - dt0 * u[IX(i, j)], y = j - dt0 * v[IX(i, j)];
    if (x < 0.5) x = 0.5; if (x > N + 0.5) x = N + 0.5; var i0 = x | 0, i1 = i0 + 1;
    if (y < 0.5) y = 0.5; if (y > N + 0.5) y = N + 0.5; var j0 = y | 0, j1 = j0 + 1;
    var s1 = x - i0, s0 = 1 - s1, t1 = y - j0, t0 = 1 - t1;
    d[IX(i, j)] = s0 * (t0 * d0[IX(i0, j0)] + t1 * d0[IX(i0, j1)]) + s1 * (t0 * d0[IX(i1, j0)] + t1 * d0[IX(i1, j1)]);
  }
  setBnd(b, d);
}
function project(u, v, p, div) {
  var h = 1 / N;
  for (var j = 1; j <= N; j++) for (var i = 1; i <= N; i++) { div[IX(i, j)] = -0.5 * h * (u[IX(i + 1, j)] - u[IX(i - 1, j)] + v[IX(i, j + 1)] - v[IX(i, j - 1)]); p[IX(i, j)] = 0; }
  setBnd(0, div); setBnd(0, p); linSolve(0, p, div, 1, 4);
  for (var j2 = 1; j2 <= N; j2++) for (var i2 = 1; i2 <= N; i2++) { u[IX(i2, j2)] -= 0.5 * (p[IX(i2 + 1, j2)] - p[IX(i2 - 1, j2)]) / h; v[IX(i2, j2)] -= 0.5 * (p[IX(i2, j2 + 1)] - p[IX(i2, j2 - 1)]) / h; }
  setBnd(1, u); setBnd(2, v);
}
var u = new Float64Array(size), v = new Float64Array(size), u0 = new Float64Array(size), v0 = new Float64Array(size), d = new Float64Array(size), d0 = new Float64Array(size);
for (var step = 0; step < 2; step++) {
  u0[IX(N / 2, N / 2)] = 10; v0[IX(N / 2, N / 2)] = 5; d0[IX(N / 4, N / 4)] = 100;
  diffuse(1, u, u0, 0.0001, 0.1); diffuse(2, v, v0, 0.0001, 0.1); project(u, v, u0, v0);
  advect(1, u0, u, u, v, 0.1); advect(2, v0, v, u, v, 0.1); project(u0, v0, u, v);
  diffuse(0, d, d0, 0.0001, 0.1); advect(0, d0, d, u0, v0, 0.1);
}
var sum = 0; for (var q = 0; q < size; q++) sum += d0[q];
print("navier " + sum.toFixed(6));
