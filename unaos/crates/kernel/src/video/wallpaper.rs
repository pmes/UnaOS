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
    *RETIRED.lock() = old;
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
    let t0 = crate::arch::ms();
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
    regen_on_theme(); // MENUSCRAFT (B642): a theme or accent change redraws the generated default and its VEIL
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
