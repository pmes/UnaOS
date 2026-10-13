// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D) owns the domain; seam `shared-core` (a no_std lib the ring-3 vessel links and the host
//! measures).
//!
//! TEMPLE (LEDGER SR147, R128) — the renderer core of the TEMPLE game: R128's FULL scene ("a super detailed version
//! of the view") traced in software on one core. One picture, two tiers: STARFIELD (B661) is the boot screen's
//! reduction of this same scene; docs/dev/evidence/temple/TEMPLE.md holds the measurement (the design record
//! docs/dev/OS/05_USER_EXPERIENCE/TEMPLE.md waits on the bar: R128 builds the game tier only past it).
//!
//! THE SCENE (world units, y up, the gem at the origin):
//! - THE ZIRCON: a convex polyhedron, the intersection of 16 half-spaces — the zircon habit, an eight-sided prism
//!   ({100} + {110}) capped by two four-sided bipyramids ({101}) — 4.6 tall. Its signed distance is
//!   `max_i(n_i . p - d_i)`; a ray meets it by the closed-form clip of that SDF's zero set (the slab test over the
//!   16 planes, fractions compared by cross-multiplication so one divide survives per clip). Shading: Schlick
//!   Fresnel; the transmitted part refracts IN, crosses the gem (Beer absorption), meets the far face (Fresnel /
//!   total internal reflection), refracts OUT, and the reflected remainder takes a SECOND bounce and exits again —
//!   once per wavelength, three wavelengths (R, G, B at n = 1.920 / 1.955 / 2.000: zircon's fire), so the gem
//!   disperses what it shows.
//! - THE BEAM: a vertical column striking the apex, single-scatter in-scattering integrated IN CLOSED FORM along the
//!   ray (a Gaussian radial profile: the integral of exp(-h(t)^2/s^2) dt over the line is exp(-h*^2/s^2) s sqrt(pi)
//!   / |d_xz|), a core and a halo; the impact SPLASH as two Lorentzian point glows at the apex; and the splash's
//!   dispersed FANS — three coloured lines per fan leaving the lower facets at the three wavelengths' angles, each a
//!   closed-form line-to-ray scatter integral.
//! - THE DARK-MATTER FLOW: the baked 3D curl field (volume.rs), sampled on seven concentric shells, each at its far
//!   crossing so the flow stands BEHIND the gem (the multi-plane approximation lensing itself uses): each shell crossing is one exact sample of the 3D field — its streamline
//!   bands as faint emissive strokes, its mass as haze, its mass gradient as that plane's deflection.
//! - THE LENSED STARS: the ray's direction, bent by the summed deflection (toward the mass: the images are pushed
//!   away from the filaments and arc around them), looks up three layers of procedural stars on a cube-mapped
//!   sphere, each a Gaussian of about a pixel.
//! - THE CAMERA: a slow orbit (radius 10.5, a gentle rise and fall), looking at the gem's waist.
//!
//! THE INTEGRATOR: one sample per pixel per pass, a jittered sub-pixel position per pass (the R2 sequence), the
//! linear radiance summed in a u32 accumulator; `resolve` averages, tone-maps (ACES fit) and encodes (gamma 2)
//! through a 4096-entry table. The still image converges as passes accumulate while the camera holds; a moving
//! camera resets the accumulator every frame.

#![no_std]

extern crate alloc;

pub mod palette;
pub mod real;
pub mod vec;
pub mod volume;

use alloc::vec::Vec;
pub use real::{k, Fx, Real};
pub use vec::{sin_cos_mrad, ExpTable, V3};
use volume::{Volume, GRID};

/// The gem's apex height (the beam strikes here) and the bipyramid's face tilt from the vertical.
pub const APEX_Y: f32 = 2.3;
const FACE_TILT_MRAD: i32 = 838; // 48 degrees
const GEM_ROT_MRAD: i32 = 170;
/// Refractive index per wavelength (R, G, B): zircon's dispersion, a little exaggerated for the eye.
pub const IOR: [f32; 3] = [1.920, 1.955, 2.000];
/// The far distance (no hit).
const BIG: f32 = 200.0;
/// The orbit.
const ORBIT_R: f32 = 10.5;
const TAN_HALF_FOV_Y: f32 = 0.4245; // 46 degrees vertical
/// The flow shells' radii and weights (Q8).
pub const SHELL_R: [f32; 7] = [3.4, 4.8, 6.6, 8.8, 12.0, 14.6, 17.0];
pub const SHELL_W: [i32; 7] = [140, 180, 205, 230, 256, 218, 180];
/// Deflection per unit of mass gradient (per cell) per shell crossing, in radians.
pub const KAPPA: f32 = 0.035;
/// The strokes' overall brightness (they are FAINT: the void stays ultra black between them).
const FLOW_GAIN: f32 = 0.34;

/// Linear light from a packed display colour (gamma 2).
pub fn lin<R: Real>(c: u32) -> V3<R> {
    let f = |b: u32| {
        let v = R::from_q16((b * 257) as i32);
        v * v
    };
    V3::new(f((c >> 16) & 255), f((c >> 8) & 255), f(c & 255))
}

/// The scene's colours, linearised once.
pub struct Pal<R> {
    pub beam: V3<R>,
    pub violet: V3<R>,
    pub cyan: V3<R>,
    pub haze: V3<R>,
    pub warm: V3<R>,
    pub cool: V3<R>,
    pub spectrum: [V3<R>; 3],
    /// Beer absorption per unit length, per channel (-ln of `zircon.transmit`).
    pub absorb: V3<R>,
    pub fire: V3<R>,
}

impl<R: Real> Pal<R> {
    fn new() -> Self {
        let tr: V3<R> = lin(palette::ZIRCON_TRANSMIT);
        // -ln(t) for t in (0, 1]: 2 * (1 - sqrt t) / (1 + sqrt t) * ... — a short series around 1 is plenty here.
        let nl = |t: R| {
            let y = (R::one() - t) / (R::one() + t);
            let y2 = y * y;
            k::<R>(2.0) * y * (R::one() + y2 * (k::<R>(1.0 / 3.0) + y2 * k::<R>(0.2)))
        };
        let sp = palette::SPLASH_SPECTRUM;
        Pal {
            beam: lin(palette::SPLASH_BEAM),
            violet: lin(palette::FLOW_VIOLET),
            cyan: lin(palette::FLOW_CYAN),
            haze: lin(palette::FLOW_HAZE),
            warm: lin(palette::STAR_WARM),
            cool: lin(palette::STAR_COOL),
            spectrum: [lin(sp[0]), lin(sp[4]), lin(sp[7])],
            absorb: V3::new(nl(tr.x), nl(tr.y), nl(tr.z)),
            fire: lin(palette::ZIRCON_FIRE),
        }
    }
}

/// The zircon: 16 outward plane normals and offsets (inside: n . p <= d).
pub struct Gem<R> {
    pub n: [V3<R>; 16],
    pub d: [R; 16],
    rb2: R,
}

impl<R: Real> Gem<R> {
    fn new() -> Self {
        let mut n = [V3::zero(); 16];
        let mut d = [R::zero(); 16];
        for j in 0..8 {
            let phi = GEM_ROT_MRAD + (j as i64 * 785_398 / 1000) as i32;
            let (s, c) = sin_cos_mrad::<R>(phi);
            n[j] = V3::new(c, R::zero(), s);
            d[j] = if j % 2 == 0 { R::one() } else { k(1.06) };
        }
        let (sa, ca) = sin_cos_mrad::<R>(FACE_TILT_MRAD);
        for j in 0..4 {
            let phi = GEM_ROT_MRAD + (j as i64 * 1_570_796 / 1000) as i32;
            let (s, c) = sin_cos_mrad::<R>(phi);
            n[8 + j] = V3::new(sa * c, ca, sa * s);
            d[8 + j] = ca * k(APEX_Y);
            n[12 + j] = V3::new(sa * c, -ca, sa * s);
            d[12 + j] = ca * k(APEX_Y);
        }
        Gem { n, d, rb2: k(5.5) }
    }

    /// The signed distance (a bound: exact on the faces, conservative off them).
    pub fn sdf(&self, p: V3<R>) -> R {
        let mut m = self.n[0].dot(p) - self.d[0];
        for i in 1..16 {
            m = m.max(self.n[i].dot(p) - self.d[i]);
        }
        m
    }

    /// From outside: the entry `t` and the face, or None.
    #[inline(always)]
    pub fn intersect(&self, o: V3<R>, dir: V3<R>) -> Option<(R, usize)> {
        let b = o.dot(dir);
        let c = o.len2() - self.rb2;
        if c > R::zero() && (b > R::zero() || b * b < c) {
            return None;
        }
        // Entry = max over entering planes of (-num)/(-den); exit = min over leaving planes of num/den.
        let (mut bn, mut bd, mut bi) = (R::zero(), R::zero(), usize::MAX);
        let (mut en, mut ed, mut have_exit) = (R::zero(), R::one(), false);
        for i in 0..16 {
            let num = self.d[i] - self.n[i].dot(o);
            let den = self.n[i].dot(dir);
            if den < R::zero() {
                let (pn, pd) = (-num, -den);
                if bi == usize::MAX || R::prod_gt(pn, bd, bn, pd) {
                    bn = pn;
                    bd = pd;
                    bi = i;
                }
            } else if den > R::zero() {
                if !have_exit || R::prod_gt(en, den, num, ed) {
                    en = num;
                    ed = den;
                    have_exit = true;
                }
            } else if num < R::zero() {
                return None;
            }
        }
        if bi == usize::MAX || !have_exit || bn <= R::zero() || !R::prod_gt(en, bd, bn, ed) {
            return None;
        }
        Some((bn / bd, bi))
    }

    /// From inside: the exit `t` and the face.
    #[inline(always)]
    pub fn exit(&self, o: V3<R>, dir: V3<R>) -> (R, usize) {
        let (mut en, mut ed, mut ei) = (R::zero(), R::one(), usize::MAX);
        for i in 0..16 {
            let den = self.n[i].dot(dir);
            if den > R::zero() {
                let num = (self.d[i] - self.n[i].dot(o)).max(R::zero());
                if ei == usize::MAX || R::prod_gt(en, den, num, ed) {
                    en = num;
                    ed = den;
                    ei = i;
                }
            }
        }
        if ei == usize::MAX {
            return (R::zero(), 0);
        }
        (en / ed, ei)
    }
}

/// Refraction INTO a denser medium of index `ior` through a surface with outward normal `n` (`d` arriving, unit).
pub fn refract_in<R: Real>(d: V3<R>, n: V3<R>, ior: R) -> V3<R> {
    let c1 = -d.dot(n);
    let eta = R::one() / ior;
    let k2 = R::one() - eta * eta * (R::one() - c1 * c1);
    d * eta + n * (eta * c1 - k2.sqrt())
}

/// Refraction OUT of the medium (`t` inside, `n` the face's outward normal): None on total internal reflection.
/// Returns the outgoing direction and the cosine on the outside (for Fresnel).
pub fn refract_out<R: Real>(t: V3<R>, n: V3<R>, ior: R) -> Option<(V3<R>, R)> {
    let c1 = t.dot(n);
    let k2 = R::one() - ior * ior * (R::one() - c1 * c1);
    if k2 <= R::zero() {
        return None;
    }
    let c2 = k2.sqrt();
    Some((t * ior + n * (c2 - ior * c1), c2))
}

#[inline(always)]
fn schlick<R: Real>(f0: R, cos: R) -> R {
    let x = (R::one() - cos).clamp01();
    let x2 = x * x;
    f0 + (R::one() - f0) * (x2 * x2 * x)
}

/// A camera for one frame: the orbit position, the screen basis (pre-scaled by the field of view), the pixel's
/// angular size (the stars' PSF) and the flow's rotation.
pub struct Camera<R> {
    pub eye: V3<R>,
    pub fwd: V3<R>,
    pub right_s: V3<R>,
    pub up_s: V3<R>,
    pub w: usize,
    pub h: usize,
    pub pix: R,
    pub flow_c: R,
    pub flow_s: R,
    star_inv_sig2: [R; 3],
}

/// The accumulator: per pixel the summed linear radiance (Q12 per channel), and the passes taken.
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub acc: Vec<[u32; 3]>,
    pub spp: u32,
}

impl Frame {
    pub fn new(w: usize, h: usize) -> Frame {
        Frame { w, h, acc: alloc::vec![[0u32; 3]; w * h], spp: 0 }
    }
    pub fn reset(&mut self) {
        for a in self.acc.iter_mut() {
            *a = [0; 3];
        }
        self.spp = 0;
    }
}

/// The passes an accumulator holds before it stops (u32 headroom: 4096 x the per-sample clamp).
pub const SPP_MAX: u32 = 4096;
const SAMPLE_MAX_Q12: i32 = (64 << 12) - 1;

/// Star layers: cells across a cube face, density (of 256), brightness, PSF width (pixels).
const STARS: [(i32, u32, f32, f32); 3] = [(360, 60, 0.55, 0.70), (110, 56, 1.4, 0.80), (28, 52, 5.0, 1.15)];

/// The whole renderer: tables, palette, gem, flow volume.
pub struct Temple<R> {
    pub exp: ExpTable<R>,
    pub pal: Pal<R>,
    pub gem: Gem<R>,
    pub vol: Volume,
    /// The two fans: each a start point on the lower facets and one direction per wavelength.
    pub fan: [(V3<R>, [V3<R>; 3]); 2],
    tone: Vec<u8>,
}

impl<R: Real> Temple<R> {
    /// Build everything (the flow bake dominates: well under a second on one core).
    pub fn new() -> Self {
        let mut tone = alloc::vec![0u8; 4096];
        for (i, t) in tone.iter_mut().enumerate() {
            // ACES (Narkowicz fit) of x = i / 256, then gamma 2.
            let x = R::from_q16((i as i32) << 8);
            let num = x * (k::<R>(2.51) * x + k(0.03));
            let den = x * (k::<R>(2.43) * x + k(0.59)) + k(0.14);
            let y = (num / den).clamp01();
            // Gamma 2 with sRGB's linear toe (12.92 y below the crossing), so the void's faint glow stays black.
            let e = y.sqrt().min(y * k(12.92));
            *t = ((e.to_q16() * 255 + 32768) >> 16).clamp(0, 255) as u8;
        }
        const DEPRESSION_MRAD: [i32; 3] = [320, 400, 500];
        const FAN_MRAD: [i32; 2] = [600, 3742];
        let mut fan = [(V3::zero(), [V3::zero(); 3]); 2];
        for (f, &phi) in fan.iter_mut().zip(FAN_MRAD.iter()) {
            let (s, c) = sin_cos_mrad::<R>(phi);
            f.0 = V3::new(c * k(0.62), k(-1.25), s * k(0.62));
            for ch in 0..3 {
                let (se, ce) = sin_cos_mrad::<R>(DEPRESSION_MRAD[ch]);
                f.1[ch] = V3::new(c * ce, -se, s * ce);
            }
        }
        Temple { exp: ExpTable::new(), pal: Pal::new(), gem: Gem::new(), vol: Volume::bake(), fan, tone }
    }

    /// The camera at orbit angle `orbit_mrad`, the flow turned by `flow_mrad`, for a `w` x `h` frame.
    pub fn camera(&self, orbit_mrad: i32, flow_mrad: i32, w: usize, h: usize) -> Camera<R> {
        let (s, c) = sin_cos_mrad::<R>(orbit_mrad);
        let (bob, _) = sin_cos_mrad::<R>(orbit_mrad / 2 + 700);
        let eye = V3::new(s * k(ORBIT_R), k::<R>(0.9) + bob * k(0.8), c * k(ORBIT_R));
        let target = V3::kv(0.0, 0.1, 0.0);
        let fwd = (target - eye).norm();
        let right = fwd.cross(V3::kv(0.0, 1.0, 0.0)).norm();
        let up = right.cross(fwd);
        let ty: R = k(TAN_HALF_FOV_Y);
        let tx = ty * R::from_i32(w as i32) / R::from_i32(h as i32);
        let pix = k::<R>(2.0 * TAN_HALF_FOV_Y) / R::from_i32(h as i32);
        let (fs, fc) = sin_cos_mrad::<R>(flow_mrad);
        let mut star_inv_sig2 = [R::zero(); 3];
        for (l, &(n, _, _, wpx)) in STARS.iter().enumerate() {
            // The PSF sigma in cell units: wpx pixels of `pix` radians, a cell being ~2/n radians.
            let sig = pix * k(wpx) * R::from_i32(n / 2);
            star_inv_sig2[l] = k::<R>(0.5) / (sig * sig);
        }
        Camera { eye, fwd, right_s: right * tx, up_s: up * ty, w, h, pix, flow_c: fc, flow_s: fs, star_inv_sig2 }
    }

    /// One pass: one jittered sample per pixel, added to `fr`.
    pub fn pass(&self, cam: &Camera<R>, fr: &mut Frame) {
        if fr.spp >= SPP_MAX {
            return;
        }
        let n = fr.spp as i64;
        let jx = (32768 + n * 49471) & 0xFFFF;
        let jy = (32768 + n * 37345) & 0xFFFF;
        let (w, h) = (fr.w as i64, fr.h as i64);
        let cols: Vec<R> = (0..w).map(|x| R::from_q16(((((x << 17) + 2 * jx) / w) - 65536) as i32)).collect();
        for y in 0..h {
            let sy = R::from_q16((65536 - (((y << 17) + 2 * jy) / h)) as i32);
            let base = cam.fwd + cam.up_s * sy;
            let row = (y * w) as usize;
            for x in 0..w as usize {
                let dir = (base + cam.right_s * cols[x]).norm();
                let c = self.radiance(cam, dir);
                let a = &mut fr.acc[row + x];
                a[0] += (c.x.to_q16() >> 4).clamp(0, SAMPLE_MAX_Q12) as u32;
                a[1] += (c.y.to_q16() >> 4).clamp(0, SAMPLE_MAX_Q12) as u32;
                a[2] += (c.z.to_q16() >> 4).clamp(0, SAMPLE_MAX_Q12) as u32;
            }
        }
        fr.spp += 1;
    }

    /// The accumulator to display pixels (0x00RRGGBB), `out` the frame's size.
    pub fn resolve(&self, fr: &Frame, out: &mut [u32]) {
        let spp = fr.spp.max(1) as u64;
        let inv = (1u64 << 32) / spp;
        for (o, a) in out.iter_mut().zip(fr.acc.iter()) {
            let t = |v: u32| self.tone[(((v as u64 * inv) >> 36) as usize).min(4095)] as u32;
            *o = t(a[0]) << 16 | t(a[1]) << 8 | t(a[2]);
        }
    }

    /// The radiance along one primary ray.
    #[inline(always)]
    pub fn radiance(&self, cam: &Camera<R>, d: V3<R>) -> V3<R> {
        let o = cam.eye;
        let hit = self.gem.intersect(o, d);
        let tmax = match hit {
            Some((t, _)) => t,
            None => k(BIG),
        };
        let (flow, defl) = self.flow(cam, o, d, tmax);
        let col = flow + self.pal.beam * (self.beam(o, d, tmax) + self.splash(o, d, tmax)) + self.fans(o, d, tmax);
        match hit {
            Some((t, i)) => col + self.shade_gem(o + d * t, i, d),
            None => col + self.stars(cam, self.lens(d, defl)),
        }
    }

    /// The beam column's single scatter along `o + t d`, t in [0, tmax]: core + halo, closed form.
    #[inline(always)]
    pub fn beam(&self, o: V3<R>, d: V3<R>, tmax: R) -> R {
        let a = (d.x * d.x + d.z * d.z).max(k(0.0064));
        let od = o.x * d.x + o.z * d.z;
        let oo = o.x * o.x + o.z * o.z;
        let h2 = (oo - od * od / a).max(R::zero());
        if h2 > k(1.5) {
            return R::zero();
        }
        let ts = -od / a;
        let ra = R::one() / a.sqrt();
        // The part of the Gaussian lying inside [0, tmax] (a linear ramp of the crossing's width).
        let half_w = k::<R>(0.35) * ra;
        let w0 = ((ts + half_w) / (half_w + half_w)).clamp01();
        let w1 = ((tmax - ts + half_w) / (half_w + half_w)).clamp01();
        let ys = o.y + d.y * ts.max(R::zero()).min(tmax);
        let yt = ((ys - k(APEX_Y - 0.05)) * k(4.0)).clamp01();
        let ywin = yt * yt * (k::<R>(3.0) - yt - yt);
        let core = self.exp.exp_neg(h2 * k(100.0)) * k(0.10 * 1.7725 * 2.4);
        let halo = self.exp.exp_neg(h2 * k(6.25)) * k(0.40 * 1.7725 * 0.30);
        (core + halo) * ra * w0.min(w1) * ywin
    }

    /// The impact's splash: two Lorentzian glows at the apex.
    #[inline(always)]
    pub fn splash(&self, o: V3<R>, d: V3<R>, tmax: R) -> R {
        let p = V3::kv(0.0, APEX_Y + 0.04, 0.0) - o;
        let ts = p.dot(d);
        if ts <= R::zero() || ts > tmax + k(0.2) {
            return R::zero();
        }
        let h2 = (p.len2() - ts * ts).max(R::zero());
        k::<R>(0.0036 * 1.1) / (h2 + k(0.0036)) + k::<R>(0.81 * 0.07) / (h2 + k(0.81))
    }

    /// The dispersed fans: two fans of three coloured lines leaving the lower facets.
    #[inline(always)]
    pub fn fans(&self, o: V3<R>, d: V3<R>, tmax: R) -> V3<R> {
        let mut out = V3::zero();
        for (a, us) in self.fan.iter() {
            let w = o - *a;
            let dw = d.dot(w);
            for ch in 0..3 {
                let u = us[ch];
                let b = d.dot(u);
                let den = R::one() - b * b;
                if den < k(0.002) {
                    continue;
                }
                let uw = u.dot(w);
                let inv = R::one() / den;
                let ts = (b * uw - dw) * inv;
                let ss = (uw - b * dw) * inv;
                if ts <= R::zero() || ts >= tmax || ss <= R::zero() {
                    continue;
                }
                let h2 = (w + d * ts - u * ss).len2();
                if h2 > k(0.04) {
                    continue;
                }
                let v = self.exp.exp_neg(h2 * k(1.0 / 0.0036)) * k(0.06 * 1.7725 * 1.3) / den.sqrt() * self.exp.exp_neg(ss * k(0.16));
                out = out + self.pal.spectrum[ch] * v;
            }
        }
        out
    }

    /// The lit void around a ray leaving the gem: the beam, the splash, a trace of the flow's light.
    #[inline(always)]
    fn env2(&self, o: V3<R>, d: V3<R>) -> V3<R> {
        let big = k(BIG);
        self.pal.beam * (self.beam(o, d, big) + self.splash(o, d, big)) + self.pal.violet * k(0.02)
    }

    /// The gem's radiance at its surface point `p` (face `i`, arriving `d`).
    pub fn shade_gem(&self, p: V3<R>, i: usize, d: V3<R>) -> V3<R> {
        let n = self.gem.n[i];
        let eps: R = k(0.002);
        let f0: R = k(0.1045);
        let c1 = -d.dot(n);
        let f = schlick(f0, c1);
        let r = d + n * (c1 + c1);
        let mut col = self.env2(p + n * eps, r) * f;
        let fire = self.pal.fire;
        for ch in 0..3 {
            let ior: R = k(IOR[ch]);
            let ab = self.pal.absorb.ch(ch);
            let t = refract_in(d, n, ior);
            let start = p - n * eps;
            let (tq, j) = self.gem.exit(start, t);
            let q = start + t * tq;
            let att = self.exp.exp_neg(ab * tq);
            let nj = self.gem.n[j];
            let (t1, f2) = match refract_out(t, nj, ior) {
                Some((out, c2)) => {
                    let f2 = schlick(f0, c2);
                    ((R::one() - f2) * self.env2(q + nj * eps, out).ch(ch), f2)
                }
                None => (R::zero(), R::one()),
            };
            // The second bounce.
            let cj = t.dot(nj);
            let t2 = t - nj * (cj + cj);
            let start2 = q - nj * eps;
            let (tq2, j2) = self.gem.exit(start2, t2);
            let q2 = start2 + t2 * tq2;
            let att2 = self.exp.exp_neg(ab * tq2);
            let nj2 = self.gem.n[j2];
            let (t2v, f3) = match refract_out(t2, nj2, ior) {
                Some((out, c3)) => {
                    let f3 = schlick(f0, c3);
                    ((R::one() - f3) * self.env2(q2 + nj2 * eps, out).ch(ch), f3)
                }
                None => (R::zero(), R::one()),
            };
            let v = (R::one() - f) * att * (t1 + f2 * att2 * (t2v + f3 * fire.ch(ch) * k(0.55)));
            col = match ch {
                0 => V3::new(col.x + v, col.y, col.z),
                1 => V3::new(col.x, col.y + v, col.z),
                _ => V3::new(col.x, col.y, col.z + v),
            };
        }
        col
    }

    /// The flow's light along the ray (up to `tmax`) and the summed mass gradient (the lens), world frame, Q16.
    pub fn flow(&self, cam: &Camera<R>, o: V3<R>, d: V3<R>, tmax: R) -> (V3<R>, [i32; 3]) {
        let b = o.dot(d);
        let c0 = o.len2();
        let half: R = R::from_q16(volume::HALF_Q16 as i32);
        let gs: R = R::from_i32(GRID as i32 - 1) / (half + half);
        let (mut vio, mut cya, mut haze) = (0i64, 0i64, 0i64);
        let mut g = [0i64; 3];
        for s in 0..SHELL_R.len() {
            let r = k::<R>(SHELL_R[s]);
            let disc = b * b - (c0 - r * r);
            if disc <= R::zero() {
                continue;
            }
            // The FAR crossing only (R128 as clarified: the flow lies BEHIND the foreground gem — no stroke between the
            // eye and the zircon).
            let t = -b + disc.sqrt();
            {
                if t <= R::zero() || t >= tmax {
                    continue;
                }
                let p = o + d * t;
                let px = p.x * cam.flow_c - p.z * cam.flow_s;
                let pz = p.x * cam.flow_s + p.z * cam.flow_c;
                let gq = [((px + half) * gs).to_q16(), ((p.y + half) * gs).to_q16(), ((pz + half) * gs).to_q16()];
                let Some(sm) = self.vol.sample(gq) else { continue };
                if sm.c == 0 {
                    continue;
                }
                let w = SHELL_W[s] as i64;
                // The stroke: the band of `a` (9 per unit), a thin line; dashed along by `b`.
                const WA: i64 = 4588; // 0.07 of a band
                let sa = ((sm.a as i64 * 9) & 0xFFFF) - 32768;
                let line = WA - sa.abs();
                let mut e = 0i64;
                if line > 0 {
                    let l = (line * line) / ((WA * WA) >> 16);
                    let sb = ((sm.b as i64 * 5) & 0xFFFF) - 32768;
                    let dash = (32768 - sb.abs()) * 2;
                    let dash = (dash * dash) >> 16;
                    let wd = 19661 + ((dash * 45875) >> 16);
                    e = (((l * wd) >> 16) * (7864 + sm.c as i64)) >> 16;
                }
                let cm = sm.c as i64;
                vio += (e * (65536 - cm) >> 16) * w;
                cya += (e * cm >> 16) * w;
                haze += ((cm * cm) >> 16) * w;
                for a in 0..3 {
                    g[a] += sm.g[a] as i64 * w;
                }
            }
        }
        let q = |v: i64| R::from_q16((v >> 8).clamp(-(i32::MAX as i64), i32::MAX as i64) as i32);
        let gain: R = k(FLOW_GAIN);
        let col = (self.pal.violet * q(vio) + self.pal.cyan * q(cya)) * gain + self.pal.haze * (q(haze) * k(0.05));
        // Back to the world frame (the field was sampled in the turned frame: grad f = Rot^T grad F).
        let (gx, gy, gz) = (q(g[0]), q(g[1]), q(g[2]));
        let wx = gx * cam.flow_c + gz * cam.flow_s;
        let wz = gz * cam.flow_c - gx * cam.flow_s;
        (col, [wx.to_q16(), gy.to_q16(), wz.to_q16()])
    }

    /// The direction bent toward the mass: d + KAPPA * (the summed gradient's part across the ray), renormalised.
    #[inline(always)]
    pub fn lens(&self, d: V3<R>, g: [i32; 3]) -> V3<R> {
        let dl = V3::new(R::from_q16(g[0]), R::from_q16(g[1]), R::from_q16(g[2])) * k(KAPPA);
        let perp = dl - d * dl.dot(d);
        (d + perp).norm()
    }

    /// The deflected direction of a primary ray (the tests' window onto the lens).
    pub fn lensed_dir(&self, cam: &Camera<R>, d: V3<R>) -> V3<R> {
        let (_, g) = self.flow(cam, cam.eye, d, k(BIG));
        self.lens(d, g)
    }

    /// Three layers of stars on a cube-mapped sphere, each a Gaussian of about a pixel.
    #[inline(always)]
    pub fn stars(&self, cam: &Camera<R>, d: V3<R>) -> V3<R> {
        let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
        let (face, u, v, m) = if ax >= ay && ax >= az {
            (if d.x > R::zero() { 0 } else { 1 }, d.z, d.y, ax)
        } else if ay >= az {
            (if d.y > R::zero() { 2 } else { 3 }, d.x, d.z, ay)
        } else {
            (if d.z > R::zero() { 4 } else { 5 }, d.x, d.y, az)
        };
        let inv = R::one() / m;
        let (u, v) = (u * inv + R::one(), v * inv + R::one());
        let mut out = V3::zero();
        for (l, &(n, dens, bright, _)) in STARS.iter().enumerate() {
            let hn = R::from_i32(n / 2);
            let qu = (u * hn).to_q16();
            let qv = (v * hn).to_q16();
            let h = volume::hash3(qu >> 16, qv >> 16, face + 8 * l as i32, 0x5EED_0001);
            if (h & 255) >= dens {
                continue;
            }
            let sx = 16384 + (((h >> 8) & 255) as i64 * 32768 / 255);
            let sy = 16384 + (((h >> 16) & 255) as i64 * 32768 / 255);
            let dx = (qu & 0xFFFF) as i64 - sx;
            let dy = (qv & 0xFFFF) as i64 - sy;
            let r2 = R::from_q16(((dx * dx + dy * dy) >> 16) as i32);
            let x = r2 * cam.star_inv_sig2[l];
            if x >= k(12.0) {
                continue;
            }
            let b = R::from_q16(((h >> 24) as i32) << 8);
            let b3 = b * b * b;
            let mag = (b3 * k(bright) + k(0.012)) * self.exp.exp_neg(x);
            let temp = R::from_q16((((h >> 4) & 15) as i32) << 12);
            let tint = self.pal.warm * temp + self.pal.cool * (R::one() - temp);
            out = out + tint * mag;
        }
        out
    }
}

impl<R: Real> Default for Temple<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a over a frame's pixels (the reference-frame checksum).
pub fn checksum(px: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &p in px {
        for b in p.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// A 2x nearest upscale (the moving orbit's frame onto a panel twice its size).
pub fn upscale2(src: &[u32], w: usize, h: usize, dst: &mut [u32]) {
    let w2 = w * 2;
    for y in 0..h {
        let s = &src[y * w..y * w + w];
        let (r0, r1) = dst[y * 2 * w2..(y * 2 + 2) * w2].split_at_mut(w2);
        for x in 0..w {
            let p = s[x];
            r0[2 * x] = p;
            r0[2 * x + 1] = p;
            r1[2 * x] = p;
            r1[2 * x + 1] = p;
        }
    }
}

/// The still frame's orbit angle (the fans open obliquely to the eye) — the measurement's and the vessel's start.
pub const STILL_ORBIT_MRAD: i32 = -350;
