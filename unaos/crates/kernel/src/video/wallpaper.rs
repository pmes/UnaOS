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
    /// DESKTOPCRAFT (B631): the VEIL — the picture box-averaged over 8 x 8 cells and blurred once
    /// (`wm::desktop::veil_gen`); every translucent strip samples it instead of re-blurring per frame.
    veil: Vec<u32>,
    vw: usize,
    vh: usize,
    /// STARFIELD (B661): the generated picture's SHIMMER (`wm::desktop::Shimmer`; boxed so its address survives the move
    /// into `WALL`; retired with the picture). `None` for a picture file.
    shim: Option<alloc::boxed::Box<crate::video::wm::desktop::Shimmer>>,
}

/// DESKTOPCRAFT (B631): the READ-ONLY snapshot the furniture painters sample (the window frame band, the menu bar,
/// the dock's plate). They paint masked and possibly on several staging cores at once, so they never take `WALL`'s
/// lock (a contended `try_lock` would flash the flat colour for a pass): they load this pointer. It names the
/// buffers of the `Wall` installed in `WALL`; when a new picture is installed the old `Wall` moves to `RETIRED` and
/// is freed only by the install AFTER that one, so a painter that loaded the old pointer finishes on live memory
/// (a paint pass is milliseconds; two installs are a decode or a generation apart). The `Snap` itself is a small
/// leaked box per install.
struct Snap {
    px: *mut u32,
    len: usize,
    w: usize,
    h: usize,
    veil: *const u32,
    vlen: usize,
    vw: usize,
    vh: usize,
    /// STARFIELD (B661): the picture's shimmer (null for a file); lives exactly as long as `px`.
    shim: *const crate::video::wm::desktop::Shimmer,
}
static SNAP: core::sync::atomic::AtomicPtr<Snap> = core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
static RETIRED: crate::sync::Mutex<Option<Wall>> = crate::sync::Mutex::new(None);
/// The installed picture is the generated default (not a file): a found `WALL.PNG` replaces it.
static PROCEDURAL: AtomicBool = AtomicBool::new(false);

/// Make `new` the backdrop (or none): publish its snapshot, then retire the old picture (see [`Snap`]).
fn install(mut new: Option<Wall>) {
    let snap = match new.as_mut() {
        Some(w) => alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Snap {
            px: w.px.as_mut_ptr(),
            len: w.px.len(),
            w: w.w,
            h: w.h,
            veil: w.veil.as_ptr(),
            vlen: w.veil.len(),
            vw: w.vw,
            vh: w.vh,
            shim: w.shim.as_deref().map_or(core::ptr::null(), |s| s as *const _),
        })),
        None => core::ptr::null_mut(),
    };
    let old = {
        let mut g = WALL.lock();
        SNAP.store(snap, Ordering::Release); #[cfg(target_arch = "x86_64")] crate::video::wm::desktop::veil_cache_invalidate();
        core::mem::replace(&mut *g, new)
    };
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))] { *RETIRED.lock() = old; } #[cfg(all(target_arch = "x86_64", feature = "wc"))] { retire(old); } veil_login_build(); // RETIREDFREE (B845): the retire stamped and retagged in one RETIRED hold, freed by `wall-reap` after the grace. LINE-NEUTRAL fold, code first
}

/// DESKTOPCRAFT (B631): the installed picture and its VEIL for the furniture painters (`None` = the flat colour).
pub fn back() -> Option<crate::video::wm::desktop::Back<'static>> {
    let p = SNAP.load(Ordering::Acquire);
    if p.is_null() {
        return None;
    }
    // SAFETY: a published `Snap` is never freed, and the buffers it names stay allocated until the second install
    // after it (see [`Snap`]); every painter's use of them ends inside its pass.
    let s = unsafe { &*p };
    if s.len < s.w * s.h || s.vlen < s.vw * s.vh {
        return None;
    }
    let (px, veil) = unsafe { (core::slice::from_raw_parts(s.px as *const u32, s.len), core::slice::from_raw_parts(s.veil, s.vlen)) };
    Some(crate::video::wm::desktop::Back { px, w: s.w, h: s.h, veil, vw: s.vw, vh: s.vh })
}

/// DESKTOPCRAFT (B631): **the default wallpaper**, generated at the panel's size (`wm::desktop::wall_gen` — STARFIELD
/// B661: dark matter on ultra black, the lensed stars, the zircon and the beam, with its shimmer list) when no picture
/// file was found. Off the paint path (the
/// default probe's worker on x86). Witness: `:: WALLPAPER: src=procedural WxH=<w>x<h> scaled=<w>x<h> letterbox=0 ms=<n> -> PASS ::`.
fn generate() -> bool {
    let Some(pi) = crate::video::panel_info_nonblocking() else { return false };
    generate_sized(pi.width, pi.height, None).is_some()
}

/// The generation itself at `pw` x `ph`; `tap` sees the finished picture before it is installed (STARTSCENE: the boot
/// splash copies it into its hold store). Returns the generator's `wall_ms`.
fn generate_sized(pw: usize, ph: usize, tap: Option<&mut dyn FnMut(&[u32])>) -> Option<u64> {
    let t0 = crate::arch::ms(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] let _ct = crate::allocator::census::enter(crate::allocator::census::WALLPAPER); // MEMLINE2 (B821): the picture is tagged for the owner census. LINE-NEUTRAL append, code first
    let epoch = crate::video::theme::epoch(); // MENUSCRAFT (B642): the accent this picture is drawn in
    let dark = crate::video::theme::is_dark(); // STARTSCENE (B662): the Light/Dark set the scene's pairs were read in
    if pw == 0 || ph == 0 {
        return None;
    }
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(pw * ph).is_err() {
        return None;
    }
    buf.resize(pw * ph, DESKTOP_BG);
    let scene = crate::video::wm::desktop::wall_gen(&mut buf, pw, ph);
    let wall_ms = crate::arch::ms().saturating_sub(t0);
    let (veil, vw, vh) = crate::video::wm::desktop::veil_gen(&buf, pw, ph)?;
    if OFF.load(Ordering::Relaxed) || (active() && !PROCEDURAL.load(Ordering::Relaxed)) {
        return None; // `wallpaper off`, or a picture file, arrived while this ran
    }
    match scene.as_ref() {
        Some(sc) => serial_println!(
            "[starfield] gen wall_ms={} stars={} lensed={} streams={} facets={} front={} glints={} groups={} surface={}x{}",
            wall_ms, sc.stars, sc.lensed, sc.streams, crate::video::wm::desktop::ZIRCON_FACETS, sc.front,
            sc.shimmer.glints.len(), sc.shimmer.groups.len(), pw, ph
        ),
        None => serial_println!("[starfield] gen wall_ms={} scene=none surface={}x{}", wall_ms, pw, ph),
    }
    if let Some(t) = tap {
        t(&buf);
    }
    if let Some(sc) = scene.as_ref() {
        *STAR_SAMPLE.lock() = (sc.sample, sc.nsample); // STARTSCENE (B662): what the GOP stage must have lit
    }
    let shim = scene.map(|sc| alloc::boxed::Box::new(sc.shimmer));
    install(Some(Wall { w: pw, h: ph, px: buf, veil, vw, vh, shim }));
    GEN_COUNT.fetch_add(1, Ordering::AcqRel); // STARTSCENE (B662)
    PROCEDURAL.store(true, Ordering::Relaxed);
    GEN_EPOCH.store(epoch, Ordering::Release); // MENUSCRAFT
    GEN_DARK.store(dark, Ordering::Release); // STARTSCENE (B662): the one theme input the scene reads
    STALE.store(true, Ordering::Release);
    crate::video::wm::desktop_repaint();
    crate::census_println!(
        ":: WALLPAPER: src=procedural WxH={}x{} scaled={}x{} letterbox=0 ms={} -> PASS ::",
        pw, ph, pw, ph, crate::arch::ms().saturating_sub(t0)
    );
    Some(wall_ms)
}

static WALL: crate::sync::Mutex<Option<Wall>> = crate::sync::Mutex::new(None);
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
    if veil_login_row(layer, py, x, w) { return; } let Some(g) = WALL.try_lock() else { return };
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
    if veil_login_paint(fb) { return; } let Some(g) = WALL.try_lock() else { return };
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
    regen_on_theme(); veil_login_witness(); // MENUSCRAFT (B642): a theme or accent change redraws the generated default and its VEIL
    poll();
    owns_backdrop() && STALE.swap(false, Ordering::AcqRel) // STARTSCENE (B662): from the splash on, not only after REALDESK's latch
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
    // LOGINRENDER (B514): the probe is two volume stats (and a decode when found) — flight 28's render task held
    // 1.3–1.9 s in `Screen::flush` with zero passes, every RETRY_MS, while the jobs worker's registry build held the
    // UnaFS volume. On x86 the try runs on a one-shot `wall-probe` task on a worker core (JOBSCAN's shape); the flush
    // only posts it and later picks up `STALE`. Inline (stamped) where there is no worker core.
    #[cfg(target_arch = "x86_64")]
    if let Some(cpu) = crate::arch::smp::worker_cpu(0) {
        if !PROBING.swap(true, Ordering::AcqRel) {
            crate::arch::sched::spawn_stack("wall-probe", probe_task, cpu, cpu, crate::arch::sched::PRIO_NORMAL, crate::arch::sched::RENDER_PATH_STACK_SIZE); // the render task's stack: the decode ran there
        }
        return;
    }
    let _w = crate::video::loginrender::stamp(crate::video::loginrender::W_WALLPAPER);
    probe(None);
}

/// LOGINRENDER (B514): a default-probe try is in flight on its worker.
#[cfg(target_arch = "x86_64")]
static PROBING: AtomicBool = AtomicBool::new(false);

/// LOGINRENDER (B514): the `wall-probe` task — one try, then exit.
#[cfg(target_arch = "x86_64")]
fn probe_task(cpu: usize) {
    crate::video::loginrender::moved(crate::video::loginrender::W_WALLPAPER);
    probe(Some(cpu));
    PROBING.store(false, Ordering::Release);
}

/// One default-probe try. `cpu` = the worker core it ran on (`None` = inline).
fn probe(cpu: Option<usize>) {
    let t0 = crate::arch::ms();
    // An empty namespace COUNTS as a try (a QEMU boot with no disk must still reach the `src=none`
    // witness after MAX_TRIES * RETRY_MS); painting is what `desktop_scene_owns_backdrop` gates, not probing.
    let n = TRIES.fetch_add(1, Ordering::Relaxed) + 1;
    let found = !crate::shell::vfs_mount_table().prefixes().is_empty() && load_default(); let found_or_made = found || (!active() && !OFF.load(Ordering::Relaxed) && generate()); let _ = found_or_made; // DESKTOPCRAFT (B631): no picture file -> the generated default, on this same try (the worker on x86); a file found on a later try replaces it
    // STARTSCENE (B662): the generated picture already installed at this panel's size (the boot splash's) is KEPT —
    // `!active()` above declines the second generation; a theme change is `regen_on_theme`'s, by epoch.
    let kept = if !found && PROCEDURAL.load(Ordering::Relaxed) && fits_panel() { " kept=generated" } else { "" };
    match cpu {
        Some(c) => serial_println!("[wallpaper] probe try={} found={}{} on=worker cpu={} ms={}", n, found as u8, kept, c, crate::arch::ms().saturating_sub(t0)),
        None => serial_println!("[wallpaper] probe try={} found={}{} on=inline ms={}", n, found as u8, kept, crate::arch::ms().saturating_sub(t0)),
    }
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
    let t0 = crate::arch::ms(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] let _ct = crate::allocator::census::enter(crate::allocator::census::WALLPAPER); // MEMLINE2 (B821): the picture is tagged for the owner census. LINE-NEUTRAL append, code first
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
    let (veil, vw, vh) = crate::video::wm::desktop::veil_gen(&buf, pw, ph).unwrap_or((Vec::new(), 0, 0)); // DESKTOPCRAFT (B631): the VEIL, once per picture
    install(Some(Wall { w: pw, h: ph, px: buf, veil, vw, vh, shim: None }));
    PROCEDURAL.store(false, Ordering::Relaxed);
    OFF.store(false, Ordering::Relaxed);
    repaint();
    crate::census_println!(
        ":: WALLPAPER: src={} WxH={}x{} scaled={}x{} letterbox={} ms={} -> PASS ::",
        path, iw, ih, sw, sh, letterbox, crate::arch::ms().saturating_sub(t0)
    );
    Ok(())
}

/// STARFIELD (B661): run `f` on the installed picture's shimmer, its buffer and size (`None` when no picture is
/// installed or it is a file). The pointers are a published `Snap`'s, so they stay valid for one paint pass (see [`Snap`]).
pub fn shimmer_with<R>(f: impl FnOnce(&crate::video::wm::desktop::Shimmer, *mut u32, usize, usize) -> R) -> Option<R> {
    let p = SNAP.load(Ordering::Acquire);
    if p.is_null() {
        return None;
    }
    // SAFETY: a published `Snap` is never freed; its buffer and shimmer live until the second install after it.
    let s = unsafe { &*p };
    if s.shim.is_null() || s.len < s.w * s.h {
        return None;
    }
    Some(f(unsafe { &*s.shim }, s.px, s.w, s.h))
}

/// Restore the flat colour.
pub fn off() {
    OFF.store(true, Ordering::Relaxed);
    install(None);
    PROCEDURAL.store(false, Ordering::Relaxed);
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

/// SESSIONWEDGE (rmbp-ledger B525, `tests sessionwedge`): default-probe tries this boot (LOGINRENDER's `wall-probe`
/// runs each on `worker_cpu(0)`, re-armed by the desktop's build at ignition).
pub fn probe_tries() -> u32 {
    TRIES.load(Ordering::Relaxed)
}

// ── MENUSCRAFT (rmbp-ledger B642) — the wallpaper and the VEIL follow the theme ────────────────────────────────────
//
// The generated default (`wm::desktop::wall_gen`) is drawn in the live accent, and the VEIL every translucent strip
// samples is built from it — so an accent (or Light/Dark) change left the old accent's picture and its blur on the
// glass. `theme::epoch` counts every change; the flush hook compares it with the epoch the installed picture was
// generated at and regenerates (picture + VEIL, one `install`) on the worker core, the probe's own path. A picture
// FILE is never regenerated: it does not depend on the theme, and its VEIL is the picture's (the tint is applied at
// sample time). Rate-limited to one try per `RETRY_MS`, so a panel that is busy does not spawn a task per flush.
// Witness: the regeneration's `:: WALLPAPER: src=procedural … -> PASS ::` line, after the theme's own.

/// The theme epoch the installed generated picture was drawn at (`u32::MAX` = none generated yet).
static GEN_EPOCH: AtomicU32 = AtomicU32::new(u32::MAX);
/// STARTSCENE (B662): the Light/Dark set the installed generated picture was drawn in.
static GEN_DARK: AtomicBool = AtomicBool::new(false);
/// The last regeneration try (ms), the rate limit.
static REGEN_MS: AtomicU64 = AtomicU64::new(0);
/// A regeneration is in flight on its worker.
static REGENERATING: AtomicBool = AtomicBool::new(false);

fn regen_on_theme() {
    if !PROCEDURAL.load(Ordering::Relaxed) || OFF.load(Ordering::Relaxed) {
        return;
    }
    let had = GEN_EPOCH.load(Ordering::Acquire);
    if had == u32::MAX || had == crate::video::theme::epoch() {
        return;
    }
    // STARTSCENE (B662) M4: the scene reads only the Light/Dark set (`SfPal::live`, never the accent), so an epoch that
    // moved only the accent or the highlight keeps the installed picture (the splash's): adopt the epoch, no second
    // generation. A Light/Dark change still regenerates.
    if GEN_DARK.load(Ordering::Acquire) == crate::video::theme::is_dark() {
        GEN_EPOCH.store(crate::video::theme::epoch(), Ordering::Release);
        serial_println!("[wallpaper] regen epoch={}->{} kept=generated reason=accent-only", had, crate::video::theme::epoch());
        return;
    }
    let now = crate::arch::ms() as u64;
    let last = REGEN_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < RETRY_MS {
        return;
    }
    if REGENERATING.swap(true, Ordering::AcqRel) {
        return;
    }
    REGEN_MS.store(now.max(1), Ordering::Relaxed);
    serial_println!("[wallpaper] regen epoch={}->{} src=procedural", had, crate::video::theme::epoch());
    #[cfg(target_arch = "x86_64")]
    if let Some(cpu) = crate::arch::smp::worker_cpu(0) {
        crate::arch::sched::spawn_stack("wall-regen", regen_task, cpu, cpu, crate::arch::sched::PRIO_NORMAL, crate::arch::sched::RENDER_PATH_STACK_SIZE);
        return;
    }
    let _ = generate();
    REGENERATING.store(false, Ordering::Release);
}

/// The `wall-regen` task — one generation, then exit.
#[cfg(target_arch = "x86_64")]
fn regen_task(_cpu: usize) {
    let _ = generate();
    REGENERATING.store(false, Ordering::Release);
}

// ── STARTSCENE (rmbp-ledger B662, R128) — ONE picture from the splash to the desktop ──────────────────────────────
//
// The boot splash's hold (`splash::hold_prepare`, on the boot core during the takeover: the heap is up, the panel size
// known, the theme live) runs THE generation and installs it here, so the hold store, the login backdrop and the
// desktop show the same pixels and the default probe keeps it (`kept=generated`). From that install the picture owns
// the backdrop (`owns_backdrop`): x86 never sets REALDESK's latch (`retire_desktop_chrome` is the Pi's and the
// Orin's), so `wm::wp_paint`, `take_stale` and the live shimmer were dark there. Design: STARFIELD.md §9.

/// Generations installed this boot (the fixture's `gen_count`).
static GEN_COUNT: AtomicU32 = AtomicU32::new(0);
/// The splash's installed picture buffer (0 = the splash generated none).
static SPLASH_PX: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// The splash installed the scene: the picture owns the backdrop from here on.
static CLAIMED: AtomicBool = AtomicBool::new(false);
/// The installed scene's 16 sampled star pixels (`wm::desktop::Scene::sample`).
static STAR_SAMPLE: crate::sync::Mutex<([(u16, u16); 16], usize)> = crate::sync::Mutex::new(([(0, 0); 16], 0));

/// STARTSCENE (B662): **the boot splash's generation** at `w` x `h` (the hold store's size): the scene is generated
/// ONCE, `tap` copies it into the hold store, and it is INSTALLED as the wallpaper. Returns the generator's `wall_ms`,
/// or `None` (a picture already installed, `wallpaper off`, or no memory) — the splash then paints its own frame.
pub fn generate_splash(w: usize, h: usize, mut tap: impl FnMut(&[u32])) -> Option<u64> {
    if OFF.load(Ordering::Relaxed) || active() {
        return None;
    }
    let ms = generate_sized(w, h, Some(&mut tap))?;
    let p = SNAP.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: a published `Snap` is never freed (see [`Snap`]).
        SPLASH_PX.store(unsafe { (*p).px } as usize, Ordering::Release);
    }
    CLAIMED.store(true, Ordering::Release);
    serial_println!("[startscene] splash gen wall_ms={} surface={}x{} on=boot", ms, w, h);
    Some(ms)
}

/// STARTSCENE (B662): does the installed picture (or the flat colour, once `off`) own the backdrop — REALDESK's latch,
/// or the splash's install. Read by `wm::wp_paint`, [`take_stale`] and the live shimmer.
pub fn owns_backdrop() -> bool {
    crate::video::desktop_scene_owns_backdrop() || CLAIMED.load(Ordering::Acquire)
}

/// The installed picture is the panel's size.
fn fits_panel() -> bool {
    let p = SNAP.load(Ordering::Acquire);
    let Some(pi) = crate::video::panel_info_nonblocking() else { return false };
    // SAFETY: a published `Snap` is never freed (see [`Snap`]).
    !p.is_null() && unsafe { (*p).w == pi.width && (*p).h == pi.height }
}

/// **`tests startscene`** (R80: a verb, never at boot) — the start scene was ONE picture: generated once (the
/// installed buffer is the one the splash installed), the splash showed it, the hold shimmered, the GOP stage's stars
/// are the picture's (all 16 sampled star pixels lit by `stars_into` on an offscreen `VOID` buffer), and the backdrop
/// under the login screen is the picture.
///
/// `:: STARTSCENE: detail installed=<0|1> held_ms=<n> lit=<n>/<m> surface=<w>x<h> ::`
/// `:: STARTSCENE: gen_count=1 splash_scene=1 gop_stars=ok hold_ticks=<n> login_backdrop=picture -> PASS ::`
pub fn startscene_selftest() {
    let gens = GEN_COUNT.load(Ordering::Acquire);
    let p = SNAP.load(Ordering::Acquire);
    // SAFETY: a published `Snap` is never freed (see [`Snap`]).
    let (inst, w, h) = if p.is_null() { (0usize, 0usize, 0usize) } else { unsafe { ((*p).px as usize, (*p).w, (*p).h) } };
    let splash_px = SPLASH_PX.load(Ordering::Acquire);
    let installed = splash_px != 0 && inst == splash_px && PROCEDURAL.load(Ordering::Relaxed);
    let gen_ok = gens == 1 && installed;
    let splash_scene = crate::splash::scene_painted();
    let (ticks, held_ms) = crate::splash::hold_stats();
    let ticks_ok = held_ms < 66 || ticks > 0;
    let (sample, ns) = *STAR_SAMPLE.lock();
    let mut lit = 0usize;
    let void = crate::video::wm::desktop::scene_void();
    if w * h > 0 && ns > 0 {
        let mut buf: Vec<u32> = Vec::new();
        if buf.try_reserve_exact(w * h).is_ok() {
            buf.resize(w * h, void);
            if crate::video::wm::desktop::stars_into(&mut buf, w, h, crate::video::wm::desktop::STAR_SEED).is_some() {
                let d = |a: u32, b: u32| [16u32, 8, 0].iter().map(|&s| (((a >> s) & 255) as i32 - ((b >> s) & 255) as i32).unsigned_abs()).sum::<u32>();
                lit = sample[..ns.min(16)].iter().filter(|&&(x, y)| d(buf[y as usize * w + x as usize], void) >= 12).count();
            }
        }
    }
    let gop_ok = ns == 16 && lit == ns;
    let backdrop = owns_backdrop() && active();
    let ok = gen_ok && splash_scene && ticks_ok && gop_ok && backdrop;
    serial_println!(":: STARTSCENE: detail installed={} held_ms={} lit={}/{} surface={}x{} ::", installed as u8, held_ms, lit, ns, w, h);
    serial_println!(
        ":: STARTSCENE: gen_count={} splash_scene={} gop_stars={} hold_ticks={} login_backdrop={} -> {} ::",
        gens, splash_scene as u8, if gop_ok { "ok" } else { "FAIL" }, ticks, if backdrop { "picture" } else { "flat" }, if ok { "PASS" } else { "FAIL" }
    );
}

// RETIREDFREE (rmbp-ledger B845, BP-video-18, MEMLINE3) — THE RETIRED PICTURE FREED AFTER ITS GRACE. Until this arc the
// picture an install retired was freed only by the install after next (two panel-sized pictures held). [`retire`]
// assigns RETIRED (the older one is freed there, as before), retags its two buffers `wallpaper-retired` for the owner
// census and stamps `RETIRED_AT`, all in ONE `RETIRED` hold; then a one-shot `wall-reap` task on a worker core (the
// `probe_task`/`regen_task` shape) sleeps to [`RETIRED_GRACE_MS`] and frees the picture — taken under `RETIRED` only
// while the stamp is the one it slept on, so a newer retire (a regen or a load installing meanwhile) restarts the wait
// instead. The grace is longer than the 1500 ms W5 gate steal: a masked painter that loaded the old `Snap` has finished
// its pass, or its core is declared dead. No worker core (or not in a task): today's behaviour. x86 `wc` only.
// Wire: `[memline] retired freed age_ms=<n>`.
/// RETIREDFREE (B845): how long a retired picture stays allocated after its retire.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub const RETIRED_GRACE_MS: u64 = 2_000;
/// RETIREDFREE (B845): when the held RETIRED picture was retired (ms, 0 = none held).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static RETIRED_AT: AtomicU64 = AtomicU64::new(0);
/// RETIREDFREE (B845): retired pictures the reaper freed this boot.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static RETIRED_FREED: AtomicU32 = AtomicU32::new(0);
/// RETIREDFREE (B845): a `wall-reap` task is pending.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
static REAPING: AtomicBool = AtomicBool::new(false);

/// RETIREDFREE (B845): [`install`]'s retire — assign, retag, stamp under one `RETIRED` hold; then arm the reaper.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn retire(old: Option<Wall>) {
    use crate::allocator::census;
    let held = old.is_some();
    {
        let mut g = RETIRED.lock();
        *g = old;
        if let Some(w) = g.as_ref() {
            census::retag(w.px.as_ptr() as usize, w.px.capacity() * core::mem::size_of::<u32>(), census::WALLPAPER_RETIRED);
            census::retag(w.veil.as_ptr() as usize, w.veil.capacity() * core::mem::size_of::<u32>(), census::WALLPAPER_RETIRED);
        }
        RETIRED_AT.store(if held { crate::arch::ms().max(1) } else { 0 }, Ordering::Release);
    }
    if !held || crate::arch::sched::current_id().is_none() || REAPING.swap(true, Ordering::AcqRel) {
        return;
    }
    match crate::arch::smp::worker_cpu(0) {
        Some(cpu) => crate::arch::sched::spawn("wall-reap", reap_task, cpu, cpu, crate::arch::sched::PRIO_NORMAL),
        None => REAPING.store(false, Ordering::Release),
    }
}

/// RETIREDFREE (B845): the `wall-reap` task — sleep to the grace, free the picture it slept on, then exit.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn reap_task(_cpu: usize) {
    loop {
        let at = RETIRED_AT.load(Ordering::Acquire);
        if at != 0 {
            let age = (crate::arch::ms() as u64).saturating_sub(at);
            if age < RETIRED_GRACE_MS {
                crate::arch::sched::sleep_ms(RETIRED_GRACE_MS - age);
                continue;
            }
            let old = {
                let mut g = RETIRED.lock();
                if RETIRED_AT.load(Ordering::Acquire) == at {
                    RETIRED_AT.store(0, Ordering::Release);
                    g.take()
                } else {
                    None
                }
            };
            if old.is_some() {
                RETIRED_FREED.fetch_add(1, Ordering::Relaxed);
                drop(old);
                serial_println!("[memline] retired freed age_ms={}", age);
            }
            continue;
        }
        REAPING.store(false, Ordering::Release);
        if RETIRED_AT.load(Ordering::Acquire) == 0 || REAPING.swap(true, Ordering::AcqRel) {
            return;
        }
    }
}

/// RETIREDFREE (B845): `(age ms of the held RETIRED picture, None = none held; pictures freed this boot)`.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn retired_state() -> (Option<u64>, u32) {
    let at = RETIRED_AT.load(Ordering::Acquire);
    (if at == 0 { None } else { Some((crate::arch::ms() as u64).saturating_sub(at)) }, RETIRED_FREED.load(Ordering::Relaxed))
}

// ── VEIL-LOGIN (rmbp-ledger B1116, BP-video-11) — TAIL-APPENDED: the login and lock backdrop is the VEIL ───────────────
// DESIGN-LANGUAGE §3.9 ("Background: the VEIL of the wallpaper (blurred, darkened 20 %)") and §2.4 `elev.4` (the scrim
// behind the login form). While the login card's row is up (`boot::screen_is_up`, set and cleared only by `login::open_as`
// / `take_down`: the login, unlock, words and lock screens) the desktop layer's two leaves, [`row`] and [`paint`], take
// the backdrop from `wm::desktop::login_put` (the field kept per install, `Look::login` over the VEIL) instead of the
// sharp picture. The field is composed once per [`install`] ([`veil_login_build`], on the installer's thread).
// Wire: `[veil] login cost build_us=<n> blit_us=<n> panel=<w>x<h> bytes=<n> a=<n> kept=<yes|half|no>` per install; once,
// `:: VEILLOGIN: at=(<x>,<y>) sharp=#<rgb> glass=#<rgb> expect=#<rgb> a=<n> cached=<0|1> err=<n> -> PASS|FAIL ::` at the first
// flush after the first painted pixel that differs from the sharp one (PASS: glass = the VEIL at `a`, err <= 2 off x86: B1122).

/// The witness latch: taken by the first painted pixel that differs from the picture's; `READY` once its values are in.
static LV_LATCHED: AtomicBool = AtomicBool::new(false);
static LV_READY: AtomicBool = AtomicBool::new(false);
static LV_SAID: AtomicBool = AtomicBool::new(false);
static LV_CACHED: AtomicBool = AtomicBool::new(false);
/// `(x << 32) | y` and `(sharp << 32) | glass` of the latched pixel.
static LV_AT: AtomicU64 = AtomicU64::new(0);
static LV_PX: AtomicU64 = AtomicU64::new(0);

/// Panel row `py`, columns `x..x + w`, of the login backdrop into `fb`'s row `fy`: `true` when the login backdrop owns
/// the row (the screen is up and a picture is installed), so the sharp picture is not painted.
fn veil_login_put(fb: &FrameBuffer, fy: usize, py: usize, x: usize, w: usize) -> bool {
    if !crate::boot::screen_is_up() {
        return false;
    }
    let Some(b) = back() else { return false };
    let latch = !LV_LATCHED.load(Ordering::Relaxed);
    let mut hit: Option<(usize, u32, u32)> = None;
    let cached = crate::video::wm::desktop::login_put(&b, py, x, w, |i, p| {
        fb.put_pixel(i, fy, p);
        if latch && hit.is_none() {
            let s = b.get(x + i, py);
            if s != p {
                hit = Some((x + i, s, p));
            }
        }
    });
    if let Some((hx, s, p)) = hit {
        if !LV_LATCHED.swap(true, Ordering::AcqRel) {
            LV_AT.store(((hx as u64) << 32) | py as u64, Ordering::Relaxed);
            LV_PX.store(((s as u64) << 32) | p as u64, Ordering::Relaxed);
            LV_CACHED.store(cached, Ordering::Relaxed);
            LV_READY.store(true, Ordering::Release);
        }
    }
    true
}

/// [`row`]'s login branch: the staged row is `layer`'s row 0.
fn veil_login_row(layer: &FrameBuffer, py: usize, x: usize, w: usize) -> bool {
    veil_login_put(layer, 0, py, x, w)
}

/// [`paint`]'s login branch: the whole back buffer, on a size match (as [`paint`] declines otherwise).
fn veil_login_paint(fb: &FrameBuffer) -> bool {
    if !crate::boot::screen_is_up() {
        return false;
    }
    let Some(b) = back() else { return false };
    if b.w != fb.width() || b.h != fb.height() {
        return false;
    }
    for y in 0..b.h {
        veil_login_put(fb, y, y, 0, b.w);
    }
    true
}

/// [`install`]'s tail: compose the login field for the new picture (or free it when the picture went), print its cost,
/// and re-arm the witness.
fn veil_login_build() {
    let Some(b) = back() else {
        crate::video::wm::desktop::login_veil_drop();
        return;
    };
    let via = if LV_VIA.swap(0, Ordering::AcqRel) == 1 { "raise" } else { "install" }; // VEIL-LOGIN-HELD (B1128): who asked
    if via == "install" && LV_OWED.load(Ordering::Acquire) && !crate::boot::screen_is_up() {
        serial_println!("[veil] login owed at=install"); // VEIL-LOGIN-HELD (B1128): dropped while no screen is up; the next raise builds
        return;
    }
    LV_OWED.store(false, Ordering::Release); veil_login_rebuild_clear(); // VEIL-HELD-REPREF (B1135): this build composes at the current look, so an owed svc rebuild is moot
    let (build_us, blit_us, bytes) = crate::video::wm::desktop::login_prefill(&b);
    let raise_us = build_us.saturating_add(blit_us); // VEIL-LOGIN-HELD (B1128): what a rebuild at the raise costs
    LV_RAISE_US.store(raise_us, Ordering::Relaxed);
    LV_HELD.store(raise_us > RAISE_BUDGET_US, Ordering::Release);
    LV_READY.store(false, Ordering::Relaxed);
    LV_SAID.store(false, Ordering::Relaxed);
    LV_LATCHED.store(false, Ordering::Release);
    serial_println!(
        "[veil] login cost build_us={} blit_us={} panel={}x{} bytes={} a={} kept={} held={} at={}",
        build_us, blit_us, b.w, b.h, bytes, crate::video::crystal::veil::Look::login().a, // VEIL-REDUCE-ROW (B1128): the applied alpha
        if bytes == 0 { "no" } else if cfg!(target_arch = "x86_64") { "yes" } else { "half" }, // VEIL-LOGIN-PI (B1122)
        if raise_us > RAISE_BUDGET_US { "yes" } else { "no" }, via // VEIL-LOGIN-HELD (B1128)
    );
}

/// [`take_stale`]'s tail (the flush, IRQs on): the latched pixel against the VEIL at the login alpha, once per build.
fn veil_login_witness() {
    if !LV_READY.load(Ordering::Acquire) || LV_SAID.swap(true, Ordering::AcqRel) {
        return;
    }
    let Some(b) = back() else { return };
    let at = LV_AT.load(Ordering::Relaxed);
    let (x, y) = ((at >> 32) as usize, (at & 0xFFFF_FFFF) as usize);
    let px = LV_PX.load(Ordering::Relaxed);
    let (sharp, glass) = ((px >> 32) as u32, px as u32);
    let a = crate::video::crystal::veil::Look::login().a; // VEIL-REDUCE-ROW (B1128): 255 under Reduce transparency
    let mut s = [0u32; 1];
    b.soft(&mut s, x, y);
    let expect = crate::video::draw::blend(s[0], 0, a as u32);
    let err = veil_login_err(glass, expect); let tol = if cfg!(target_arch = "x86_64") { 0 } else { 2 }; let ok = err <= tol && glass != sharp; // VEIL-LOGIN-PI (B1122): the half field is exact to <= 2 LSB
    serial_println!(
        ":: VEILLOGIN: at=({},{}) sharp=#{:06x} glass=#{:06x} expect=#{:06x} a={} cached={} err={} -> {} ::",
        x, y, sharp, glass, expect, a, LV_CACHED.load(Ordering::Relaxed) as u8, err, if ok { "PASS" } else { "FAIL" }
    );
}

// ── VEIL-LOGIN-PI (rmbp-ledger B1122, BP-video-11) — TAIL-APPENDED ─────────────────────────────────────────────────────
/// The witness's error: the largest per-channel difference of two pixels (0 on x86's full field; <= 2 from aarch64's
/// half field, `wm::desktop::login_half_put`).
fn veil_login_err(a: u32, b: u32) -> u32 {
    let d = |s: u32| ((a >> s) & 255).abs_diff((b >> s) & 255);
    d(16).max(d(8)).max(d(0))
}

// ── VEIL-LOGIN-HELD (rmbp-ledger B1128, BP-video-11) — TAIL-APPENDED: the login field is freed while no screen is up ──
// The field (x86 full, aarch64 half) stays allocated only when its rebuild could not hide under the raise: each build
// measures `build_us + blit_us` (the cost line's own numbers); above the 20 ms route budget the field is HELD through
// a down (x86 2880x1800: ~72.6 ms by B1116's arithmetic), at or under it the field is dropped at `login::take_down`
// ([`veil_login_down`]) and rebuilt by [`veil_login_build`] at `login::open_as` ([`veil_login_raise`]) before the
// screen is marked up, so the raise's own paint copies the kept rows. A moved Reduce transparency pref drops it too
// ([`veil_login_stale`]): the next build composes at the new alpha (`veil_key` is unchanged).
// Wire: `[veil] login down held=<yes|no> bytes=<n> raise_us=<n> budget_us=20000` per down; the cost line's
// ` held=<yes|no> at=<install|raise>`; `:: VEILLOGIN:` once per raise (re-armed at every raise).

/// The route budget the raise's rebuild must fit (the 20 ms a key or press route is held to).
const RAISE_BUDGET_US: u64 = 20_000;
/// The last built field survives a down (its rebuild is over [`RAISE_BUDGET_US`]).
static LV_HELD: AtomicBool = AtomicBool::new(true);
/// The field was dropped (a down, a moved pref): the next raise rebuilds it.
static LV_OWED: AtomicBool = AtomicBool::new(false);
/// `build_us + blit_us` of the last build.
static LV_RAISE_US: AtomicU64 = AtomicU64::new(0);
/// Who calls [`veil_login_build`] next: 0 = `install`, 1 = the raise.
static LV_VIA: AtomicU32 = AtomicU32::new(0);

/// `login::take_down`, after the screen is marked down: free the field unless its rebuild would not fit the raise.
pub fn veil_login_down() {
    let bytes = crate::video::wm::desktop::login_veil_bytes();
    let held = LV_HELD.load(Ordering::Acquire);
    if !held && bytes > 0 {
        crate::video::wm::desktop::login_veil_drop();
        LV_OWED.store(true, Ordering::Release);
    }
    serial_println!(
        "[veil] login down held={} bytes={} raise_us={} budget_us={}",
        if held { "yes" } else { "no" }, bytes, LV_RAISE_US.load(Ordering::Relaxed), RAISE_BUDGET_US
    );
}

/// `login::open_as`, before the screen is marked up: re-arm the witness, and rebuild a dropped field (the cost line
/// printed again, `at=raise`) while no painter reads it.
pub fn veil_login_raise() {
    LV_READY.store(false, Ordering::Relaxed);
    LV_SAID.store(false, Ordering::Relaxed);
    LV_LATCHED.store(false, Ordering::Release);
    if LV_OWED.load(Ordering::Acquire) {
        LV_VIA.store(1, Ordering::Release);
        veil_login_build();
    }
}

/// The Reduce transparency pref moved (`crystal::login_reduce_load`): drop the field; the next raise rebuilds it.
pub fn veil_login_stale() {
    if veil_login_rebuild_defer() { return; } // VEIL-HELD-REPREF (B1135): a held field is kept and recomposed on the x86 device-service pass (file tail)
    crate::video::wm::desktop::login_veil_drop();
    LV_OWED.store(true, Ordering::Release);
}

// ── VEIL-HELD-REPREF (rmbp-ledger B1135, BP-video-11) — TAIL-APPENDED: a held field is recomposed off the raise ──────
// VEIL-LOGIN-HELD (B1128) dropped even a HELD field on a Reduce transparency move, so the next raise paid the whole
// build inside its route (x86 2880x1800 ~72.6 ms). Now, on x86, a held field with bytes and nothing owed is KEPT and
// owed to the device-service pass (`wm::pace_service` -> [`veil_login_svc`], after B1129's `close_owed_run`): ONE cell,
// [`LV_REBUILD`] = `(gen << 32) | next_row` (row `u32::MAX` = idle; one relaxed load when idle). The pass recomposes the
// kept rows IN PLACE (`wm::desktop::login_refill_rows`, one row per `LOGIN_VEIL` hold, so a painter sees each row old or
// new, never torn) in slices of `max(1, 131072 / w)` rows (2880 px: 45 rows ~1.8 ms), so the ~1 kHz lane is never held
// for the whole build; a raise meanwhile blits whatever is kept (LV_OWED stays false: the raise does not build). A move
// mid-rebuild bumps `gen` and the pass's CAS fails: the next slice restarts at row 0. The last slice reuses
// `login_prefill` for the blit timing and bytes (every row a hit), stores the raise cost as [`veil_login_build`] does,
// re-arms the witness and prints the cost line with `at=svc`. A field not held, or a contended byte count, keeps
// B1128's drop; aarch64 (no `pace_service`) runs the same slices on a one-shot task (VEIL-HELD-ARM, file tail). Wire: `[veil] login cost … at=svc`; a field re-keyed or dropped under the
// pass (an install, `wallpaper off`) leaves it idle: `[veil] login rebuild left at=svc row=<y>`.

/// The owed in-place rebuild: `(gen << 32) | next_row`; row [`LV_RB_IDLE`] = nothing owed.
static LV_REBUILD: AtomicU64 = AtomicU64::new(LV_RB_IDLE as u64);
/// The idle row sentinel.
const LV_RB_IDLE: u32 = u32::MAX;
/// The rebuild's accumulated compose time (the cost line's `build_us=`), written only by the pass.
static LV_RB_US: AtomicU64 = AtomicU64::new(0);

/// [`veil_login_stale`]'s head: a HELD field with bytes and nothing owed is kept and its rebuild owed to the pass (`true`):
/// x86's `pace_service`, or on aarch64 (VEIL-HELD-ARM, B1141) ONE `veil-rebuild` task on `sched::worker_cpu_apart`,
/// spawned only when the cell was idle (a runner already holds an owed cell); anything else, or no free core on
/// aarch64, answers `false` (B1128's drop).
fn veil_login_rebuild_defer() -> bool {
    if !(LV_HELD.load(Ordering::Acquire) && !LV_OWED.load(Ordering::Acquire) && crate::video::wm::desktop::login_veil_bytes() > 0) {
        return false;
    }
    #[cfg(target_arch = "aarch64")]
    let Some(cpu) = crate::arch::sched::worker_cpu_apart() else { return false };
    let prev = match LV_REBUILD.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| Some((((v >> 32) + 1) << 32) & !0xFFFF_FFFF)) {
        Ok(v) | Err(v) => v,
    };
    #[cfg(target_arch = "aarch64")]
    {
        if prev as u32 == LV_RB_IDLE {
            let _ = crate::arch::sched::spawn("veil-rebuild", veil_rebuild_task, 0, cpu);
        }
    }
    let _ = prev;
    true
}

/// [`veil_login_build`]'s clear: an owed rebuild is moot after a build at the current look (the gen moves, so a slice
/// in flight fails its CAS).
fn veil_login_rebuild_clear() {
    let _ = LV_REBUILD.fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| Some(((((v >> 32) + 1) << 32) & !0xFFFF_FFFF) | LV_RB_IDLE as u64));
}

/// `wm::pace_service`'s taker (x86 `wc`, after `close_owed_run`) and the aarch64 `veil-rebuild` task's body (VEIL-HELD-ARM,
/// B1141): one relaxed load when nothing is owed; else one bounded slice of the in-place rebuild, and on the last slice
/// the cost line with `at=svc`. Answers whether the cell is still owed (`false` only when it was idle or this call's own
/// CAS left it idle), so the aarch64 runner loops to completion and a later owing finds an idle cell and spawns anew.
pub fn veil_login_svc() -> bool {
    let v = LV_REBUILD.load(Ordering::Relaxed);
    if v as u32 == LV_RB_IDLE {
        return false;
    }
    let v = LV_REBUILD.load(Ordering::Acquire);
    let y0 = v as u32;
    if y0 == LV_RB_IDLE {
        return false;
    }
    let g0 = v & !0xFFFF_FFFF;
    let Some(b) = back() else {
        return LV_REBUILD.compare_exchange(v, g0 | LV_RB_IDLE as u64, Ordering::AcqRel, Ordering::Acquire).is_err();
    };
    if y0 == 0 {
        LV_RB_US.store(0, Ordering::Relaxed);
    }
    let (fw, fh) = if cfg!(target_arch = "x86_64") { (b.w, b.h) } else { (b.w / 2 + 1, b.h / 2 + 1) }; // VEIL-HELD-ARM (B1141): aarch64 keeps the half field (B1122)
    let n = (131_072 / fw.max(1)).max(1);
    let Some((next, us)) = crate::video::wm::desktop::login_refill_rows(&b, y0 as usize, n) else {
        if LV_REBUILD.compare_exchange(v, g0 | LV_RB_IDLE as u64, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            serial_println!("[veil] login rebuild left at=svc row={}", y0);
            return false;
        }
        return true;
    };
    let build_us = LV_RB_US.load(Ordering::Relaxed).saturating_add(us);
    LV_RB_US.store(build_us, Ordering::Relaxed);
    if next < fh {
        let _ = LV_REBUILD.compare_exchange(v, g0 | next as u64, Ordering::AcqRel, Ordering::Acquire);
        return true;
    }
    if LV_REBUILD.compare_exchange(v, g0 | LV_RB_IDLE as u64, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return true; // the pref moved during the last slice: the next pass restarts at row 0
    }
    let (_, blit_us, bytes) = crate::video::wm::desktop::login_prefill(&b);
    let raise_us = build_us.saturating_add(blit_us);
    LV_RAISE_US.store(raise_us, Ordering::Relaxed);
    LV_HELD.store(raise_us > RAISE_BUDGET_US, Ordering::Release);
    LV_READY.store(false, Ordering::Relaxed);
    LV_SAID.store(false, Ordering::Relaxed);
    LV_LATCHED.store(false, Ordering::Release);
    serial_println!(
        "[veil] login cost build_us={} blit_us={} panel={}x{} bytes={} a={} kept={} held={} at=svc",
        build_us, blit_us, b.w, b.h, bytes, crate::video::crystal::veil::Look::login().a,
        if bytes == 0 { "no" } else if cfg!(target_arch = "x86_64") { "yes" } else { "half" }, if raise_us > RAISE_BUDGET_US { "yes" } else { "no" }
    );
    false
}

// ── VEIL-HELD-ARM (rmbp-ledger B1141, BP-video-11) — TAIL-APPENDED: aarch64's held half field recomposed off the raise ─
// aarch64 has no `pace_service`, so B1135 kept B1128's drop there: a Reduce transparency move freed a HELD half field
// and the next raise paid its whole build in the route. Now [`veil_login_rebuild_defer`] owes it to ONE `veil-rebuild`
// task on `sched::worker_cpu_apart` (B1136's one-shot shape: c2 on the Pi, c1 on the Orin; no free core keeps the drop),
// spawned only when [`LV_REBUILD`] was idle, whose body runs [`veil_login_svc`]'s slices to completion and exits. The
// slices are half rows (`wm::desktop::login_refill_rows`' aarch64 arm, `LOGIN_VEIL` held one row at a time): 1920x1200
// (961 x 601, 2,310,244 B) is 136 rows a slice, 5 slices; 640x480 is one slice and never held. No periodic pass
// (ARM-PACE-PASS owed). Wire: `[veil] login cost … kept=half … at=svc` from that core.

/// VEIL-HELD-ARM: the one-shot runner — the owed rebuild's slices until the cell is idle, then it exits.
#[cfg(target_arch = "aarch64")]
fn veil_rebuild_task(_: usize) {
    while veil_login_svc() {}
}
