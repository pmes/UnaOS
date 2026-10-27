//! CHARTER: Kernel — wm
//!
//! KERNEL-DISPLAY-ICC (LEDGER SR372; BP-video-25, handlers.md BP-handlers-4's icc_core clause) — the panel's display
//! transform, the LAST stage of the scanout copy filter. Every pixel the glass holds is sRGB (Facet converts a tagged
//! still at open, SR367); the panel is not. ONE `icc_core::DisplayTransform` (sRGB to the matrix/TRC panel the EDID
//! describes: its 10-bit primaries + white, its gamma byte, its sRGB-default bit) is built ONCE, at the a11y filter's
//! first latch (`a11yfilter::changed`, which `settings::a11y_load("login")` makes on every login; by then the EDID —
//! firmware or the iGPU lane's AUX read — is published), and the filter's funnel (`a11yfilter::each` / `in_place` /
//! `color`, which every writer to the scanout already calls) runs it after the colour filter. No second hook set, no
//! post-pass over the scanout (BP-video-25).
//!
//! THE LOCK. `XF` is a `spin::Once`, written once by [`latch`] (the settings service thread) and read lock-free by
//! every writer on every core afterwards; `ON` publishes it (Release after the Once) to `a11yfilter::set`, which folds
//! it into the filter's ACTIVE flag. An identity panel (sRGB) or a refusal stores `None`: ACTIVE stays as the filter
//! alone sets it, one relaxed load per call site, as before.
//!
//! THE WIRE, once: `[display] icc src=edid:<fw|aux|none> primaries=r<x>,<y> g<x>,<y> b<x>,<y> w<x>,<y> gamma=<g x 100>
//! srgb=<0|1> -> <active|identity|refused:<why>>` (xy in thousandths). The cost rides the filter's existing
//! `[a11y] filter=… pass_us=… px=…` line (the stage runs inside `each` / `in_place`'s timed bracket).
use core::sync::atomic::{AtomicBool, Ordering::{Acquire, Release}};

use icc_core::{DisplayTransform, Refusal};
use unaos_boot_info::PixelFormat;

/// The transform, `None` when the panel is sRGB or its colour bytes were refused. Written once.
static XF: spin::Once<Option<DisplayTransform>> = spin::Once::new();
/// A non-identity transform is built.
static ON: AtomicBool = AtomicBool::new(false);

/// Build the transform from the published EDID, once; print the one line. True exactly once: on the call that built
/// an ACTIVE transform (the caller then takes its full-repaint path).
pub fn latch() -> bool {
    let mut first = false;
    XF.call_once(|| {
        first = true;
        build()
    });
    if !first || !matches!(XF.get(), Some(Some(_))) {
        return false;
    }
    ON.store(true, Release);
    true
}

/// Is a non-identity display transform live?
#[inline]
pub fn on() -> bool {
    ON.load(Acquire)
}

/// The refusal's own words.
fn why(r: Refusal) -> &'static str {
    match r {
        Refusal::Malformed(w) | Refusal::Unsupported(w) => w,
        _ => "profile",
    }
}

fn build() -> Option<DisplayTransform> {
    let src = super::edidsrc::name();
    let Some(block) = super::edid_block() else {
        serial_println!("[display] icc src=edid:{} primaries=- gamma=- srgb=- -> refused:no-edid", src);
        return None;
    };
    let e = match icc_core::edid_colour(&block) {
        Ok(e) => e,
        Err(r) => {
            serial_println!("[display] icc src=edid:{} primaries=- gamma=- srgb=- -> refused:{}", src, why(r));
            return None;
        }
    };
    let (verdict, reason, xf) = match DisplayTransform::from_edid_colour(&e) {
        Ok(x) if x.is_identity() => ("identity", "", None),
        Ok(x) => ("active", "", Some(x)),
        Err(r) => ("refused:", why(r), None),
    };
    let t = |v: f32| (v * 1000.0 + 0.5) as u32;
    let c = e.chroma;
    serial_println!(
        "[display] icc src=edid:{} primaries=r{},{} g{},{} b{},{} w{},{} gamma={} srgb={} -> {}{}",
        src, t(c[0][0]), t(c[0][1]), t(c[1][0]), t(c[1][1]), t(c[2][0]), t(c[2][1]), t(c[3][0]), t(c[3][1]),
        (e.gamma * 100.0 + 0.5) as u32, e.srgb as u8, verdict, reason
    );
    xf
}

/// Run the transform over 4-byte pixels bound for a scanout of `fmt` (`Bgr`'s little-endian word is the logical
/// `0x00RRGGBB`, so its bytes are B, G, R; `Rgb`'s are R, G, B). Other formats pass unchanged; a no-op while off.
#[inline]
pub fn rows(fmt: PixelFormat, buf: &mut [u8]) {
    if let Some(Some(x)) = XF.get() {
        match fmt {
            PixelFormat::Bgr => x.apply_bgra(buf),
            PixelFormat::Rgb => x.apply(buf),
            _ => {}
        }
    }
}

/// One logical `0x00RRGGBB` colour on the panel; the top byte passes through. The identity while off.
#[inline]
pub fn word(c: u32) -> u32 {
    match XF.get() {
        Some(Some(x)) => {
            let o = x.rgb([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            (c & 0xFF00_0000) | (o[0] as u32) << 16 | (o[1] as u32) << 8 | o[2] as u32
        }
        _ => c,
    }
}
