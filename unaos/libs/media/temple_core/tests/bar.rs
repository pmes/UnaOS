// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TEMPLE (SR147): the host tests and THE BAR's measurement (R106's front gate: measured before the vessel).
//
//   refraction_angle     — Snell on one ray, per wavelength, both scalars; the dispersion's order; TIR past critical.
//   lens_one_star        — the deflection of one primary ray through the flow against an independent reference
//                          (the mass's finite differences summed over the same shell crossings), and the star
//                          displacement in pixels.
//   reference_frame_64   — a 64x64 Q16.16 frame's FNV-1a checksum (integer arithmetic: the ring-3 program's own).
//   fx_tracks_f32        — the two scalars render the same picture (mean display difference, 64x64).
//   bar                  — the measurement: per scalar, the 960x600 still converging pass by pass, and the 480x300
//                          orbit (accumulator reset every frame, 2x upscale to 960x600 inside the timing). One core
//                          (one thread). `TEMPLE_EVIDENCE=<dir>` writes the PNGs and the numbers there. The still's
//                          bar is per scalar: fx16 <= 3000 ms (R128), f32 <= 2000 ms (B876). Each scalar also prints
//                          one `TEMPLE phases` line: a 960x600 pass split by the core's public per-phase fns.
//
// Run: `cargo test -- --nocapture --test-threads=1` (the profile in Cargo.toml optimises the test build).

use std::time::Instant;
use temple_core::{checksum, refract_in, refract_out, upscale2, volume, Frame, Fx, Real, Temple, V3, KAPPA, SHELL_R, SHELL_W, STILL_ORBIT_MRAD};

// ---- refraction ------------------------------------------------------------------------------------------------

fn snell<R: Real>(tol: f32) {
    let n = V3::<R>::kv(1.0, 0.0, 0.0);
    // 30 degrees from the inward normal.
    let d = V3::<R>::kv(-0.866_025_4, -0.5, 0.0);
    let mut sins = [0f32; 3];
    for ch in 0..3 {
        let ior = temple_core::IOR[ch];
        let t = refract_in(d, n, R::k(ior));
        let st = (t.y.to_q16() as f32 / 65536.0).abs();
        let len = t.len2().to_q16() as f32 / 65536.0;
        println!("TEMPLE refraction [{}] ch={} n={:.3} sin_in=0.5000 sin_out={:.5} n*sin_out={:.5} |t|^2={:.5} angle_out={:.3}deg", R::NAME, ch, ior, st, ior * st, len, st.asin().to_degrees());
        assert!((ior * st - 0.5).abs() < tol, "Snell: n sin(t) = {} want 0.5", ior * st);
        assert!((len - 1.0).abs() < tol * 4.0, "unit refracted ray");
        sins[ch] = st;
    }
    assert!(sins[2] < sins[1] && sins[1] < sins[0], "dispersion order: blue bends most");
    // Leaving through a face: 20 degrees inside refracts out at asin(n sin 20); 35 degrees is past critical (TIR).
    let ior = R::k(temple_core::IOR[1]);
    let t_in = V3::<R>::kv(0.939_692_6, 0.342_020_1, 0.0);
    let (out, _) = refract_out(t_in, n, ior).expect("20 degrees leaves");
    let so = (out.y.to_q16() as f32 / 65536.0).abs();
    let want = temple_core::IOR[1] * 0.342_020_1;
    println!("TEMPLE refraction [{}] exit sin_in=0.34202 sin_out={:.5} want {:.5}; 35deg -> TIR", R::NAME, so, want);
    assert!((so - want).abs() < tol * 2.0);
    let t_tir = V3::<R>::kv(0.819_152, 0.573_576_4, 0.0);
    assert!(refract_out(t_tir, n, ior).is_none(), "35 degrees inside a zircon is total internal reflection");
}

#[test]
fn refraction_angle() {
    snell::<f32>(1e-4);
    snell::<Fx>(2e-3);
}

// ---- the lens ---------------------------------------------------------------------------------------------------

#[test]
fn lens_one_star() {
    let tp = Temple::<f32>::new();
    let (w, h) = (960usize, 600usize);
    let cam = tp.camera(STILL_ORBIT_MRAD, 0, w, h);
    // The primary ray (on a 48 x 30 sweep of the screen, missing the gem) the flow deflects most.
    let mut best = (0f32, V3::<f32>::zero(), 0usize, 0usize);
    for sy in 0..30 {
        for sx in 0..48 {
            let px = (sx as f32 + 0.5) / 48.0 * 2.0 - 1.0;
            let py = 1.0 - (sy as f32 + 0.5) / 30.0 * 2.0;
            let d = (cam.fwd + cam.right_s * px + cam.up_s * py).norm();
            if tp.gem.intersect(cam.eye, d).is_some() {
                continue;
            }
            let dl = tp.lensed_dir(&cam, d);
            let m = (dl - d).len2().sqrt();
            if m > best.0 {
                best = (m, d, sx, sy);
            }
        }
    }
    let (mag, d, sx, sy) = best;
    let dl = tp.lensed_dir(&cam, d);
    // The reference: the same shell crossings in f64, the mass gradient by central differences of the trilinear
    // field at +-1/8 cell (exact inside a cell), weighted, summed, projected across the ray.
    let (o, dd) = ([cam.eye.x as f64, cam.eye.y as f64, cam.eye.z as f64], [d.x as f64, d.y as f64, d.z as f64]);
    let b = o[0] * dd[0] + o[1] * dd[1] + o[2] * dd[2];
    let c0 = o[0] * o[0] + o[1] * o[1] + o[2] * o[2];
    let half = volume::HALF_Q16 as f64 / 65536.0;
    let gs = (volume::GRID as f64 - 1.0) / (2.0 * half);
    let mut g = [0f64; 3];
    let mut crossings = 0;
    for (s, &r) in SHELL_R.iter().enumerate() {
        let disc = b * b - (c0 - (r as f64) * (r as f64));
        if disc <= 0.0 {
            continue;
        }
        for t in [-b + disc.sqrt()] {
            if t <= 0.0 {
                continue;
            }
            let p = [o[0] + dd[0] * t, o[1] + dd[1] * t, o[2] + dd[2] * t];
            let q = [((p[0] + half) * gs * 65536.0) as i32, ((p[1] + half) * gs * 65536.0) as i32, ((p[2] + half) * gs * 65536.0) as i32];
            if tp.vol.sample(q).is_none() {
                continue;
            }
            crossings += 1;
            for a in 0..3 {
                let (mut qp, mut qm) = (q, q);
                qp[a] += 8192;
                qm[a] -= 8192;
                let fd = (tp.vol.mass_at(qp) - tp.vol.mass_at(qm)) as f64 / 0.25; // byte*256 per cell
                g[a] += fd / 65536.0 * SHELL_W[s] as f64 / 256.0;
            }
        }
    }
    let k = KAPPA as f64;
    let dl_ref = [g[0] * k, g[1] * k, g[2] * k];
    let along = dl_ref[0] * dd[0] + dl_ref[1] * dd[1] + dl_ref[2] * dd[2];
    let perp = [dl_ref[0] - dd[0] * along, dl_ref[1] - dd[1] * along, dl_ref[2] - dd[2] * along];
    let got = [(dl.x - d.x) as f64, (dl.y - d.y) as f64, (dl.z - d.z) as f64];
    let lr = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
    let lg = (got[0] * got[0] + got[1] * got[1] + got[2] * got[2]).sqrt();
    let cos = (perp[0] * got[0] + perp[1] * got[1] + perp[2] * got[2]) / (lr * lg).max(1e-12);
    let pix = cam.pix as f64;
    let fxt = Temple::<Fx>::new();
    let fcam = fxt.camera(STILL_ORBIT_MRAD, 0, w, h);
    let fd = V3::new(Fx::k(d.x), Fx::k(d.y), Fx::k(d.z));
    let fdl = fxt.lensed_dir(&fcam, fd);
    let fm = (((fdl.x.0 - fd.x.0) as f64).powi(2) + ((fdl.y.0 - fd.y.0) as f64).powi(2) + ((fdl.z.0 - fd.z.0) as f64).powi(2)).sqrt() / 65536.0;
    println!(
        "TEMPLE lens: ray at sweep ({},{}) of 48x30, {} shell crossings: deflection {:.5} rad = {:.1} px (f32), {:.5} rad = {:.1} px (fx16); reference {:.5} rad, cos {:.4}",
        sx, sy, crossings, mag, mag as f64 / pix, fm, fm / pix, lr, cos
    );
    assert!(mag > 0.0005 && mag < 0.08, "a lensed star moves by a visible, bounded angle");
    assert!(cos > 0.95, "the deflection points along the mass gradient across the ray (cos {})", cos);
    assert!(lg / lr > 0.8 && lg / lr < 1.25, "the deflection's size matches the reference ({} vs {})", lg, lr);
    assert!((fm - mag as f64).abs() < 0.2 * mag as f64 + 0.0003, "the Q16.16 lens agrees with f32");
    // A ray that misses every shell's mass is not bent: straight up from the eye, outside the grid's mass.
    let up = V3::<f32>::kv(0.0, 1.0, 0.0);
    let du = tp.lensed_dir(&cam, up);
    assert!((du - up).len2() < 1e-6 || tp.vol.sample([0, 0, 0]).is_some());
}

// ---- the reference frame ------------------------------------------------------------------------------------------

/// Pinned from the measurement run (docs/dev/evidence/temple/TEMPLE.md): the 64x64, 4-pass Q16.16 frame.
const REFERENCE_64: Option<u64> = Some(0xccb6_2dbc_8bf8_7eed);

fn frame<R: Real>(tp: &Temple<R>, w: usize, h: usize, passes: u32) -> Vec<u32> {
    let cam = tp.camera(STILL_ORBIT_MRAD, 0, w, h);
    let mut fr = Frame::new(w, h);
    for _ in 0..passes {
        tp.pass(&cam, &mut fr);
    }
    let mut out = vec![0u32; w * h];
    tp.resolve(&fr, &mut out);
    out
}

#[test]
fn reference_frame_64() {
    let a = frame(&Temple::<Fx>::new(), 64, 64, 4);
    let b = frame(&Temple::<Fx>::new(), 64, 64, 4);
    let sum = checksum(&a);
    let lit = a.iter().filter(|&&p| p != 0).count();
    println!("TEMPLE reference 64x64 fx16 4 passes: checksum={:#018x} lit_px={}/4096", sum, lit);
    assert_eq!(a, b, "deterministic");
    assert!(lit > 400, "the scene is in the frame");
    if let Some(want) = REFERENCE_64 {
        assert_eq!(sum, want, "the reference frame moved");
    }
}

fn mean_diff(a: &[u32], b: &[u32]) -> (f64, f64) {
    let (mut s, mut big, mut n) = (0f64, 0usize, 0usize);
    for (&x, &y) in a.iter().zip(b.iter()) {
        for sh in [16, 8, 0] {
            let d = (((x >> sh) & 255) as i32 - ((y >> sh) & 255) as i32).abs();
            s += d as f64;
            if d > 8 {
                big += 1;
            }
            n += 1;
        }
    }
    (s / n as f64, big as f64 * 100.0 / n as f64)
}

#[test]
fn fx_tracks_f32() {
    let a = frame(&Temple::<Fx>::new(), 64, 64, 4);
    let b = frame(&Temple::<f32>::new(), 64, 64, 4);
    let (m, big) = mean_diff(&a, &b);
    println!("TEMPLE fx16 vs f32 64x64: mean |d| = {:.3} LSB, {:.2}% of channels > 8 LSB", m, big);
    assert!(m < 6.0, "the integer scalar renders the f32 picture");
}

// ---- THE BAR ----------------------------------------------------------------------------------------------------

struct Still {
    ms_at: Vec<f64>,
    d4_8: (f64, f64),
    d8_16: (f64, f64),
    img16: Vec<u32>,
}

fn still<R: Real>(tp: &Temple<R>) -> Still {
    let (w, h) = (960, 600);
    let cam = tp.camera(STILL_ORBIT_MRAD, 0, w, h);
    let mut fr = Frame::new(w, h);
    let mut out = vec![0u32; w * h];
    let (mut i4, mut i8) = (Vec::new(), Vec::new());
    let mut ms_at = Vec::new();
    let t0 = Instant::now();
    for p in 1..=16u32 {
        tp.pass(&cam, &mut fr);
        tp.resolve(&fr, &mut out); // a progressive display resolves every pass: inside the timing
        ms_at.push(t0.elapsed().as_secs_f64() * 1000.0);
        if p == 4 {
            i4 = out.clone();
        }
        if p == 8 {
            i8 = out.clone();
        }
    }
    Still { ms_at, d4_8: mean_diff(&i4, &i8), d8_16: mean_diff(&i8, &out), img16: out }
}

fn orbit<R: Real>(tp: &Temple<R>, frames: u32) -> (f64, f64, Vec<u32>) {
    let (w, h) = (480, 300);
    let mut fr = Frame::new(w, h);
    let mut out = vec![0u32; w * h];
    let mut big = vec![0u32; w * h * 4];
    let mut worst = 0f64;
    let t0 = Instant::now();
    for f in 0..frames {
        let tf = Instant::now();
        let cam = tp.camera(STILL_ORBIT_MRAD + f as i32 * 12, f as i32 * 4, w, h);
        fr.reset();
        tp.pass(&cam, &mut fr);
        tp.resolve(&fr, &mut out);
        upscale2(&out, w, h, &mut big);
        worst = worst.max(tf.elapsed().as_secs_f64() * 1000.0);
    }
    let total = t0.elapsed().as_secs_f64();
    (frames as f64 / total, worst, out)
}

fn png(path: &std::path::Path, w: usize, h: usize, px: &[u32]) {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = ty.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            let p = px[y * w + x];
            raw.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    // zlib, stored blocks.
    let mut z = vec![0x78u8, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out).unwrap();
}

fn report<R: Real>(lines: &mut Vec<String>, ev: Option<&std::path::Path>) -> (f64, f64) {
    let t0 = Instant::now();
    let tp = Temple::<R>::new();
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let s = still(&tp);
    let (fps, worst, oimg) = orbit(&tp, 24);
    let per_sample_ns = s.ms_at[15] * 1e6 / (16.0 * 960.0 * 600.0);
    let conv = if s.d4_8.0 <= 0.5 && s.d4_8.1 < 0.5 {
        (4, s.ms_at[3])
    } else if s.d8_16.0 <= 0.5 && s.d8_16.1 < 0.5 {
        (8, s.ms_at[7])
    } else {
        (16, s.ms_at[15])
    };
    let n = R::NAME;
    let bar_ms = if n == Fx::NAME { 3000.0 } else { 2000.0 };
    lines.push(format!("TEMPLE bar [{}] build (tables + flow bake) {:.0} ms", n, build_ms));
    lines.push(format!(
        "TEMPLE bar [{}] still 960x600: 1 spp {:.0} ms, 4 spp {:.0} ms, 8 spp {:.0} ms, 16 spp {:.0} ms ({:.0} ns per sample incl. resolve)",
        n, s.ms_at[0], s.ms_at[3], s.ms_at[7], s.ms_at[15], per_sample_ns
    ));
    lines.push(format!(
        "TEMPLE bar [{}] still convergence: mean |img4-img8| {:.3} LSB ({:.2}% ch > 8), mean |img8-img16| {:.3} LSB ({:.2}% ch > 8); converged (mean <= 0.5 LSB, < 0.5% > 8) at {} spp{} = {:.0} ms -> {} (bar <= {:.0} ms)",
        n, s.d4_8.0, s.d4_8.1, s.d8_16.0, s.d8_16.1, conv.0, if conv.0 == 16 { " or later (not reached by 8)" } else { "" }, conv.1,
        if conv.0 < 16 && conv.1 <= bar_ms { "MEETS" } else { "MISSES" }, bar_ms
    ));
    lines.push(format!(
        "TEMPLE bar [{}] orbit 480x300 (reset every frame, 2x upscale to 960x600 timed): {:.2} fps, worst frame {:.0} ms -> {} (bar >= 10 fps)",
        n, fps, worst, if fps >= 10.0 { "MEETS" } else { "MISSES" }
    ));
    if let Some(dir) = ev {
        png(&dir.join(format!("temple-still-960x600-16spp-{}.png", n)), 960, 600, &s.img16);
        png(&dir.join(format!("temple-orbit-480x300-{}.png", n)), 480, 300, &oimg);
    }
    (conv.1, fps)
}

#[test]
fn bar() {
    let ev = std::env::var("TEMPLE_EVIDENCE").ok().map(std::path::PathBuf::from);
    if let Some(d) = ev.as_ref() {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut lines = Vec::new();
    lines.push(format!("TEMPLE bar host: {} logical cpus, one thread; cpu: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), cpu_name()));
    report::<Fx>(&mut lines, ev.as_deref());
    report::<f32>(&mut lines, ev.as_deref());
    lines.push(phases::<Fx>());
    lines.push(phases::<f32>());
    for l in &lines {
        println!("{}", l);
    }
    if let Some(d) = ev.as_ref() {
        std::fs::write(d.join("bar.txt"), lines.join("\n") + "\n").unwrap();
    }
}

fn cpu_name() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

/// One 960x600 pass split by phase (B876): each of the core's public per-phase fns timed over every primary ray of
/// the frame, beside one whole `pass` and one `resolve`. No instrumentation in the core; the phases overlap the
/// pass's own work, so they read as shares of it, not as a sum.
fn phases<R: Real>() -> String {
    use std::hint::black_box as bb;
    let tp = Temple::<R>::new();
    let (w, h) = (960usize, 600usize);
    let cam = tp.camera(STILL_ORBIT_MRAD, 0, w, h);
    let mut dirs = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let px = R::from_q16(((((x as i64) << 17) + 65536) / w as i64 - 65536) as i32);
            let py = R::from_q16((65536 - ((((y as i64) << 17) + 65536) / h as i64)) as i32);
            dirs.push((cam.fwd + cam.up_s * py + cam.right_s * px).norm());
        }
    }
    let o = cam.eye;
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let far = R::k(200.0);
    let tm = |hit: &Option<(R, usize)>| match hit {
        Some((t, _)) => *t,
        None => far,
    };
    let t = Instant::now();
    for d in &dirs {
        bb((cam.fwd + cam.up_s * d.y + cam.right_s * d.x).norm());
    }
    let norm = ms(t);
    let mut hits = Vec::with_capacity(dirs.len());
    let t = Instant::now();
    for d in &dirs {
        hits.push(tp.gem.intersect(o, *d));
    }
    let intersect = ms(t);
    let mut defl = Vec::with_capacity(dirs.len());
    let t = Instant::now();
    for (d, hit) in dirs.iter().zip(&hits) {
        defl.push(tp.flow(&cam, o, *d, tm(hit)));
    }
    let flow = ms(t);
    let t = Instant::now();
    for (d, hit) in dirs.iter().zip(&hits) {
        bb(tp.beam(o, *d, tm(hit)));
    }
    let beam = ms(t);
    let t = Instant::now();
    for (d, hit) in dirs.iter().zip(&hits) {
        bb(tp.splash(o, *d, tm(hit)));
    }
    let splash = ms(t);
    let t = Instant::now();
    for (d, hit) in dirs.iter().zip(&hits) {
        bb(tp.fans(o, *d, tm(hit)));
    }
    let fans = ms(t);
    let t = Instant::now();
    for (d, hit) in dirs.iter().zip(&hits) {
        if let Some((tt, i)) = hit {
            bb(tp.shade_gem(o + *d * *tt, *i, *d));
        }
    }
    let shade_gem = ms(t);
    let t = Instant::now();
    for ((d, hit), (_, g)) in dirs.iter().zip(&hits).zip(&defl) {
        if hit.is_none() {
            bb(tp.stars(&cam, tp.lens(*d, *g)));
        }
    }
    let lens_stars = ms(t);
    let mut fr = Frame::new(w, h);
    let t = Instant::now();
    tp.pass(&cam, &mut fr);
    let pass = ms(t);
    let mut out = vec![0u32; w * h];
    let t = Instant::now();
    tp.resolve(&fr, &mut out);
    let resolve = ms(t);
    format!(
        "TEMPLE phases [{}] 960x600 pass={:.0} ns_px={:.0} flow={:.0} fans={:.0} lens_stars={:.0} shade_gem={:.0} intersect={:.0} norm={:.0} beam={:.0} splash={:.0} resolve={:.0}",
        R::NAME, pass, pass * 1e6 / (w * h) as f64, flow, fans, lens_stars, shade_gem, intersect, norm, beam, splash, resolve
    )
}
