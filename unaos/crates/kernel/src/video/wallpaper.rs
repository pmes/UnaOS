//! WALLPAPER — a desktop background image under the compositor's flat `wm::DESKTOP_BG`.
//!
//! FINDING: every backdrop fill in the compositor (`wm::stage_fill` on the glass, `Screen::fill_screen`
//! / `paint_desktop_scene` on the desktop layer) paints ONE colour. This module holds a panel-sized
//! `0x00RRGGBB` buffer decoded by `facet::decode_file` (the viewer's own PNG pipeline: zlib, five
//! filters, box downscale) and those two fill sites read it instead of the colour where it exists.
//! A picture smaller than the panel is centred and LETTERBOXED with `DESKTOP_BG` (the buffer is seeded
//! with it); a picture larger than the panel is box-downscaled by `facet::fit`'s integer factor (never
//! upscaled: the decoder is a downscaler). File is capped at 4 MB.
//! FACETANIM (B358): a non-PNG picture goes through `pixel_core::decode_first_frame` — an animated GIF
//! or WebP is its FIRST frame, and frames 1..N are never composited.
//!
//! LOOKUP: `/home/<user>/Desktop/WALL.PNG`, then the volume root `/WALL.PNG`, through the mount table.
//! WHEN: [`poll`] runs from `Screen::flush` on the desktop layer (rate-limited, five tries) so the
//! volume having mounted late is tolerated; `login::close_into_session` re-arms it for the session
//! user's Desktop. VERB: `wallpaper <path>` reloads live, `wallpaper off` restores the colour.
//! WITNESS: `:: WALLPAPER: src=<path|none> WxH=<w>x<h> scaled=<w>x<h> letterbox=<0|1> ms=<n> -> PASS ::`.
//! The buffer lock is only ever `try_lock`ed by painters (they fall back to the flat colour for that
//! row) and the loader holds it just for a pointer swap: no heap in the paint path.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::video::wm::DESKTOP_BG;
use crate::video::framebuffer::FrameBuffer;

/// File cap: the brief's 4 MB.
const CAP: u64 = 4 * 1024 * 1024;

struct Wall {
    w: usize,
    h: usize,
    px: Vec<u32>,
}

static WALL: spin::Mutex<Option<Wall>> = spin::Mutex::new(None);
/// The desktop layer must repaint (a load or an `off` just changed the backdrop).
static STALE: AtomicBool = AtomicBool::new(false);
/// A default-path probe has succeeded or run out of tries (0..=MAX_TRIES attempts; `DONE` latches).
static TRIES: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicBool = AtomicBool::new(false);
static LAST_TRY_MS: AtomicU64 = AtomicU64::new(0);
/// `wallpaper off` was typed: the default probe must not resurrect the picture.
static OFF: AtomicBool = AtomicBool::new(false);
const MAX_TRIES: u32 = 5;
const RETRY_MS: u64 = 2000;

/// Is a picture loaded? (lock-free-ish: `try_lock`, a busy answer reads as "no" for this row.)
pub fn active() -> bool {
    match WALL.try_lock() {
        Some(g) => g.is_some(),
        None => false,
    }
}

/// Compose the wallpaper row `py`, columns `x..x+w`, into `layer` (a one-row staging framebuffer,
/// row 0). Columns the buffer cannot cover keep the flat colour already in the row.
pub fn row(layer: &FrameBuffer, py: usize, x: usize, w: usize) {
    let Some(g) = WALL.try_lock() else { return };
    let Some(wall) = g.as_ref() else { return };
    if py >= wall.h {
        return;
    }
    let src = &wall.px[py * wall.w..(py + 1) * wall.w];
    for i in 0..w {
        if let Some(&c) = src.get(x + i) {
            layer.put_pixel(i, 0, c);
        }
    }
}

/// Blit the whole buffer into `fb` (the desktop layer's back buffer). Declines on a size mismatch.
pub fn paint(fb: &FrameBuffer) {
    let Some(g) = WALL.try_lock() else { return };
    let Some(wall) = g.as_ref() else { return };
    if wall.w != fb.width() || wall.h != fb.height() {
        return;
    }
    for y in 0..wall.h {
        let src = &wall.px[y * wall.w..(y + 1) * wall.w];
        for (x, &c) in src.iter().enumerate() {
            fb.put_pixel(x, y, c);
        }
    }
}

/// `Screen::flush`'s hook: retry the default probe when due, and report (and clear) a pending
/// repaint of the desktop layer.
pub fn take_stale() -> bool {
    poll();
    crate::video::desktop_scene_owns_backdrop() && STALE.swap(false, Ordering::AcqRel)
}

/// Re-arm the default probe (a new session user has a new `~/Desktop`).
pub fn rearm() {
    OFF.store(false, Ordering::Relaxed);
    TRIES.store(0, Ordering::Relaxed);
    DONE.store(false, Ordering::Release);
}

fn poll() {
    if DONE.load(Ordering::Acquire) {
        return;
    }
    let now = crate::arch::ms() as u64;
    let last = LAST_TRY_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < RETRY_MS {
        return;
    }
    LAST_TRY_MS.store(now.max(1), Ordering::Relaxed);
    if OFF.load(Ordering::Relaxed) {
        return;
    }
    // An empty namespace COUNTS as a try (a QEMU boot with no disk must still reach the `src=none`
    // witness after MAX_TRIES * RETRY_MS); painting is what `desktop_scene_owns_backdrop` gates, not probing.
    let n = TRIES.fetch_add(1, Ordering::Relaxed) + 1;
    let found = !crate::shell::vfs_mount_table().prefixes().is_empty() && load_default();
    if found || n >= MAX_TRIES {
        DONE.store(true, Ordering::Release);
        if !found && !active() {
            witness_none();
        }
    }
}

/// `~/Desktop/WALL.PNG` then `/WALL.PNG`; the first that exists is loaded.
pub fn load_default() -> bool {
    let mut cands: Vec<String> = Vec::new();
    #[cfg(feature = "login")]
    {
        let mut nb = [0u8; crate::fs::users::NAME_MAX];
        if let Some(n) = crate::fs::users::whoami(&mut nb) {
            let mut hb = [0u8; crate::fs::users::HOME_MAX];
            if let Some(hn) = crate::fs::users::home_of(&nb[..n], &mut hb) {
                if let Ok(h) = core::str::from_utf8(&hb[..hn]) {
                    cands.push(alloc::format!("{}/Desktop/WALL.PNG", h.trim_end_matches('/')));
                }
            }
        }
    }
    cands.push(String::from("/WALL.PNG"));
    let mt = crate::shell::vfs_mount_table();
    for p in cands {
        if mt.stat(&p).is_ok() {
            return load(&p).is_ok();
        }
    }
    false
}

/// Decode `path` and make it the backdrop. Emits the witness line either way.
pub fn load(path: &str) -> Result<(), String> {
    let t0 = crate::arch::ms();
    let Some(pi) = crate::video::panel_info_nonblocking() else {
        return Err(String::from("panel busy"));
    };
    let (pw, ph) = (pi.width, pi.height);
    let (px, sw, sh, iw, ih) = match crate::video::facet::decode_file(path, CAP, pw, ph) {
        Ok(t) => t,
        Err(e) => {
            let why = e.reason();
            serial_println!(
                ":: WALLPAPER: src={} WxH=0x0 scaled=0x0 letterbox=0 ms={} reason={} -> FAIL ::",
                path, crate::arch::ms().saturating_sub(t0), why
            );
            return Err(why);
        }
    };
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(pw * ph).is_err() {
        return Err(String::from("out of memory"));
    }
    buf.resize(pw * ph, DESKTOP_BG);
    let (ox, oy) = ((pw - sw) / 2, (ph - sh) / 2);
    for y in 0..sh {
        let d = (oy + y) * pw + ox;
        buf[d..d + sw].copy_from_slice(&px[y * sw..(y + 1) * sw]);
    }
    let letterbox = (sw != pw || sh != ph) as u32;
    *WALL.lock() = Some(Wall { w: pw, h: ph, px: buf });
    OFF.store(false, Ordering::Relaxed);
    repaint();
    crate::census_println!(
        ":: WALLPAPER: src={} WxH={}x{} scaled={}x{} letterbox={} ms={} -> PASS ::",
        path, iw, ih, sw, sh, letterbox, crate::arch::ms().saturating_sub(t0)
    );
    Ok(())
}

/// Restore the flat colour.
pub fn off() {
    OFF.store(true, Ordering::Relaxed);
    *WALL.lock() = None;
    repaint();
    witness_none();
}

fn witness_none() {
    crate::census_println!(":: WALLPAPER: src=none WxH=0x0 scaled=0x0 letterbox=0 ms=0 -> PASS ::");
}

/// Both layers: the glass (queued desktop erase, drained through `stage_fill`) and the desktop
/// layer (`STALE`, serviced by the next `Screen::flush`).
fn repaint() {
    STALE.store(true, Ordering::Release);
    crate::video::wm::desktop_repaint();
}

/// Shell verb body: `wallpaper <path>` / `wallpaper off`. Returns the operator line.
pub fn cmd(arg: &str, resolved: &str) -> String {
    if arg == "off" {
        off();
        return String::from("wallpaper: off (flat desktop colour)");
    }
    match load(resolved) {
        Ok(()) => alloc::format!("wallpaper: {}", resolved),
        Err(e) => alloc::format!("wallpaper: {}: {}", resolved, e),
    }
}
