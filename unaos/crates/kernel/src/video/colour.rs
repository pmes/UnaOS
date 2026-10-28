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
//! THE LOCK. `XF` is a `spin::RwLock`, written only by [`latch`] (the settings service thread, or the a11y row press
//! that calls `a11yfilter::changed`) and held for writing only across a `mem::replace` (the old transform is dropped
//! after the guard). Every writer on every core reads it through `try_read`, never blocking: a reader that meets the
//! swap passes that chunk unconverted, and the swap's caller takes `changed`'s full repaint, which copies every pixel
//! again. `ON` publishes it (Release after the swap) to `a11yfilter::set`, which folds it into the filter's ACTIVE flag;
//! `rows` / `word` test `ON` first. An identity panel (sRGB) or a refusal stores `None`: ACTIVE stays as the filter
//! alone sets it, one relaxed load per call site, as before. `LATCHED` (the key value the live transform was built
//! under) is held only for its compare and its store; `BUSY` makes a concurrent latch return false.
//!
//! THE FILE (DISPLAYICCFILE, rmbp-ledger B972). Principia's `system.display.icc` names an installed display profile (an
//! absolute path on the volume, set by `pref set`; no Settings face yet, DISPLAY-ICC-FACE). Present and accepted by
//! icc_core (`Profile::parse`, `DisplayTransform::from_profile`: matrix/TRC only), it wins over the EDID; refused, the
//! EDID is built as before; unset, the EDID alone, its line unchanged. THE RE-LATCH: `changed` calls [`latch`] on every
//! call, and a key value that differs from the one latched (PrefChanged marks the a11y filter's dirty bit, settings.rs
//! `bus_changes_inner`, so `a11y_load("prefchanged")` makes the call; or a session switch) rebuilds and takes the full
//! repaint. The pre-login glass stays unconverted (DISPLAY-ICC-PRELOGIN owed).
//!
//! THE WIRE, once per latch: `[display] icc src=edid:<fw|aux|none> primaries=r<x>,<y> g<x>,<y> b<x>,<y> w<x>,<y>
//! gamma=<g x 100> srgb=<0|1> -> <active|identity|refused:<why>>` (xy in thousandths), or `[display] icc src=file:<path>
//! bytes=<n> desc="<desc>" -> <active|identity>`, or `[display] icc src=file:<path> bytes=<n|-> -> refused:<why>
//! fallback=edid` followed by the EDID line; a re-latch ends every line with ` relatch=1`. The cost rides the filter's
//! existing `[a11y] filter=… pass_us=… px=…` line (the stage runs inside `each` / `in_place`'s timed bracket).
use core::sync::atomic::{AtomicBool, Ordering::{AcqRel, Acquire, Release}};

use alloc::string::String;
use icc_core::{DisplayTransform, Refusal};
use unaos_boot_info::PixelFormat;

/// The transform, `None` when the panel is sRGB, its colour bytes were refused, or nothing is latched yet.
static XF: spin::RwLock<Option<DisplayTransform>> = spin::RwLock::new(None);
/// A non-identity transform is built.
static ON: AtomicBool = AtomicBool::new(false);
/// The `system.display.icc` value the live transform was built under (`None` = never latched; empty = unset).
static LATCHED: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
/// A latch is building.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Build the transform when the key's value differs from the one latched (the first call always builds); print the
/// line. True when the caller must take its full-repaint path: on the first latch, iff an ACTIVE transform was built;
/// on a re-latch, iff the old or the new transform is live.
pub fn latch() -> bool {
    if BUSY.compare_exchange(false, true, AcqRel, Acquire).is_err() {
        return false;
    }
    let key = crate::prefs::text(crate::prefs::key::DISPLAY_ICC).unwrap_or_default();
    let relatch = match LATCHED.lock().as_deref() {
        None => false,
        Some(k) if k == key => {
            BUSY.store(false, Release);
            return false;
        }
        Some(_) => true,
    };
    let xf = build(&key, if relatch { " relatch=1" } else { "" });
    let on = xf.is_some();
    let was = core::mem::replace(&mut *XF.write(), xf);
    ON.store(on, Release);
    *LATCHED.lock() = Some(key);
    BUSY.store(false, Release);
    if relatch { was.is_some() || on } else { on }
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

fn build(key: &str, tail: &str) -> Option<DisplayTransform> {
    if !key.is_empty() {
        match from_file(key) {
            Ok((n, desc, x)) => {
                let (verdict, xf) = if x.is_identity() { ("identity", None) } else { ("active", Some(x)) };
                let d = desc.map(|d| d.replace('"', "'"));
                serial_println!("[display] icc src=file:{} bytes={} desc=\"{}\" -> {}{}", key, n, d.as_deref().unwrap_or("-"), verdict, tail);
                return xf;
            }
            Err((Some(n), w)) => {
                serial_println!("[display] icc src=file:{} bytes={} -> refused:{} fallback=edid{}", key, n, w, tail);
            }
            Err((None, w)) => {
                serial_println!("[display] icc src=file:{} bytes=- -> refused:{} fallback=edid{}", key, w, tail);
            }
        }
    }
    let src = super::edidsrc::name();
    let Some(block) = super::edid_block() else {
        serial_println!("[display] icc src=edid:{} primaries=- gamma=- srgb=- -> refused:no-edid{}", src, tail);
        return None;
    };
    let e = match icc_core::edid_colour(&block) {
        Ok(e) => e,
        Err(r) => {
            serial_println!("[display] icc src=edid:{} primaries=- gamma=- srgb=- -> refused:{}{}", src, why(r), tail);
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
        "[display] icc src=edid:{} primaries=r{},{} g{},{} b{},{} w{},{} gamma={} srgb={} -> {}{}{}",
        src, t(c[0][0]), t(c[0][1]), t(c[1][0]), t(c[1][1]), t(c[2][0]), t(c[2][1]), t(c[3][0]), t(c[3][1]),
        (e.gamma * 100.0 + 0.5) as u32, e.srgb as u8, verdict, reason, tail
    );
    xf
}

/// Run the transform over 4-byte pixels bound for a scanout of `fmt` (`Bgr`'s little-endian word is the logical
/// `0x00RRGGBB`, so its bytes are B, G, R; `Rgb`'s are R, G, B). Other formats pass unchanged; a no-op while off.
#[inline]
pub fn rows(fmt: PixelFormat, buf: &mut [u8]) {
    if !on() {
        return;
    }
    let Some(g) = XF.try_read() else { return };
    if let Some(x) = g.as_ref() {
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
    if !on() {
        return c;
    }
    let Some(g) = XF.try_read() else { return c };
    let Some(x) = g.as_ref() else { return c };
    let o = x.rgb([(c >> 16) as u8, (c >> 8) as u8, c as u8]);
    (c & 0xFF00_0000) | (o[0] as u32) << 16 | (o[1] as u32) << 8 | o[2] as u32
}

// DISPLAYICCFILE (rmbp-ledger B972; BP-video-25, handlers.md §5 KERNEL-DISPLAY-ICC's DISPLAY-ICC-FILE) — the installed
// display profile `system.display.icc` names, read through the mount table every VFS client reads (wallpaper, Facet)
// and converted by icc_core alone. Tail-appended.

/// The largest display profile read (a matrix/TRC profile is a few KiB; a `vcgt` tag can add tens).
const FILE_MAX: u64 = 256 * 1024;

/// Read and build the profile at `path`: `(bytes, its desc, the transform)`, or `(bytes when known, why)` refused.
fn from_file(path: &str) -> Result<(usize, Option<String>, DisplayTransform), (Option<usize>, &'static str)> {
    if !path.starts_with('/') {
        return Err((None, "not-absolute"));
    }
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(path).map_err(|_| (None, "no-file"))?;
    if matches!(st.kind, crate::fs::vfs::NodeKind::Dir) {
        return Err((None, "eisdir"));
    }
    let n = st.size as usize;
    if st.size == 0 || st.size > FILE_MAX {
        return Err((Some(n), "size"));
    }
    let bytes = mt.read(path, 0, n).map_err(|_| (Some(n), "unreadable"))?;
    let p = icc_core::Profile::parse(&bytes).map_err(|r| (Some(n), why(r)))?;
    let x = DisplayTransform::from_profile(&p).map_err(|r| (Some(n), why(r)))?;
    Ok((bytes.len(), icc_core::profile::description(&bytes), x))
}
