//! WEBGL1 (SR136) pixel known-answer tests: each draws through the whole pipeline (GLSL compile, link, vertex
//! fetch, clip, rasterize, fragment shading, per-fragment operations) and asserts readPixels values.
//!
//! Run: `cargo test -p gl_core --test pixels -- --nocapture` (the 256×256 test prints `WEBGL1 ops/pixel`).

mod common;
use common::*;
use gl_core::consts::*;
use gl_core::texture::Pixels;

const RED_FS: &str = "precision mediump float;\nvoid main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 1.0); }\n";

/// A flat red triangle over the lower-left half of a 64×64 buffer: inside red, outside the clear colour, and
/// the top-left rule on the shared diagonal of a two-triangle quad (no pixel drawn twice: blending ONE/ONE of
/// 0.5 would show 1.0 on a doubly covered pixel).
#[test]
fn flat_triangle() {
    let mut g = ctx(64, 64);
    let p = program(&mut g, PASS_VS, RED_FS);
    attrib(&mut g, p, "pos", 2, &[-1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
    g.clear_color([0.0, 0.0, 1.0, 1.0]);
    g.clear(COLOR_BUFFER_BIT);
    g.draw_arrays(TRIANGLES, 0, 3);
    assert_eq!(g.get_error(), NO_ERROR);
    assert_eq!(read(&mut g, 5, 5), [255, 0, 0, 255], "inside");
    assert_eq!(read(&mut g, 60, 60), [0, 0, 255, 255], "outside");
    assert_eq!(read(&mut g, 20, 40), [255, 0, 0, 255], "inside near the hypotenuse");
    assert_eq!(read(&mut g, 40, 30), [0, 0, 255, 255], "outside past the hypotenuse");

    // Top-left rule: a quad split on its diagonal, half-intensity additive — every pixel exactly once.
    let mut g = ctx(16, 16);
    let p = program(&mut g, PASS_VS, "precision mediump float;\nvoid main() { gl_FragColor = vec4(0.5, 0.0, 0.0, 1.0); }\n");
    attrib(&mut g, p, "pos", 2, &QUAD);
    g.clear(COLOR_BUFFER_BIT);
    g.enable(BLEND, true);
    g.blend_func_separate(ONE, ONE, ONE, ONE);
    g.draw_arrays(TRIANGLES, 0, 6);
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(read(&mut g, x, y)[0], 128, "pixel ({x},{y}) covered once");
        }
    }
}

/// A gradient triangle: per-vertex red / green / blue, the interpolated colour checked at three pixels
/// against the barycentric weights of their centres (w = 1, so linear = perspective-correct here).
#[test]
fn gradient_triangle() {
    let vs = "attribute vec2 pos; attribute vec3 col; varying vec3 v;\nvoid main() { v = col; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    let fs = "precision mediump float; varying vec3 v;\nvoid main() { gl_FragColor = vec4(v, 1.0); }\n";
    let mut g = ctx(64, 64);
    let p = program(&mut g, vs, fs);
    attrib(&mut g, p, "pos", 2, &[-1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
    attrib(&mut g, p, "col", 3, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    g.draw_arrays(TRIANGLES, 0, 3);
    assert_eq!(g.get_error(), NO_ERROR);
    for (x, y) in [(2, 2), (40, 10), (10, 40)] {
        let (b1, b2) = ((x as f32 + 0.5) / 64.0, (y as f32 + 0.5) / 64.0);
        let b0 = 1.0 - b1 - b2;
        let q = |v: f32| (v * 255.0 + 0.5) as u8;
        let want = [q(b0), q(b1), q(b2), 255];
        let got = read(&mut g, x, y);
        assert!(close(got, want, 1), "({x},{y}): got {got:?} want {want:?}");
    }
}

/// A 2×2 texture (red, green / blue, white) on a quad filling a 4×4 buffer: NEAREST reproduces the quadrants,
/// LINEAR blends 3:1 at pixel (1, 0) — u = 0.75 texels, so 0.75·red + 0.25·green — with CLAMP_TO_EDGE.
#[test]
fn textured_quad_nearest_vs_linear() {
    let vs = "attribute vec2 pos; varying vec2 uv;\nvoid main() { uv = pos * 0.5 + 0.5; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    let fs = "precision mediump float; uniform sampler2D t; varying vec2 uv;\nvoid main() { gl_FragColor = texture2D(t, uv); }\n";
    let texels: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
    for linear in [false, true] {
        let mut g = ctx(4, 4);
        let p = program(&mut g, vs, fs);
        attrib(&mut g, p, "pos", 2, &QUAD);
        let t = g.create_texture();
        g.bind_texture(TEXTURE_2D, t);
        g.tex_image_2d(TEXTURE_2D, 0, RGBA, 2, 2, 0, RGBA, UNSIGNED_BYTE, Pixels::Bytes(&texels));
        let f = if linear { LINEAR } else { NEAREST } as i32;
        g.tex_parameter(TEXTURE_2D, TEXTURE_MIN_FILTER, f);
        g.tex_parameter(TEXTURE_2D, TEXTURE_MAG_FILTER, f);
        g.tex_parameter(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as i32);
        g.tex_parameter(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as i32);
        let loc = g.get_uniform_location(p, "t");
        g.uniform(loc.as_ref(), gl_core::program::UKind::Int, 1, &[0.0], false);
        g.draw_arrays(TRIANGLES, 0, 6);
        assert_eq!(g.get_error(), NO_ERROR);
        if linear {
            assert!(close(read(&mut g, 0, 0), [255, 0, 0, 255], 1), "corner clamps to red");
            assert!(close(read(&mut g, 1, 0), [191, 64, 0, 255], 1), "3:1 red/green, got {:?}", read(&mut g, 1, 0));
            assert!(close(read(&mut g, 3, 3), [255, 255, 255, 255], 1), "white corner");
        } else {
            assert_eq!(read(&mut g, 1, 0), [255, 0, 0, 255]);
            assert_eq!(read(&mut g, 2, 0), [0, 255, 0, 255]);
            assert_eq!(read(&mut g, 1, 2), [0, 0, 255, 255]);
            assert_eq!(read(&mut g, 3, 3), [255, 255, 255, 255]);
        }
    }
}

/// Depth ordering with LESS: a far red quad, then a near green one, then the far red again — green stays.
#[test]
fn depth_ordering() {
    let vs = "attribute vec2 pos; uniform float z;\nvoid main() { gl_Position = vec4(pos, z, 1.0); }\n";
    let fs = "precision mediump float; uniform vec4 c;\nvoid main() { gl_FragColor = c; }\n";
    let mut g = ctx(8, 8);
    let p = program(&mut g, vs, fs);
    attrib(&mut g, p, "pos", 2, &QUAD);
    g.enable(DEPTH_TEST, true);
    g.clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT);
    let (zl, cl) = (g.get_uniform_location(p, "z"), g.get_uniform_location(p, "c"));
    let f = gl_core::program::UKind::Float;
    for (z, c) in [(0.5, [1.0, 0.0, 0.0, 1.0]), (-0.5, [0.0, 1.0, 0.0, 1.0]), (0.5, [1.0, 0.0, 0.0, 1.0])] {
        g.uniform(zl.as_ref(), f, 1, &[z], false);
        g.uniform(cl.as_ref(), f, 4, &c, false);
        g.draw_arrays(TRIANGLES, 0, 6);
    }
    assert_eq!(g.get_error(), NO_ERROR);
    assert_eq!(read(&mut g, 4, 4), [0, 255, 0, 255]);
    // GREATER now lets the far quad through.
    g.depth_func(GREATER);
    g.draw_arrays(TRIANGLES, 0, 6);
    assert_eq!(read(&mut g, 4, 4), [255, 0, 0, 255]);
}

/// SRC_ALPHA / ONE_MINUS_SRC_ALPHA: half-transparent red over opaque blue = (128, 0, 128) with alpha
/// 0.5·0.5 + 1·0.5 = 0.75 → 191.
#[test]
fn alpha_blending() {
    let mut g = ctx(4, 4);
    let p = program(&mut g, PASS_VS, "precision mediump float;\nvoid main() { gl_FragColor = vec4(1.0, 0.0, 0.0, 0.5); }\n");
    attrib(&mut g, p, "pos", 2, &QUAD);
    g.clear_color([0.0, 0.0, 1.0, 1.0]);
    g.clear(COLOR_BUFFER_BIT);
    g.enable(BLEND, true);
    g.blend_func_separate(SRC_ALPHA, ONE_MINUS_SRC_ALPHA, SRC_ALPHA, ONE_MINUS_SRC_ALPHA);
    g.draw_arrays(TRIANGLES, 0, 6);
    assert_eq!(g.get_error(), NO_ERROR);
    assert!(close(read(&mut g, 2, 2), [128, 0, 128, 191], 1), "got {:?}", read(&mut g, 2, 2));
}

/// Every builtin-function category of GLSL ES §8, plus user functions / loops / arrays / structs: pixel x of
/// an 8×1 buffer runs category x and writes green when every value matches, red otherwise.
#[test]
fn builtin_categories() {
    let fs = r#"
precision highp float;
uniform sampler2D tex;
struct S { float a; vec2 b; };
bool near(float a, float b) { return abs(a - b) < 1e-3; }
bool nearv(vec4 a, vec4 b) { return all(lessThan(abs(a - b), vec4(1e-3))); }
float total(float v[4]) { float s = 0.0; for (int i = 0; i < 4; i++) { s += v[i]; } return s; }
void twice(inout float x, out float y) { y = x; x = x * 2.0; }
void main() {
  float x = gl_FragCoord.x;
  bool ok = false;
  if (x < 1.0) {
    ok = near(radians(180.0), 3.1415927) && near(degrees(3.1415927), 180.0) && near(sin(0.5), 0.4794255)
      && near(cos(0.5), 0.8775826) && near(tan(0.5), 0.5463025) && near(asin(0.5), 0.5235988)
      && near(acos(0.5), 1.0471976) && near(atan(1.0), 0.7853982) && near(atan(1.0, -1.0), 2.3561945);
  } else if (x < 2.0) {
    ok = near(pow(2.0, 10.0), 1024.0) && near(exp(1.0), 2.7182818) && near(log(2.7182818), 1.0)
      && near(exp2(3.0), 8.0) && near(log2(8.0), 3.0) && near(sqrt(16.0), 4.0) && near(inversesqrt(4.0), 0.5);
  } else if (x < 3.0) {
    ok = near(abs(-2.5), 2.5) && near(sign(-3.0), -1.0) && near(floor(-1.5), -2.0) && near(ceil(1.2), 2.0)
      && near(fract(1.75), 0.75) && near(mod(7.0, 3.0), 1.0) && near(mod(-1.0, 3.0), 2.0) && near(min(2.0, 3.0), 2.0)
      && near(max(vec2(1.0, 5.0), 2.0).y, 5.0) && near(clamp(5.0, 0.0, 1.0), 1.0) && near(mix(0.0, 10.0, 0.25), 2.5)
      && near(step(0.5, 0.7), 1.0) && near(smoothstep(0.0, 1.0, 0.5), 0.5);
  } else if (x < 4.0) {
    ok = near(length(vec3(3.0, 4.0, 0.0)), 5.0) && near(distance(vec2(1.0, 1.0), vec2(4.0, 5.0)), 5.0)
      && near(dot(vec3(1.0, 2.0, 3.0), vec3(4.0, 5.0, 6.0)), 32.0)
      && nearv(vec4(cross(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0)), 0.0), vec4(0.0, 0.0, 1.0, 0.0))
      && near(normalize(vec2(3.0, 4.0)).x, 0.6)
      && nearv(vec4(faceforward(vec2(1.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 0.0)), 0.0, 0.0), vec4(-1.0, 0.0, 0.0, 0.0))
      && nearv(vec4(reflect(vec2(1.0, -1.0), vec2(0.0, 1.0)), 0.0, 0.0), vec4(1.0, 1.0, 0.0, 0.0))
      && nearv(vec4(refract(vec2(0.0, -1.0), vec2(0.0, 1.0), 1.0), 0.0, 0.0), vec4(0.0, -1.0, 0.0, 0.0));
  } else if (x < 5.0) {
    mat2 a = mat2(1.0, 2.0, 3.0, 4.0);
    mat2 b = mat2(2.0);
    mat2 c = matrixCompMult(a, a);
    vec2 v = a * vec2(1.0, 1.0);
    vec2 w = vec2(1.0, 1.0) * a;
    mat2 p = a * b;
    ok = near(c[1][1], 16.0) && near(v.x, 4.0) && near(v.y, 6.0) && near(w.x, 3.0) && near(w.y, 7.0) && near(p[1][0], 6.0);
  } else if (x < 6.0) {
    bvec3 l = lessThan(vec3(1.0, 2.0, 3.0), vec3(2.0, 2.0, 2.0));
    ok = l.x && !l.y && !l.z && all(lessThanEqual(ivec2(1, 2), ivec2(1, 3))) && any(greaterThan(vec2(0.0, 5.0), vec2(1.0, 4.0)))
      && all(greaterThanEqual(vec2(2.0), vec2(2.0, 1.0))) && all(equal(bvec2(true, false), bvec2(true, false)))
      && any(notEqual(ivec3(1, 2, 3), ivec3(1, 2, 4))) && all(not(bvec2(false, false)));
  } else if (x < 7.0) {
    float arr[4];
    for (int i = 0; i < 4; i++) { arr[i] = float(i) + 1.0; }
    S s = S(2.0, vec2(3.0, 4.0));
    s.b.y += 1.0;
    float a = 5.0;
    float y;
    twice(a, y);
    int k = 7 / 2;
    ok = near(total(arr), 10.0) && near(s.b.y, 5.0) && near(a, 10.0) && near(y, 5.0) && k == 3
      && (s.a > 1.0 ? true : false) && near(arr[k], 4.0);
  } else {
    vec4 want = vec4(51.0, 102.0, 153.0, 204.0) / 255.0;
    ok = nearv(texture2D(tex, vec2(0.5)), want) && nearv(texture2DProj(tex, vec3(1.0, 1.0, 2.0)), want)
      && nearv(texture2D(tex, vec2(0.25), 1.0), want);
  }
  gl_FragColor = ok ? vec4(0.0, 1.0, 0.0, 1.0) : vec4(1.0, 0.0, 0.0, 1.0);
}
"#;
    let mut g = ctx(8, 1);
    let p = program(&mut g, PASS_VS, fs);
    attrib(&mut g, p, "pos", 2, &QUAD);
    let t = g.create_texture();
    g.bind_texture(TEXTURE_2D, t);
    g.tex_image_2d(TEXTURE_2D, 0, RGBA, 1, 1, 0, RGBA, UNSIGNED_BYTE, Pixels::Bytes(&[51, 102, 153, 204]));
    g.tex_parameter(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST as i32);
    g.draw_arrays(TRIANGLES, 0, 6);
    assert_eq!(g.get_error(), NO_ERROR);
    let names = ["angle/trig", "exponential", "common", "geometric", "matrix", "vector relational", "functions/loops/arrays/structs", "texture lookup"];
    for (x, n) in names.iter().enumerate() {
        assert_eq!(read(&mut g, x as i32, 0), [0, 255, 0, 255], "category {n} (pixel {x})");
    }
}

/// OES_standard_derivatives through helper invocations: dFdx(gl_FragCoord.x) = 1, dFdy(gl_FragCoord.y) = 1,
/// and a mipmapped texture sampled with implicit LOD picks the level the screen footprint asks for (a 4×4
/// checker whose 1×1 level is grey, drawn at 1×1 → grey).
#[test]
fn derivatives_and_mip_selection() {
    let mut g = ctx(4, 4);
    g.ext.standard_derivatives = true;
    let fs = "#extension GL_OES_standard_derivatives : enable\nprecision highp float;\nvoid main() { float a = dFdx(gl_FragCoord.x); float b = dFdy(gl_FragCoord.y); gl_FragColor = vec4(a, b, fwidth(gl_FragCoord.x), 1.0); }\n";
    let p = program(&mut g, PASS_VS, fs);
    attrib(&mut g, p, "pos", 2, &QUAD);
    g.draw_arrays(TRIANGLES, 0, 6);
    assert_eq!(g.get_error(), NO_ERROR);
    assert_eq!(read(&mut g, 1, 1), [255, 255, 255, 255]);
    assert!(g.stats.helper_invocations > 0);

    let vs = "attribute vec2 pos; varying vec2 uv;\nvoid main() { uv = pos * 0.5 + 0.5; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    let fs = "precision mediump float; uniform sampler2D t; varying vec2 uv;\nvoid main() { gl_FragColor = texture2D(t, uv); }\n";
    let mut g = ctx(1, 1);
    let p = program(&mut g, vs, fs);
    attrib(&mut g, p, "pos", 2, &QUAD);
    let mut checker = Vec::new();
    for i in 0..16 {
        let v = if (i + i / 4) % 2 == 0 { 255 } else { 0 };
        checker.extend_from_slice(&[v, v, v, 255]);
    }
    let t = g.create_texture();
    g.bind_texture(TEXTURE_2D, t);
    g.tex_image_2d(TEXTURE_2D, 0, RGBA, 4, 4, 0, RGBA, UNSIGNED_BYTE, Pixels::Bytes(&checker));
    g.generate_mipmap(TEXTURE_2D);
    g.tex_parameter(TEXTURE_2D, TEXTURE_MIN_FILTER, NEAREST_MIPMAP_NEAREST as i32);
    g.draw_arrays(TRIANGLES, 0, 6);
    assert_eq!(g.get_error(), NO_ERROR);
    let px = read(&mut g, 0, 0);
    assert!(close(px, [128, 128, 128, 255], 2), "level 2 (1×1) of the checker is grey, got {px:?}");
}

/// A 256×256 triangle with the gradient program, the debug-build cost instrument: prints fragment ops per
/// pixel (the design's "measure and report ops/pixel").
#[test]
fn ops_per_pixel_256() {
    let vs = "attribute vec2 pos; attribute vec3 col; varying vec3 v;\nvoid main() { v = col; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    let fs = "precision mediump float; varying vec3 v;\nvoid main() { gl_FragColor = vec4(v, 1.0); }\n";
    let mut g = ctx(256, 256);
    let p = program(&mut g, vs, fs);
    attrib(&mut g, p, "pos", 2, &[-1.0, -1.0, 1.0, -1.0, -1.0, 1.0]);
    attrib(&mut g, p, "col", 3, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    g.draw_arrays(TRIANGLES, 0, 3);
    assert_eq!(g.get_error(), NO_ERROR);
    let s = g.stats;
    assert!(s.fragments > 32_000 && s.fragments < 33_500, "half of 256² covered, got {}", s.fragments);
    println!("WEBGL1 ops/pixel = {:.2} ({} fragment ops / {} fragments; {} vertex ops / {} vertices)", s.fragment_ops as f64 / s.fragments as f64, s.fragment_ops, s.fragments, s.vertex_ops, s.vertices);
    assert!(s.fragment_ops / s.fragments <= 8);
}

/// Points and lines: a 5-pixel point is a 5×5 square centred on its vertex; a horizontal line covers its
/// columns once, the last pixel excluded.
#[test]
fn points_and_lines() {
    let vs = "attribute vec2 pos;\nvoid main() { gl_PointSize = 5.0; gl_Position = vec4(pos, 0.0, 1.0); }\n";
    let mut g = ctx(16, 16);
    let p = program(&mut g, vs, RED_FS);
    attrib(&mut g, p, "pos", 2, &[0.0625, 0.0625, -0.9375, -0.5625, 0.9375, -0.5625]);
    g.draw_arrays(POINTS, 0, 1);
    // Vertex at window (8.5, 8.5): pixels 6..=10 in x and y.
    assert_eq!(read(&mut g, 6, 6), [255, 0, 0, 255]);
    assert_eq!(read(&mut g, 10, 10), [255, 0, 0, 255]);
    assert_eq!(read(&mut g, 11, 8), [0, 0, 0, 0]);
    g.draw_arrays(LINES, 1, 2);
    // From window (0.5, 3.5) to (15.5, 3.5): columns 0..15 of row 3, column 15 excluded.
    assert_eq!(read(&mut g, 0, 3), [255, 0, 0, 255]);
    assert_eq!(read(&mut g, 14, 3), [255, 0, 0, 255]);
    assert_eq!(read(&mut g, 15, 3), [0, 0, 0, 0]);
    assert_eq!(g.get_error(), NO_ERROR);
}
