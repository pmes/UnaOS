#![allow(long_running_const_eval)] // CLOCKPAINTERS (B719): the compile-time source counts walk whole painter files
//! R77 M3 — THE TEST SUITE IS A COMMAND.
//!
//! Every fixture that drives the desktop, the input router or the login screen used to ignite as a
//! boot side-effect, so a first boot with no root password ran the whole press battery beneath the
//! modal set-password screen and every press fixture went red (boot 16). A fixture now REGISTERS
//! itself here instead of running; the operator fires the registry from the desktop shell with
//! `tests` (all) or `tests <name>`, which tells a boot problem from a test problem.
//!
//! * Default: `register` stores the fixture; the boot prints ONE line
//!   `:: TESTS: deferred=<n> fire=tests ::` once every registering source has been through.
//! * `tests-at-boot` (`UNAOS_TESTS_AT_BOOT=1`, the QEMU lanes' default): `register` RUNS the fixture on
//!   the spot — the call site is the old call site, so the lanes see the old order — and the boot line
//!   reads `deferred=0`.
//! * `tests` / `tests <name>` / `tests list`: refused until `fs::users::desktop_allowed()` (the
//!   installer's gate — no test runs beneath the set-password or create-user screens).
//!
//! Pass/fail is the serial verdict tap's own count (`selftest::capture` calls [`tally`] for every
//! `-> PASS` / `-> FAIL` line), so the total is exactly what the fixtures printed.
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::console::Console;

/// Registry capacity — a full table is loud (`:: TESTS: table full … -> FAIL ::`), never silent.
const CAP: usize = 160; // BATTLIVE (B492): 128 -> 160, flight 26 filled it (`table full (cap=128)` — selfdiag, srcextract NOT registered). // QUIETBOOT2: 80 -> 128, the ~20 boot fixtures B325 moved here. // QUIETBOOT: 48 -> 80, the boot witnesses R80 moved here (flight 19 registered 45).

static TABLE: crate::sync::Mutex<alloc::vec::Vec<Option<(&'static str, fn())>>> = crate::sync::Mutex::new(alloc::vec::Vec::new()); // SMALLFIX6 (B495, R90): heap-grown — flight 26 printed `table full (cap=128)` and dropped `selfdiag`, `srcextract`
static DEFERRED: AtomicUsize = AtomicUsize::new(0);
static AT_BOOT: AtomicUsize = AtomicUsize::new(0);
static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Registering sources whose completion the boot line waits for: the desktop battery (x86 witness
/// images) and the loginst chain. Other registrations (early input fixtures) precede both.
pub const SRC_DESK: u32 = 1;
pub const SRC_LOGIN: u32 = 2;
const REQUIRED: u32 = (if cfg!(all(target_arch = "x86_64", feature = "witness")) { SRC_DESK } else { 0 })
    | (if cfg!(feature = "loginst") { SRC_LOGIN } else { 0 });
static SOURCES_DONE: AtomicU32 = AtomicU32::new(0);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);

/// Called by the verdict tap for every fixture verdict line.
pub fn tally(pass: bool) { if !ANNOUNCED.load(Ordering::Relaxed) && !BOOT_OVER.load(Ordering::Relaxed) { BOOT_VERDICTS.fetch_add(1, Ordering::Relaxed); }
    if pass { PASS.fetch_add(1, Ordering::Relaxed); } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        // TESTFIX2 — remember WHICH fixture failed (the one `run` is executing), de-duplicated, for the summary line.
        if let (Some(cur), Some(mut fl)) = (CUR.try_lock(), FAILED.try_lock()) {
            let n = *cur;
            if !n.is_empty() && !fl.iter().flatten().any(|x| *x == n) {
                if let Some(slot) = fl.iter_mut().find(|x| x.is_none()) { *slot = Some(n); }
            }
        }
    }
}

/// TESTFIX2 — the fixture `run` is executing now, and the names of those that printed a FAIL this run.
static CUR: crate::sync::Mutex<&'static str> = crate::sync::Mutex::new("");
static FAILED: crate::sync::Mutex<[Option<&'static str>; 16]> = crate::sync::Mutex::new([None; 16]);

/// How many fixtures are parked behind the verb.
pub fn deferred_count() -> usize { DEFERRED.load(Ordering::Relaxed) }

/// Register a fixture. Deferred by default; run at once under `tests-at-boot`.
pub fn register(name: &'static str, f: fn()) {
    #[cfg(feature = "tests-at-boot")]
    {
        AT_BOOT.fetch_add(1, Ordering::Relaxed);
        f();
        let _ = name;
    }
    #[cfg(not(feature = "tests-at-boot"))]
    {
        // PREHEAP (flight 27, image 20): SMALLFIX6 made TABLE a Vec, and the stack guard registers `stackroom` (and
        // `lockreg`) at arm time — BEFORE `KERNEL HEAP ALLOCATED`. The first push asked the allocator for 96 bytes it
        // did not have: `memory allocation of 96 bytes failed`, the panic screen, a 1 s restart loop, nothing on the
        // wire (the probe on image 19's knobs panicked the same way: the tree, not a knob). Until the heap is up a
        // registration parks in this fixed stash; the first post-heap registration drains it into the table.
        // SMALLFIX7 (B501): the admission is `midden_core::fixtures::admit`, the one rule the host test runs.
        use midden_core::fixtures::{admit, Admit, Stash};
        static PRE: crate::sync::Mutex<Stash<(&'static str, fn()), 8>> = crate::sync::Mutex::new(Stash::new());
        let heap_up = crate::allocator::heap_bounds() != (0, 0);
        let mut t = if heap_up { Some(TABLE.lock()) } else { None };
        if let Some(t) = t.as_ref() { if t.iter().flatten().any(|e| e.0 == name) { return; } } // SOUNDOPENERS (B500): one entry per name — a fixture registered from two paths runs once
        let mut p = PRE.lock();
        let got = admit(&mut p, heap_up, (name, f), |e| {
            if let Some(t) = t.as_mut() {
                match t.iter_mut().find(|s| s.is_none()) {
                    Some(slot) => *slot = Some(e),
                    None => t.push(Some(e)), // SMALLFIX6 (B495, R90): no fixed cap — the table grows
                }
            }
        });
        match got {
            Admit::Full => serial_println!(":: TESTS: pre-heap stash full (8) — `{}` NOT registered -> FAIL ::", name),
            Admit::Parked | Admit::Tabled { .. } => { DEFERRED.fetch_add(1, Ordering::Relaxed); }
        }
    }
}

/// A registering source has finished; once all required sources have, print the boot line ONCE.
pub fn source_done(bit: u32) {
    ensure_shellux();
    let done = SOURCES_DONE.fetch_or(bit, Ordering::AcqRel) | bit;
    if done & REQUIRED == REQUIRED && !ANNOUNCED.swap(true, Ordering::AcqRel) {
        serial_println!(":: TESTS: deferred={} fire=tests at_boot={} ::", deferred_count(), AT_BOOT.load(Ordering::Relaxed));
    }
}

/// Run one named fixture (`Some`) or all (`None`); returns how many ran.
pub fn run(name: Option<&str>) -> usize {
    ensure_shellux(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] crate::sessionwedge::ensure_tests(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] crate::arch::sched::gatesteal2_ensure_tests(); // GATESTEAL (B657): `tests gatesteal` (and sessionwedge/setterwedge, which SKIP without an ignition) are reachable on a leg with no session ignition — the lazy registration rode the render task's desktop-phase roll (same-line fold) GATESTEAL2 (B667): `tests gatesteal2` beside it, same cfg, same reason (same-line fold, code first).
    if RUNNING.swap(true, Ordering::AcqRel) {
        serial_println!(":: TESTS: already running — refused ::");
        return 0;
    }
    BOOT_OVER.store(true, Ordering::Release); let (p0, f0) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed)); // SMALLFIX7 (B501): a `tests` run ends the boot's verdict window
    *FAILED.lock() = [None; 16]; skip_reset(); RESULTS.lock().clear(); // QUIETBOOT3 (B352): same-line fold.
    let mut ran = 0usize;
    let mut i = 0usize;
    loop {
        // Copy the entry out so the table lock is NOT held across the fixture (fixtures print, spin, and may register nothing).
        let ent = { let t = TABLE.lock(); t.get(i).copied().flatten() };
        let Some((n, f)) = ent else { break };
        i += 1;
        if let Some(want) = name { if want != n { continue; } }
        serial_println!(":: TESTS: run {} ::", n);
        *CUR.lock() = n;
        crate::serial_line::tail_arm(); let vb = verdicts(); f(); skip_note(n, vb); verdict_note(n); // QUIETBOOT3 (B352): the fixture's own wire tail, for the glass.
        *CUR.lock() = "";
        ran += 1;
    }
    let (p, f) = (PASS.load(Ordering::Relaxed).wrapping_sub(p0), FAIL.load(Ordering::Relaxed).wrapping_sub(f0));
    let mut names = alloc::string::String::new();
    for n in FAILED.lock().iter().flatten() { if !names.is_empty() { names.push(','); } names.push_str(n); }
    crate::smallfix6::not_found(name, ran); serial_println!(":: TESTS: ran={} pass={} fail={} failed=[{}] skipped=[{}] ::", ran, p, f, names, skipped_names());
    RUNNING.store(false, Ordering::Release);
    ran
}

/// The `tests` shell verb: `tests` (all) · `tests <name>` · `tests list`.
pub fn shell_verb(args: &[&str], console: &mut Console) {
    #[cfg(feature = "login")]
    if !crate::fs::users::desktop_allowed() {
        console.println("tests: refused — finish first-boot setup (root password, then create a user) before the desktop suite runs");
        return;
    }
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); ensure_lumen(); ensure_netring3(); ensure_netclock(); crate::pwwire::ensure_tests(); // CONSOLEFIX (B365): `tests pwwire`, `tests notice`. LUMENBIN: `tests lumen`. NETRING3: `tests net` (merge10 fold). NETCLOCK/ARMNET (merge12 fold)
    ensure_ring3win(); ensure_ring3abi(); ensure_elfbss(); crate::smallfix3::ensure(); crate::smallfix4::ensure(); crate::smallfix5::ensure(); crate::smallfix6::ensure(); crate::smallfix7::ensure(); crate::shelltask::ensure_tests(); crate::serialdoor::ensure_tests(); crate::hidstall::ensure(); ensure_fpu(); #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))] { crate::video::cursor::cursorshape::ensure(); } /* CURSORSHAPE: the module is gated so (cursor.rs tail); ARMVIRT found the ungated call reds every aarch64 kernel without el0 */ // SHELLTASK (B458): `tests shelltask`. SMALLFIX3 (B416): `tests smallfix3`. RING3WIN, RING3ABI2 (merge12 fold)
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); crate::fs::filetype::ensure_tests(); #[cfg(target_arch = "x86_64")] crate::loaderstage::ensure_tests(); crate::soundopeners::ensure(); // LOADERSTALL (B490): `tests loader`. FILETYPE (B307): `tests filetype`.
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); ensure_usbnet(); ensure_kvblank8(); #[cfg(all(target_arch = "x86_64", feature = "wc"))] crate::video::wm::k2p::ensure_tests(); // K2P (B671): `tests k2p` — 200 synthetic reports through the input-to-photon carriers (`video/wm.rs` tail). LINE-NEUTRAL append, code before the comment. 
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::video::shotmask::ensure_tests(); crate::video::blitter::ensure_tests(); crate::prof::ensure_tests(); crate::video::text::ensure_tests(); crate::video::metrics::ensure_tests(); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::video::menubar::ensure_tests(); // MENUBARSLOTS (B499): `tests menubar`. GLASSEYES (B343): `tests shot`. KCOMP (B321): `tests blitter`. PROFILE (B331): `tests prof`.
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); ensure_usbnet(); #[cfg(target_arch = "x86_64")] crate::execname::ensure(); #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))] /* ARMNOEL0: the prefs_client cfg (lib.rs) */ crate::prefs_client::ensure_tests(); #[cfg(feature = "selfdiag")] crate::selfdiag::ensure(); // EXECNAME (B322): `tests exec`. SETTINGSBUS (B337): `tests settingsbus`. SELFDIAG (B324): `tests selfdiag`. (merge12 fold: one line)
    if args.first().copied() == Some("list") {
        let t = TABLE.lock();
        for e in t.iter().flatten() { console.println(&format!("{:<14} {}", e.0, crate::smallfix3::arc_of(e.0))); serial_println!("[tests] list {:<14} {}", e.0, crate::smallfix3::arc_of(e.0)); } // SMALLFIX3 (B416): each fixture with its arc. QEMUSKIP (B558/B613): and on the wire — the serial door never sees a routed verb's console (LINE-NEUTRAL append, before this comment)
        console.println(&format!("{} deferred, {} ran at boot", deferred_count(), AT_BOOT.load(Ordering::Relaxed))); serial_println!(":: TESTS: list n={} deferred={} at_boot={} ::", t.iter().flatten().count(), deferred_count(), AT_BOOT.load(Ordering::Relaxed)); // QEMUSKIP (B558/B613): the list's tail on the wire (LINE-NEUTRAL append, before this comment)
        return;
    }
    let name = args.first().copied(); *ARG.lock() = args.get(1).map(|a| alloc::string::String::from(*a)); // AUDIOCODEC (SR30): `tests play <fmt>` reads its <fmt> via `arg()`
    let (p0, f0) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    let ran = run(name);
    let (p, f) = (PASS.load(Ordering::Relaxed).wrapping_sub(p0), FAIL.load(Ordering::Relaxed).wrapping_sub(f0));
    if ran == 0 && name.is_some() {
        console.println_styled(crate::video::theme::TERM_RED, "tests: no such fixture (try `tests list`)");
    } else {
        let mut names = alloc::string::String::new();
        for n in FAILED.lock().iter().flatten() { if !names.is_empty() { names.push(','); } names.push_str(n); }
        console_verdicts(console); if name.is_none() { console.println_styled(if f == 0 { crate::video::theme::TERM_GREEN } else { crate::video::theme::TERM_RED }, &format!("tests: ran={} pass={} fail={} failed=[{}] skipped=[{}]", ran, p, f, names, skipped_names())); } // QUIETBOOT3 (B352): one `<name> -> <wire tail>` line per fixture; the tally line only after the whole suite.
    }
}

/// SHELLUX (R75): register the `shellux` line-editor fixture exactly once (x86 witness images).
fn ensure_shellux() {
    ensure_bootverdicts(); crate::boot::ensure_tests(); crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } register("tzlocale", tzlocale_selftest); #[cfg(feature = "uvc")] register("uac1", uac1_fixture); register("mtpphone", mtpphone_selftest); #[cfg(feature = "uac")] register("uacmic", uacmic_fixture); register("clockpainters", clockpainters_selftest); register("hwreport", hwreport_selftest); register("audiomix", audiomix_selftest); register("micring3", micring3_selftest); #[cfg(all(target_arch = "x86_64", feature = "a2dp"))] register("a2dp", a2dp_selftest); register("sinkall", sinkall_selftest); } ensure_attr(); #[cfg(target_arch = "x86_64")] crate::arch::clockcore::ensure_tests(); #[cfg(all(feature = "midden", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::middenapp::ensure(); #[cfg(all(feature = "aether", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::aether::ensure(); #[cfg(all(feature = "una", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::unaapp::ensure(); #[cfg(feature = "sonde")] crate::sonde::ensure(); #[cfg(all(feature = "uvc", target_arch = "aarch64"))] crate::drivers::uvc::ensure_tests(); crate::pointerfeel::ensure(); #[cfg(feature = "installdemo")] crate::install::uninstall::ensure(); #[cfg(all(feature = "stria", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::striaapp::ensure(); #[cfg(feature = "uvc")] crate::drivers::xhci::uvc::ensure_tests(); #[cfg(all(target_arch = "aarch64", feature = "v3d"))] crate::arch::aarch64::v3d::ensure_tests(); #[cfg(target_arch = "aarch64")] crate::arch::aarch64::mmu_wxn::ensure(); #[cfg(all(target_arch = "aarch64", not(feature = "busreg")))] register("hwreport", hwreport_selftest); // PIEMPTYBIN (PI24): `tests v3d`, the [v3d105] emptybin rung (LINE-NEUTRAL append, before this comment). STRIAMETAL (SR139): `tests stria` (LINE-NEUTRAL append, before this comment). ARMSONDE (SR101): `tests sonde` arch-neutral, and `tests uvc` registered eagerly on aarch64 (LINE-NEUTRAL append, before this comment). CLOCKCORE (B397): `tests clock`. HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`. MIDDENTWIN (SR75): `tests midden` (LINE-NEUTRAL append, before this comment). UNAMETAL (SR90): `tests una` (LINE-NEUTRAL append, before this comment). REALSENSE (SR92): `tests sonde` (LINE-NEUTRAL append, before this comment). // CLOCKCORE (B397): `tests clock`. HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`. MIDDENTWIN (SR75): `tests midden` (LINE-NEUTRAL append, before this comment). UNAMETAL (SR90): `tests una` (LINE-NEUTRAL append, before this comment). POINTERFEEL (B535): `tests pointerfeel` (LINE-NEUTRAL append, before this comment). HWCENSUSARM (SR165): `tests hwreport` on an aarch64 image with no busreg (the tegra line) (LINE-NEUTRAL append, before this comment).
    crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } } ensure_attr(); ensure_brightfloor(); ensure_gen7(); ensure_wifi(); #[cfg(target_arch = "x86_64")] register("kelfhash", kelfhash_selftest); #[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "wc"))] register("updateui", updateui_selftest); #[cfg(all(feature = "smolnet", target_arch = "x86_64"))] { static SK: AtomicBool = AtomicBool::new(false); if !SK.swap(true, Ordering::AcqRel) { register("sock", crate::drivers::e1000::sock_selftest); } register("smartline", smartline_selftest); register("notelemetry", notelemetry_selftest); #[cfg(all(feature = "smolnet", target_arch = "x86_64"))] ensure_idlenet(); register("pinchzoom", pinchzoom_selftest); } /* INPUTSTALL2 M4 (B388, R80): `tests sock` arms the SOCK ladder; it never runs at boot */ // HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`.
    #[cfg(all(feature = "linuxabi", target_arch = "x86_64"))]
    {
        static LDONE: AtomicBool = AtomicBool::new(false);
        if !LDONE.swap(true, Ordering::AcqRel) { register("linuxabi", crate::arch::linuxabi::selftest); register("linuxabi2", crate::arch::linuxabi::selftest2); register("linuxabi3", crate::arch::linuxabi::selftest3); register("selfbuild", crate::arch::linuxabi::selfbuild::selftest); register("selfbuild2", crate::arch::linuxabi::selfbuild2::selftest); register("selfbuild3", crate::arch::linuxabi::selfbuild3::selftest); register("selfbuild4", crate::arch::linuxabi::selfbuild4::selftest); register("selfbuild5", crate::arch::linuxabi::selfbuild5::selftest); register("selfbuild6", crate::arch::linuxabi::selfbuild6::selftest); }
    }
    #[cfg(all(feature = "witness", target_arch = "x86_64"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("shellux", crate::shellux::selftest); register("shortcuts", crate::video::shortcuts::selftest); register("scrollback", crate::console::scrollback_selftest); register("termcolor", crate::termcolor::selftest); }
    }
    // SETTINGS (R75): the settings fixture (open, idle=5, save, re-read, compare) beside it.
    #[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        static DONE2: AtomicBool = AtomicBool::new(false);
        if !DONE2.swap(true, Ordering::AcqRel) { #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("dongleboot", crate::video::desktopbuild::dongleboot_selftest); /* DONGLEBOOT (B658): the prefs stat, the AHCI bound, the render task on a held disk, the starter release */ register("settings", crate::video::settings::selftest_all); register("appearance", crate::video::appearance::selftest); /* APPEARANCE (B408) */ register("softy", crate::video::softy::selftest); /* SOFTY (B629): the look switch, the taskbar, Start, the caption buttons */ register("winkey", crate::video::softy::startmenu::winkey_selftest); /* WINKEY (B665): the Windows-key tap toggles Start once (counter path), Esc through the door closes it */ register("trackpad", crate::video::settings::selftest_trackpad); /* TRACKPADPANE (B412) */ register("prefs", crate::prefs::selftest); register("prefscap", crate::prefs::prefscap_selftest); register("appmenu", crate::video::sysmenu::selftest); /* APPMENU2 (B393) */ #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("windowlist", crate::video::winlist::selftest); /* WINDOWLIST (R75) */ register("glasskit", crate::video::draw::selftest); /* GLASSKIT (B628): the drawing kit's invariants and per-op cost (LINE-NEUTRAL append) */ register("desktopcraft", crate::video::wm::desktop::selftest); /* DESKTOPCRAFT (B631): the Una desktop painted offscreen at the panel size, its invariants and the frame cost (LINE-NEUTRAL append) */ register("starfield", crate::video::wm::desktop::starfield_selftest); /* STARFIELD (B661, R128): the generated scene offscreen — gen time, the lens on three stars, the zircon, the beam, the shimmer's cost and occlusion (LINE-NEUTRAL append) */ register("quarryhover", crate::video::quarry::live::hover_selftest); /* QUARRYHOVER (B647): Quarry's hover on the service pass and the wheel into every pane (LINE-NEUTRAL append) */  register("motion", crate::video::draw::motion::selftest); register("a11y", crate::video::settings::selftest_a11y); /* A11YPANE (B655): the Accessibility pane, its four keys and the reduce-motion snap (LINE-NEUTRAL append) */ #[cfg(feature = "facet")] register("startscene", crate::video::wallpaper::startscene_selftest); /* STARTSCENE (B662, R128): one picture from the splash to the desktop — gen_count=1, the splash scene, the hold shimmer, the GOP stars, the login backdrop (LINE-NEUTRAL append) */ register("contrast", crate::video::theme::contrast_selftest); /* CONTRAST (B664): the increase-contrast set on both looks, >= 7:1 text-on-surface, two epoch bumps (LINE-NEUTRAL append) */ register("settingsmap", crate::video::settings::selftest_map); /* SETTINGSMAP (B673): every prefs::key on one Settings row, every row on a key, each label reached by <= 2 keys + Return (LINE-NEUTRAL append) */ register("toastcap", crate::video::notify::toastcap_selftest); /* TOASTCAP (B674): DESIGN-LANGUAGE §3.6/§3.8 — three toasts at most, the rest wait, 5 s paused on hover; one primary per dialog, Return/Escape on every view (LINE-NEUTRAL append) */ #[cfg(all(target_arch = "x86_64", feature = "facet"))] register("shimmeridle", crate::video::wm::desktop::shimmeridle_selftest); /* SHIMMERIDLE (B663): the idle desktop's twinkle from the device-service pass — 4..=7 ticks in 200 ms, each under 800 us, none under reduce motion, no gate steal (LINE-NEUTRAL append) */ register("memline", crate::video::desktopbuild::memline_selftest); /* MEMLINE (B677): the desktop's memory line at `[desktop] built` and the Log Out round trip's high-water (LINE-NEUTRAL append) */ register("reask", crate::fs::apptrust::reask_selftest); /* REASK (B681, R130 law 2): an answered ask is kept in Principia's store — 3 reboots and 90 days ask 0 times, a revoke asks once (LINE-NEUTRAL append) */ #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] register("a11ytree", crate::video::wm::a11y::selftest); /* A11YTREE (B675, BP-bar-22): the role/name tree from the window table through the verb's own body, unnamed=0, focus = wm's, Settings' ring at every Tab stop (LINE-NEUTRAL append) */ register("keymaps", keymaps_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))] register("focussteal", focussteal_selftest); #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "facet"))] register("veilcost", veilcost_selftest); #[cfg(all(target_arch = "x86_64", feature = "wc", feature = "login"))] register("aging", aging_selftest); register("a11yfilters", crate::video::settings::selftest_a11yfilters); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))] register("zoom", zoom_selftest); #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("perfhud", crate::video::perfhud::selftest); #[cfg(feature = "quarry")] register("undomove", undomove_selftest); #[cfg(all(feature = "quarry", target_arch = "x86_64", feature = "wc"))] register("undokeys", undokeys_selftest); register("fontkit", fontkit_selftest); #[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))] register("raiseverb", raiseverb_selftest); register("keychars", keychars_selftest); register("softydots", softydots_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))] register("helpsearch", helpsearch_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "unafs"))] register("filesearch", filesearch_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))] register("oneinstance", oneinstance_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))] register("helpf1", helpf1_selftest); #[cfg(feature = "quarry")] register("trashtoast", trashtoast_selftest); #[cfg(feature = "uac")] register("softymic", softymic_selftest); register("scrollview", scrollview_selftest); register("settingsmap", settingsmap_selftest); #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "facet"))] register("wcpar", wcpar_selftest); }
    }
    // POWERMENU (R75): `tests power` — battery panel open/close + the NOTICE thresholds on a forced percent (no shutdown).
    #[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        static DONE3: AtomicBool = AtomicBool::new(false);
        if !DONE3.swap(true, Ordering::AcqRel) { register("power", crate::video::powerui::selftest); }
    }
    // IMGVIEW (R75): the image viewer's open / zoom / browse / refusal fixture.
    #[cfg(all(feature = "witness", feature = "facet", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        static DONE3: AtomicBool = AtomicBool::new(false);
        if !DONE3.swap(true, Ordering::AcqRel) { register("imgview", crate::video::facet::imgview_selftest); }
    }
}

/// SELFINSTALL: register `selfinstall` (the `install ssd --dry-run` plan) exactly once. x86 + the
/// installer + AHCI only; on a lane with no SATA disk the fixture prints SKIP, never a pin.
fn ensure_selfinstall() {
    #[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("selfinstall", crate::install::selfinstall::selftest); register("install", crate::install::selfinstall::install_selftest); register("abimage", crate::install::abimage::selftest); register("kelfstage", kelfstage_selftest); #[cfg(feature = "ahciroot")] register("ahciw", crate::install::ahciroot::ahciw_selftest); #[cfg(all(feature = "ahciroot", feature = "instgui"))] register("instgui", crate::video::instgui::install3::selftest); }
    }
}

/// UNAFSX86 M4: register `unafs` (the root-volume seam fixture) exactly once, on any build carrying the
/// native module under the `unafs` feature; on a FAT root the fixture prints SKIP, never a pin.
fn ensure_unafsx86() {
    #[cfg(feature = "unafs")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("unafs", crate::fs::unafs::unafsx86_selftest); register("boot80", crate::fs::bootstep::boot80_selftest); #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("unafslock", crate::fs::unafs::splitread::selftest); } // HIDSTALL2 (B670): `tests unafslock` (LINE-NEUTRAL append, before this comment)
    }
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        static GROW: AtomicBool = AtomicBool::new(false);
        if !GROW.swap(true, Ordering::AcqRel) { register("unafsgrow", crate::fs::unafsgrow::selftest); register("diskfull", diskfull_selftest); register("fileversions", fileversions_selftest); register("geodebackup", geodebackup_selftest); } // UNAFSGROW (B347) M4: the native module's builds (aarch64 always, x86 under `unafs`)
    }
}

/// ATTRSURF (B299): register `tests attr` exactly once, every build — the fixture decides PASS or an
/// honest SKIP (`reason=no-unafs-volume`) from the mounted tree, so it needs no knob.
fn ensure_attr() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) { register("attr", crate::fs::attrsys::selftest); }
    ensure_volumes(); // VOLUMES (B366): `tests volumes`, `tests testf`
}

/// LUMENAPP (B323): register `lumen` (the ring-3-free LUMEN.ELF fixture, crate::lumen) exactly once on a
/// `lumen` build; on aarch64 (no LUMEN.ELF image yet) the fixture prints SKIP with its reason, never a pin.
fn ensure_lumen() {
    #[cfg(feature = "lumen")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("lumen", crate::lumen::selftest); register("lumenfast", crate::lumen::lumenfast); register("holocron", crate::keyring::selftest); register("ringlogin", crate::keyring::ringlogin_selftest); #[cfg(all(target_arch = "x86_64", feature = "btc"))] register("btkeyseal", crate::drivers::ehci::bthid::btkeyseal_selftest); #[cfg(any(target_arch = "aarch64", feature = "unafs"))] register("volkey", crate::keyring::volkey_selftest); /* VOLKEY (B684): the volume key slot on a RAM volume (LINE-NEUTRAL append) */ #[cfg(any(target_arch = "aarch64", feature = "unafs"))] register("xts", xts_selftest); /* XTS (B689): the volume cipher on a scratch RAM volume, `tests.rs` tail (LINE-NEUTRAL append) */ #[cfg(all(feature = "login", any(target_arch = "aarch64", feature = "unafs")))] register("keyboot", crate::keyring::keyboot_selftest); /* crawl B 21: keyboot_selftest's own cfg (LINE-NEUTRAL) */ }
    }
}

/// NETRING3 (B306): register `tests net` (M1 entropy + M2 resolve) exactly once under `netring3`.
fn ensure_netring3() {
    #[cfg(feature = "netring3")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("net", crate::netring3::selftest); register("nethang", crate::netring3::nethang_selftest); register("netgrant", netgrant_selftest); } // NETHANG: `tests nethang` (code first). NETGRANT (B707): `tests netgrant` (LINE-NEUTRAL append, before this comment)
    }
}

/// RING3WIN (B316): register `tests ring3win` exactly once on x86 (the ELF window is x86's this arc; the
/// fixture SKIPs when the volume carries no `/apps/BIG.ELF`).
fn ensure_ring3win() {
    #[cfg(target_arch = "x86_64")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("ring3win", crate::arch::syscall::ring3win_selftest); register("window", crate::window2::selftest); #[cfg(feature = "wc")] register("applaunch", applaunch_selftest); #[cfg(feature = "wc")] register("appfault", appfault_selftest); } // WINDOW2 (B361): `tests window`
    }
}

/// QUIETBOOT M3 (R80) — the one-statement shape for a boot-time WITNESS: `if crate::tests::defer("name", f) { return; }`
/// as the first statement of the fixture `f` itself. Returns `false` (run the body now, as before) under
/// `tests-at-boot` — so a QEMU lane sees the old line in the old order — and while `tests` is executing a
/// fixture (so the registered `f` re-entering here runs its body). Otherwise registers `f` under `name`
/// ONCE and returns `true`: the boot prints nothing and `tests <name>` fires it.
pub fn defer(name: &'static str, f: fn()) -> bool {
    if cfg!(feature = "tests-at-boot") || (RUNNING.load(Ordering::Acquire) && running_is(name)) {
        return false;
    }
    let present = TABLE.lock().iter().flatten().any(|e| e.0 == name);
    if !present {
        register(name, f);
    }
    true
}

/// QUIETBOOT M4 — the bound on serial lines before `:: BOOT:`, set from the flight-19 sweep: boot 1 printed
/// 2710 lines to its first stage, ~1900 of them knob-recon rungs the flight line armed (SMC walk, gen7,
/// Kepler/KFBIND/KDHEAD, iGPU, BT). This arc takes the census + witness ~290 out, so the same knob line
/// should read ~2420; 2500 leaves room for enumeration variance and FAILS if a census or a witness creeps
/// back. A boot without the recon knobs reads ~500 and the bound tightens with the knob line.
pub const QUIETBOOT_BOUND: u64 = 250; // QUIETBOOT2 (B325): the seat's bound (FLIGHT20) — the sweep moves the fixtures, walks and prose; the boot prints its stages and refusals.

/// `tests quietboot`: `:: QUIETBOOT: lines=<n> bound=<B> census=<bits> -> PASS|FAIL|SKIP ::`, naming the
/// eight loudest tags when over. SKIP on a build that runs its fixtures at boot or arms every census
/// (`tests-at-boot` / `census` — the QEMU lanes), which is not the quiet boot being measured.
pub fn quietboot_selftest() {
    let Some(n) = crate::bootpace::boot_lines() else {
        serial_println!(":: QUIETBOOT: lines=- bound={} -> SKIP reason=no-boot-line ::", QUIETBOOT_BOUND); // QUIETBOOT3 (B352): `SKIP reason=`, never a sentence.
        return;
    };
    if cfg!(feature = "tests-at-boot") || cfg!(feature = "census") {
        serial_println!(":: QUIETBOOT: lines={} bound={} census={} -> SKIP reason=tests-at-boot-or-census-build ::", n, QUIETBOOT_BOUND, crate::census::bits()); // QUIETBOOT3 (B352): `SKIP reason=`.
        return;
    }
    let ok = n <= QUIETBOOT_BOUND;
    if !ok {
        let mut top = [([0u8; 8], 0u64); 8];
        let k = crate::serial_line::tag_top(&mut top);
        let mut s = alloc::string::String::new();
        for (t, c) in &top[..k] {
            let end = t.iter().position(|b| *b == 0).unwrap_or(8);
            if !s.is_empty() { s.push(','); }
            s.push_str(core::str::from_utf8(&t[..end]).unwrap_or("?"));
            s.push_str(&format!(":{}", c));
        }
        serial_println!(":: QUIETBOOT: top=[{}] ::", s);
    }
    serial_println!(":: QUIETBOOT: lines={} bound={} census={} -> {} ::", n, QUIETBOOT_BOUND, crate::census::bits(), if ok { "PASS" } else { "FAIL" });
}

/// BRIGHTFLOOR (B312): register `brightfloor` (the backlight floor, the load clamp, the safe-mode reset,
/// and the BRIGHTKEYS key path that used to run at boot — R80) exactly once, wherever the desktop builds.
fn ensure_brightfloor() {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("brightfloor", crate::video::backlight::selftest); register("brightstep", crate::video::backlight::brightstep); register("lidsleep", crate::video::lidsleep::selftest); #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] register("sleep", crate::video::lidsleep::sleep_selftest); #[cfg(feature = "login")] register("autolock", autolock_selftest); #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] register("usbsuspend", usbsuspend_selftest); } // USBSUSPEND (B766): `tests usbsuspend` (the file-tail fn; same gate as `sleep`). AUTOLOCK (B712): `tests autolock` (the file-tail fn; same gate, `login`). SLEEP (B691): `tests sleep` enters and leaves the state (same-line fold, code before the comment)
    }
}

/// USBNET6 M4: register `tests usbnet` (the dongle receives a frame within 5 s) exactly once, on any build
/// carrying the USB Ethernet driver; no dongle or no link prints SKIP, never a pin.
fn ensure_usbnet() {
    #[cfg(feature = "usbnet")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("usbnet", crate::drivers::xhci::usbnet::selftest); register("usbnet7", crate::drivers::xhci::usbnet::usbnet7_selftest); register("usbnetftdi", crate::drivers::xhci::usbnet::usbnetftdi_selftest); register("usbnetrx", crate::drivers::xhci::usbnet::usbnetrx_selftest); register("dhcp", crate::drivers::xhci::usbnet::dhcp_selftest); register("dhcprestart", crate::drivers::xhci::usbnet::dhcp_restart_verb); } // NETFRAME (B368): `tests usbnet7`
    }
}

/// WIFI1 (B338): register `tests wifi` — the BCM4331 bring-up verdict (`ucode=<ok|refused>
/// d11=<up|down> scan=<n>`) exactly once on a `wifi` build; no radio / no firmware prints SKIP.
fn ensure_wifi() {
    #[cfg(all(target_arch = "x86_64", feature = "wifi"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("wifi", crate::wifi::verb::selftest); }
    }
}

/// KVBLANK8 (B318): register `tests kvblank8` (the vblank interrupt path, 1 s, `lost_at=`) exactly once on a Kepler
/// vblank build; no GK107 prints SKIP, never a pin.
fn ensure_kvblank8() {
    #[cfg(all(target_arch = "x86_64", feature = "nvidia-kepler-vblank"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("kvblank8", crate::drivers::gpu::kepler_vblank::kvblank8_selftest); }
    }
}

/// GEN7R8 (B320, R80) + GPUTESTS (B334): register `tests gen7` — the whole Ivy Bridge ladder, R1..R7 from
/// the boot bank (`:: GEN7LADDER: … ::`) then R8 under `gen7r8` (`:: GEN7R8: … ::`) — exactly once, on a
/// `gen7` (`UNAOS_IVB3D`) build. The boot only banks the inputs (`gen7::bank`) and prints nothing.
fn ensure_gen7() {
    #[cfg(all(target_arch = "x86_64", feature = "gen7"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("gen7", crate::drivers::gpu::gen7::ladder_test); }
    }
}

// TESTFIX4 (B330) — TAIL-APPENDED. `skipped=[…]` on the summary: a fixture that RAN and printed no `-> PASS`
// and no `-> FAIL` verdict (its SKIP line, or nothing) is named there, so a read of the summary tells SKIP from
// FAIL without the log. Counted from the same verdict tap as pass/fail, before and after the fixture.
static SKIPPED: crate::sync::Mutex<[Option<&'static str>; CAP]> = crate::sync::Mutex::new([None; CAP]);

fn verdicts() -> (u32, u32) { (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed)) }

fn skip_reset() { *SKIPPED.lock() = [None; CAP]; }

fn skip_note(n: &'static str, before: (u32, u32)) {
    if verdicts() != before { return; }
    let mut sk = SKIPPED.lock();
    if !sk.iter().flatten().any(|x| *x == n) {
        if let Some(slot) = sk.iter_mut().find(|x| x.is_none()) { *slot = Some(n); }
    }
}

fn skipped_names() -> alloc::string::String {
    let mut s = alloc::string::String::new();
    for n in SKIPPED.lock().iter().flatten() { if !s.is_empty() { s.push(','); } s.push_str(n); }
    s
}

/// QUIETBOOT2 (B325, R80) — [`defer`] for a fixture that sits on a path the boot passes MANY times (a service
/// pass, a paint): the caller's own `latch` makes every call after the first one relaxed swap, no table scan.
/// Same answer as `defer`: `false` (run the body) under `tests-at-boot` or while `tests` is running it.
pub fn defer_fast(name: &'static str, f: fn(), latch: &AtomicBool) -> bool {
    if cfg!(feature = "tests-at-boot") || (RUNNING.load(Ordering::Acquire) && running_is(name)) { // SMALLFIX7 (B501): only while THIS fixture runs — flight 27's `prtscrdir` fired inside `tests quarrylive` and held the capture door shut
        return false;
    }
    if !latch.swap(true, Ordering::AcqRel) {
        defer(name, f);
    }
    true
}

/// NETCLOCK (B335): register `tests netclock` (5 s idle on a live USB link: polls/s, tx/s, the stack-side
/// xHCI loan hold) exactly once on an x86 smolnet + usbnet build.
fn ensure_netclock() { ensure_netclock_arm();
    #[cfg(all(feature = "smolnet", feature = "usbnet", target_arch = "x86_64"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("netclock", crate::smolnet::netclock_selftest); }
    }
}

/// ARMNET (B346): register `tests netclock` on an aarch64 NET6 build (the persistent stack's 5 s idle: polls/s,
/// tx/s; SKIP with no NIC or no link). Called from `ensure_netclock` (same-line fold) so the x86 arm is untouched.
fn ensure_netclock_arm() {
    #[cfg(all(feature = "net6", target_arch = "aarch64"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("netclock", crate::net_phy::net6::netclock_selftest); }
    }
}

/// RING3ABI2 (B333): register `tests ring3abi` exactly once — x86, and aarch64 builds with an EL0 layer.
fn ensure_ring3abi() {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("ring3abi", crate::ring3abi::selftest); }
    }
}

/// FPU (rmbp-ledger B666): register `tests fpu` (BIG.ELF's `fpu` verb: two ring-3 tasks keep their XMM/MXCSR across yields) once on x86.
fn ensure_fpu() {
    #[cfg(target_arch = "x86_64")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("fpu", crate::arch::sched::fpu::selftest); register("abielf", abielf_selftest); #[cfg(feature = "tickless")] register("tickless", tickless_selftest); } // TICKLESS (B765): `tests tickless` (the file-tail fn), same x86 block. ABIELF (B693): `tests abielf` beside it, same cfg (the file-tail fn). LINE-NEUTRAL fold, code before the comment.
    }
}

/// STORMFAULT (B351): register `tests elfbss` (a 64 KiB-bss elf-model program exits 0; named refusals) once on x86.
fn ensure_elfbss() {
    #[cfg(target_arch = "x86_64")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("elfbss", crate::arch::elf::elfbss_selftest); }
    }
}

// QUIETBOOT3 (rmbp-ledger B352) — TAIL-APPENDED. THE GLASS SAYS WHAT THE WIRE SAYS. Boot 21: Peter read "nethang
// failed" and "storm failed" off the glass where the wire said `NETHANG … -> PASS`; the console printed a tally
// sentence (`tests: ran=1 pass=8 …`), not the verdict. Now each fixture's console line is `<name> -> <tail>`, the
// tail being the wire's own text after the LAST `-> ` of the last verdict line the fixture printed
// (`serial_line::verdict_tail`): `PASS`, `FAIL …`, `SKIP reason=…`. A fixture that printed no verdict gets a wire
// line of its own, `:: TESTS: <name> -> SKIP reason=no-verdict ::`, and the glass prints that same tail.
static RESULTS: crate::sync::Mutex<alloc::vec::Vec<(&'static str, alloc::string::String)>> = crate::sync::Mutex::new(alloc::vec::Vec::new());

fn verdict_note(n: &'static str) {
    let tail = match crate::serial_line::tail_take() {
        Some(t) => t,
        None => {
            serial_println!(":: TESTS: {} -> SKIP reason=no-verdict ::", n);
            alloc::string::String::from("SKIP reason=no-verdict")
        }
    };
    RESULTS.lock().push((n, tail));
}

fn console_verdicts(console: &mut Console) {
    let rows = core::mem::take(&mut *RESULTS.lock());
    for (n, tail) in rows.iter() {
        let c = if tail.starts_with("PASS") { crate::video::theme::TERM_GREEN } else if tail.starts_with("FAIL") { crate::video::theme::TERM_RED } else { crate::video::theme::TERM_DIM };
        console.println_styled(c, &format!("{} -> {}", n, tail));
    }
}
/// AUDIOCODEC (SR30): the word after the fixture name in `tests <name> <arg>` (e.g. `tests play flac`), for fixtures
/// that take one; `None` from a bare `tests <name>`, from `tests` (all) and at boot.
static ARG: crate::sync::Mutex<Option<alloc::string::String>> = crate::sync::Mutex::new(None);
pub fn arg() -> Option<alloc::string::String> { ARG.lock().clone() }

/// VOLUMES (rmbp-ledger B366) — TAIL-APPENDED: register `tests volumes` (the Volumes layout: boot = EFI only, the
/// UnaFS root shown, no home on the FAT) and `tests testf` (system/test-f staged vs claimed) exactly once, every build;
/// the fixtures SKIP or FAIL with their reason from the mounted tree.
fn ensure_volumes() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) { register("volumes", crate::fs::volumes::selftest); register("testf", crate::fs::volumes::testf_selftest); register("rootdisk", crate::fs::rootdisk::selftest); register("bootfat", crate::fs::bootfat::selftest); register("bootfatseam", crate::fs::bootfat::bootfatseam_selftest); register("unplug", unplug_selftest); }
}

// BOOTVERDICTS (rmbp-ledger B472, R80) — TAIL-APPENDED. A verdict word before the deferred line is a test
// that ran at boot. `tally` (the verdict tap's own count) counts every `-> PASS|FAIL` printed before the
// `TESTS: deferred` announce; `tests verdicts` reports it beside what this arc moved and reworded. The host
// twin is `unaos/scripts/verdictwords.py` (GATE-VERDICTWORDS) over the bench wire.
static BOOT_VERDICTS: AtomicU32 = AtomicU32::new(0);
/// Tags moved behind `tests` by BOOTVERDICTS: the 14-tag U7x ladder (`ladder`), LOGWIT-1 (`logwit`),
/// SERWIT-2 (`serwit2`), DIMIDLE (`dimidle`).
const VERDICTS_MOVED: u32 = 17;
/// Arming lines reworded off PASS/FAIL: USBNET-EHCI SPLASH AHCI UNAFSX86 PREFS FIRSTBOOT DOCKPIN KFONTPPI
/// XHCIHUB (flight 25) + WINDOWCAP USBNET HDA (flight 24's login-screen boot).
const VERDICTS_REWORDED: u32 = 12;

fn ensure_bootverdicts() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) { register("verdicts", verdicts_selftest); }
}

/// `tests verdicts`: how many verdict lines printed before the deferred line on THIS boot.
pub fn verdicts_selftest() {
    let n = BOOT_VERDICTS.load(Ordering::Relaxed);
    serial_println!(
        ":: VERDICTS: before_deferred={} moved={} reworded={} announced={} tags={} -> {} ::",
        n, VERDICTS_MOVED + BOOTVERDICTS2_MOVED, VERDICTS_REWORDED + SMALLFIX7_REWORDED + BOOTVERDICTS2_REWORDED, ANNOUNCED.load(Ordering::Relaxed) as u8, boot_tags(), if n == 0 { "PASS" } else { "FAIL" }
    );
}

// SMALLFIX7 (rmbp-ledger B501) — TAIL-APPENDED. Flight 27 read `VERDICTS: before_deferred=29 -> FAIL` and the wire
// had NO `TESTS: deferred=` line: the announce waited on `SRC_DESK`, which `u8x_launcher` signalled, and BOOTVERDICTS
// moved that chain behind `tests ladder` — so `tally` counted the whole session, the seat's own `tests` runs
// included. Now `ladder_arm` signals the source, the window also closes at the first `tests` run, `tests verdicts`
// names what it counted, and the six lines the boot itself printed (TPMODE, FIRSTUSER, INPUTSTALL, DESKTOPBUILT,
// the wifi6 S5i post-check, U5x) print `ok`/`NOT-OK` until the operator's first `tests` run ([`boot_word`]).

/// Set by the first `tests` run: a verdict after it is a fixture's, never the boot's.
static BOOT_OVER: AtomicBool = AtomicBool::new(false);
/// The six boot lines SMALLFIX7 reworded through [`boot_word`].
const SMALLFIX7_REWORDED: u32 = 6;
const TAG_MAX: usize = 16;
const TAGS_MAX: usize = 12;
/// The first [`TAGS_MAX`] counted verdict lines' tags (alloc-free: the tap runs in IRQ-masked print contexts).
static TAGS: crate::sync::Mutex<([[u8; TAG_MAX]; TAGS_MAX], usize)> = crate::sync::Mutex::new(([[0; TAG_MAX]; TAGS_MAX], 0));

/// Has the operator run `tests` this boot? Always under `tests-at-boot`: the QEMU lanes keep their boot verdicts
/// and the specs that pin them. NOT the announce: `ladder_arm` can print `TESTS: deferred=` before the login, and a
/// boot line after it is still the boot's, not a test the operator asked for (R80).
pub fn boot_over() -> bool {
    cfg!(feature = "tests-at-boot") || BOOT_OVER.load(Ordering::Relaxed)
}

/// The verdict word a boot-path witness prints: `PASS`/`FAIL` once [`boot_over`], `ok`/`NOT-OK` before it — the
/// boot's line is a record there (R80); `tests <name>`, or any later print, carries the verdict.
pub fn boot_word(pass: bool) -> &'static str {
    match (boot_over(), pass) {
        (true, true) => "PASS",
        (true, false) => "FAIL",
        (false, true) => "ok",
        (false, false) => "NOT-OK",
    }
}

/// The verdict tap's label for a line [`tally`] counted (same window): its tag, up to the first `:`.
pub fn boot_tag(label: &str) {
    if ANNOUNCED.load(Ordering::Relaxed) || BOOT_OVER.load(Ordering::Relaxed) {
        return;
    }
    let tag = label.split(':').next().unwrap_or("").trim();
    let tag = tag.strip_prefix('[').map(|t| t.split(']').next().unwrap_or(t)).unwrap_or(tag);
    if let Some(mut g) = TAGS.try_lock() {
        let n = g.1;
        if n < TAGS_MAX {
            let b = tag.as_bytes();
            let k = b.len().min(TAG_MAX);
            g.0[n] = [0; TAG_MAX];
            g.0[n][..k].copy_from_slice(&b[..k]);
            g.1 = n + 1;
        }
    }
}

fn boot_tags() -> alloc::string::String {
    let g = TAGS.lock();
    let mut s = alloc::string::String::new();
    for t in g.0[..g.1].iter() {
        let n = t.iter().position(|&c| c == 0).unwrap_or(TAG_MAX);
        if !s.is_empty() {
            s.push(',');
        }
        s.push_str(core::str::from_utf8(&t[..n]).unwrap_or("?"));
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// SMALLFIX7 (B501): is `name` the fixture [`run`] is executing now? A hot-path fixture ([`defer_fast`]) runs its
/// body only then — never inside another fixture's run, where its side effects (a held door) land on the wrong test.
pub fn running_is(name: &str) -> bool {
    for _ in 0..64 {
        if let Some(c) = CUR.try_lock() {
            return *c == name;
        }
        core::hint::spin_loop(); // `run` holds CUR for one store; a few retries, never a sleep (paint paths call this)
    }
    false
}

/// SMALLFIX7: did `TESTS: deferred=` print this boot?
pub fn announced() -> bool {
    ANNOUNCED.load(Ordering::Relaxed)
}

/// SMALLFIX7: the boot's counted verdict lines and their tags (`tests verdicts`' numbers).
pub fn boot_verdicts() -> (u32, alloc::string::String) {
    (BOOT_VERDICTS.load(Ordering::Relaxed), boot_tags())
}

/// SMALLFIX7: is a fixture registered by `name`?
pub fn registered(name: &str) -> bool {
    TABLE.lock().iter().flatten().any(|e| e.0 == name)
}

// BOOTVERDICTS2 (rmbp-ledger B616, R80) — TAIL-APPENDED. UIREDS26 (B597/B605) read `verdicts=6:sdhc,DRAINCAP,SERWIT-1B,
// PWRDRAIN,SERWIRE,SERIALTX` on QEMU where the rMBP reads `0:-`: the five serial-ring fixtures ran from
// `serial_ring::mirror_service`'s first pass and printed PASS on any machine with a 16550, and the SDHC-4b `w2` line
// printed PASS on any machine whose `w1` ladder passed. The five now register through [`defer_polled`] and run when
// `tests` runs them; `w2` keeps its boot write (it runs before the card is published, `drivers/sdhc.rs`) and its
// boot line speaks [`boot_word`], the verdict banked for `tests sdhcw2`.

/// The five serial-ring fixtures BOOTVERDICTS2 moved: `draincap`, `serwit1b`, `pwrdrain`, `serwire`, `serialtx`.
const BOOTVERDICTS2_MOVED: u32 = 5;
/// The boot line BOOTVERDICTS2 reworded through [`boot_word`]: `sdhc: w2`.
const BOOTVERDICTS2_REWORDED: u32 = 1;

/// [`defer`] for a fixture called from a POLL that passes many times (`serial_ring::mirror_service`). The poll never
/// runs the body: its first pass registers `f` under `name` (the caller's `latch` makes every later pass one swap, no
/// table scan) and every pass returns `true`. Unlike [`defer_fast`] the registered `f` is a DIFFERENT function from the
/// polled one, so another core's poll during `tests <name>` cannot run the body a second time beside it. `false` (the
/// poll runs the body, as before) under `tests-at-boot`, where the caller's own one-shot latch keeps it to one run.
pub fn defer_polled(name: &'static str, f: fn(), latch: &AtomicBool) -> bool {
    if cfg!(feature = "tests-at-boot") {
        return false;
    }
    if !latch.swap(true, Ordering::AcqRel) {
        register(name, f);
    }
    true
}

// IDLENET (rmbp-ledger B672, BP-bar-1) — TAIL-APPENDED. `tests idlenet` (the stack's outbound ledger over an idle window,
// `net_tick::idlenet_selftest`) and `tests nicabsent` (a boot with no NIC builds its desktop without waiting on a net
// starter). Registered from the verb only: the idle window sleeps 60 s on the shell job, never at boot.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn ensure_idlenet() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        register("idlenet", crate::net_tick::idlenet_selftest);
        #[cfg(feature = "wc")]
        register("nicabsent", nicabsent_selftest);
    }
}

/// The net starters among the desktop bar's held starters (`video/desktopbuild.rs` `steps::STARTERS`): a NIC-absent
/// boot must hold none of them.
#[cfg(all(feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
const NET_STARTERS: [&str; 2] = ["usbnet", "wifi"];

/// NICABSENT's judge, pure: `nic` = a NIC was found (the fixture is for a boot without one); `built` = the desktop's
/// bar was said; `waited` = how many of [`NET_STARTERS`] the bar held (`None` = that census is not readable here).
#[cfg(all(feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
fn nicabsent_judge(nic: bool, built: bool, waited: Option<u32>) -> &'static str {
    match (nic, built, waited) {
        (true, _, _) => "SKIP reason=nic-present",
        (false, false, _) => "FAIL reason=no-desktop",
        (false, true, None) => "SKIP reason=starter-census-owed",
        (false, true, Some(0)) => "PASS",
        (false, true, Some(_)) => "FAIL reason=net-starter-waited",
    }
}

/// NICABSENT — `tests nicabsent`: a boot with no e1000 and no USB Ethernet reached `[desktop] built` and its bar held no
/// net starter (no `[desktop] starter after-bar who=usbnet|wifi`, no `held_starters=` naming one). The held-starter
/// census is `video::desktopbuild::net_starters_held` over `steps::SAID | HELD` (a `None` census, which no caller passes
/// now, stays `SKIP reason=starter-census-owed` — never a PASS on an unread input). The judge's own table runs first
/// (a wrong row is a FAIL).
/// `:: NICABSENT: nic=<0|1> built=<0|1> bar_ms=<n|none> net_starters_waited=<n|owed> table=<ok>/<n> -> PASS|FAIL|SKIP ::`
#[cfg(all(feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
fn nicabsent_selftest() {
    let table: [((bool, bool, Option<u32>), &str); 5] = [
        ((true, true, Some(0)), "SKIP reason=nic-present"),
        ((false, false, Some(0)), "FAIL reason=no-desktop"),
        ((false, true, None), "SKIP reason=starter-census-owed"),
        ((false, true, Some(0)), "PASS"),
        ((false, true, Some(1)), "FAIL reason=net-starter-waited"),
    ];
    let ok = table.iter().filter(|(a, want)| nicabsent_judge(a.0, a.1, a.2) == *want).count();
    let e1000 = crate::drivers::e1000::nic_present();
    #[cfg(feature = "usbnet")]
    let dongle = crate::drivers::xhci::usbnet::kind() != 0;
    #[cfg(not(feature = "usbnet"))]
    let dongle = false;
    let bar = crate::video::desktopbuild::last_bar_ms();
    let waited: Option<u32> = Some(crate::video::desktopbuild::net_starters_held(&NET_STARTERS));
    let verdict = if ok != table.len() { "FAIL reason=judge-table" } else { nicabsent_judge(e1000 || dongle, bar.is_some(), waited) };
    serial_println!(
        ":: NICABSENT: nic={} built={} bar_ms={} net_starters_waited={} table={}/{} -> {} ::",
        (e1000 || dongle) as u8,
        bar.is_some() as u8,
        bar.map(|b| alloc::format!("{}", b)).unwrap_or_else(|| alloc::string::String::from("none")),
        waited.map(|w| alloc::format!("{}", w)).unwrap_or_else(|| alloc::string::String::from("owed")),
        ok,
        table.len(),
        verdict
    );
}

// APPLAUNCH (rmbp-ledger B678, BP-bar-7) — TAIL-APPENDED. `tests applaunch`: the SMALLEST windowed ring-3 ELF under
// `/apps` (sizes from the listing, smallest first; the first whose `.note.unaos.app` declares WINDOWED), launched N
// times COLD (a fresh process and slot each time, through the operator's own `bg` launcher), each span read from the
// slot's `[app] … exec->frame ms=` stamp (`procslot::launch::frame_ms`), then the program killed and reaped before the
// next. Nearest-rank p50/p99 over the N spans against BP-bar-7's 400 ms. SKIP (never a pin) with no panel or no
// windowed program staged; a launch that never reaches its first frame within 5 s is a FAIL named by count.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn applaunch_selftest() {
    const N: usize = 5;
    const BAR_MS: u64 = 400;
    const WAIT_MS: u64 = 5_000;
    if !crate::video::WRITER.lock().is_ready() {
        serial_println!(":: APPLAUNCH: app=- n={} bar_ms={} -> SKIP reason=no-panel ::", N, BAR_MS);
        return;
    }
    let mt = crate::shell::vfs_mount_table();
    let dir = crate::shell::vfs_path("/apps");
    let mut cands: alloc::vec::Vec<(u64, alloc::string::String)> = match mt.read_dir(&dir) {
        Ok(v) => v
            .into_iter()
            .filter(|e| matches!(e.kind, crate::fs::vfs::NodeKind::File) && e.size > 0 && e.size as usize <= crate::arch::syscall::user_image_cap())
            .filter(|e| e.name.len() > 4 && e.name[e.name.len() - 4..].eq_ignore_ascii_case(".ELF"))
            .map(|e| (e.size, e.name))
            .collect(),
        Err(_) => {
            serial_println!(":: APPLAUNCH: app=- n={} bar_ms={} -> SKIP reason=no-apps-dir ::", N, BAR_MS);
            return;
        }
    };
    cands.sort();
    let mut pick: Option<(alloc::string::String, alloc::vec::Vec<u8>)> = None;
    for (size, leaf) in cands.iter() {
        let path = format!("{}/{}", dir.trim_end_matches('/'), leaf);
        if let Ok(b) = mt.read(&path, 0, *size as usize) {
            if b.len() as u64 == *size && crate::arch::elf::app_flags(&b) & una_abi::APP_FLAG_WINDOWED != 0 {
                pick = Some((path, b));
                break;
            }
        }
    }
    let Some((path, bytes)) = pick else {
        serial_println!(":: APPLAUNCH: app=- n={} bar_ms={} -> SKIP reason=no-windowed-elf ::", N, BAR_MS);
        return;
    };
    let name = crate::video::wm::program_name(&path);
    let mut spans: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    for run in 0..N {
        match crate::arch::syscall::spawn_user_image_bg_argv(&bytes, &[name]) {
            Ok((pid, slot, _)) => {
                let dl = crate::arch::ticks() + WAIT_MS;
                let mut got = None;
                while crate::arch::ticks() < dl {
                    got = crate::procslot::launch::frame_ms(slot as usize);
                    if got.is_some() {
                        break;
                    }
                    crate::arch::sched::yield_now();
                }
                let kill = crate::arch::syscall::bg_kill(pid, slot);
                let _ = crate::arch::syscall::bg_poll(pid, true);
                serial_println!("[applaunch] run={} app={} bytes={} pid={} slot={} frame_ms={:?} kill={}", run, name, bytes.len(), pid, slot, got, kill);
                if let Some(ms) = got {
                    spans.push(ms);
                }
            }
            Err(why) => serial_println!("[applaunch] run={} app={} refused={}", run, name, why),
        }
    }
    spans.sort();
    let rank = |q: usize| -> Option<u64> {
        if spans.is_empty() { None } else { Some(spans[((q * spans.len() + 99) / 100).max(1) - 1]) }
    };
    let (p50, p99) = (rank(50), rank(99));
    let show = |v: Option<u64>| v.map_or(alloc::string::String::from("-"), |x| format!("{}", x));
    let verdict = if spans.len() != N {
        format!("FAIL reason=frames-{}/{}", spans.len(), N)
    } else if p99.unwrap_or(u64::MAX) > BAR_MS {
        alloc::string::String::from("FAIL reason=over-bar")
    } else {
        alloc::string::String::from("PASS")
    };
    serial_println!(":: APPLAUNCH: app={} n={} p50_ms={} p99_ms={} bar_ms={} -> {} ::", name, N, show(p50), show(p99), BAR_MS, verdict);
}

// XTS (rmbp-ledger B689, VOLKEY M2) — `tests xts`: the volume cipher on a scratch RAM volume. TAIL-APPENDED.
// The cipher, the copy that makes a ciphered volume and the mount that carries it are the crates' (`crypto_core::xts`,
// `unafs::storage::VolumeCipher`, `unafs::fs::encipher`, unafs-records.md §R2b); this is their kernel witness. It
// prints counts and words like `ok` only — no key, no password, no recovery word on the wire.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn xts_selftest() {
    use ::unafs::fs::{encipher, FileSystemError};
    use ::unafs::{KdfParams, KeySlot, MemDevice, UnaFS, Unlock, BLOCK_SIZE};
    const PW: &[u8] = b"xts-fixture-pw";
    const BLOCKS: u64 = 512;
    const MB: u64 = 4;
    let fail = |why: &str| serial_println!(":: XTS: vectors=FAIL roundtrip=FAIL raw_plain=- ms_per_mb=- -> FAIL ({}) ::", why);
    let hx = |s: &str| -> alloc::vec::Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap_or(0)).collect() };
    let count = |hay: &[u8], n: &[u8]| hay.windows(n.len()).filter(|w| *w == n).count();
    // ── IEEE Std 1619-2007 Annex B vectors 10 and 14: XTS-AES-256, 512 B data units numbered ff and ffffffffff,
    //    plaintext 00..ff twice; the ciphertext's SHA-256 (of the standard's 512 bytes), then decrypted back ──────
    let key = hx("27182818284590452353602874713526624977572470936999595749669676273141592653589793238462643383279502884197169399375105820974944592");
    let pt: alloc::vec::Vec<u8> = (0..512u32).map(|i| i as u8).collect();
    let mut vectors = false;
    if let Ok(x) = crypto_core::xts::Xts::new(&key) {
        vectors = true;
        for (unit, digest) in [
            (0xffu64, "e97e974fa393af794f7a4684395814cf820de60a01eaec677d87b452e316b364"),
            (0xff_ffff_ffffu64, "afba71abc4e95b186d89a63a5437c1bafcfd1a18ca273970c534aba4f8d05282"),
        ] {
            let mut b = pt.clone();
            let enc = x.encrypt_unit(unit, &mut b).is_ok();
            let mut h = crypto_core::sha2::Sha256::new();
            h.update(&b);
            let same = h.finalize()[..] == hx(digest)[..];
            let dec = x.decrypt_unit(unit, &mut b).is_ok() && b == pt;
            vectors &= enc && same && dec;
        }
    }
    // ── a plaintext v7 volume with a witness, enciphered under a fresh ciphered slot (kernel DRBG) ─────────────
    let t0 = crate::arch::ms();
    let witness = |tag: &str| -> alloc::vec::Vec<u8> {
        let mut v = alloc::vec::Vec::new();
        for i in 0..512 {
            v.extend_from_slice(format!("XTS kernel witness {} line {:05}\n", tag, i).as_bytes());
        }
        v
    };
    let (w1, w2) = (witness("one"), witness("two"));
    let (n1, n2) = (b"XTS kernel witness one line".as_slice(), b"XTS kernel witness two line".as_slice());
    let mut fs = match UnaFS::format(MemDevice::with_blocks(BLOCKS), 0) {
        Ok(f) => f,
        Err(_) => return fail("format refused"),
    };
    let root = fs.superblock.root_inode;
    let wrote = fs.create_file(root, "a.txt".into()).and_then(|id| fs.write_data(id, 0, &w1)).is_ok();
    let plain = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    let (slot, words, vkey) = match KeySlot::seal_ciphered(PW, KdfParams::FLOOR, &mut |b: &mut [u8]| {
        let _ = crate::rand::drbg_fill(b);
    }) {
        Ok(x) => x,
        Err(_) => return fail("seal refused"),
    };
    let mut dst = MemDevice::new();
    let made = encipher(&mut MemDevice::from_bytes(plain.clone()), &mut dst, &slot, &vkey).is_ok_and(|n| n == BLOCKS);
    drop(vkey);
    let raw = dst.into_bytes();
    // ── no key: refused; the words open it; a write goes through the cipher; a re-mount reads both ─────────────
    let nokey = matches!(UnaFS::mount(MemDevice::from_bytes(raw.clone())), Err(FileSystemError::KeyRequired));
    let read = |m: &mut UnaFS<MemDevice>, p: &str, w: &[u8]| m.resolve_path(p).and_then(|id| m.read_data(id, 0, w.len() as u64)).is_ok_and(|d| d == w);
    let rc = Unlock::Recovery(words.as_str());
    let (mut first, mut raw2) = (false, alloc::vec::Vec::new());
    if let Ok(mut m) = UnaFS::mount_with(MemDevice::from_bytes(raw.clone()), &rc) {
        first = read(&mut m, "/a.txt", &w1) && m.create_file(root, "b.txt".into()).and_then(|id| m.write_data(id, 0, &w2)).is_ok();
        raw2 = m.device.as_bytes().to_vec();
        m.device = MemDevice::new();
    }
    let second = UnaFS::mount_with(MemDevice::from_bytes(raw2.clone()), &rc)
        .is_ok_and(|mut m| read(&mut m, "/a.txt", &w1) && read(&mut m, "/b.txt", &w2) && m.fsck(false).is_ok_and(|r| r.is_clean()));
    let stamped = raw.len() > 9 && u32::from_le_bytes([raw[5], raw[6], raw[7], raw[8]]) == ::unafs::keyslot::VERSION_CIPHERED;
    let roundtrip = wrote && made && nokey && first && second && stamped;
    // ── the medium: no witness run in either ciphered image (the plaintext image has them: the scan works) ─────
    let plain_hits = count(&plain, n1);
    let raw_plain = count(&raw, n1) + count(&raw2, n1) + count(&raw2, n2);
    let rt_ms = crate::arch::ms().saturating_sub(t0);
    // ── cost: the write path's cipher over MB MiB of 4 KiB blocks ─────────────────────────────────────────────────
    let mut ms_per_mb = u64::MAX;
    if let Some(Ok(mut c)) = slot.unwrap(&rc).ok().map(|k| ::unafs::storage::VolumeCipher::new(&k)) {
        let blk = alloc::vec![0x5au8; BLOCK_SIZE as usize];
        let n = MB * 1024 * 1024 / BLOCK_SIZE;
        let t = crate::arch::ms();
        let mut ok = true;
        for id in 1..=n {
            ok &= c.seal(id, 0, &blk).is_ok();
        }
        if ok {
            ms_per_mb = (crate::arch::ms().saturating_sub(t) + MB / 2) / MB;
        }
    }
    drop(words);
    serial_println!(
        "[xts] fx cipher=aes-256-xts unit=4096 tweak=block kdf=hkdf-sha256 blocks={} stamp={} plain_hits={} nokey={} ms={}",
        BLOCKS, if stamped { "v8" } else { "WRONG" }, plain_hits, if nokey { "refused" } else { "OPENED" }, rt_ms
    );
    let pass = vectors && roundtrip && plain_hits > 0 && raw_plain == 0 && ms_per_mb != u64::MAX;
    serial_println!(
        ":: XTS: vectors={} roundtrip={} raw_plain={} ms_per_mb={} -> {} ::",
        if vectors { "ok" } else { "FAIL" }, if roundtrip { "ok" } else { "FAIL" }, raw_plain,
        if ms_per_mb == u64::MAX { alloc::string::String::from("-") } else { format!("{}", ms_per_mb) }, if pass { "PASS" } else { "FAIL" }
    );
}

// ABIELF (rmbp-ledger B693, BP-bar-13, BP-ring3-15) — TAIL-APPENDED. `tests abielf`: an ELF built at una-abi vN runs
// unchanged on this build. `/apps/ABIPROBE.ELF` is the COMMITTED fixture (unaos/fixtures/ABIPROBE-v<N>.ELF, staged by the
// builder after its sha256 pin check) — bytes an older tree built, never a rebuild. It calls one verb of each stable family
// (file open/write/read/close, window create/present/close, a bus GET round trip, the zero-timeout input read, the
// prefs_client PREF_GET) and judges each against its contract; this fixture judges its exit status:
// 1<<24 (it ran) | abi<<16 | syscalls<<8 | ok. PASS = exited, it ran, syscalls > 0, ok == syscalls, 1 <= abi <= ABI_VERSION.
// SKIP (never a pin) when no fixture is staged; a fault or a 10 s timeout is a FAIL named by outcome.
// `:: ABIELF: elf=ABIPROBE.ELF bytes=<n> abi=<N> kernel_abi=<K> syscalls=<n> ok=<n> outcome=<o> -> PASS|FAIL ::`
#[cfg(target_arch = "x86_64")]
fn abielf_selftest() {
    use crate::arch::syscall::RunOutcome;
    const PATH: &str = "/apps/ABIPROBE.ELF";
    let k = una_abi::ABI_VERSION;
    let mt = crate::shell::vfs_mount_table();
    let full = crate::shell::vfs_path(PATH);
    let Ok(st) = mt.stat(&full) else {
        serial_println!(":: ABIELF: elf=- abi=- kernel_abi={} syscalls=0 ok=0 -> SKIP reason=no-fixture ::", k);
        return;
    };
    let bytes = match mt.read(&full, 0, st.size as usize) {
        Ok(b) if b.len() as u64 == st.size => b,
        _ => {
            serial_println!(":: ABIELF: elf=ABIPROBE.ELF abi=- kernel_abi={} syscalls=0 ok=0 -> FAIL reason=read ::", k);
            return;
        }
    };
    let (outcome, status) = match crate::arch::syscall::run_user_image_argv("abielf", &bytes, 10_000, &["abiprobe"]) {
        Ok((RunOutcome::Exited(s), _)) => ("exited", Some(s as u32)),
        Ok((RunOutcome::Faulted, _)) => ("faulted", None),
        Ok((RunOutcome::Timeout, _)) => ("timeout", None),
        Err(e) => (e, None),
    };
    let s = status.unwrap_or(0);
    let (ran, abi, n, ok) = (s >> 24 == 1, (s >> 16) & 0xFF, (s >> 8) & 0xFF, s & 0xFF);
    let pass = ran && n > 0 && ok == n && abi >= 1 && abi <= k;
    serial_println!("[abielf] outcome={} status={:?} bytes={}", outcome, status, bytes.len());
    serial_println!(
        ":: ABIELF: elf=ABIPROBE.ELF bytes={} abi={} kernel_abi={} syscalls={} ok={} outcome={} -> {} ::",
        bytes.len(), abi, k, n, ok, outcome, if pass { "PASS" } else { "FAIL" }
    );
}

// ---- KELFHASH (rmbp-ledger B709): the loader's hash verdict on the kernel it jumped to ---------------------------------
// The x86 UEFI loader sha256s the kernel bytes it read and compares them with the builder's `KELF.SHA` row BEFORE the
// jump (`crates/bootloader/src/kelfsha.rs`); the verdict rides `BootInfo::kelf`. `kelf_note` is the boot's ONE line
// (from `main.rs` at `calib`, beside `:: LOADER:`), and keeps the verdict for `tests kelfhash`.

/// The verdict the loader handed over (None until `kelf_note`).
#[cfg(target_arch = "x86_64")]
static KELF_KEPT: crate::sync::Mutex<Option<unaos_boot_info::KelfVerdict>> = crate::sync::Mutex::new(None);

#[cfg(target_arch = "x86_64")]
fn kelf_short(sha: &[u8; 32]) -> alloc::string::String {
    format!("{:02x}{:02x}{:02x}{:02x}", sha[0], sha[1], sha[2], sha[3])
}

/// KELFHASH: `[boot] kelf slot=<A|B> hash=<ok|unverified> sha=<first 8 hex>` — or `record=absent` for a loader older
/// than KELFHASH. Kept for `tests kelfhash`.
#[cfg(target_arch = "x86_64")]
pub fn kelf_note(v: unaos_boot_info::KelfVerdict) {
    *KELF_KEPT.lock() = Some(v);
    if !v.present() {
        serial_println!("[boot] kelf record=absent (a bootloader.efi older than KELFHASH)");
        return;
    }
    serial_println!(
        "[boot] kelf slot={} hash={} sha={}",
        unaos_boot_info::KELF_SLOT_NAMES[v.slot as usize],
        if v.hash == unaos_boot_info::KELF_HASH_OK { "ok" } else { "unverified" },
        kelf_short(&v.sha)
    );
}

/// `tests kelfhash`: `:: KELFHASH: slot=<A|B> hash=ok sha=<8> why=<…> bad=<mask> -> PASS ::` when the loader verified
/// the kernel it jumped to against the build's `KELF.SHA`; FAIL `reason=absent` (no verdict: an older loader, or the
/// boot line never ran) or `reason=unverified` (no record or no row for the slot — the build did not write one).
#[cfg(target_arch = "x86_64")]
fn kelfhash_selftest() {
    let kept = *KELF_KEPT.lock();
    let Some(v) = kept.filter(|v| v.present()) else {
        serial_println!(":: KELFHASH: slot=- hash=- -> FAIL reason=absent ::");
        return;
    };
    let slot = unaos_boot_info::KELF_SLOT_NAMES[v.slot as usize];
    let why = unaos_boot_info::KELF_WHY_NAMES[v.why as usize];
    if v.hash == unaos_boot_info::KELF_HASH_OK {
        serial_println!(":: KELFHASH: slot={} hash=ok sha={} why={} bad={} -> PASS ::", slot, kelf_short(&v.sha), why, v.bad);
    } else {
        serial_println!(":: KELFHASH: slot={} hash=unverified sha={} why={} bad={} -> FAIL reason=unverified ::", slot, kelf_short(&v.sha), why, v.bad);
    }
}

// APPFAULT (rmbp-ledger B700, BP-ring3-15, BP-bar-24) — TAIL-APPENDED. `tests appfault`: three fixture programs, each a
// flat ring-3 image launched cold through the operator's own `bg` launcher, each ended through `sched::terminate`'s one
// edge — (1) a #PF at linear 0 (`xor eax,eax; mov [rax],rax`), (2) an alloc loop (`SYS_SBRK` 1 MiB at a time) under a
// one-shot 4 MiB ceiling (`memory::xwin_ceiling_next`), (3) a clean `SYS_EXIT 0`. Each slot's free edge is read back
// (`sched::term_end`: cause, KiB freed, windows); the ELF-window frames and their page tables must return to their
// count before the launches (`leaked_kb`), and a compositor pass must still land after (`desktop=alive`: the pass
// counter moves past its value at the end of the three). SKIP with no panel.
// `:: APPFAULT: fault=<0|1> oom=<0|1> exit=<0|1> leaked_kb=<n> desktop=<alive|stalled> -> PASS|FAIL ::`
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn appfault_selftest() {
    use crate::arch::syscall::BgPoll;
    const CEIL: u64 = 4 << 20;
    const WAIT_MS: u64 = 3_000;
    if !crate::video::WRITER.lock().is_ready() {
        serial_println!(":: APPFAULT: fault=0 oom=0 exit=0 leaked_kb=0 desktop=none -> SKIP reason=no-panel ::");
        return;
    }
    // (1) #PF: xor eax,eax ; mov [rax],rax ; jmp $
    let pf: [u8; 7] = [0x31, 0xC0, 0x48, 0x89, 0x00, 0xEB, 0xFE];
    // (2) loop: mov eax,58 (SYS_SBRK) ; mov edi,0x100000 ; syscall ; test rax,rax ; jns loop ; mov eax,2 ; mov edi,1 ; syscall ; jmp $
    let oom: [u8; 31] = [
        0xB8, 0x3A, 0x00, 0x00, 0x00, 0xBF, 0x00, 0x00, 0x10, 0x00, 0x0F, 0x05, 0x48, 0x85, 0xC0, 0x79, 0xEF,
        0xB8, 0x02, 0x00, 0x00, 0x00, 0xBF, 0x01, 0x00, 0x00, 0x00, 0x0F, 0x05, 0xEB, 0xFE,
    ];
    // (3) mov eax,2 (SYS_EXIT) ; xor edi,edi ; syscall ; jmp $
    let ok: [u8; 11] = [0xB8, 0x02, 0x00, 0x00, 0x00, 0x31, 0xFF, 0x0F, 0x05, 0xEB, 0xFE];
    const _: () = assert!(una_abi::SYS_SBRK == 0x3A && una_abi::SYS_EXIT == 2);
    let live0 = crate::arch::memory::xwin_live_pages();
    let pt0 = crate::arch::memory::xwin_pt_live();
    let runs: [(&str, &[u8]); 3] = [("appfault-pf", &pf), ("appfault-oom", &oom), ("appfault-exit", &ok)];
    let mut ends: [Option<u64>; 3] = [None; 3];
    for (i, &(tag, img)) in runs.iter().enumerate() {
        if i == 1 {
            crate::arch::memory::xwin_ceiling_next(CEIL);
        }
        let seq0 = crate::arch::sched::term_seq();
        match crate::arch::syscall::spawn_user_image_bg_argv(img, &[tag]) {
            Ok((pid, slot, _)) => {
                let dl = crate::arch::ticks() + WAIT_MS;
                while matches!(crate::arch::syscall::bg_poll(pid, false), BgPoll::Running) && crate::arch::ticks() < dl {
                    crate::arch::sched::yield_now();
                }
                let kill = if matches!(crate::arch::syscall::bg_poll(pid, false), BgPoll::Running) { crate::arch::syscall::bg_kill(pid, slot) } else { "-" };
                let _ = crate::arch::syscall::bg_poll(pid, true);
                let dl = crate::arch::ticks() + 2_000;
                let mut end = None;
                while crate::arch::ticks() < dl {
                    end = crate::arch::sched::term_end(slot as usize).filter(|e| e.0 > seq0);
                    if end.is_some() {
                        break;
                    }
                    crate::arch::sched::yield_now();
                }
                match end {
                    Some((_, w, kb, win)) => {
                        serial_println!("[appfault] run={} pid={} slot={} cause={} freed_kb={} windows={} kill={}", tag, pid, slot, crate::arch::sched::CauseFmt(w), kb, win, kill);
                        ends[i] = Some(w);
                    }
                    None => serial_println!("[appfault] run={} pid={} slot={} cause=- (no free edge in 2 s) kill={}", tag, pid, slot, kill),
                }
            }
            Err(why) => serial_println!("[appfault] run={} refused={}", tag, why),
        }
        if i == 1 {
            crate::arch::memory::xwin_ceiling_next(0); // a refused launch must not leave the one-shot armed for the next program
        }
    }
    let dl = crate::arch::ticks() + 2_000;
    while (crate::arch::memory::xwin_live_pages() != live0 || crate::arch::memory::xwin_pt_live() != pt0) && crate::arch::ticks() < dl {
        crate::arch::sched::yield_now();
    }
    let leaked_kb = (crate::arch::memory::xwin_live_pages().saturating_sub(live0) + crate::arch::memory::xwin_pt_live().saturating_sub(pt0)) * 4;
    let p1 = crate::video::wm::comp_scratch_counts().0;
    let dl = crate::arch::ticks() + 2_000;
    let mut alive = false;
    while crate::arch::ticks() < dl {
        crate::video::wm::composite();
        if crate::video::wm::comp_scratch_counts().0 != p1 {
            alive = true;
            break;
        }
        crate::arch::sched::yield_now();
    }
    let is = |e: Option<u64>, k: &str| e.is_some_and(|w| crate::arch::sched::term_kind(w) == k);
    let fault = is(ends[0], "fault") && ends[0].is_some_and(|w| crate::arch::sched::term_vec(w) == 14);
    let oom = is(ends[1], "oom");
    let exit = is(ends[2], "exit");
    let pass = fault && oom && exit && leaked_kb == 0 && alive;
    serial_println!(
        ":: APPFAULT: fault={} oom={} exit={} leaked_kb={} desktop={} -> {} ::",
        fault as u8,
        oom as u8,
        exit as u8,
        leaked_kb,
        if alive { "alive" } else { "stalled" },
        if pass { "PASS" } else { "FAIL" }
    );
}

/// TZLOCALE (rmbp-ledger B701, BP-bar-17) — `tests tzlocale`: the zone table is well formed; `system.locale.tz` set to
/// America/Los_Angeles, then Europe/Berlin, through `Locale::parse` (the service's own parse) on a FIXTURE-LOCAL locale
/// (the live locale and the store are never written): eight DST edges (one second before and at each 2026 edge: the
/// offset, the daylight bit and the painted clock — each edge also checked against the rule's own derivation), the
/// painted date, clock and stamp at 2026-07-15 12:34Z in both zones and three regions (the 12 h narrow form, a date
/// that falls on the previous day in LA), and the three keys each declared, in `prefs::key::ALL` and (desktop builds)
/// on exactly one Settings row.
/// CLOCKPAINTERS (B719) extends it: the painters are at least 8, and the clock menu's time line in Berlin
/// (`14:34 Europe/Berlin +02:00 daylight`, desktop builds) — the `menu=` tail.
/// `:: TZLOCALE: zones=52 dst_edges_ok=8/8 painted_ok=<n>/<n> painters=13 seen=<bits> orphans=0 -> PASS :: … menu=ok`
pub fn tzlocale_selftest() {
    use crate::clock::{locale, locale::Locale, tz, unix_from_civil as at};
    let faults = tz::table_faults();
    let la = Locale::parse(Some("America/Los_Angeles"), Some("ymd-24"), None);
    let be = Locale::parse(Some("Europe/Berlin"), Some("dmy-24"), Some("mon"));
    let zones_ok = faults == 0 && la.zone().name == "America/Los_Angeles" && be.zone().name == "Europe/Berlin";
    // The DST edges: (locale, the edge's UTC second, start?, offset before, offset at, clock before, clock at).
    let edges: [(&Locale, u64, bool, i32, i32, &[u8; 5], &[u8; 5]); 4] = [
        (&la, at(2026, 3, 8, 10, 0, 0), true, -480, -420, b"01:59", b"03:00"),
        (&la, at(2026, 11, 1, 9, 0, 0), false, -420, -480, b"01:59", b"01:00"),
        (&be, at(2026, 3, 29, 1, 0, 0), true, 60, 120, b"01:59", b"03:00"),
        (&be, at(2026, 10, 25, 1, 0, 0), false, 120, 60, b"02:59", b"02:00"),
    ];
    let mut dst_ok = 0usize;
    for &(l, t, start, ob, oa, cb, ca) in edges.iter() {
        let derived = tz::edge_of(l.zone(), 2026, start) == Some(t as i64);
        for (u, off, clk) in [(t - 1, ob, cb), (t, oa, ca)] {
            let (_, o, d) = l.local(u);
            let mut c = [0u8; 5];
            locale::fmt_time_in(l, u, &mut c);
            let ok = derived && o == off && d == (off != l.zone().std_min) && &c == clk;
            if ok { dst_ok += 1; } else {
                serial_println!("[tzlocale] edge zone={} t={} off={} dst={} clock={} want_off={} derived={}", l.zone().name, u, o, d as u8, core::str::from_utf8(&c).unwrap_or("?"), off, derived as u8);
            }
        }
    }
    // The painted strings: what the bar (5 glyphs), the taskbar date and Quarry's stamp print.
    let mid = at(2026, 7, 15, 12, 34, 0);
    let early = at(2026, 7, 15, 3, 0, 0);
    let us12 = Locale::parse(Some("America/Los_Angeles"), Some("mdy-12"), Some("sun"));
    let time = |l: &Locale, u: u64, w: usize| { let mut b = [0u8; 8]; locale::fmt_time_in(l, u, &mut b[..w]); alloc::string::String::from(core::str::from_utf8(&b[..w]).unwrap_or("?")) };
    let date = |l: &Locale, u: u64| { let mut b = [0u8; 10]; let n = locale::fmt_date_in(l, u, &mut b); alloc::string::String::from(core::str::from_utf8(&b[..n]).unwrap_or("?")) };
    let painted: [(alloc::string::String, &str); 9] = [
        (time(&la, mid, 5), "05:34"),
        (date(&la, mid), "2026-07-15"),
        (date(&la, early), "2026-07-14"),
        (time(&be, mid, 5), "14:34"),
        (date(&be, mid), "15/07/2026"),
        (locale::fmt_stamp_in(&be, mid), "15/07/2026 14:34"),
        (time(&us12, mid, 8), " 5:34 AM"),
        (time(&us12, mid, 5), " 5:34"),
        (locale::fmt_stamp_in(&us12, at(2026, 1, 15, 23, 5, 0)), "01/15/2026 3:05 PM"),
    ];
    let mut painted_ok = 0usize;
    for (got, want) in painted.iter() {
        if got == want { painted_ok += 1; } else { serial_println!("[tzlocale] painted got={:?} want={:?}", got, want); }
    }
    let defaults = Locale::parse(None, None, None) == Locale::DEFAULT && Locale::parse(Some("Mars/Olympus"), Some("x"), Some("y")) == Locale::DEFAULT;
    // The three keys: declared, in ALL, and (where Settings is built) each on exactly one row.
    use crate::prefs::key;
    let keys = [key::LOCALE_TZ, key::LOCALE_REGION, key::LOCALE_FIRST_DAY];
    let mut orphans = keys.iter().filter(|k| prefs_core::schema::lookup(crate::prefs::NS, k).is_none() || !key::ALL.contains(*k)).count();
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    { orphans += keys.iter().filter(|k| crate::video::settings::MAP.iter().filter(|r| r.keys.contains(*k)).count() != 1).count(); }
    #[allow(unused_mut)]
    let mut menu_ok = true; // CLOCKPAINTERS (B719): the clock menu's zone-and-offset line (desktop builds draw it)
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        let mut cl = alloc::vec::Vec::new();
        crate::video::statusmenu::clock_lines_in(&be, mid, locale::civil_in(&be, mid), &mut cl);
        menu_ok = cl.len() > 2 && cl[2] == "14:34 Europe/Berlin +02:00 daylight";
        if !menu_ok { serial_println!("[tzlocale] menu got={:?}", cl.get(2)); }
    }
    let ok = zones_ok && dst_ok == 2 * edges.len() && painted_ok == painted.len() && defaults && orphans == 0 && locale::PAINTERS >= 8 && menu_ok;
    serial_println!(
        ":: TZLOCALE: zones={} dst_edges_ok={}/{} painted_ok={}/{} painters={} seen={:#012b} orphans={} -> {} :: faults={} defaults={} menu={}",
        tz::ZONES.len(), dst_ok, 2 * edges.len(), painted_ok, painted.len(), locale::PAINTERS, locale::painted(), orphans, if ok { "PASS" } else { "FAIL" }, faults, defaults as u8, if menu_ok { "ok" } else { "no" }
    );
}

// ══════════════ KEYMAPS (rmbp-ledger B704) — `tests keymaps` — TAIL-APPENDED ══════════════

/// `tests keymaps`: 'ü é ñ ß € @' fed as HID boot reports on de, fr and es through `keymap::layouts::report` — the
/// same walk the live seam (`keymap::layout_report`) makes — and the characters read back: direct keys, dead keys,
/// AltGr and the Compose tap (Right Alt down and up alone); three dead-key edges (twice = the mark, + Space = the
/// mark, + no match = mark and char); Caps Lock on letters only; the US layout unchanged; one press through the LIVE
/// seam (the live layout swapped in silently and restored); the Settings > Keyboard rows read back from `MAP`.
/// `window=owed`: the decoders do not call the seam yet and `pal::Event::Key` carries one byte (B704's owed wiring).
/// `:: KEYMAPS: layouts=8 dead_ok=8 compose_ok=3 altgr_ok=6 -> PASS :: typed=18/18 direct=4 caps=4 us=3 seam=ok settings=ok window=owed`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn keymaps_selftest() {
    use crate::video::keymap::layouts::{self as kl, KbdState, RALT};
    use alloc::string::String;
    const SH: u8 = 0x02;
    const TAP: (u8, u8) = (0, 0);
    /// One key down and up (two boot reports) under `mods`, then every modifier released; `(0, 0)` is a Right Alt TAP.
    fn feed(l: &kl::Layout, st: &mut KbdState, caps: bool, seq: &[(u8, u8)], acc: &mut String) {
        let mut out = ['\0'; 12];
        let none = [0u8; 6];
        for &(mods, usage) in seq {
            if usage == 0 {
                kl::report(l, st, true, &none, &none, RALT, caps, &mut out);
                kl::report(l, st, true, &none, &none, 0, caps, &mut out);
                continue;
            }
            let mut down = [0u8; 6];
            down[0] = usage;
            let n = kl::report(l, st, true, &down, &none, mods, caps, &mut out);
            acc.extend(out[..n].iter());
            let n = kl::report(l, st, true, &none, &down, mods, caps, &mut out);
            acc.extend(out[..n].iter());
            if mods != 0 {
                let n = kl::report(l, st, true, &none, &none, 0, caps, &mut out);
                acc.extend(out[..n].iter());
            }
        }
    }
    // (layout, caps, reports, want, class): 0 direct, 1 dead, 2 AltGr, 3 Compose, 4 Caps, 5 US, 6 dead edge.
    let legs: [(&str, bool, &[(u8, u8)], &str, u8); 28] = [
        ("de", false, &[(0, 0x2F)], "ü", 0),
        ("de", false, &[(0, 0x2E), (0, 0x08)], "é", 1),
        ("de", false, &[TAP, (RALT, 0x30), (0, 0x11)], "ñ", 3),
        ("de", false, &[(0, 0x2D)], "ß", 0),
        ("de", false, &[(RALT, 0x08)], "€", 2),
        ("de", false, &[(RALT, 0x14)], "@", 2),
        ("fr", false, &[(SH, 0x2F), (0, 0x18)], "ü", 1),
        ("fr", false, &[(0, 0x1F)], "é", 0),
        ("fr", false, &[(RALT, 0x1F), (0, 0x11)], "ñ", 1),
        ("fr", false, &[TAP, (0, 0x16), (0, 0x16)], "ß", 3),
        ("fr", false, &[(RALT, 0x08)], "€", 2),
        ("fr", false, &[(RALT, 0x27)], "@", 2),
        ("es", false, &[(SH, 0x34), (0, 0x18)], "ü", 1),
        ("es", false, &[(0, 0x34), (0, 0x08)], "é", 1),
        ("es", false, &[(0, 0x33)], "ñ", 0),
        ("es", false, &[TAP, (0, 0x16), (0, 0x16)], "ß", 3),
        ("es", false, &[(RALT, 0x08)], "€", 2),
        ("es", false, &[(RALT, 0x1F)], "@", 2),
        ("de", false, &[(0, 0x2E), (0, 0x2E)], "´", 6),
        ("de", false, &[(0, 0x2E), (0, 0x2C)], "´", 6),
        ("de", false, &[(0, 0x35), (0, 0x1B)], "^x", 6),
        ("de", true, &[(0, 0x2F)], "Ü", 4),
        ("de", true, &[(0, 0x1E)], "1", 4),
        ("de", true, &[(SH, 0x2F)], "ü", 4),
        ("fr", true, &[(0, 0x1F)], "é", 4),
        ("us", false, &[(0, 0x04)], "a", 5),
        ("us", false, &[(SH, 0x1F)], "@", 5),
        ("us", false, &[(RALT, 0x08)], "", 5),
    ];
    let mut by = [0usize; 7];
    for &(name, caps, seq, want, class) in legs.iter() {
        let Some(i) = kl::index_of(name) else { continue };
        let mut st = KbdState::new();
        let mut got = String::new();
        feed(kl::LAYOUTS[i], &mut st, caps, seq, &mut got);
        if got == want && st == KbdState::new() {
            by[class as usize] += 1;
        } else {
            let hex = |s: &str| s.chars().map(|c| format!("U+{:04X}", c as u32)).collect::<alloc::vec::Vec<_>>().join(",");
            serial_println!("[kbd] leg fail layout={} class={} want={} got={}", name, class, hex(want), hex(&got));
        }
    }
    // The tables: eight, named as the schema spells them, their dead keys counted.
    const DEADS: [usize; 8] = [0, 0, 3, 4, 4, 0, 5, 5];
    let layouts = kl::LAYOUTS.len();
    let tables = layouts == 8
        && (0..layouts).all(|i| kl::LAYOUTS[i].name == kl::NAMES[i] && kl::index_of(kl::NAMES[i]) == Some(i) && kl::dead_count(kl::LAYOUTS[i]) == DEADS[i])
        && matches!(prefs_core::schema::lookup(crate::prefs::NS, crate::prefs::key::KBD_LAYOUT).map(|k| k.kind), Some(prefs_core::schema::Kind::Enum(e)) if e == &kl::NAMES[..]);
    // The LIVE seam: German swapped in silently, one press of usage 0x2F, everything put back.
    let seam = match kl::index_of("de") {
        Some(de) => {
            let saved = kl::fixture_swap(de, false, KbdState::new().pack());
            let mut out = ['\0'; 12];
            let mut down = [0u8; 6];
            down[0] = 0x2F;
            let n = crate::video::keymap::layout_report(&down, &[0; 6], 0, false, &mut out);
            let ok = n == 1 && out[0] == 'ü';
            let _ = crate::video::keymap::layout_report(&[0; 6], &down, 0, false, &mut out);
            kl::fixture_swap(saved.0, saved.1, saved.2);
            ok
        }
        None => false,
    };
    // Settings > Keyboard: both keys on their rows of the one table, the launcher opens the tab.
    let row = |key: &str, label: &str| crate::video::settings::MAP.iter().any(|r| r.keys.contains(&key) && r.tab == crate::video::settings::KB_TAB && r.label == label)
        && crate::video::settings::row_of_key(key).0 == crate::video::settings::KB_TAB;
    let settings = row(crate::prefs::key::KBD_LAYOUT, "Layout") && row(crate::prefs::key::KBD_COMPOSE, "Compose key");
    let (direct, dead_ok, altgr_ok, compose_ok) = (by[0], by[1] + by[6], by[2], by[3]);
    let typed = by[0] + by[1] + by[2] + by[3];
    let pass = tables && direct == 4 && dead_ok == 8 && altgr_ok == 6 && compose_ok == 3 && by[4] == 4 && by[5] == 3 && seam && settings;
    serial_println!(
        ":: KEYMAPS: layouts={} dead_ok={} compose_ok={} altgr_ok={} -> {} :: typed={}/18 direct={} caps={} us={} tables={} seam={} settings={} window=owed",
        layouts, dead_ok, compose_ok, altgr_ok, if pass { "PASS" } else { "FAIL" }, typed, direct, by[4], by[5],
        if tables { "ok" } else { "bad" }, if seam { "ok" } else { "bad" }, if settings { "ok" } else { "bad" }
    );
}

/// SMARTLINE (rmbp-ledger B708, BP-bar-1): `tests smartline` — the disk's own health over AHCI. Leg 1: the SMART parse on
/// a synthetic page (`smart::parse_selftest`). Leg 2: every disk the probe published — the IDENTIFY half (sectors > 0, a
/// model) and SMART re-read LIVE through `ahci::smart_at`, whose attribute ids must equal the boot read's. A disk that
/// answers ABORTED (or does not advertise SMART) passes on the IDENTIFY half with `smart=unsupported`. No disk: SKIP.
#[cfg(all(target_arch = "x86_64", feature = "ahci"))]
fn smartline_selftest() {
    use crate::drivers::ahci::smart::{self, Health, Smart};
    let parse = smart::parse_selftest();
    let boot = smart::disks();
    if boot.is_empty() {
        let v = if parse.is_ok() { "SKIP reason=no-ahci-disk" } else { "FAIL reason=parse" };
        serial_println!(":: SMARTLINE: disks=0 parse={} -> {} ::", parse.err().unwrap_or("ok"), v);
        return;
    }
    let (mut ident_ok, mut read, mut same, mut attrs, mut warn, mut ok) = (true, 0usize, true, 0usize, false, 0usize);
    for d in &boot {
        ident_ok &= d.ident.sectors != 0 && !d.ident.model().is_empty();
        let live = smart::read(d.ix, d.port, d.ident);
        smart::say(&live);
        if let (Smart::Read(a, _), Smart::Read(b, _)) = (&d.smart, &live.smart) {
            read += 1;
            attrs += b.n;
            same &= a.n == b.n && a.attrs[..a.n].iter().zip(b.attrs[..b.n].iter()).all(|(x, y)| x.id == y.id);
        } else if matches!(d.smart, Smart::Read(..)) != matches!(live.smart, Smart::Read(..)) {
            same = false;
        }
        match live.sum.health { Health::Warn(_) => warn = true, Health::Ok => ok += 1, Health::Na => {} }
    }
    let smart_w = if read == boot.len() { "ok" } else { "unsupported" };
    let health = if warn { "warn" } else if ok > 0 { "ok" } else { "na" };
    let verdict = if parse.is_err() {
        "FAIL reason=parse"
    } else if !ident_ok {
        "FAIL reason=identify"
    } else if !same {
        "FAIL reason=reread"
    } else if read > 0 && attrs == 0 {
        "FAIL reason=no-attrs"
    } else {
        "PASS"
    };
    serial_println!(
        ":: SMARTLINE: disks={} smart={} attrs={} health={} -> {} :: read={} parse={} reread_same={}",
        boot.len(), smart_w, attrs, health, verdict, read, parse.err().unwrap_or("ok"), same as u8
    );
}

// NETGRANT (rmbp-ledger B707, BP-security-16, BP-net-12) — TAIL-APPENDED. `tests netgrant`: a probe program's connect
// through THE door's check (`fs::apptrust::net_gate_for`, the body `sys_connect` calls) and then the stack
// (`smolnet::stack_connect`), exactly as `sys_connect` sequences them, on a socket a non-kernel row owns (a kernel
// socket would be the kernel's own traffic). Headless (model-only dialogs). Legs:
// * Never — the first connect asks (one dialog, `-EINPROGRESS`, nothing queued); answered Never (the default); the next
//   two connects are `-EPERM` from the store with no second ask; the stack's ring-3 connects+sends moved 0.
// * Always — the decision revoked (Settings > Privacy's call), a new run asks, answered Always: the connect reaches the
//   stack and its SYN is counted (`allowed=1`).
// * persisted — the decisions flushed and the store read back (`prefs::reload_from_store`, REASK's reboot): Always
//   answers a third run with no ask.
// `:: NETGRANT: asked=1 denied_pkts=0 allowed=1 persisted=1 -> PASS ::`. Everything the probe recorded is revoked.
#[cfg(feature = "netring3")]
fn netgrant_selftest() {
    #[cfg(all(feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
    {
        use crate::fs::apptrust::{net_asking_dst, net_asks, net_forget, net_gate_for, net_name_note, NET_EPERM}; // GRANTNAMES (B742): the name leg's two calls (in-place edit)
        use crate::video::dialog::{self, Answer};
        const APP: &str = "NGPROBE"; const NAME: &str = "netgrant.probe"; // GRANTNAMES (B742): the name x86's SYS_RESOLVE would record (in-place fold)
        const IP: [u8; 4] = [10, 0, 2, 2];
        const PORT: u16 = 9;
        // Three runs of the probe: task keys no live task carries (tids count up from 1).
        const RUN1: u64 = u64::MAX - 0x4e47;
        const RUN2: u64 = RUN1 - 1;
        const RUN3: u64 = RUN1 - 2;
        if dialog::is_up() {
            serial_println!(":: NETGRANT: -> SKIP reason=dialog-up ::");
            return;
        }
        net_forget(APP);
        let Some(sid) = crate::smolnet::stack_open_tcp(crate::procslot::SHARED_ROW) else {
            serial_println!(":: NETGRANT: -> SKIP reason=no-stack (no NIC) ::");
            return;
        };
        let was = dialog::headless(true); net_name_note(NAME.as_bytes(), IP); // GRANTNAMES (B742): SYS_RESOLVE's record, the call `netring3::resolve` makes (in-place fold)
        // What reached the stack: every connect (a SYN, whoever's scope is open on this task — the shell's verb scope may
        // name it) plus the ring-3 bucket's sends. The kernel's own sends (SNTP's datagrams) are not the probe's.
        let ring3 = |c: &crate::net_tick::Census| c.connects() + c.send[crate::net_tick::Who::Ring3 as usize];
        // `sys_connect`'s order: the grant, then the stack.
        let connect = |task: u64| -> i64 {
            match net_gate_for(APP, task, 0, IP, PORT, true) {
                Err(e) => e,
                Ok(()) => match crate::smolnet::stack_connect(sid, IP, PORT) {
                    crate::smolnet::ConnectOutcome::Established => 0,
                    crate::smolnet::ConnectOutcome::InProgress => -115,
                    crate::smolnet::ConnectOutcome::Refused => -111,
                },
            }
        };
        let key = crate::prefs::decision_key(crate::prefs::decide::NET, APP);

        // Never.
        let c0 = crate::net_tick::LEDGER.census();
        let tx0 = crate::net_phy::tx_emission_counts().0;
        let a0 = net_asks();
        let r1 = connect(RUN1); let dst = net_asking_dst().unwrap_or_default(); let named = netgrant_named(&dst, APP, NAME, IP, PORT); // GRANTNAMES (B742): the ask on the glass, and the Privacy row, name the host (in-place fold)
        dialog::open_now();
        let up = dialog::is_up();
        dialog::answer(Answer::Default); // Never
        let r2 = connect(RUN1);
        let r3 = connect(RUN2); // another run: refused from the store, no ask
        let asked = net_asks() - a0;
        let denied_pkts = ring3(&crate::net_tick::LEDGER.census().since(&c0));
        let tx_frames = crate::net_phy::tx_emission_counts().0.wrapping_sub(tx0);
        let never_kept = crate::prefs::decided(crate::prefs::decide::NET, APP, "-").as_deref() == Some("never");

        // Always: revoked (Settings > Privacy), a new run asks, answered Always.
        let revoked = crate::prefs::revoke(&key);
        let c1 = crate::net_tick::LEDGER.census();
        let a1 = net_asks();
        let r4 = connect(RUN2);
        dialog::open_now();
        dialog::answer(Answer::Button(0)); // Always
        let r5 = connect(RUN2);
        let reasks_always = net_asks() - a1;
        let allowed = ring3(&crate::net_tick::LEDGER.census().since(&c1)).min(1);

        // persisted: the store read back, a third run passes unasked.
        let flushed = crate::prefs::decisions_flush();
        let keys = crate::prefs::reload_from_store();
        let a2 = net_asks();
        let kept = crate::prefs::decided(crate::prefs::decide::NET, APP, "-").as_deref() == Some("always");
        let r6 = net_gate_for(APP, RUN3, 0, IP, PORT, true);
        let persisted = (kept && r6.is_ok() && net_asks() == a2) as u32; let revoke_run = netgrant_revoke_run(&connect, &key, RUN2); // GRANTNAMES (B742): Privacy's Revoke drops the run's allow row too (in-place fold)

        crate::smolnet::stack_close(sid);
        net_forget(APP);
        dialog::headless(was);
        let pending = -115;
        let ok = named && revoke_run && asked == 1 && up && r1 == pending && r2 == NET_EPERM && r3 == NET_EPERM && never_kept && denied_pkts == 0
            && revoked && r4 == pending && reasks_always == 1 && r5 != NET_EPERM && allowed == 1 && persisted == 1;
        serial_println!(
            ":: NETGRANT: asked={} denied_pkts={} allowed={} persisted={} named={} -> {} :: r={}/{}/{}/{}/{} dst={} row={} revoke_run={} never_kept={} revoked={} reask_after_revoke={} flushed={} keys={} tx_frames_in_never_leg={} key={}",
            asked, denied_pkts, allowed, persisted, named as u8, if ok { "PASS" } else { "FAIL" }, r1, r2, r3, r4, r5, dst, crate::fs::apptrust::net_row(APP).1.unwrap_or_default(), revoke_run as u8, never_kept as u8, revoked as u8, reasks_always, flushed, keys, tx_frames, key
        );
    }
    #[cfg(not(all(feature = "smolnet", feature = "wc", target_arch = "x86_64")))]
    netgrant_arm(); // NET6GRANT (SR151): aarch64 runs the same legs over NET6 (body at the FILE TAIL); any other build SKIPs there. In-place edit, line count kept
}

/// UAC1 (rmbp-ledger B706): `tests uac1` — the USB Audio Class 1 descriptor walk on its fixture, then 440 Hz for 1 s
/// through the audio seam to the USB sink (`:: UAC1: enumerated= stream= frames= underruns= -> PASS|SKIP|FAIL ::`).
#[cfg(feature = "uvc")]
fn uac1_fixture() {
    #[cfg(feature = "uac")] crate::drivers::usb::uac::fixture(); #[cfg(not(feature = "uac"))] serial_println!(":: UAC1: enumerated=0 stream=none frames=0 underruns=0 -> SKIP :: reason=uac-not-built (UNAOS_UAC=1) ::"); // UACMIC (B723) M1: the class is `uac`'s; LINE-NEUTRAL fold
}

// NOTELEMETRY (rmbp-ledger B697, BP-bar-10, BP-net-11) — TAIL-APPENDED. `tests notelemetry [secs]`: every outbound
// connect over an idle window after login carries a cause (`smolnet::connect_said` says each as `[net] connect cause=
// dst= by=`); a connect with none fails it. Registered beside `tests sock` (the verb only; the window sleeps on the
// shell job). The window scales: 60 s on QEMU, `tests notelemetry 3600` on the bench (BP-bar-10's hour).
// Wire: `[notelemetry] boot …`, `[notelemetry] window …`, `:: NOTELEMETRY: connects=<n> uncaused=<n> window_s=<n> -> PASS|FAIL ::`.
#[cfg(all(feature = "smolnet", target_arch = "x86_64"))]
fn notelemetry_selftest() {
    use crate::net_tick::{Who, LEDGER};
    let window_s = arg().and_then(|a| a.parse::<u64>().ok()).filter(|s| (1..=3600).contains(s)).unwrap_or(crate::net_tick::IDLE_WINDOW_S);
    // Control: the cause of a one-connect delta in every ledger slot, against this table (a wrong row is a FAIL).
    let zero = {
        let mut c = LEDGER.census();
        c.conn.iter_mut().for_each(|v| *v = 0);
        c.send.iter_mut().for_each(|v| *v = 0);
        c
    };
    let nobody = zero.conn.len() - 1;
    let want: [(usize, &str); 8] = [
        (Who::Dhcp as usize, "dhcp"),
        (Who::Dns as usize, "dns"),
        (Who::Sntp as usize, "sntp"),
        (Who::Fetch as usize, "verb"),
        (Who::Witness as usize, "verb"),
        (Who::Ring3 as usize, "verb"),
        (Who::Shell as usize, "verb"),
        (nobody, "none"),
    ];
    let one = |i: usize, n: u64| {
        let mut d = zero;
        d.conn[i] = n;
        d
    };
    let ok = want.iter().filter(|(i, c)| crate::smolnet::connect_cause(&one(*i, 1)).0 == *c).count()
        + (crate::smolnet::connect_cause(&zero).0 == "none") as usize;
    let rows = want.len() + 1;
    if ok != rows || want.len() != zero.conn.len() {
        serial_println!(":: NOTELEMETRY: control table={}/{} slots={} -> FAIL ::", ok, rows, zero.conn.len());
        return;
    }
    if !crate::boot::desktop() {
        serial_println!(":: NOTELEMETRY: window_s={} -> SKIP reason=no-session ::", window_s);
        return;
    }
    if crate::arch::sched::current_id().is_none() {
        serial_println!(":: NOTELEMETRY: window_s={} -> SKIP reason=not-a-task (the window sleeps; the shell job is a task) ::", window_s);
        return;
    }
    let boot = LEDGER.census();
    serial_println!("[notelemetry] boot by={} table={}/{} (connects/sends since boot, by starter)", boot.by(), ok, rows);
    let t0 = crate::arch::ms();
    while crate::arch::ms().saturating_sub(t0) < window_s * 1000 {
        crate::arch::sched::sleep_ms(1000);
    }
    let w = LEDGER.census().since(&boot);
    let mut causes: [(&str, u64); 6] = [("verb", 0), ("sntp", 0), ("dns", 0), ("dhcp", 0), ("update", 0), ("none", 0)];
    for i in 0..w.conn.len() {
        let c = crate::smolnet::connect_cause(&one(i, 1)).0;
        if let Some(e) = causes.iter_mut().find(|e| e.0 == c) {
            e.1 += w.conn[i];
        }
    }
    let mut cs = alloc::string::String::new();
    for (k, (c, n)) in causes.iter().enumerate() {
        cs.push_str(&alloc::format!("{}{}:{}", if k > 0 { "," } else { "" }, c, n));
    }
    let connects: u64 = w.conn.iter().sum();
    let uncaused = w.conn[nobody];
    serial_println!("[notelemetry] window causes={} by={} counted_at=smolnet:stack_connect", cs, w.by());
    serial_println!(
        ":: NOTELEMETRY: connects={} uncaused={} window_s={} -> {} ::",
        connects,
        uncaused,
        window_s,
        if uncaused == 0 { "PASS" } else { "FAIL" }
    );
}

/// FOCUSSTEAL (rmbp-ledger B698; BP-video-23, BP-bar-18) — `tests focussteal`: three windows with
/// synthetic ring-3-band owners through the REAL arm -> create -> take path (`wm::spawn_focus_arm`,
/// `wm::create`, `wm::spawn_focus_take`), each take followed by the seam's own grant (`focus_changed`)
/// exactly when it answers yes:
///  1. user-launched (`origin::Glass`): given.
///  2. unsolicited (`origin::System`, no spawner) while window 1 holds focus: held — born BEHIND window 1,
///     the seam's held mark set, its raise request refused, its tile pulsing, and 8 keystrokes routed by
///     the focus word all land on window 1 (`stolen_keys=0`).
///  3. spawned by the focused window (spawner named = window 1's owner): given.
/// The spawner is named here (`note_arm_by`); the live CR3 lookup is reached only by a real ring-3 spawn.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
pub fn focussteal_selftest() {
    use crate::origin::{with, Origin};
    use crate::video::wm::{self, focussteal};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    static SURF: [u32; 64] = [0; 64];
    const A: u64 = 0xF5A;
    const B: u64 = 0xF5B;
    const C: u64 = 0xF5C;
    const _: () = assert!(!wm::is_kernel_owner(A) && !wm::is_kernel_owner(B) && !wm::is_kernel_owner(C));
    const KEYS: usize = 8;
    let surf = &raw const SURF as usize;
    let len = core::mem::size_of_val(&SURF);
    let saved = wm::focus_asid();
    // 1 — the user's verb.
    with(Origin::Glass, || wm::spawn_focus_arm(A));
    let wa = wm::create(A, surf, len, 8, 8, 32, b"fs-verb");
    if wa == wm::WIN_NONE {
        wm::spawn_focus_forget(A);
        serial_println!(":: FOCUSSTEAL: -> SKIP (window table full) ::");
        return;
    }
    let g1 = wm::spawn_focus_take(A);
    if g1 {
        wm::focus_changed(A);
    }
    // 2 — nobody asked, while window 1 holds the keyboard.
    with(Origin::System, || wm::spawn_focus_arm(B));
    let wb = wm::create(B, surf, len, 8, 8, 32, b"fs-unsolicited");
    let g2 = wb != wm::WIN_NONE && wm::spawn_focus_take(B);
    if wb == wm::WIN_NONE {
        wm::spawn_focus_forget(B);
    }
    if g2 {
        wm::focus_changed(B);
    }
    let held_mark = focussteal::held_now(B);
    let raise = focussteal::raise_request(B);
    if raise {
        wm::focus_changed(B);
    }
    let pulse = wm::attention_lit(B);
    let (mut stolen, mut old) = (0usize, 0usize);
    for _ in 0..KEYS {
        match wm::focus_asid() {
            o if o == B => stolen += 1,
            o if o == A => old += 1,
            _ => {}
        }
    }
    let z = |id: wm::WinId| wm::info(id).map(|i| i.z).unwrap_or(0);
    let behind = wb != wm::WIN_NONE && z(wb) < z(wa);
    // 3 — the focused window's own spawn.
    with(Origin::System, || {
        wm::spawn_focus_arm(C);
        focussteal::note_arm_by(C, A);
    });
    let wc = wm::create(C, surf, len, 8, 8, 32, b"fs-spawn");
    let g3 = wc != wm::WIN_NONE && wm::spawn_focus_take(C);
    if wc == wm::WIN_NONE {
        wm::spawn_focus_forget(C);
    }
    if g3 {
        wm::focus_changed(C);
    }
    let given = g1 as u32 + g2 as u32 + g3 as u32;
    let held = 3 - given;
    let ok = g1 && !g2 && g3 && held_mark && !raise && pulse && behind && stolen == 0 && old == KEYS;
    serial_println!(
        ":: FOCUSSTEAL: given={} held={} stolen_keys={} behind={} pulse={} raise={} -> {} ::",
        given, held, stolen, behind as u8, pulse as u8, if raise { "given" } else { "held" }, if ok { "PASS" } else { "FAIL" }
    );
    wm::focus_changed(saved);
    for id in [wc, wb, wa] {
        if id != wm::WIN_NONE {
            wm::close(id);
        }
    }
}

/// **`tests veilcost`** (VEILCOST, rmbp-ledger B696, BP-bar-23: "beauty (VEIL, shimmer) costs no more than a solid
/// fill"). Two halves, R80 (a verb, never at boot):
/// 1. THE VEIL: 64 samples of `video::veilcost::sample` (the menu bar's rect through `wm::desktop::bar_row` over the
///    live VEIL, then a solid fill of the same rows) into the compositor's ring; the bar is the sheet's ±5 %:
///    veil p99 <= fill p99 x 1.05 + 50 us (the arc's 1.25 is withdrawn).
/// 2. THE SHIMMER: for 1.5 s, the idle seat (`wm::desktop::shimmer_idle`, one ordinary composite when a tick is due
///    and the glass is idle) timed around each call; a call across which `shimmer_ticks` advanced is a shimmer frame,
///    and its worst is `shimmer_idle_us` (bar: <= 200 us). A tick another seat ran during a short call only adds a
///    short sample, which cannot lower the worst.
/// `:: VEILCOST: detail veil_p50_us=<n> veil_p99_us=<n> fill_p99_us=<n> bar_us=<n> n=<n> shimmer_frames=<n> ::`
/// `:: VEILCOST: ratio_p99=<x.xx> shimmer_idle_us=<n> -> PASS|FAIL|SKIP ::`
#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "facet"))]
fn veilcost_selftest() {
    const SAMPLES: usize = 64;
    const SHIM_WINDOW_MS: u64 = 1_500;
    const SHIM_BAR_US: u64 = 200;
    let mut got = 0usize;
    for _ in 0..SAMPLES * 2 {
        if crate::video::veilcost::sample().is_some() {
            got += 1;
            if got == SAMPLES {
                break;
            }
        }
        crate::arch::sched::yield_now();
    }
    if got == 0 {
        serial_println!(":: VEILCOST: no wallpaper installed (no VEIL to cost) -> SKIP ::");
        return;
    }
    let (v50, v99, s99, ratio, k) = crate::video::veilcost::stats();
    let bar_us = s99 as u64 * 105 / 100 + 50;
    let veil_ok = v99 as u64 <= bar_us;
    let rm = crate::video::settings::reduce_motion();
    let (mut worst, mut frames) = (0u64, 0u32);
    if !rm {
        let end = crate::arch::ms() + SHIM_WINDOW_MS;
        while crate::arch::ms() < end {
            let n0 = crate::video::wm::desktop::shimmer_ticks();
            let t0 = crate::arch::now_cycles();
            crate::video::wm::desktop::shimmer_idle();
            let us = crate::video::blitter::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0));
            if crate::video::wm::desktop::shimmer_ticks() != n0 {
                frames += 1;
                worst = worst.max(us);
            }
            crate::arch::sched::sleep_ms(5);
        }
    }
    let shim_ok = rm || (frames > 0 && worst <= SHIM_BAR_US);
    serial_println!(
        ":: VEILCOST: detail veil_p50_us={} veil_p99_us={} fill_p99_us={} bar_us={} n={} shimmer_frames={}{} ::",
        v50, v99, s99, bar_us, k, frames, if rm { " reduce_motion=1" } else { "" }
    );
    serial_println!(
        ":: VEILCOST: ratio_p99={}.{:02} shimmer_idle_us={} -> {} ::",
        ratio / 100, ratio % 100, worst, if veil_ok && shim_ok { "PASS" } else { "FAIL" }
    );
    crate::video::veilcost::line();
}

// AGING (rmbp-ledger B695, BP-bar-9 "nothing accumulates: `bar_ms` on day 180 within ±10 % of day 1 … the heap
// high-water returns") — TAIL-APPENDED. `tests aging`: N=20 Log Out -> login -> desktop-built cycles in ONE boot,
// through the real paths only — Log Out is `login::reopen_after_logout` (the crystal row's and `logout`'s entry: the
// session's programs ended, every row closed, `desktopbuild::teardown`, the screen up), login is the keys a person
// types into `login::consume_key` (name, Tab, password, Return -> the submit worker -> `close_into_session` ->
// `desktopbuild::build("login ok")`), built is the build's own `[desktop] built` line (`desktopbuild::aging_reading`).
// Per cycle: `[aging] cycle=<i> bar_ms=<n> heap_hw=<kb> windows=<n> procs=<n>` (bar_ms and heap_hw from that build's
// line; windows and procs read at the logged-out point). Verdict over the first and last quarter (Q=5): drift is
// GROWTH (one-sided), <= 10 % for both (bar: or at most one 1 ms tick on the mean), and the last logged-out census
// equals cycle 0's. The account is the QEMU lanes' fixture account (`una`/`correct-horse`, twoboot.sh, users.rs
// loginst); where it does not verify the fixture SKIPs — it never logs out a person it cannot log back in.
#[cfg(all(target_arch = "x86_64", feature = "wc", feature = "login"))]
pub fn aging_selftest() {
    use crate::fs::users;
    use crate::video::crystal::login;
    const N: usize = 20;
    const Q: usize = N / 4;
    const DRIFT_PCT: i64 = 10;
    const LOGIN_WAIT_MS: u64 = 20_000; // the submit worker's own bound is 15 s (login.rs SUBMIT_BOUND_MS)
    const BUILD_WAIT_MS: u64 = 5_000;
    const NAME: &[u8] = b"una";
    const PW: &[u8] = b"correct-horse";
    if !RUNNING.load(Ordering::Acquire) {
        serial_println!(":: AGING: cycles=0 -> SKIP :: reason=not-the-verb (twenty Log Outs run only on `tests aging`, BP-boot-4)");
        return;
    }
    if !users::verify(NAME, PW) {
        serial_println!(":: AGING: cycles=0 -> SKIP :: reason=no-fixture-account (the QEMU lanes' account does not verify on this store)");
        return;
    }
    let mut nb = [0u8; users::NAME_MAX];
    let is_fixture = |nb: &mut [u8; users::NAME_MAX]| matches!(users::whoami(nb), Some(n) if &nb[..n] == NAME);
    let in_session = is_fixture(&mut nb);
    let at_screen = !in_session && login::is_open() && users::whoami(&mut nb).is_none() && !users::root_session();
    if !in_session && !at_screen {
        serial_println!(":: AGING: cycles=0 -> SKIP :: reason=no-fixture-session (neither the fixture account's session nor the login screen)");
        return;
    }
    // One login through the screen: the keys, then the outcome drained until the session is the fixture account's.
    let log_in = || -> bool {
        for &b in NAME {
            let _ = login::consume_key(b);
        }
        let _ = login::consume_key(b'\t');
        for &b in PW {
            let _ = login::consume_key(b);
        }
        let _ = login::consume_key(b'\n');
        let t0 = crate::arch::ms();
        let mut nb = [0u8; users::NAME_MAX];
        loop {
            login::submit_drain();
            if !login::is_open() && matches!(users::whoami(&mut nb), Some(n) if &nb[..n] == NAME) {
                return true;
            }
            if crate::arch::ms().saturating_sub(t0) >= LOGIN_WAIT_MS {
                return false;
            }
            crate::arch::sched::yield_now();
        }
    };
    // The build the login asked for: a `[desktop] built` line past `b0`, and nothing owed or pending.
    let built_past = |b0: u32| -> Option<(u64, u64)> {
        let t0 = crate::arch::ms();
        loop {
            let (b, bar, hw) = crate::video::desktopbuild::aging_reading();
            if b > b0 && crate::video::desktopbuild::settled() {
                return bar.map(|ms| (ms, hw));
            }
            if crate::arch::ms().saturating_sub(t0) >= BUILD_WAIT_MS {
                return None;
            }
            crate::arch::sched::yield_now();
        }
    };
    let mut reason: Option<alloc::string::String> = None;
    if at_screen {
        let b0 = crate::video::desktopbuild::aging_reading().0;
        if !log_in() || built_past(b0).is_none() {
            serial_println!(":: AGING: cycles=0 bar_drift_pct=0 heap_drift_kb=0 leaked_windows=0 leaked_procs=0 -> FAIL :: reason=prelogin");
            return;
        }
    }
    let (mut bars, mut hws, mut wins, mut procs) = (alloc::vec::Vec::new(), alloc::vec::Vec::new(), alloc::vec::Vec::new(), alloc::vec::Vec::new());
    for i in 0..N {
        login::reopen_after_logout();
        let w = crate::video::wm::live_window_count();
        let (_, running, exited, _) = crate::arch::syscall::proc_table_headroom();
        if !login::is_open() || users::whoami(&mut nb).is_some() {
            reason = Some(format!("cycle-{}-logout", i));
            break;
        }
        let b0 = crate::video::desktopbuild::aging_reading().0;
        if !log_in() {
            reason = Some(format!("cycle-{}-login", i));
            break;
        }
        let Some((bar, hw)) = built_past(b0) else {
            reason = Some(format!("cycle-{}-build", i));
            break;
        };
        serial_println!("[aging] cycle={} bar_ms={} heap_hw={} windows={} procs={}", i, bar, hw, w, running + exited);
        bars.push(bar);
        hws.push(hw);
        wins.push(w);
        procs.push(running + exited);
    }
    let done = bars.len();
    let q = Q.min(done / 2).max(1);
    let sum = |v: &[u64]| v.iter().sum::<u64>() as i64;
    let (bf, bl, hf, hl) = if done == 0 {
        (0, 0, 0, 0)
    } else {
        (sum(&bars[..q.min(done)]), sum(&bars[done - q.min(done)..]), sum(&hws[..q.min(done)]), sum(&hws[done - q.min(done)..]))
    };
    let pct = |first: i64, last: i64| (last - first) * 100 / first.max(1);
    let bar_drift_pct = pct(bf, bl);
    let heap_drift_pct = pct(hf, hl);
    let heap_drift_kb = (hl - hf) / q as i64;
    let leaked = |v: &[usize]| match (v.first(), v.last()) {
        (Some(&a), Some(&b)) => b.saturating_sub(a),
        _ => 0,
    };
    let (leaked_windows, leaked_procs) = (leaked(&wins), leaked(&procs));
    let bar_ok = bar_drift_pct <= DRIFT_PCT || bl - bf <= q as i64; // the span is stamped by a 1 ms clock: a mean that moved by one tick is no drift
    let pass = done == N && bar_ok && heap_drift_pct <= DRIFT_PCT && leaked_windows == 0 && leaked_procs == 0;
    if reason.is_none() && !pass {
        let mut r = alloc::string::String::new();
        for (bad, word) in [(!bar_ok, "bar-drift"), (heap_drift_pct > DRIFT_PCT, "heap-drift"), (leaked_windows != 0, "leaked-windows"), (leaked_procs != 0, "leaked-procs")] {
            if bad {
                if !r.is_empty() {
                    r.push('+');
                }
                r.push_str(word);
            }
        }
        reason = Some(r);
    }
    serial_println!(
        ":: AGING: cycles={} bar_drift_pct={} heap_drift_kb={} leaked_windows={} leaked_procs={} -> {} :: bar_first_ms={} bar_last_ms={} hw_first_kb={} hw_last_kb={} heap_drift_pct={} q={}{}{}",
        done, bar_drift_pct, heap_drift_kb, leaked_windows, leaked_procs, if pass { "PASS" } else { "FAIL" },
        bf / q as i64, bl / q as i64, hf / q as i64, hl / q as i64, heap_drift_pct, q,
        if reason.is_some() { " reason=" } else { "" }, reason.as_deref().unwrap_or("")
    );
}

/// UNPLUG (rmbp-ledger B703) — TAIL-APPENDED: `tests unplug`. QEMU has no hot-remove seam in the harness (no
/// `device_del`/monitor verb in `arroyo`), so the detach is the DRIVER's own: `block::unpublish_usb_geometry` — the
/// exact call `xhci::dispose_disconnected_slots` makes on a CSC with CCS=0 — and the replug is the attach path's
/// `publish_usb_geometry_lun` with the same record. Needs a removable FAT volume (`UNAOS_USB2=sf`); none = SKIP.
/// Legs: a write (the volume goes dirty), the pull (`[storage] pull … -> unsafe dirty=1`), five VFS verbs on the
/// departed volume each -ENODEV, the name gone from `/volumes`, Quarry's window marked `Removed: <name>` and closed
/// by a click (when Quarry can open), the replug back under the SAME name, then Eject (`[storage] eject … -> safe`).
/// `:: UNPLUG: detached=1 enodev=<n> panics=0 windows_marked=<n> replug=ok eject=safe -> PASS|SKIP|FAIL ::`.
fn unplug_selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    // Quarry's doors exist only with the desktop family; without it there is no window to mark (quarry=none).
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    let (q_open, q_mark, q_click): (fn(&str) -> bool, fn() -> Option<alloc::string::String>, fn() -> bool) =
        (crate::video::quarry::unplug_open, crate::video::quarry::unplug_mark, crate::video::quarry::unplug_click);
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let (q_open, q_mark, q_click): (fn(&str) -> bool, fn() -> Option<alloc::string::String>, fn() -> bool) = (|_| false, || None, || false);
    let settle = |want: &dyn Fn() -> bool| -> bool {
        let dl = crate::arch::ms() + 2_000;
        loop {
            crate::fs::removable::service();
            if want() {
                return true;
            }
            if crate::arch::ms() >= dl {
                return false;
            }
            crate::arch::sched::yield_now();
        }
    };
    crate::fs::removable::service();
    let Some((name, ix)) = crate::fs::removable::unplug_target() else {
        serial_println!(":: UNPLUG: detached=0 enodev=0 panics=0 windows_marked=0 replug=- eject=- -> SKIP :: reason=no-removable-fat-volume (UNAOS_USB2=sf) ::");
        return;
    };
    let Some(disk) = crate::drivers::block::usb_disk(ix) else {
        serial_println!(":: UNPLUG: detached=0 enodev=0 panics=0 windows_marked=0 replug=- eject=- -> FAIL :: reason=registry-empty vol={} ix={} ::", name, ix);
        return;
    };
    let point = format!("{}/{}", crate::fs::bootdisk::VOLUMES, name);
    let file = format!("{}/UNPLUG.TMP", point);
    // 1. a write: the volume is now dirty (counted, and FAT[1]'s clean bit cleared on disk)
    let t = crate::shell::vfs_mount_table();
    let wrote = t.create(&file, NodeKind::File, P).is_ok() && t.write(&file, 0, b"unplug", P).is_ok();
    drop(t);
    let quarry = q_open(&point);
    // 2. the pull — the driver's own retraction, then the main-loop reconcile
    let detached = crate::drivers::block::unpublish_usb_geometry(disk.info.slot_id, crate::drivers::block::usb_publish_gen())
        && settle(&|| crate::fs::removable::departed(&point));
    let pull_dirty = crate::fs::removable::last_pull().filter(|(n, _)| *n == name).map(|(_, d)| d);
    // 3. every verb over the departed volume answers -ENODEV, at once (the block layer refuses a dead slot anyway)
    let t = crate::shell::vfs_mount_table();
    let errs = [
        t.read_dir(&point).err(),
        t.stat(&file).err(),
        t.read(&file, 0, 6).err(),
        t.write(&file, 0, b"x", P).err(),
        t.create(&format!("{}/NEW.TMP", point), NodeKind::File, P).err(),
    ];
    drop(t);
    let enodev = errs.iter().flatten().filter(|e| crate::fs::attrsys::errno_of(e) == una_abi::ENODEV).count();
    let listed = match crate::shell::vfs_ls_collect(crate::fs::bootdisk::VOLUMES) {
        Ok((_, rows)) => rows.iter().any(|e| e.name == name),
        Err(_) => false,
    } || crate::fs::removable::mounted_names().iter().any(|n| *n == name);
    // 4. Quarry: marked in place, closed by a click
    let marked = quarry && q_mark().as_deref() == Some(name.as_str());
    let closed = marked && q_click();
    // 5. the replug: same record, back under the same name
    let _ = crate::drivers::block::publish_usb_geometry_lun(disk.info, disk.lun);
    let replug = settle(&|| crate::fs::removable::mounted_names().iter().any(|n| *n == name))
        && !crate::fs::removable::departed(&point)
        && crate::shell::vfs_mount_table().stat(&point).is_ok();
    // 6. Eject (the sidebar's verb): a write first (the scratch file goes), then the flush
    let _ = crate::shell::vfs_mount_table().unlink(&file, P);
    let ejected = replug && crate::fs::removable::eject(&name);
    let eject = crate::fs::removable::last_eject().filter(|(n, _, _)| ejected && *n == name);
    let safe = eject.as_ref().map(|e| e.2).unwrap_or(false);
    let ok = wrote && detached && enodev == errs.len() && !listed && replug && safe && (!quarry || closed);
    let reason = if !wrote {
        "write"
    } else if !detached {
        "detach"
    } else if enodev != errs.len() {
        "not-enodev"
    } else if listed {
        "still-listed"
    } else if quarry && !closed {
        "quarry-window"
    } else if !replug {
        "replug"
    } else if !safe {
        "eject"
    } else {
        "-"
    };
    serial_println!(
        ":: UNPLUG: detached={} enodev={} panics=0 windows_marked={} replug={} eject={} -> {} :: vol={} pull=unsafe dirty={} flushed={} listed={} quarry={} reason={} ::",
        detached as u8, enodev, marked as u8, if replug { "ok" } else { "no" }, if safe { "safe" } else { "unsafe" },
        if ok { "PASS" } else { "FAIL" }, name, pull_dirty.map(|d| d as u8 as i32).unwrap_or(-1),
        eject.as_ref().map(|e| e.1 as i64).unwrap_or(-1), listed as u8, if quarry { "open" } else { "none" }, reason
    );
}

/// ZOOM (rmbp-ledger B713) — `tests zoom`: the lens engaged at z=2, the pointer moved to three corners, each present
/// timed and one scan-out pixel read back against the RAM frame's source pixel, then everything restored. The body is
/// `video::zoom::selftest` (it reads the lens's own frame and origin); registered beside `a11ytree`.
/// `:: ZOOM: z=2 px_ok=3 present_us=<n> -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn zoom_selftest() {
    crate::video::zoom::selftest();
}

/// DISKFULL (rmbp-ledger B699, BP-storage-18, BP-bar-26) — `tests diskfull`: a full UnaFS volume answers `-ENOSPC`,
/// never a panic, never a half-written transaction, and the user is told once.
///
/// On a RAM-backed volume (`MemDevice`, [`DF_BLOCKS`] blocks — the crate's own backend, the root untouched):
/// 1. fill it with [`DF_FILE`]-block files until it is under `LOW_PCT` % free (the status bar's disk line);
/// 2. ONE more write — a file larger than the free space — through `diskfull::write_or_full`, the root's own mapping:
///    it must answer `Backend("nospace")` and `attrsys::errno_of` must render it `-ENOSPC` (`enospc=1`);
/// 3. roll back as the root does (re-mount from the committed root: the crate left the failed write's allocations in
///    RAM, `residue=` counts them): the file's committed size is 0, the free count is the pre-write count, fsck clean
///    (`tx_rolled=1`);
/// 4. the same write again: `-ENOSPC` again, and the toast's once-a-minute floor holds it (`toasts=1`, `repeat=suppressed`);
/// 5. delete fill files, then the same write succeeds and reads back (`after_delete=ok`).
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
const DF_BLOCKS: u64 = 1024;
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
const DF_FILE: u64 = 24;

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
pub fn diskfull_selftest() {
    use crate::fs::unafs::diskfull;
    use ::unafs::{MemDevice, UnaFS, BLOCK_SIZE};
    const VOL: &str = "diskfull-ram";
    let fail = |why: &str| serial_println!(":: DISKFULL: enospc=0 panics=0 tx_rolled=0 after_delete=no toasts=0 -> FAIL ({}) ::", why);
    let Ok(mut fs) = UnaFS::format(MemDevice::with_blocks(DF_BLOCKS), 0) else { return fail("format of the RAM volume failed") };
    let root = fs.superblock.root_inode;
    diskfull::reset();
    // 1. fill to the low line
    let chunk = alloc::vec![0x5au8; (DF_FILE * BLOCK_SIZE) as usize];
    let mut files = 0usize;
    while fs.free_blocks() >= DF_FILE * 2 && files < DF_BLOCKS as usize {
        let Ok(id) = fs.create_file(root, format!("fill{}", files)) else { return fail("fill create failed") };
        if diskfull::write_or_full(&mut fs, VOL, id, 0, &chunk).is_err() {
            return fail("a fill write failed above the low line");
        }
        files += 1;
    }
    let low = diskfull::low_of(fs.free_blocks(), DF_BLOCKS);
    // 2. one more write, bigger than the room left
    let Ok(over) = fs.create_file(root, "over.bin".into()) else { return fail("create of the over file failed") };
    let free0 = fs.free_blocks();
    let big = alloc::vec![0xa5u8; ((free0 + 16) * BLOCK_SIZE) as usize];
    let r1 = diskfull::write_or_full(&mut fs, VOL, over, 0, &big);
    let errno = r1.as_ref().err().map(crate::fs::attrsys::errno_of);
    let enospc = (errno == Some(una_abi::ENOSPC)) as u32;
    let toast1 = diskfull::after_hold();
    // 3. the rollback: re-mount from the committed root, as the root's discard does
    let residue = free0.saturating_sub(fs.free_blocks());
    let dev = core::mem::take(&mut fs.device);
    drop(fs);
    let Ok(mut fs) = UnaFS::mount(dev) else { return fail("re-mount after the full write failed") };
    let size0 = fs.read_inode(over).map(|i| i.size).unwrap_or(u64::MAX);
    let fsck = fs.fsck(false).is_ok_and(|rep| rep.is_clean());
    let tx_rolled = size0 == 0 && fs.free_blocks() == free0 && fsck;
    // 4. again: still -ENOSPC, and the toast floor holds it
    let r2 = diskfull::write_or_full(&mut fs, VOL, over, 0, &big);
    let repeat = r2.as_ref().err().map(crate::fs::attrsys::errno_of) == Some(una_abi::ENOSPC);
    let toast2 = diskfull::after_hold();
    let dev = core::mem::take(&mut fs.device);
    drop(fs);
    let Ok(mut fs) = UnaFS::mount(dev) else { return fail("re-mount after the repeat write failed") };
    // 5. delete, then the same write lands
    let mut deleted = 0usize;
    for k in 0..files.min(3) {
        if fs.unlink(root, &format!("fill{}", k)).is_ok() {
            deleted += 1;
        }
    }
    let after = diskfull::write_or_full(&mut fs, VOL, over, 0, &big).is_ok()
        && fs.read_data(over, 0, big.len() as u64).is_ok_and(|d| d == big)
        && fs.fsck(false).is_ok_and(|rep| rep.is_clean());
    let (_, toasts, suppressed) = diskfull::counts();
    let pass = enospc == 1 && tx_rolled && after && toasts == 1 && toast1 && !toast2 && repeat && low.is_some();
    serial_println!(
        ":: DISKFULL: enospc={} panics=0 tx_rolled={} after_delete={} toasts={} -> {} :: vol={} blocks={} files={} low_pct={} free_kb={} write_kb={} residue={} size_after={} fsck={} repeat={} suppressed={} deleted={}",
        enospc, tx_rolled as u32, if after { "ok" } else { "no" }, toasts, if pass { "PASS" } else { "FAIL" },
        VOL, DF_BLOCKS, files, low.map_or(alloc::string::String::from("-"), |p| format!("{}", p)), free0 * (BLOCK_SIZE / 1024),
        big.len() / 1024, residue, size0, if fsck { "clean" } else { "dirty" }, if repeat && !toast2 { "suppressed" } else { "no" }, suppressed, deleted
    );
}

// AUTOLOCK (rmbp-ledger B712, BP-bar-15/20) — TAIL-APPENDED, so no line above moves. `tests autolock` (R80: registered,
// never at boot unless `tests-at-boot`): Win+L through the live resolver (`keymap::resolve_edge`, the Windows key of the
// live look and keyboard held, an L press edge) must answer `LockScreen`; the router's arm (`login::lock`) locks; two
// keystrokes offered at the router's first gate (`users::screen_key`) must both be the screen's (`leaked_keys` counts a
// key the screen did not take, i.e. one the focused window would have read); a wrong password is refused and counted;
// the right one opens it (the worker's verdict drained here, bounded 15 s, as the render pass would); the window table's
// fingerprint (`lock::fingerprint`: every app row's id, owner, minimised bit, title and frame, then the focused row) is
// the same before and after. The credential is the loginst fixture account's (`una`); any other session SKIPs — the
// fixture never learns a password it was not given. One line:
// `:: AUTOLOCK: entered=<0|1> leaked_keys=<n> reopened=<ok|FAIL> table_same=<0|1> ms=<n> -> PASS|FAIL|SKIP :: …`
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn autolock_selftest() {
    use crate::video::{crystal::login, keymap, lock};
    const NAME: &[u8] = b"una";
    const PW: &[u8] = b"correct-horse";
    const WRONG: &[u8] = b"wrong-horse";
    let mut nb = [0u8; crate::fs::users::NAME_MAX];
    let is_una = |nb: &mut [u8; crate::fs::users::NAME_MAX]| matches!(crate::fs::users::whoami(nb), Some(k) if &nb[..k] == NAME);
    if !is_una(&mut nb) {
        serial_println!(":: AUTOLOCK: entered=0 leaked_keys=0 reopened=- table_same=- ms=- -> SKIP reason=session-not-fixture-user (needs the loginst session una) ::");
        return;
    }
    if lock::active() || login::is_open() {
        serial_println!(":: AUTOLOCK: entered=0 leaked_keys=0 reopened=- table_same=- ms=- -> SKIP reason=screen-up ::");
        return;
    }
    let feed = |s: &[u8]| { for &b in s { let _ = crate::fs::users::screen_key(b); } };
    let settle = || {
        let t0 = crate::arch::ms();
        loop {
            login::submit_drain();
            if !login::submit_busy() || crate::arch::ms().wrapping_sub(t0) > 15_000 { break; }
            crate::arch::sched::yield_now();
        }
    };
    let before = lock::fingerprint();
    let role = keymap::start_role();
    let win = if role != 0 { role } else { crate::drivers::xhci::HID_MOD_GUI };
    let mut cur = [0u8; 6];
    cur[0] = 0x0F;
    let chord = keymap::resolve_edge(keymap::active(), &cur, &[0u8; 6], win);
    let token = chord.map(|(_, t)| t).unwrap_or("none");
    let chord_ok = matches!(chord, Some((keymap::Action::LockScreen, _)));
    let entered = chord_ok && login::lock() && lock::active();
    let hidden = lock::hidden();
    let mut leaked = 0u32;
    if entered {
        for b in [b'q', 8u8] {
            if !crate::fs::users::screen_key(b) { leaked += 1; }
        }
    }
    let w0 = lock::wrong_count();
    if entered { feed(WRONG); let _ = crate::fs::users::screen_key(b'\n'); settle(); }
    let wrong = lock::wrong_count().wrapping_sub(w0);
    let refused = entered && lock::active() && wrong == 1; let l2 = lock2_locked(entered); // LOCK2 (B728): a window created while locked (file tail). LINE-NEUTRAL fold, code first.
    if entered { feed(PW); let _ = crate::fs::users::screen_key(b'\n'); settle(); }
    let reopened = entered && !lock::active() && !login::is_open() && is_una(&mut nb); let (painted, hole_ms) = lock2_unlocked(l2); let resumed = lock::last_resumed(); // LOCK2 (B728): it paints within one frame of the unlock, then is closed before the fingerprint (file tail). LINE-NEUTRAL fold, code first.
    let after = lock::fingerprint();
    let same = before == after;
    let ms = lock::last_open_ms(); let prefs = lockprefs_leg(); let gets = lockchanged_leg(); let deleted = lockdeleted_leg(); // PREFSDELETED (B755): the deleted-domain leg (file tail). // LOCKCHANGED (B758): the lock's GETs per lock-key and per other-key change (file tail). LOCKPREFS (B748): a bus write of the delay is the lock's within one service pass (file tail). LINE-NEUTRAL fold, code first.
    let pass = entered && leaked == 0 && refused && reopened && same && ms.is_some() && painted && resumed >= 1 && prefs.0 && gets.0 && deleted.0; // PREFSDELETED (B755): + deleted. // LOCKCHANGED (B758): + gets. LOCK2 (B728): + the resume pass and the one-frame paint; the words leg is owed (`words_path=owed`, see the file tail)
    serial_println!(
        ":: AUTOLOCK: entered={} leaked_keys={} reopened={} table_same={} ms={} resumed={} hole_ms={} words_path={} prefs={} -> {} :: chord={} wrong={} windows_hidden={} painted={} gets={} deleted={}",
        entered as u8, leaked, if reopened { "ok" } else { "FAIL" }, same as u8, ms.map_or(alloc::string::String::from("-"), |m| alloc::format!("{}", m)), resumed, hole_ms.map_or(alloc::string::String::from("-"), |m| alloc::format!("{}", m)), "owed", prefs.1,
        if pass { "PASS" } else { "FAIL" }, token, wrong, hidden, painted as u8, gets.1, deleted.1
    );
}

/// UNDOMOVE (rmbp-ledger B716, BP-bar-14): `:: UNDOMOVE: pushed=4 undone=4 redone=4 refused=1 leftovers=0 -> PASS ::` —
/// Quarry's journal (`video/quarry/undo.rs`) driven through the SAME wrappers the menu, the keys and the drop call, on a
/// scratch folder under the user's home through the mount table (the namespace Quarry's ops are confined to): a move, a
/// rename, a Trash and a new folder push four entries; four undos put every object back (checked by stat), four redos
/// re-apply them; a file put into the new folder makes the next undo REFUSED (`not-empty`) and drops that entry. The
/// session's own journal is set aside for the run and put back. `leftovers` = the scratch folder still present after the
/// clean-up + Trash entries still pointing into it.
#[cfg(all(feature = "quarry", feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn undomove_selftest() {
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use crate::video::quarry::live::ops;
    use crate::video::quarry::undo;
    let t = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    if t.stat(&home).is_err() {
        let _ = t.create(&home, NodeKind::Dir, P);
    }
    let base = format!("{}/UndoMoveTest", home);
    let _ = ops::op_delete(&base);
    let saved = undo::fixture_take();
    let (a, b) = (format!("{}/A", base), format!("{}/B", base));
    let (mv, mv_to) = (format!("{}/MOVE.TXT", a), format!("{}/MOVE.TXT", b));
    let (rn, rn_to) = (format!("{}/OLD.TXT", a), format!("{}/NEW.TXT", a));
    let gone = format!("{}/GONE.TXT", a);
    let nf = format!("{}/NEWDIR", a);
    let mk = |p: &str| t.create(p, NodeKind::File, P).and_then(|_| t.write(p, 0, p.as_bytes(), P)).is_ok();
    let made = ops::op_mkdir(&base).is_ok() && ops::op_mkdir(&a).is_ok() && ops::op_mkdir(&b).is_ok() && mk(&mv) && mk(&rn) && mk(&gone);
    let at = |p: &str| t.stat(p).is_ok();
    let fwd = || at(&mv_to) && !at(&mv) && at(&rn_to) && !at(&rn) && !at(&gone) && at(&nf);
    let back = || at(&mv) && !at(&mv_to) && at(&rn) && !at(&rn_to) && at(&gone) && !at(&nf);
    // the four verbs, through the journal's wrappers
    let verbs_ok = made && undo::rename(&mv, &mv_to).is_ok() && undo::rename(&rn, &rn_to).is_ok() && undo::trash(&gone).is_ok() && undo::mkdir(&nf).is_ok();
    let pushed = undo::depth().0;
    let fwd1 = verbs_ok && fwd();
    // four undos, newest first: the folder goes, the Trash restores, the rename and the move go back
    let undone = (0..4).filter(|_| undo::undo().is_ok()).count();
    let back_ok = back();
    // four redos
    let redone = (0..4).filter(|_| undo::redo().is_ok()).count();
    let fwd2 = fwd();
    // the refused undo: the new folder is no longer empty
    let filled = mk(&format!("{}/X.TXT", nf));
    let before = undo::depth().0;
    let refused = (filled && undo::undo() == Err("not-empty") && undo::depth().0 + 1 == before && at(&nf)) as u32;
    // clean-up: the trashed file out of the Trash, then the scratch tree
    for e in crate::fs::trash::entries().into_iter().filter(|e| e.orig.starts_with(&base)) {
        if crate::fs::trash::restore(&e.name).is_ok() {
            let _ = ops::op_delete(&e.orig);
        }
    }
    let _ = ops::op_delete(&base);
    let leftovers = at(&base) as usize + crate::fs::trash::entries().iter().filter(|e| e.orig.starts_with(&base)).count();
    undo::fixture_put(saved);
    serial_println!("[undomove] made={} verbs={} fwd={} back={} fwd_again={} filled={} base={}", made, verbs_ok, fwd1, back_ok, fwd2, filled, base);
    let pass = pushed == 4 && fwd1 && undone == 4 && back_ok && redone == 4 && fwd2 && refused == 1 && leftovers == 0;
    serial_println!(
        ":: UNDOMOVE: pushed={} undone={} redone={} refused={} leftovers={} -> {} ::",
        pushed, undone, redone, refused, leftovers, if pass { "PASS" } else { "FAIL" }
    );
}

// MTPPHONE (rmbp-ledger B715, BP-bar-25): `tests mtpphone` — the MTP class driver (`drivers/xhci/mtp.rs`) against a
// SYNTHETIC responder over an in-RAM transport (`mtp::fixture::Responder`): every transaction's command, data and response
// containers cross `ptp::codec` both ways. The responder is a phone: an MTP DeviceInfo (VendorExtensionID 6, "microsoft.com:
// 1.0"), two storages, 40 objects (5 folders, a 3 MB picture). Legs: the Microsoft OS descriptor codecs (string 0xEE, the
// Extended Compat ID "MTP", and a non-MTP twin refused); enumerate -> the tree in ONE GetObjectPropList; a responder that
// refuses 0x9805 -> the per-folder walk -> the same 40 rows; the 3 MB object read by 64 KiB GetPartialObject chunks, byte for
// byte; a 100 000-byte SendObjectInfo + SendObject into Download, read back; DeleteObject and the handle gone.
// MTPWRITE (rmbp-ledger B727): a 5 GiB object announced by SendObjectPropList; then the responder ATTACHED as the session's
// phone (`mtp::fixture::attach`, only with no PTP/MTP device present) and driven THROUGH THE VFS (`MountTable::new()` ->
// `fs/ptpfs.rs`): a 100 000-byte copy in 32 KiB writes (the short tail sends it), stat's id = the handle, read back; a folder
// made (an association on the responder) with a pending file listed inside it and dropped; rmdir; unlink; a 5 GiB sparse object
// read at 4 GiB + 1 (GetPartialObject64) and across 4 GiB.
// `:: MTPPHONE: enumerated=1 storages=2 objects=40 read=ok write=ok delete=ok ms=<n> vfs_write=ok mkdir=ok big_read=ok -> PASS ::`
#[cfg(feature = "uvc")]
fn mtpphone_selftest() {
    use crate::drivers::xhci::ptp::mtp::{self, fixture, Transport};
    use crate::drivers::xhci::ptp::{codec, op, rc, Phase};
    let t0 = crate::arch::ms();
    let mut why: Option<&'static str> = None;
    let mut miss = |w: &'static str, ok: bool| {
        if !ok && why.is_none() {
            why = Some(w);
        }
        ok
    };
    // M1: the OS descriptor and the DeviceInfo.
    let (s, x) = fixture::os_descriptors();
    let mut other = x;
    other[18] = b'N'; // "NTP": a compatibleID that is not MTP
    let os = miss("os-descriptor", mtp::os_string_vendor_code(&s) == Some(0x54) && mtp::compat_id_is_mtp(&x) && !mtp::compat_id_is_mtp(&other) && mtp::os_string_vendor_code(&s[..17]).is_none());
    let mut r = fixture::Responder::new(false);
    let opened = r.txn(op::OPEN_SESSION, &[1], Phase::None).map(|v| v.code == rc::OK).unwrap_or(false);
    let mut raw = alloc::vec::Vec::new();
    let di = match r.txn(op::GET_DEVICE_INFO, &[], Phase::In { cap: 64 << 10, into: &mut raw }) {
        Ok(v) if v.code == rc::OK => codec::device_info(&raw),
        _ => None,
    };
    let Some(di) = di else {
        serial_println!(":: MTPPHONE: enumerated=0 storages=0 objects=0 read=- write=- delete=- ms={} -> FAIL why=deviceinfo ::", crate::arch::ms().saturating_sub(t0));
        return;
    };
    let ext_ops = [mtp::mop::GET_OBJECT_PROPS_SUPPORTED, mtp::mop::GET_OBJECT_PROP_LIST, mtp::mop::SEND_OBJECT_PROP_LIST].iter().all(|o| di.ops.contains(o));
    let enumerated = miss("enumerate", opened && os && mtp::is_mtp(&di, false) && ext_ops);
    let mut raw = alloc::vec::Vec::new();
    let sids = match r.txn(op::GET_STORAGE_IDS, &[], Phase::In { cap: 4096, into: &mut raw }) {
        Ok(v) if v.code == rc::OK => codec::u32_array(&raw).unwrap_or_default(),
        _ => alloc::vec::Vec::new(),
    };
    // M2: the tree in one pass, and the walk that must agree with it.
    let (nst, nobj, fast_rows) = match mtp::enumerate(&mut r, &di, &sids) {
        Ok(l) => {
            let names_ok = l.storages.len() == 2 && l.storages[0].1 == "Internal shared storage" && l.storages[1].1 == "SD card";
            miss("storage-names", names_ok);
            miss("friendly", l.friendly.as_deref() == Some("Una Phone"));
            miss("props-fast", l.fast && l.props_supported == 8);
            let big = l.objs.iter().find(|o| o.handle == fixture::BIG);
            miss("prop-row", big.map(|o| o.size == fixture::BIG_SIZE && o.parent == 2 && o.storage == fixture::S1 && o.mtime.is_some()).unwrap_or(false));
            let mut rows: alloc::vec::Vec<(u32, alloc::string::String, u32, u64, u32)> = l.objs.iter().map(|o| (o.handle, o.name.clone(), o.parent, o.size, o.storage)).collect();
            rows.sort();
            (l.storages.len(), l.objs.len(), rows)
        }
        Err(e) => {
            serial_println!("[mtp] fixture enumerate error {}", e);
            miss("enumerate-error", false);
            (0, 0, alloc::vec::Vec::new())
        }
    };
    let mut w = fixture::Responder::new(true);
    let _ = w.txn(op::OPEN_SESSION, &[1], Phase::None);
    let mut walk_rows = alloc::vec::Vec::new();
    if let Ok(l) = mtp::enumerate(&mut w, &di, &sids) {
        miss("walk-chosen", !l.fast && l.walker.is_some());
        if let Some(mut wk) = l.walker {
            for _ in 0..1000 {
                if wk.done() {
                    break;
                }
                match wk.step(&mut w, 32) {
                    Ok(v) => walk_rows.extend(v.into_iter().map(|o| (o.handle, o.name, o.parent, o.size, o.storage))),
                    Err(_) => break,
                }
            }
        }
    }
    walk_rows.sort();
    miss("walk-agrees", walk_rows == fast_rows);
    // Reads: the 3 MB object in 64 KiB chunks, a range inside it, a range across its end.
    let want = r.bytes_of(fixture::BIG).unwrap_or_default();
    let whole = mtp::read_partial(&mut r, fixture::BIG, 0, fixture::BIG_SIZE as usize, 64 << 10).unwrap_or_default();
    let range = mtp::read_partial(&mut r, fixture::BIG, 1_000_000, 4096, 64 << 10).unwrap_or_default();
    let tail = mtp::read_partial(&mut r, fixture::BIG, fixture::BIG_SIZE - 100, 4096, 64 << 10).unwrap_or_default();
    let read_ok = miss("read", whole.len() as u64 == fixture::BIG_SIZE && whole == want && range[..] == want[1_000_000..1_004_096] && tail.len() == 100);
    // M3: a write into Download, read back whole; the SendObjectPropList dataset round-trips; the delete.
    let data: alloc::vec::Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(13) >> 3) as u8).collect();
    let h = mtp::send_object(&mut r, fixture::S1, fixture::DOWNLOAD, "UNA-COPY.TXT", &data).ok();
    let mut back = alloc::vec::Vec::new();
    let got = h.map(|h| r.txn(op::GET_OBJECT, &[h], Phase::In { cap: 1 << 20, into: &mut back }).map(|v| v.code == rc::OK).unwrap_or(false)).unwrap_or(false);
    let props = mtp::prop_list(&mtp::enc_send_props("BIG.MOV", 5_000_000_000)).unwrap_or_default();
    let props_ok = props.len() == 1 && props[0].name == "BIG.MOV" && props[0].size == 5_000_000_000;
    let write_ok = miss("write", got && back == data && h.map(|h| r.bytes_of(h).as_deref() == Some(&data[..])).unwrap_or(false) && props_ok);
    let delete_ok = match h {
        Some(h) => {
            let del = mtp::delete_object(&mut r, h).is_ok();
            let mut sink = alloc::vec::Vec::new();
            let gone = r.txn(op::GET_OBJECT_INFO, &[h], Phase::In { cap: 4096, into: &mut sink }).map(|v| v.code == rc::INVALID_OBJECT_HANDLE).unwrap_or(false);
            miss("delete", del && gone && !r.holds(h))
        }
        None => miss("delete", false),
    };
    // MTPWRITE M2: a file of 4 GiB and over is announced by SendObjectPropList (64-bit size), never ObjectInfo's 32 bits.
    let big_h = mtp::announce(&mut r, fixture::S1, fixture::DOWNLOAD, "BIG5.MOV", fixture::HUGE_SIZE).ok();
    let big_sent = big_h.and_then(|h| r.info_of(h)).map(|(_, size, name)| size == fixture::HUGE_SIZE && name == "BIG5.MOV").unwrap_or(false);
    let big_tail = big_h.and_then(|h| mtp::read_partial(&mut r, h, fixture::HUGE_SIZE - 10, 64, 64 << 10).ok().map(|v| v.len() == 10 && v.iter().enumerate().all(|(i, b)| *b == fixture::pat(h, fixture::HUGE_SIZE - 10 + i as u64)))).unwrap_or(false);
    miss("big-send", big_sent && big_tail);
    // MTPWRITE M3: the same phone THROUGH THE VFS.
    let (vfs_write, mkdir, big_read) = mtpwrite_vfs(&mut miss);
    let counts = miss("counts", nst == 2 && nobj == fixture::OBJECTS);
    let ms = crate::arch::ms().saturating_sub(t0);
    serial_println!("[mtp] fixture txns={} walk_txns={} fast_rows={} walk_rows={} whole={} written={}", r.txns, w.txns, fast_rows.len(), walk_rows.len(), whole.len(), data.len());
    let pass = enumerated && counts && read_ok && write_ok && delete_ok && vfs_write && mkdir && big_read && why.is_none();
    let okf = |b: bool| if b { "ok" } else { "fail" };
    if pass {
        serial_println!(":: MTPPHONE: enumerated=1 storages={} objects={} read=ok write=ok delete=ok ms={} vfs_write=ok mkdir=ok big_read=ok -> PASS ::", nst, nobj, ms);
    } else {
        serial_println!(
            ":: MTPPHONE: enumerated={} storages={} objects={} read={} write={} delete={} ms={} vfs_write={} mkdir={} big_read={} -> FAIL why={} ::",
            enumerated as u8, nst, nobj, okf(read_ok), okf(write_ok), okf(delete_ok), ms, okf(vfs_write), okf(mkdir), okf(big_read), why.unwrap_or("-")
        );
    }
}

// FILEVERSIONS (rmbp-ledger B714, BP-bar-14) — TAIL-APPENDED. `tests fileversions`: the file system keeps the
// previous contents of a file when a write replaces them — on a scratch RAM volume, through the kernel seam
// (`fs::unafs::versions_arm` = the bound mount's policy, `versions_on` / `read_version_on` / `restore_version_on`).
// writes: ten writes of /doc.txt keep 8 versions with their bytes (the ring's cap); restore: version 7 becomes
// the contents and the replaced contents are the newest version; bound: a 60-block file written three times
// passes 10 % of the volume, so the oldest versions anywhere (the doc's) go first and the kept blocks fit the
// bound; v7reader: the stamp is still 7 and a plain mount (no versioning: the older readers' view) opens the
// image, reads the kept version and fscks clean. `[fs] version inode= kept= bytes=` comes from the seam.
// `:: FILEVERSIONS: writes=10 kept=8 restore=ok bound=ok v7reader=ok -> PASS|FAIL ::`
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn fileversions_selftest() {
    use ::unafs::{MemDevice, UnaFS, BLOCK_SIZE};
    use crate::fs::unafs::{read_version_on, restore_version_on, versions_arm, versions_on};
    const BLOCKS: u64 = 1024;
    let content = |tag: u8, len: usize| -> alloc::vec::Vec<u8> { (0..len).map(|i| tag.wrapping_mul(31).wrapping_add(i as u8)).collect() };
    let mut fs = match UnaFS::format(MemDevice::with_blocks(BLOCKS), 0) {
        Ok(f) => f,
        Err(e) => {
            serial_println!(":: FILEVERSIONS: writes=0 kept=0 restore=- bound=- v7reader=- -> FAIL reason=format({:?}) ::", e);
            return;
        }
    };
    versions_arm(&mut fs);
    let root = fs.superblock.root_inode;
    let mut writes = 0u32;
    if let Ok(id) = fs.create_file(root, "doc.txt".into()) {
        for i in 0..10u8 {
            writes += fs.write_data(id, 0, &content(i, 5000)).is_ok() as u32;
        }
    }
    let kept = versions_on(&mut fs, "/doc.txt").map(|v| v.len()).unwrap_or(0);
    let bytes_ok = (0..kept).all(|n| read_version_on(&mut fs, "/doc.txt", n).is_ok_and(|d| d == content(8 - n as u8, 5000)));
    // restore: the oldest kept (contents 1) back; the replaced contents (9) are now the newest version.
    let restore = restore_version_on(&mut fs, "/doc.txt", 7).is_ok()
        && fs.resolve_path("/doc.txt").and_then(|id| fs.read_data(id, 0, 5000)).is_ok_and(|d| d == content(1, 5000))
        && read_version_on(&mut fs, "/doc.txt", 0).is_ok_and(|d| d == content(9, 5000))
        && fs.fsck(false).is_ok_and(|r| r.is_clean());
    // bound: 60 blocks x 2 kept versions = 120 > 102 — the doc's versions (the oldest) go, then the big file's first.
    let big = alloc::vec![0x5au8; 60 * BLOCK_SIZE as usize];
    let mut wrote_big = fs.create_file(root, "big.bin".into()).is_ok();
    for t in 0..3u8 {
        let mut b = big.clone();
        b[0] = t;
        wrote_big &= fs.resolve_path("/big.bin").and_then(|id| fs.write_data(id, 0, &b)).is_ok();
    }
    let ledger_kept = fs.version_ledger().map(|l| l.kept()).unwrap_or(u64::MAX);
    let bound = wrote_big
        && ledger_kept <= fs.version_bound_blocks()
        && versions_on(&mut fs, "/doc.txt").is_ok_and(|v| v.is_empty())
        && versions_on(&mut fs, "/big.bin").is_ok_and(|v| v.len() == 1)
        && fs.fsck(false).is_ok_and(|r| r.is_clean());
    // v7reader: stamp 7; a plain mount opens it, reads the kept version, fscks clean.
    let raw = fs.device.as_bytes().to_vec();
    drop(fs);
    let stamped = raw.len() > 9 && &raw[0..5] == b"UNAFS" && u32::from_le_bytes([raw[5], raw[6], raw[7], raw[8]]) == 7;
    let v7reader = stamped
        && UnaFS::mount(MemDevice::from_bytes(raw)).is_ok_and(|mut m| {
            !m.versioning()
                && read_version_on(&mut m, "/big.bin", 0).is_ok_and(|d| d.len() == big.len() && d[0] == 1)
                && m.fsck(false).is_ok_and(|r| r.is_clean() && r.versions == 1)
        });
    let face = fileversions_face(); let pass = writes == 10 && kept == 8 && bytes_ok && restore && bound && v7reader && face.0; // QUARRYVERSIONS (B732): the Quarry face's legs (`fileversions_face`, file tail). LINE-NEUTRAL fold, code before the comment.
    serial_println!(
        ":: FILEVERSIONS: writes={} kept={} restore={} bound={} v7reader={} {} -> {} ::",
        writes, kept, if restore { "ok" } else { "FAIL" }, if bound { "ok" } else { "FAIL" }, if v7reader { "ok" } else { "FAIL" }, face.1,
        if pass { "PASS" } else { "FAIL" }
    );
}

// FONTKIT (rmbp-ledger B730): `tests fontkit` — every type role carries (wght, opsz) in theme's ONE table, the title
// face draws them, and the variable UI face renders them: `Title` at `type.title2`'s size inks more at `wght` 600 than
// at 400, and its advance at `opsz` 14 differs from `opsz` 28 (Inter's text and display designs). A static face (no
// `wght`/`opsz`) ignores both: SKIP, not FAIL. Registered beside `undomove`, same cfg.
// `:: FONTKIT: roles=<n> variable=<0|1> wght_ok=1 opsz_ok=1 -> PASS|SKIP ::`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn fontkit_selftest() {
    let Some(p) = crate::video::text::fontkit_probe() else {
        serial_println!(":: FONTKIT: roles=0 variable=0 wght_ok=0 opsz_ok=0 -> SKIP :: (no face loaded: see the [kfont] load line)");
        return;
    };
    let wght_ok = p.ink_600 > p.ink_400;
    let opsz_ok = p.w_opsz14 - p.w_opsz28 > 0.01 || p.w_opsz28 - p.w_opsz14 > 0.01;
    serial_println!(
        "[fontkit] face={} size={:.2} title_axes={} ink_600={} ink_400={} w_opsz14={:.2} w_opsz28={:.2} rows={}",
        p.face, p.size, if p.title_ok { "ok" } else { "MISSING" }, p.ink_600, p.ink_400, p.w_opsz14, p.w_opsz28, p.rows
    );
    let table_ok = p.roles == p.rows && p.title_ok;
    let verdict = if !table_ok {
        "FAIL"
    } else if !p.variable {
        "SKIP"
    } else if wght_ok && opsz_ok {
        "PASS"
    } else {
        "FAIL"
    };
    serial_println!(":: FONTKIT: roles={} variable={} wght_ok={} opsz_ok={} -> {} ::", p.roles, p.variable as u8, wght_ok as u8, opsz_ok as u8, verdict);
}

// KELFSTAGE (rmbp-ledger B720, BP-boot-17, BP-bar-8) — TAIL-APPENDED. KELF.SHA follows ABIMAGE's slots: the stage
// writes the slot-B row, the promotion respells it as slot A, and the loader's A->B hash fallback is a TRIAL the next
// DESKTOPBUILT PASS promotes (`install/abimage.rs`, `crates/bootloader/src/kelfsha.rs`). `tests kelfstage`: stage the
// running image's own `/boot/KERNEL.ELF`, check the two rows, simulate the promotion on the record (the pure rewrite;
// the boot volume's kernel is never renamed by a fixture), check the trial rules, unstage and check the B row went.
// `:: KELFSTAGE: rows=2 promoted_row=ok fallback_trial=ok cuts=<n> halts=0 healed=<n>/<n> keyed=ok -> PASS ::` (STAGEWINDOW
// B733 adds the cut leg, `kelfstage_cuts` at the file tail).

/// KELFSTAGE: the slot the loader jumped to (0 = A, 1 = B), from the verdict `kelf_note` kept; `None` for a loader
/// older than KELFHASH (`install::abimage::should_promote` then keeps ABIMAGE's rule).
#[cfg(target_arch = "x86_64")]
pub fn kelf_booted_slot() -> Option<u8> {
    (*KELF_KEPT.lock()).filter(|v| v.present()).map(|v| v.slot & 1)
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelfstage_selftest() {
    use crate::install::abimage::{self, kelfsha};
    use crate::install::abslot;
    let before = abimage::kelf_record();
    let a_before = before.as_deref().and_then(|r| kelfsha::find(r, kelfsha::SLOT_A));
    let src = alloc::format!("/boot/{}", abslot::ELF);
    let staged = abimage::stage_image(&src);
    let rec = abimage::kelf_record();
    // The bytes the stage left as KERNEL.NEW, read back whole: the B row must be THEIR sha256 and pass the loader's check.
    let new_bytes = {
        let mt = crate::shell::vfs_mount_table();
        let p = alloc::format!("/boot/{}", abslot::NEW);
        mt.stat(&p).ok().and_then(|st| mt.read(&p, 0, st.size as usize).ok())
    };
    let (rows, rows_ok, b_sha) = match (&staged, rec.as_deref(), new_bytes.as_deref()) {
        (Ok(st), Some(r), Some(nb)) => {
            let n = r.split(|&c| c == b'\n').filter(|l| !l.is_empty()).count();
            let sha = crypto_core::sha2::sha256(nb);
            let b = kelfsha::find(r, kelfsha::SLOT_B);
            let a_kept = a_before.is_none() || kelfsha::find(r, kelfsha::SLOT_A) == a_before;
            let ok = b.is_some_and(|b| b.sha == sha && b.sha == st.sha)
                && kelf_chk(Some(r), kelfsha::SLOT_B, nb) == kelfsha::Check::Ok
                && a_kept
                && n == if a_before.is_some() { 2 } else { 1 };
            (n, ok, Some(sha))
        }
        _ => (0, false, None),
    };
    // The promotion's rewrite, on the record: one row, slot A, file=KERNEL.ELF, B's sha — and the loader's A check
    // of the promoted bytes (KERNEL.NEW's) says ok.
    let promoted_ok = match (rec.as_deref(), b_sha, new_bytes.as_deref()) {
        (Some(r), Some(sha), Some(nb)) => {
            let mut out = [0u8; kelfsha::MAX];
            kelfsha::promoted(r, &mut out).is_some_and(|n| {
                let p = &out[..n];
                p.split(|&c| c == b'\n').filter(|l| !l.is_empty()).count() == 1
                    && kelfsha::find(p, kelfsha::SLOT_B).is_none()
                    && kelfsha::find(p, kelfsha::SLOT_A).is_some_and(|a| a.sha == sha)
                    && p.windows(16).any(|w| w == b"file=KERNEL.ELF ")
                    && kelf_chk(Some(p), kelfsha::SLOT_A, nb) == kelfsha::Check::Ok
            })
        }
        _ => false,
    };
    // The fallback trial: the loader's A->B fallback marks `why=trial` with no stage; the kernel promotes a trial only
    // when it is running slot B; an older loader keeps ABIMAGE's rule; the loader's next decision on that record is ELF.
    let cases: [(&str, bool, bool, Option<u8>, bool); 5] = [
        ("fallback-nostage", false, true, Some(1), true),
        ("fallback-staged", true, true, Some(1), true),
        ("torn-new-ran-A", true, true, Some(0), false),
        ("old-loader", true, true, None, true),
        ("no-trial", true, false, Some(1), false),
    ];
    let mut case_ok = 0u32;
    for (name, st, tr, booted, want) in cases {
        let got = abimage::should_promote(st, tr, booted);
        case_ok += (got == want) as u32;
        serial_println!("[kelfstage] case={} promote={} -> {}", name, got as u8, if got == want { "ok" } else { "WRONG" });
    }
    let d = abslot::decide(None, Some(abslot::REC_TRIAL));
    let fallback_ok = case_ok == 5 && d.slot == abslot::Slot::Elf && d.why == abslot::Why::Normal;
    let cleaned = abimage::unstage().is_ok();
    let after = abimage::kelf_record();
    let unstaged_ok = cleaned
        && after.as_deref().map_or(true, |r| kelfsha::find(r, kelfsha::SLOT_B).is_none())
        && after.as_deref().and_then(|r| kelfsha::find(r, kelfsha::SLOT_A)) == a_before;
    serial_println!(
        "[kelfstage] stage={} rows={} a_row={} unstaged={}",
        match &staged { Ok(_) => "ok", Err(e) => *e },
        rows,
        if a_before.is_some() { "builder" } else { "absent" },
        unstaged_ok as u8
    );
    let cut = kelfstage_cuts(); // STAGEWINDOW (B733): every power cut of THE promotion, on a RAM directory
    let pass = rows == 2 && rows_ok && promoted_ok && fallback_ok && unstaged_ok && cut.pass();
    let w = |b: bool| if b { "ok" } else { "bad" };
    serial_println!(
        ":: KELFSTAGE: rows={} promoted_row={} fallback_trial={} cuts={} halts={} healed={}/{} keyed={} -> {} ::",
        rows, w(rows_ok && promoted_ok), w(fallback_ok && unstaged_ok), cut.cuts, cut.halts, cut.healed, cut.cuts, w(cut.keyed), if pass { "PASS" } else { "FAIL" }
    );
}

// NET6GRANT (LEDGER SR151, BP-security-17, BP-net-12) — TAIL-APPENDED. `tests netgrant` on aarch64: NETGRANT's legs
// (Never, Always after a revoke, persisted across a store read-back) through the SAME check the NET6 doors call
// (`fs::apptrust::net_gate_for`, the body `net_gate_asid` runs for `net6_sys_connect`), then the NET6 stack
// (`net_phy::net6::connect`), in `net6_sys_connect`'s order, on a kernel-owned NET6 socket. aarch64 counts no connect on
// the outbound ledger (BP-net-11), so "the stack was never entered" is counted at the door: every call that passed the
// check into `net6::connect` (`denied_pkts` in the Never leg, `allowed` in the Always leg); the NIC's other transmits in
// the Never leg are said for the reader. The name leg (M2): SYS_RESOLVE's record (`apptrust::net_name_note`, the call
// aarch64's `sys_resolve` makes) for the probe's ip, and the ask on the glass names it — `dst=<name>(<ip:port>)`.
// `:: NETGRANT: asked=1 denied_pkts=0 allowed=1 persisted=1 named=1 arch=aarch64 -> PASS ::`.
#[cfg(all(feature = "netring3", not(all(feature = "smolnet", feature = "wc", target_arch = "x86_64"))))]
fn netgrant_arm() {
    #[cfg(all(feature = "net6", feature = "desktop_firmware", target_arch = "aarch64"))]
    {
        use crate::fs::apptrust::{net_asking_dst, net_asks, net_forget, net_gate_for, net_name_note, NET_EPERM};
        use crate::video::dialog::{self, Answer};
        const APP: &str = "NGPROBE";
        const NAME: &str = "netgrant.probe";
        const IP: [u8; 4] = [10, 0, 2, 2];
        const PORT: u16 = 9;
        const RUN1: u64 = u64::MAX - 0x4e47;
        const RUN2: u64 = RUN1 - 1;
        const RUN3: u64 = RUN1 - 2;
        static ENTERED: AtomicU32 = AtomicU32::new(0);
        if dialog::is_up() {
            serial_println!(":: NETGRANT: arch=aarch64 -> SKIP reason=dialog-up ::");
            return;
        }
        net_forget(APP);
        let Some(sid) = crate::net_phy::net6::open(u64::MAX, true) else {
            serial_println!(":: NETGRANT: arch=aarch64 -> SKIP reason=no-stack (no NIC adapter) ::");
            return;
        };
        let was = dialog::headless(true);
        net_name_note(NAME.as_bytes(), IP);
        let connect = |task: u64| -> i64 {
            match net_gate_for(APP, task, 0, IP, PORT, true) {
                Err(e) => e,
                Ok(()) => {
                    ENTERED.fetch_add(1, Ordering::Relaxed);
                    match crate::net_phy::net6::connect(sid, IP, PORT) {
                        crate::net_phy::net6::ConnectOutcome::Established => 0,
                        crate::net_phy::net6::ConnectOutcome::InProgress => -115,
                        crate::net_phy::net6::ConnectOutcome::Refused => -111,
                    }
                }
            }
        };
        let key = crate::prefs::decision_key(crate::prefs::decide::NET, APP);
        let other_tx = || {
            let (t, arp, dhcp) = crate::net_phy::tx_emission_counts();
            t.wrapping_sub(arp).wrapping_sub(dhcp)
        };

        // Never (and the name on the ask).
        let e0 = ENTERED.load(Ordering::Relaxed);
        let tx0 = other_tx();
        let a0 = net_asks();
        let r1 = connect(RUN1);
        let dst = net_asking_dst().unwrap_or_default();
        let named = dst == alloc::format!("{}({}.{}.{}.{}:{})", NAME, IP[0], IP[1], IP[2], IP[3], PORT);
        dialog::open_now();
        let up = dialog::is_up();
        dialog::answer(Answer::Default); // Never
        let r2 = connect(RUN1);
        let r3 = connect(RUN2);
        let asked = net_asks() - a0;
        let denied_pkts = ENTERED.load(Ordering::Relaxed) - e0;
        let tx_other = other_tx().wrapping_sub(tx0);
        let never_kept = crate::prefs::decided(crate::prefs::decide::NET, APP, "-").as_deref() == Some("never");

        // Always: revoked, a new run asks, answered Always.
        let revoked = crate::prefs::revoke(&key);
        let e1 = ENTERED.load(Ordering::Relaxed);
        let a1 = net_asks();
        let r4 = connect(RUN2);
        dialog::open_now();
        dialog::answer(Answer::Button(0)); // Always
        let r5 = connect(RUN2);
        let reasks_always = net_asks() - a1;
        let allowed = (ENTERED.load(Ordering::Relaxed) - e1).min(1);

        // persisted: the store read back, a third run passes unasked.
        let flushed = crate::prefs::decisions_flush();
        let keys = crate::prefs::reload_from_store();
        let a2 = net_asks();
        let kept = crate::prefs::decided(crate::prefs::decide::NET, APP, "-").as_deref() == Some("always");
        let r6 = net_gate_for(APP, RUN3, 0, IP, PORT, true);
        let persisted = (kept && r6.is_ok() && net_asks() == a2) as u32;

        crate::net_phy::net6::close(sid);
        net_forget(APP);
        dialog::headless(was);
        let pending = -115;
        let ok = asked == 1 && up && named && r1 == pending && r2 == NET_EPERM && r3 == NET_EPERM && never_kept && denied_pkts == 0
            && revoked && r4 == pending && reasks_always == 1 && r5 != NET_EPERM && allowed == 1 && persisted == 1;
        serial_println!(
            ":: NETGRANT: asked={} denied_pkts={} allowed={} persisted={} named={} arch=aarch64 -> {} :: r={}/{}/{}/{}/{} dst={} never_kept={} revoked={} reask_after_revoke={} flushed={} keys={} tx_other_in_never_leg={} key={}",
            asked, denied_pkts, allowed, persisted, named as u8, if ok { "PASS" } else { "FAIL" }, r1, r2, r3, r4, r5, dst, never_kept as u8, revoked as u8, reasks_always, flushed, keys, tx_other, key
        );
    }
    #[cfg(not(all(feature = "net6", feature = "desktop_firmware", target_arch = "aarch64")))]
    serial_println!(":: NETGRANT: -> SKIP reason=needs-x86-smolnet-wc-or-aarch64-net6-desktop ::");
}

/// RAISEVERB (rmbp-ledger B722; BP-video-23, BP-bar-18) — `tests raiseverb`: a program asks for its own window to come
/// forward, through the verb's own body (`wm::focussteal::raise_verb`, what `BUS_VERB_WINDOW_RAISE`'s x86 arm calls
/// once it has resolved the window id to its owner) with a synthetic ring-3-band owner that does not hold focus:
///  1. unsolicited (nobody launched it by a verb): PULSED — the reply is no, the tile pulses, nothing moves;
///  2. after the user's verb (`origin::Glass` armed, inside the 2 s window): GIVEN;
///  3. a window another owner holds: -EPERM, and no pulse is asked for.
/// The grant itself (`user_input_set_active` + `focus_changed`) is the arch seam's and needs a real slot; the
/// fixture counts the verdicts. The arming is forgotten after, so the owner leaves no verb record behind.
/// `:: RAISEVERB: asked=2 given=1 pulsed=1 foreign=EPERM -> PASS ::`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn raiseverb_selftest() {
    use crate::origin::{with, Origin};
    use crate::video::wm::{self, focussteal};
    const R: u64 = 0xF6A;
    const F: u64 = 0xF6B;
    const _: () = assert!(!wm::is_kernel_owner(R) && !wm::is_kernel_owner(F));
    let unfocused = wm::focus_asid() != R;
    // 1 — nobody asked for this program.
    focussteal::forget(R);
    let a1 = focussteal::raise_verb(R, R);
    let pulse = wm::attention_lit(R);
    // 2 — the user's verb launched it a moment ago.
    with(Origin::Glass, || focussteal::note_arm_by(R, 0));
    let a2 = focussteal::raise_verb(R, R);
    focussteal::forget(R);
    // 3 — another program's window.
    let a3 = focussteal::raise_verb(R, F);
    let asks = [a1, a2];
    let given = asks.iter().filter(|a| **a == Ok(true)).count();
    let pulsed = asks.iter().filter(|a| **a == Ok(false)).count();
    let foreign = match a3 {
        Err(e) if e == una_abi::EPERM => "EPERM",
        Ok(_) => "allowed",
        Err(_) => "other",
    };
    let ok = unfocused && a1 == Ok(false) && pulse && a2 == Ok(true) && a3 == Err(una_abi::EPERM);
    serial_println!(
        ":: RAISEVERB: asked={} given={} pulsed={} foreign={} -> {} ::",
        asks.len(), given, pulsed, foreign, if ok { "PASS" } else { "FAIL" }
    );
    if !ok {
        serial_println!("[wm] raiseverb unfocused={} pulse={} first={:?} second={:?}", unfocused as u8, pulse as u8, a1, a2);
    }
}

// UPDATEUI (rmbp-ledger B726, BP-bar-8) — TAIL-APPENDED. Settings > About > System image (`video/settings.rs` `sysimg`,
// `install/abimage.rs`): the loader's KELF verdict kept by `kelf_note` is read back here for the row.

/// The loader's verdict as `kelf_note` kept it (`None` before the boot line ran).
#[cfg(target_arch = "x86_64")]
pub fn kelf_kept() -> Option<unaos_boot_info::KelfVerdict> {
    *KELF_KEPT.lock()
}

/// `tests updateui`: `:: UPDATEUI: listed=1 staged=1 refused=1 text=ok -> PASS ::` — the System image pane's model driven
/// through its own latches (`sysimg::pick` / `remove`, run by `sysimg::run_in`, the service pass's body) over two RAM
/// sticks in the fixture's own mount table (`abimage::fixture_table`: `UPDTEST` = a copy of the running kernel + a
/// `SRC.SHA` stamp, `NOSHA` = a kernel and no stamp): (1) the list offers `UPDTEST` alone; (2) a pick stages it (the
/// `stage` verb's path) and the rows read `7e57ab1e from UPDTEST` and "Restart to try it — …"; (3) a pick of `NOSHA` is
/// refused (`no-src-sha`) and changes nothing; (4) the loader's fallback write is stood in for, the next boot's note
/// taken, and the rows read "… set aside (boot <n>)"; (5) Remove leaves no stage and nothing set aside. SKIP when the
/// user has a stage (the fixture never replaces it).
#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "wc"))]
fn updateui_selftest() {
    use crate::install::abimage;
    use crate::video::settings::sysimg;
    let pre = abimage::status();
    if pre.staged.is_some() || pre.set_aside.is_some() {
        serial_println!(":: UPDATEUI: listed=- staged=- refused=- text=- -> SKIP :: reason=a-stage-exists (the fixture never replaces the user's stage) ::");
        return;
    }
    let (t, vols) = match abimage::fixture_table() {
        Ok(x) => x,
        Err(w) => {
            serial_println!(":: UPDATEUI: listed=0 staged=0 refused=0 text=- -> FAIL :: reason={} ::", w);
            return;
        }
    };
    let (good, bad) = (vols[0].as_str(), vols[1].as_str());
    // (1) the list
    sysimg::stale();
    sysimg::run_in(&t, Some(&vols));
    let (offered, _) = sysimg::listed();
    let listed = offered.len();
    let list_ok = listed == 1 && offered[0] == good;
    // (2) the stage through the pane
    sysimg::pick(good);
    sysimg::run_in(&t, Some(&vols));
    let st = abimage::status();
    let staged = st.staged.as_ref().is_some_and(|s| s.src == good && s.sha == "7e57ab1e");
    let rows1 = sysimg::lines();
    let text1 = rows1[1].starts_with("7e57ab1e from UPDTEST") && rows1[2] == sysimg::NOTE_STAGED;
    // (3) the refusal: no SRC.SHA on the stick; the stage made in (2) is untouched
    sysimg::pick(bad);
    sysimg::run_in(&t, Some(&vols));
    let (_, said) = sysimg::listed();
    let kept = abimage::status().staged.is_some_and(|s| s.src == good);
    let refused = said.ends_with("not staged: no-src-sha") && kept;
    // (4) the loader's fallback (stood in for), the next boot's note, the row
    let n = abimage::fixture_fallback();
    sysimg::run_in(&t, Some(&vols));
    let rows2 = sysimg::lines();
    let text2 = n.is_some() && rows2[1] == "set aside" && rows2[2].starts_with("The staged image did not reach the desktop and was set aside (boot ");
    // (5) Remove
    sysimg::remove();
    sysimg::run_in(&t, Some(&vols));
    let after = abimage::status();
    let clean = after.staged.is_none() && after.set_aside.is_none();
    if !clean {
        let _ = abimage::unstage();
    }
    sysimg::stale();
    serial_println!("[update] ui fixture staged_row=\"{}\" note=\"{}\" aside_note=\"{}\" removed={}", rows1[1], rows1[2], rows2[2], clean as u8);
    let text = text1 && text2 && clean;
    let pass = list_ok && staged && refused && text;
    serial_println!(
        ":: UPDATEUI: listed={} staged={} refused={} text={} -> {} ::",
        listed, staged as u8, refused as u8, if text { "ok" } else { "bad" }, if pass { "PASS" } else { "FAIL" }
    );
}

// UACMIC (rmbp-ledger B723) — TAIL-APPENDED. `tests uacmic`: USB Audio Class 1 capture through the ONE capture seam
// (`audio::source`), the mic dot on then off with the driver's count, and the microphone grant (a non-kernel app is
// asked, never streamed). `:: UACMIC: enumerated= capture= frames= dot_on= dot_off= grant= -> PASS|SKIP|FAIL ::`.
#[cfg(feature = "uac")]
fn uacmic_fixture() {
    crate::drivers::usb::uac::mic_fixture();
}

/// MTPWRITE (B727): `tests mtpphone`'s VFS legs — (vfs_write, mkdir, big_read). The responder is attached as the session's
/// phone for the legs and detached before the verdict (its objects are then read back from it directly).
#[cfg(feature = "uvc")]
fn mtpwrite_vfs(miss: &mut impl FnMut(&'static str, bool) -> bool) -> (bool, bool, bool) {
    use crate::drivers::xhci::ptp::mtp::fixture;
    use crate::fs::vfs::{MountTable, NodeKind};
    const P: &str = "root";
    let vol = match fixture::attach(fixture::Responder::new(false).with_big()) {
        Ok(v) => v,
        Err(e) => {
            serial_println!("[mtpwrite] attach refused: {}", e);
            miss("attach", false);
            return (false, false, false);
        }
    };
    let t = MountTable::new();
    let dl = alloc::format!("/volumes/{}/Internal shared storage/Download", vol);
    let veto_open = t.write_veto(&dl).map(|v| v.is_none()).unwrap_or(false);
    // The copy: Quarry's shape (create, then 32 KiB writes; the short tail sends), then stat (the handle) and a read back.
    let data: alloc::vec::Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(29) >> 2) as u8).collect();
    let f = alloc::format!("{}/UNA-VFS.TXT", dl);
    let mut wrote = t.create(&f, NodeKind::File, P).is_ok();
    for (k, c) in data.chunks(32 << 10).enumerate() {
        wrote &= t.write(&f, (k * (32 << 10)) as u64, c, P).map(|n| n == c.len()).unwrap_or(false);
    }
    let st = t.stat(&f).ok();
    let fh = st.as_ref().and_then(|s| s.id).map(|i| i as u32);
    let back = t.read(&f, 0, 200_000).unwrap_or_default();
    let listed = t.read_dir(&dl).map(|v| v.iter().any(|e| e.name == "UNA-VFS.TXT" && e.size == 100_000)).unwrap_or(false);
    // The folder: made, listed as a folder; a pending file inside it listed then dropped (never sent); rmdir.
    let d = alloc::format!("{}/UnaFolder", dl);
    let made = t.create(&d, NodeKind::Dir, P).ok();
    let dh = made.as_ref().and_then(|s| s.id).map(|i| i as u32);
    let dir_listed = t.read_dir(&dl).map(|v| v.iter().any(|e| e.name == "UnaFolder" && matches!(e.kind, NodeKind::Dir))).unwrap_or(false);
    let inner = alloc::format!("{}/NOTE.TXT", d);
    let inner_ok = t.create(&inner, NodeKind::File, P).is_ok() && t.write(&inner, 0, b"una note\n", P).is_ok() && t.read_dir(&d).map(|v| v.len() == 1 && v[0].size == 9).unwrap_or(false);
    let inner_gone = t.unlink(&inner, P).is_ok() && t.read_dir(&d).map(|v| v.is_empty()).unwrap_or(false);
    let rmdir = t.remove_dir(&d, P).is_ok() && t.stat(&d).is_err();
    // The delete of the copied file, through the VFS.
    let unlinked = t.unlink(&f, P).is_ok() && t.stat(&f).is_err();
    // 5 GiB sparse: a read at 4 GiB + 1, and one across 4 GiB.
    let b = alloc::format!("{}/BIG.MOV", dl);
    let size_ok = t.stat(&b).map(|s| s.size == fixture::HUGE_SIZE).unwrap_or(false);
    let at = (4u64 << 30) + 1;
    let hi = t.read(&b, at, 4096).unwrap_or_default();
    let hi_ok = hi.len() == 4096 && hi.iter().enumerate().all(|(i, x)| *x == fixture::pat(fixture::HUGE, at + i as u64));
    let across_at = (4u64 << 30) - 100;
    let across = t.read(&b, across_at, 200).unwrap_or_default();
    let across_ok = across.len() == 200 && across.iter().enumerate().all(|(i, x)| *x == fixture::pat(fixture::HUGE, across_at + i as u64));
    drop(t);
    let r = fixture::detach();
    let on_phone = match (&r, fh) {
        // after the unlink the responder no longer holds the copy; the bytes were checked by the VFS read above
        (Some(r), Some(h)) => !r.holds(h),
        _ => false,
    };
    let folder_on_phone = match (&r, dh) {
        (Some(r), Some(h)) => !r.holds(h), // made (a handle came back) and removed by rmdir
        _ => false,
    };
    serial_println!(
        "[mtpwrite] vol={} veto_open={} wrote={} id={:?} back={} listed={} mkdir_handle={:?} dir_listed={} inner={} inner_gone={} rmdir={} unlinked={} big_size={} hi={} across={}",
        vol, veto_open as u8, wrote as u8, fh, back.len(), listed as u8, dh, dir_listed as u8, inner_ok as u8, inner_gone as u8, rmdir as u8, unlinked as u8, size_ok as u8, hi_ok as u8, across_ok as u8
    );
    let vfs_write = miss("vfs-write", veto_open && wrote && fh.is_some() && back == data && listed && unlinked && on_phone);
    let mkdir = miss("mkdir", dh.is_some() && dir_listed && inner_ok && inner_gone && rmdir && folder_on_phone);
    let big_read = miss("big-read", size_ok && hi_ok && across_ok);
    (vfs_write, mkdir, big_read)
}

// ══════════════ KEYCHARS (rmbp-ledger B718) — `tests keychars` — TAIL-APPENDED ══════════════

/// `tests keychars`: German swapped in silently (`layouts::fixture_swap`), `ü é ß € @` fed as HID boot report pairs
/// (ü; dead ´ then e; ß; AltGr+E; AltGr+Q) through `drivers::xhci::hid_layout_events` — the decoders' own layout call,
/// minus the push, so the fixture never races the live drain — and the characters read back three ways: as typed
/// (`chars=`), routed by `user_input_route` into a fixture slot's ring and read back by type and payload
/// (`INPUT_EV_CHAR`, or `INPUT_EV_KEY_DOWN` for the ASCII `@`) (`window=`), and typed into a scratch Quarry edit
/// field through `ops::key_char`'s body (`field=`). `altgr=ok`: every press claimed on de (so `hid_key_ascii` skips
/// it), Right Alt cleared from the chord modifiers on de and kept on us, AltGr+Q claimed on de and not on us.
/// `:: KEYCHARS: layout=de chars=5 window=5 field=5 altgr=ok arch=<arch> wake=ok repeat=owed -> PASS :: claimed=6/6 routed=5 ring=5` — KEYCHARSARM (SR153): both desktops; aarch64 reads the window back by ring depth (`keychars_arm`, tail); `wake=`/`repeat_byte=` bodies at the tail
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn keychars_selftest() {
    #[cfg(target_arch = "x86_64")] use crate::arch::x86_64::syscall as sc; #[cfg(target_arch = "aarch64")] use keychars_arm as sc; // KEYCHARSARM (SR153): the arch's ring router
    use crate::drivers::xhci as hx;
    use crate::pal::Event;
    use crate::video::keymap::layouts::{self as kl, KbdState, RALT};
    const WANT: [char; 5] = ['ü', 'é', 'ß', '€', '@'];
    const SEQ: [(u8, u8); 6] = [(0, 0x2F), (0, 0x2E), (0, 0x08), (0, 0x2D), (RALT, 0x08), (RALT, 0x14)];
    let (Some(de), Some(us)) = (kl::index_of("de"), kl::index_of("us")) else {
        serial_println!(":: KEYCHARS: layout=missing chars=0 window=0 field=0 altgr=bad -> FAIL ::");
        return;
    };
    let count = |got: &[char]| got.iter().zip(WANT.iter()).filter(|(a, b)| a == b).count();
    let saved = kl::fixture_swap(de, false, KbdState::new().pack());
    // The presses: down, up, then every modifier released — the shape a keyboard sends.
    let none = [0u8; 6];
    let mut evs: alloc::vec::Vec<Event> = alloc::vec::Vec::new();
    let mut claimed = 0usize;
    for &(mods, u) in SEQ.iter() {
        let mut down = [0u8; 6];
        down[0] = u;
        claimed += hx::hid_layout_claim(u, mods, false).is_some() as usize;
        let mut out = [Event::None; 12];
        let n = hx::hid_layout_events(&down, &none, mods, false, &mut out);
        evs.extend_from_slice(&out[..n]);
        let n = hx::hid_layout_events(&none, &down, mods, false, &mut out);
        evs.extend_from_slice(&out[..n]);
        if mods != 0 {
            let n = hx::hid_layout_events(&none, &none, 0, false, &mut out);
            evs.extend_from_slice(&out[..n]);
        }
    }
    let typed: alloc::vec::Vec<char> = evs.iter().filter_map(|e| match *e { Event::Key(b) => Some(b as char), Event::Char(c) => Some(c), _ => None }).collect();
    let chars_n = if typed.len() == WANT.len() { count(&typed) } else { 0 };
    // AltGr: the chord modifiers and the claim on de, then on us.
    let altgr_de = hx::hid_chord_mods(RALT | 0x02) == 0x02 && hx::hid_layout_claim(0x14, RALT, false) == Some(b'@'); let repeat_byte = keychars_repeat_byte(); // KEYCHARSARM: on de, the bytes the typematic feeds hand the tracker
    let _ = kl::fixture_swap(us, false, KbdState::new().pack());
    let altgr_us = hx::hid_chord_mods(RALT) == RALT && hx::hid_layout_claim(0x14, RALT, false).is_none();
    kl::fixture_swap(saved.0, saved.1, saved.2);
    let altgr = altgr_de && altgr_us && claimed == SEQ.len();
    // The window: a fixture slot (the last private slot, +1-biased) takes the focus, the events go through the ring
    // router, the ring is read back, the focus is put back.
    let slot = crate::procslot::SLOT_ID_MAX as u64;
    let focus0 = sc::user_input_active();
    sc::user_input_set_active(slot);
    let routed = evs.iter().filter(|e| matches!(sc::user_input_route(**e), Event::Unknown)).count();
    let mut ring = [0u64; 16];
    let rn = sc::user_input_take(slot, &mut ring);
    sc::user_input_set_active(focus0);
    let win: alloc::vec::Vec<char> = ring[..rn]
        .iter()
        .filter_map(|&p| match una_abi::input_ev_type(p) {
            una_abi::INPUT_EV_CHAR => una_abi::input_ev_char(p),
            una_abi::INPUT_EV_KEY_DOWN => Some((p & 0xFF) as u8 as char),
            _ => None,
        })
        .collect();
    #[cfg(target_arch = "x86_64")] let window_n = count(&win); #[cfg(target_arch = "aarch64")] let window_n = { let _ = &win; if rn == routed { routed } else { 0 } }; // KEYCHARSARM: aarch64 has no ring read-back; queued by the real router AND the ring grew by as many
    // The kernel text field: Quarry's edit field, a scratch rename (none while a person has a field open).
    #[cfg(feature = "quarry")]
    let field: Option<alloc::string::String> = crate::video::quarry::live::ops::key_char_fixture(&typed);
    #[cfg(not(feature = "quarry"))]
    let field: Option<alloc::string::String> = None;
    let field_n = field.as_deref().map(|f| { let v: alloc::vec::Vec<char> = f.chars().collect(); if v.len() == WANT.len() { count(&v) } else { 0 } }).unwrap_or(0);
    let wake = keychars_wake(); let pass = chars_n == 5 && window_n == 5 && field_n == 5 && altgr && wake && repeat_byte; // CHARREPEAT (SR156): `repeat_byte` now also drives the tracker with a held `ü` and a held de `z` (`keychars_repeat_byte`, tail); `repeat=` is its word
    serial_println!(
        ":: KEYCHARS: layout=de chars={} window={} field={} altgr={} arch={} wake={} repeat={} -> {} :: claimed={}/{} routed={} ring={} field_state={} repeat_byte={} window_by={}",
        chars_n, window_n, field_n, if altgr { "ok" } else { "bad" }, if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" }, if wake { "ok" } else { "bad" }, keychars_repeat_word(), if pass { "PASS" } else { "FAIL" },
        claimed, SEQ.len(), routed, rn, if field.is_some() { "scratch" } else { "busy-or-absent" }, if repeat_byte { "ok" } else { "bad" }, if cfg!(target_arch = "aarch64") { "depth" } else { "payload" }
    );
}

// ══════════════ CLOCKPAINTERS (rmbp-ledger B719) — `tests clockpainters` — TAIL-APPENDED ══════════════

/// Occurrences of `needle` in `hay`, at compile time (the FATVERB source-law shape in shell.rs: one forward walk,
/// a first-byte reject, then the full compare).
const fn src_count(hay: &[u8], needle: &[u8]) -> usize {
    let (mut i, mut hits) = (0usize, 0usize);
    while i + needle.len() <= hay.len() {
        if hay[i] == needle[0] {
            let mut k = 0usize;
            while k < needle.len() && hay[i + k] == needle[k] {
                k += 1;
            }
            if k == needle.len() {
                hits += 1;
            }
        }
        i += 1;
    }
    hits
}

/// The grep-shaped leg, folded at COMPILE time (the files are read by `include_bytes!` inside a const, so no source
/// byte reaches the image): the painter files that call the clock module's UTC split themselves. Every civil print in
/// them goes through `clock::locale`; the split lives only in `clock.rs` and `clock/`.
#[allow(long_running_const_eval)]
const CLOCK_UTC_SPLITS: usize = {
    const N: &[u8] = b"civil_from_unix(";
    src_count(include_bytes!("video/menubar.rs"), N)
        + src_count(include_bytes!("video/softy/taskbar.rs"), N)
        + src_count(include_bytes!("video/quarry/live.rs"), N)
        + src_count(include_bytes!("video/statusmenu.rs"), N)
        + src_count(include_bytes!("video/softy/startmenu.rs"), N)
        + src_count(include_bytes!("shell.rs"), N)
        + src_count(include_bytes!("video/quarry/getinfo.rs"), N)
        + src_count(include_bytes!("video/notify.rs"), N)
        + src_count(include_bytes!("video/login.rs"), N)
        + src_count(include_bytes!("fs/attrfacts.rs"), N) // CIVILREST (B738): the Taken date, the Privacy dates, the
        + src_count(include_bytes!("video/settings.rs"), N) // appearance hour (whose split was the anchor's own hour,
        + src_count(include_bytes!("video/appearance.rs"), N) // counted by its spelling below)
        + src_count(include_bytes!("video/appearance.rs"), b"/ 3600) % 24")
};

/// The retired display-offset knob: named by no kernel reader and no arroyo line (compile time, as above).
#[allow(long_running_const_eval)]
const CLOCK_KNOB_SITES: usize = {
    const N: &[u8] = b"UNAOS_TZ_MIN";
    src_count(include_bytes!("clock.rs"), N) + src_count(include_bytes!("arch/x86_64/rtc.rs"), N) + src_count(include_bytes!("../../../arroyo"), N)
        + src_count(include_bytes!("../../../scripts/k8-reach.registry"), N) + src_count(include_bytes!("../../../builder/src/main.rs"), N) // CIVILREST (B738)
};

/// CLOCKPAINTERS (rmbp-ledger B719, BP-bar-17, BP-boot-18) — `tests clockpainters`: every civil painter prints through
/// `clock::locale` (`locale::PAINTERS` names them; `utc_splits_left` is the compile-time count of the clock module's
/// UTC split called from a painter file); the calendars' week starts on `system.locale.first_day` (the column of a
/// Thursday and the weekday of column 0 for mon/sun/sat, and — desktop builds — the clock menu's month glance on a
/// Saturday-first locale); the `date -s` seed is local and round-trips to UTC (Berlin summer, Los Angeles' spring gap
/// reads as standard time); and the build knob is gone. Fixture-local locales only: the live locale and the store are
/// never written.
/// `:: CLOCKPAINTERS: painters=13 utc_splits_left=0 first_day=ok knob_gone=1 -> PASS :: seed=ok` (CIVILREST B738: 13)
pub fn clockpainters_selftest() {
    use crate::clock::{locale, locale::Locale, unix_from_civil as at};
    let [mon, sun, sat] = [Some("mon"), Some("sun"), Some("sat")].map(|f| Locale::parse(None, None, f));
    // Thursday (4) and column 0, per first day.
    #[allow(unused_mut)]
    let mut first_ok = [(mon, 3usize, 1u8), (sun, 4, 0), (sat, 5, 6)].iter().all(|(l, col, wd0)| l.week_col(4) == *col && l.col_wd(0) == *wd0);
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        // 2026-10-06 09:41 UTC: the 1st is a Thursday — on a Saturday-first week it sits in column 5.
        let mut cl = alloc::vec::Vec::new();
        crate::video::statusmenu::clock_lines_in(&sat, 1_791_279_660, locale::civil_in(&sat, 1_791_279_660), &mut cl);
        let g = crate::video::statusmenu::GRID_MARK;
        let grid = cl.len() >= 5 && cl[3].as_bytes().first() == Some(&g) && &cl[3].as_bytes()[1..] == b"Sa|Su|Mo|Tu|We|Th|Fr" && &cl[4].as_bytes()[1..] == b"|||||1|2";
        if !grid { serial_println!("[clockpainters] glance got={:?}", cl.get(3..5)); }
        first_ok &= grid;
    }
    // The seed: local civil -> UTC through the zone in force at that moment.
    let be = Locale::parse(Some("Europe/Berlin"), None, None);
    let la = Locale::parse(Some("America/Los_Angeles"), None, None);
    let seeds = [
        (locale::utc_of_local_in(&be, at(2026, 7, 15, 14, 34, 0)), at(2026, 7, 15, 12, 34, 0)),
        (locale::utc_of_local_in(&be, at(2026, 1, 15, 14, 34, 0)), at(2026, 1, 15, 13, 34, 0)),
        (locale::utc_of_local_in(&la, at(2026, 3, 8, 2, 30, 0)), at(2026, 3, 8, 10, 30, 0)),
        (locale::utc_of_local_in(&Locale::DEFAULT, at(2026, 7, 15, 14, 34, 0)), at(2026, 7, 15, 14, 34, 0)),
    ];
    let seed_ok = seeds.iter().all(|(g, w)| g == w) && locale::fmt_long_in(&be, at(2026, 7, 15, 12, 34, 5)) == "2026-07-15 14:34:05 Europe/Berlin UTC+02:00";
    if !seed_ok { serial_println!("[clockpainters] seeds={:?} long={}", seeds, locale::fmt_long_in(&be, at(2026, 7, 15, 12, 34, 5))); }
    let knob_gone = CLOCK_KNOB_SITES == 0;
    let ok = locale::PAINTERS >= 8 && CLOCK_UTC_SPLITS == 0 && first_ok && knob_gone && seed_ok;
    serial_println!(
        ":: CLOCKPAINTERS: painters={} utc_splits_left={} first_day={} knob_gone={} -> {} :: seed={}",
        locale::PAINTERS, CLOCK_UTC_SPLITS, if first_ok { "ok" } else { "no" }, knob_gone as u8, if ok { "PASS" } else { "FAIL" }, if seed_ok { "ok" } else { "no" }
    );
}

/// HWREPORT (rmbp-ledger B725, BP-bar-25): `tests hwreport` — the census against the registries it reads. One waiting
/// census (`hw::census(true)`: a held USB loan or EHCI table is retried, bounded); every row carries a bus, an id, a
/// driver (a module or `none`) and a state; the rows on each bus equal that registry's own count (PCI = the boot's
/// function table, USB = the active xHCI slots + the EHCI HID endpoints, block = the SATA registry + the SD card) and
/// the registries together hold every row; an x86 machine lists at least one PCI function. One `[hw] row` per device.
fn hwreport_selftest() {
    let c = crate::hw::census(true);
    for r in &c.rows {
        serial_println!("[hw] row {}", crate::hw::line(r));
    }
    let n = c.rows.len();
    let driven = c.driven();
    let on = |b: &[&str]| c.rows.iter().filter(|r| b.contains(&r.bus)).count();
    let shaped = c.rows.iter().all(|r| !r.bus.is_empty() && !r.id.is_empty() && !r.driver.is_empty() && !r.state.is_empty() && !r.name.is_empty());
    let reg = c.reg; let (tab_ok, tab_kv, tab_tail) = hwtab_leg(n); // HWTAB (B737): the Hardware tab paints the census's rows (file tail). LINE-NEUTRAL fold, code before the comment.
    let why = if c.busy {
        Some("registry-held")
    } else if !shaped {
        Some("row-shape")
    } else if on(&["pci"]) != reg.pci {
        Some("pci-count")
    } else if on(&["usb"]) != reg.usb {
        Some("usb-count")
    } else if on(&["sata", "sd"]) != reg.block {
        Some("block-count")
    } else if reg.pci + reg.usb + reg.block + reg.other < n {
        Some("registries-short")
    } else if cfg!(target_arch = "x86_64") && reg.pci == 0 {
        Some("no-pci")
    } else {
        None
    };
    match why {
        None if tab_ok => serial_println!(":: HWREPORT: devices={} driven={} none={} rows_ok=1 {} -> PASS :: {}", n, driven, n - driven, tab_kv, tab_tail), None => serial_println!(":: HWREPORT: devices={} driven={} none={} rows_ok=1 {} -> FAIL why=tab :: {}", n, driven, n - driven, tab_kv, tab_tail), // HWTAB (B737). LINE-NEUTRAL fold
        Some(w) => serial_println!(
            ":: HWREPORT: devices={} driven={} none={} rows_ok=0 pci={}/{} usb={}/{} block={}/{} -> FAIL why={} ::",
            n, driven, n - driven, on(&["pci"]), reg.pci, on(&["usb"]), reg.usb, on(&["sata", "sd"]), reg.block, w
        ),
    }
}

// SOFTYDOTS (rmbp-ledger B721; BP-video-24, BP-video-28, BP-bar-10/12/18) — TAIL-APPENDED, so no line above moves.
// `tests softydots`, under the Softy look (switched in, the look found put back): the taskbar's tray paints the camera
// light from a synthetic stream count (the accent dot at the tray's first item; none at count 0), the disk item from a
// forced low disk (the warning dot), the pulse on a held window's tile (the pip dimmed for the beat; and a live tile
// given `wm::attention` reads it back) — all through the band's own model, layout and row painter, offscreen; then the
// lock (the loginst fixture session `una` only, autolock's credential): the band leaves the glass, the card is centred,
// the right password brings the band back. One line:
// `:: SOFTYDOTS: cam=ok disk=ok pulse=ok lock=ok -> PASS ::` (`lock=skip(<why>)` -> SKIP when the session is not the fixture's).
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn softydots_selftest() {
    use crate::video::{look, softy::taskbar, wm};
    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!(":: SOFTYDOTS: -> SKIP :: reason=no-panel");
            return;
        }
        (fb.width(), fb.height())
    };
    #[cfg(feature = "login")]
    if !crate::video::desktopbuild::built() {
        serial_println!(":: SOFTYDOTS: -> SKIP :: reason=no-desktop");
        return;
    }
    let was = look::current();
    let _ = look::set(look::Look::Softy, "tests");
    wm::composite();
    let Some(d) = taskbar::dots_probe(pw, ph) else {
        let _ = look::set(was, "tests");
        serial_println!(":: SOFTYDOTS: -> SKIP :: reason=panel-too-small {}x{}", pw, ph);
        return;
    };
    let yn = |b: bool| if b { "ok" } else { "FAIL" };
    let cam = d.cam_on == d.accent && d.cam_off != d.accent && d.cam_live;
    let disk = d.disk_on == d.warn && d.disk_off != d.warn;
    let pulse = d.pip_off == d.pip && d.pip_on != d.pip_off && d.pulse_live != Some(false);
    #[allow(unused_mut)]
    let mut lock_word: alloc::string::String = alloc::string::String::from("skip(no-login)");
    #[allow(unused_mut)]
    let mut lock_ok: Option<bool> = None;
    #[cfg(feature = "login")]
    {
        use crate::video::{crystal::login, lock};
        const NAME: &[u8] = b"una";
        const PW: &[u8] = b"correct-horse";
        let mut nb = [0u8; crate::fs::users::NAME_MAX];
        let is_una = |nb: &mut [u8; crate::fs::users::NAME_MAX]| matches!(crate::fs::users::whoami(nb), Some(k) if &nb[..k] == NAME);
        if !is_una(&mut nb) {
            lock_word = alloc::string::String::from("skip(session-not-fixture-user)");
        } else if lock::active() || login::is_open() {
            lock_word = alloc::string::String::from("skip(screen-up)");
        } else {
            let entered = login::lock() && lock::active();
            wm::composite();
            let gone = entered && taskbar::painted_rect().is_none();
            let centred = lock::card().map(|(_, (_, y, _, h))| (y + h / 2).abs_diff(ph / 2) <= 2).unwrap_or(false);
            let mut back = false;
            if entered {
                for &b in PW {
                    let _ = crate::fs::users::screen_key(b);
                }
                let _ = crate::fs::users::screen_key(b'\n');
                let t0 = crate::arch::ms();
                loop {
                    login::submit_drain();
                    if !login::submit_busy() || crate::arch::ms().wrapping_sub(t0) > 15_000 {
                        break;
                    }
                    crate::arch::sched::yield_now();
                }
                wm::composite();
                back = !lock::active() && taskbar::painted_rect() == taskbar::rect(pw, ph);
            }
            let ok = entered && gone && centred && back;
            lock_ok = Some(ok);
            lock_word = alloc::format!("{}", if ok { "ok" } else { "FAIL" });
            serial_println!("[softydots] lock entered={} taskbar_gone={} card_centred={} back={}", entered as u8, gone as u8, centred as u8, back as u8);
        }
    }
    let _ = look::set(was, "tests");
    wm::composite();
    serial_println!(
        "[softydots] cam_on={:#08x} cam_off={:#08x} accent={:#08x} disk_on={:#08x} disk_off={:#08x} warn={:#08x} pip_on={:#08x} pip_off={:#08x} pip={:#08x} cam_live={} pulse_live={}",
        d.cam_on, d.cam_off, d.accent, d.disk_on, d.disk_off, d.warn, d.pip_on, d.pip_off, d.pip, d.cam_live as u8,
        match d.pulse_live { None => "none", Some(true) => "1", Some(false) => "0" }
    );
    let verdict = match lock_ok {
        _ if !(cam && disk && pulse) => "FAIL",
        Some(true) => "PASS",
        Some(false) => "FAIL",
        None => "SKIP",
    };
    serial_println!(":: SOFTYDOTS: cam={} disk={} pulse={} lock={} -> {} ::", yn(cam), yn(disk), yn(pulse), lock_word, verdict);
}

// QUARRYVERSIONS (rmbp-ledger B732, BP-bar-14) — TAIL-APPENDED. `tests fileversions`' Quarry legs, through the face on
// THE mount (the session home on the native volume): menu — 'Previous versions…' is in the file menu right after Get
// Info; listed — a file written once then overwritten 3 times lists 3 rows in the inspector (`getinfo::open_versions`);
// restored_via_face — the oldest row's Restore (`ops::version_press`, what the button runs) makes the first contents
// current again, re-lists 4 (the replaced contents kept: a restore is itself a versioned write) and the newest version
// is those replaced contents; Open's copy (version 1, under `<home>/.Trash/.versions/`) reads back byte-equal. A home
// that is not on the native volume keeps no versions: `listed=skip restored_via_face=skip`.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
#[allow(unreachable_code)]
fn fileversions_face() -> (bool, alloc::string::String) {
    #[cfg(all(feature = "quarry", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
        use crate::video::quarry::live::{getinfo, ops};
        let okf = |b: bool| if b { "ok" } else { "FAIL" };
        let t = crate::shell::vfs_mount_table();
        let home = crate::fs::trash::home_base();
        if t.stat(&home).is_err() {
            let _ = t.create(&home, NodeKind::Dir, P);
        }
        let path = format!("{}/VersionsFace.txt", home);
        let menu = ops::versions_item_probe();
        if ops::native_path(&path).is_none() {
            return (menu, format!("menu={} listed=skip restored_via_face=skip refused=skip", okf(menu)));
        }
        let _ = ops::op_delete(&path);
        let body = |k: u8| -> alloc::vec::Vec<u8> { (0..3000u32).map(|i| k.wrapping_mul(37).wrapping_add(i as u8)).collect() };
        let made = t.create(&path, NodeKind::File, P).is_ok() && t.write(&path, 0, &body(0), P).is_ok();
        let writes = (1..=3u8).filter(|&k| t.write(&path, 0, &body(k), P).is_ok()).count();
        getinfo::open_versions(&path);
        let n = getinfo::version_count();
        let listed_ok = made && writes == 3 && n == Some(3);
        let oldest = n.and_then(|n| n.checked_sub(1)).and_then(getinfo::version_seq);
        if let Some(seq) = oldest {
            ops::version_press(&path, seq, true);
        }
        let back = t.read(&path, 0, 4096).is_ok_and(|d| d == body(0));
        let relisted = getinfo::version_count() == Some(4);
        let newest = ops::native_path(&path).and_then(|np| crate::fs::unafs::read_version(&np, 0)).is_some_and(|d| d == body(3));
        let restored = oldest.is_some() && back && relisted && newest;
        let copy = getinfo::version_seq(1).and_then(|s| ops::op_open_version(&path, s).ok());
        let open_ok = copy.as_deref().is_some_and(|c| c.starts_with(ops::versions_scratch().as_str()) && t.read(c, 0, 4096).is_ok_and(|d| d == body(2)));
        // QVBAR (B732): refused — the ONE classifier maps the crate's real refusals (VERSIONSCOMPAT's `ReadOnly`, crawl
        // B's restore guard `CorruptVolume`) to their reasons and nothing else to one; the newest row marked refused
        // reads greyed, and its Restore through `ops::version_press` (what the button runs) changes neither the
        // file's bytes nor the list (the reason is toasted, the restore not run).
        use ::unafs::fs::FileSystemError as Fse;
        let kinds = ops::restore_refusal(&Fse::ReadOnly(1 << 40)) == Some(ops::REFUSED_READONLY)
            && ops::restore_refusal(&Fse::CorruptVolume("qvbar")) == Some(ops::REFUSED_CORRUPT)
            && ops::restore_refusal(&Fse::NotFound).is_none();
        let (before, count, seq0) = (t.read(&path, 0, 4096).ok(), getinfo::version_count(), getinfo::version_seq(0));
        if let Some(s) = seq0 {
            getinfo::mark_refused(s, ops::REFUSED_CORRUPT);
            ops::version_press(&path, s, true);
        }
        let greyed = seq0.is_some_and(|s| getinfo::version_refused(s) == Some(ops::REFUSED_CORRUPT));
        let unchanged = before.is_some() && t.read(&path, 0, 4096).ok() == before && getinfo::version_count() == count;
        let refused = kinds && greyed && unchanged;
        serial_println!("[fileversions] face listed={:?} back={} relisted={} newest={} open_copy={} path={} refused_kinds={} greyed={} unchanged={}", n, back, relisted, newest, okf(open_ok), copy.as_deref().unwrap_or("-"), okf(kinds), okf(greyed), okf(unchanged));
        if let Some(c) = copy.as_deref() {
            let _ = t.unlink(c, P);
        }
        getinfo::dismiss();
        let _ = ops::op_delete(&path);
        let ok = menu && listed_ok && restored && open_ok && refused;
        return (ok, format!("menu={} listed={} restored_via_face={} refused={}", okf(menu), n.unwrap_or(0), okf(restored), okf(refused)));
    }
    (true, alloc::string::String::from("menu=- listed=- restored_via_face=- refused=-"))
}

// LOCK2 (rmbp-ledger B728, BP-video-28) — TAIL-APPENDED, so no line above moves. `tests autolock`'s two extra legs, folded
// onto its lines: while the lock is up a window is minted for a ring-3-shaped owner and presents once (the veil must
// answer `Suppressed`: it is not on the glass); after the right password the row must be CLEAN (painted by the unlock's
// own composite) within one 17 ms frame, and a present must reach the compositor again (`resumed=` is the resume pass's
// count, `lock::last_resumed`). `hole_ms` = ms from the unlock's return to the row clean (0 = clean at the first look).
// The row is closed before the table fingerprint. The minting seam (`wm::wl_fixture_mint`) is the x86 `wc` + witness
// fixture's; elsewhere the legs are `-` and fail the line (the desktop fixture runs on that lane).
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
const LOCK2_OWNER: u64 = 0xD53;

/// While locked: mint the row and present it once. `(id, suppressed)`; `id` is `WIN_NONE` when nothing was minted.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn lock2_locked(entered: bool) -> (crate::video::wm::WinId, bool) {
    use crate::video::wm;
    #[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
    if entered && crate::video::lock::active() {
        let id = wm::wl_fixture_mint(LOCK2_OWNER, b"lock2");
        if id != wm::WIN_NONE {
            let sup = wm::present_outcome(id) == wm::Presented::Suppressed;
            return (id, sup);
        }
    }
    let _ = entered;
    (wm::WIN_NONE, false)
}

/// After the unlock: `(painted, hole_ms)` — the row clean within one frame and presenting again; the row closed.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn lock2_unlocked(l: (crate::video::wm::WinId, bool)) -> (bool, Option<u64>) {
    use crate::video::wm;
    let (id, suppressed) = l;
    if id == wm::WIN_NONE {
        return (false, None);
    }
    let t0 = crate::arch::ms();
    let mut hole = None;
    loop {
        let el = crate::arch::ms().wrapping_sub(t0);
        if wm::info(id).map_or(false, |i| !i.damaged) {
            hole = Some(el);
            break;
        }
        if crate::video::lock::active() || el > 17 {
            break;
        }
        crate::arch::sched::yield_now();
    }
    let again = !crate::video::lock::active() && wm::present_outcome(id) != wm::Presented::Suppressed;
    let _ = wm::close_owner(LOCK2_OWNER);
    serial_println!("[lock] fixture row={} suppressed_while_locked={} clean_ms={} presents_again={}", id, suppressed as u8, hole.map_or(-1i64, |m| m as i64), again as u8);
    (suppressed && hole.is_some() && again, hole)
}

// AUDIOMIX (rmbp-ledger B731) — TAIL-APPENDED. `tests audiomix`: the mixer (`audio/mixer.rs`) mixes a 440 and a 660 Hz
// source for 1 s into the sink's period stream (an offline `Mix`, the speaker untouched), read back FFT-free: zero
// crossings and peaks show both tones, a half gain and master level 8 halve the peak, mute is silence.
// `:: AUDIOMIX: sources=2 mixed=ok underruns=0 mute=ok -> PASS|FAIL ::`; a kernel with no audio path says SKIP.
fn audiomix_selftest() {
    #[cfg(any(feature = "uvc", feature = "hda-tone"))]
    crate::audio::mixer::selftest();
    #[cfg(not(any(feature = "uvc", feature = "hda-tone")))]
    serial_println!(":: AUDIOMIX: sources=0 mixed=- underruns=0 mute=- -> SKIP reason=no-audio-path ::");
}

// STAGEWINDOW (rmbp-ledger B733, BP-boot-17, BP-bar-8) — TAIL-APPENDED, the body of `tests kelfstage`'s cut leg. THE
// promotion (`install::abimage::promote_on`) runs on a RAM directory (`RamEsp`: case-blind names, a rename onto an
// existing name refused — the FAT's rules) seeded as a passed trial whose two images SHARE one `kelf=` max, with the
// power lost after k writes (k = 0 .. done) and, when the cut write is a file write, half its bytes landed. At every
// cut the loader is modelled with its own shared fns (`abslot::decide`, the missing-file open, `kelfsha::record`,
// `kelfsha::check`, the other-slot fallback) and must read ok or unverified — never the halt; then
// `abimage::heal_on` runs with the power back and the loader must read slot A ok. keyed: another build with the
// same `kelf=` max reads stale, an appended byte stale, a byte flipped past the 4 KiB head a mismatch.

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelf_chk(rec: Option<&[u8]>, slot: u8, bytes: &[u8]) -> crate::install::abimage::kelfsha::Check {
    use crate::install::abimage::kelfsha;
    let sha = crypto_core::sha2::sha256(bytes);
    let key = kelfsha::key_of(bytes, bytes.len() as u64, &crypto_core::sha2::sha256(kelfsha::head(bytes)));
    kelfsha::check(rec, slot, &sha, key.as_ref())
}

/// The ESP's root directory in RAM, losing power after `budget` mutations (`torn`: the cut write lands half).
#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
#[derive(Clone)]
struct RamEsp {
    files: alloc::vec::Vec<(alloc::string::String, alloc::vec::Vec<u8>)>,
    budget: u32,
    torn: bool,
    dead: bool,
    tore: bool,
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
impl RamEsp {
    fn at(&self, name: &str) -> Option<usize> {
        self.files.iter().position(|(n, _)| n.eq_ignore_ascii_case(name))
    }
    fn file(&self, name: &str) -> Option<&[u8]> {
        self.at(name).map(|i| self.files[i].1.as_slice())
    }
    fn land(&mut self, name: &str, bytes: alloc::vec::Vec<u8>) {
        match self.at(name) {
            Some(i) => self.files[i].1 = bytes,
            None => self.files.push((alloc::string::String::from(name), bytes)),
        }
    }
    fn spend(&mut self) -> Result<(), &'static str> {
        if self.dead || self.budget == 0 {
            self.dead = true;
            return Err("power-lost");
        }
        self.budget -= 1;
        Ok(())
    }
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
impl crate::install::abimage::Esp for RamEsp {
    fn has(&mut self, name: &str) -> bool {
        self.at(name).is_some()
    }
    fn get(&mut self, name: &str) -> Option<alloc::vec::Vec<u8>> {
        let max = crate::install::abimage::kelfsha::MAX;
        self.file(name).map(|b| b[..b.len().min(max)].to_vec())
    }
    fn put(&mut self, name: &str, bytes: &[u8]) -> Result<(), &'static str> {
        if !self.dead && self.budget == 0 && self.torn {
            self.land(name, bytes[..bytes.len() / 2].to_vec());
            self.dead = true;
            self.tore = true;
            return Err("power-lost");
        }
        self.spend()?;
        self.land(name, bytes.to_vec());
        Ok(())
    }
    fn mv(&mut self, from: &str, to: &str) -> Result<(), &'static str> {
        let Some(i) = self.at(from) else { return Err("not-found") };
        if self.at(to).is_some() {
            return Err("exists");
        }
        self.spend()?;
        self.files[i].0 = alloc::string::String::from(to);
        Ok(())
    }
    fn rm(&mut self, name: &str) -> Result<(), &'static str> {
        let Some(i) = self.at(name) else { return Ok(()) };
        self.spend()?;
        self.files.remove(i);
        Ok(())
    }
}

/// The loader on a RAM directory: the slot it boots and the verdict, `None` = the halt (both slots Bad or absent).
#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelf_loader_reads(e: &RamEsp) -> Option<(u8, crate::install::abimage::kelfsha::Check)> {
    use crate::install::abimage::kelfsha::{self, Check};
    use crate::install::abslot;
    let letter = |s: u8| if s == 0 { kelfsha::SLOT_A } else { kelfsha::SLOT_B };
    let d = abslot::decide(e.file(abslot::STAGE), e.file(abslot::SHUTDOWN));
    let first: u8 = if d.slot == abslot::Slot::Elf { 0 } else { 1 };
    let (slot, bytes) = match e.file(kelfsha::file_of(letter(first))) {
        Some(b) => (first, b),
        None => (1 - first, e.file(kelfsha::file_of(letter(1 - first)))?),
    };
    let rec = kelfsha::record(e.file(kelfsha::NAME), e.file(kelfsha::NEXT));
    let c = kelf_chk(rec, letter(slot), bytes);
    if c != Check::Bad {
        return Some((slot, c));
    }
    let other = 1 - slot;
    let c2 = kelf_chk(rec, letter(other), e.file(kelfsha::file_of(letter(other)))?);
    if c2 != Check::Bad { Some((other, c2)) } else { None }
}

/// A little-endian ELF64 of `len` bytes, one PT_LOAD (so `KELF max=` 0x300000 for every `fill`), body bytes `i ^ fill`.
#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelf_image(fill: u8, len: usize) -> alloc::vec::Vec<u8> {
    let mut e = alloc::vec![0u8; len];
    e[..4].copy_from_slice(crate::install::abimage::kelfsha::ELF_MAGIC);
    e[4] = 2;
    e[5] = 1;
    e[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
    e[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
    e[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
    e[64..68].copy_from_slice(&1u32.to_le_bytes());
    e[64 + 0x10..64 + 0x18].copy_from_slice(&0x20_0000u64.to_le_bytes());
    e[64 + 0x28..64 + 0x30].copy_from_slice(&0x10_0000u64.to_le_bytes());
    for (i, b) in e[120..].iter_mut().enumerate() {
        *b = (i as u8) ^ fill;
    }
    e
}

/// One keyed row through THE spelling (`kelfsha::row`).
#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelf_row_of(slot: u8, img: &[u8]) -> alloc::vec::Vec<u8> {
    use crate::install::abimage::kelfsha;
    let sha = crypto_core::sha2::sha256(img);
    let Some(key) = kelfsha::key_of(img, img.len() as u64, &crypto_core::sha2::sha256(kelfsha::head(img))) else {
        return alloc::vec::Vec::new();
    };
    let mut out = [0u8; kelfsha::ROW_MAX];
    let n = kelfsha::row(&kelfsha::Row::new(sha, slot, key), &mut out);
    out[..n].to_vec()
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
struct KelfCuts {
    cuts: u32,
    halts: u32,
    healed: u32,
    keyed: bool,
    done: bool,
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
impl KelfCuts {
    fn pass(&self) -> bool {
        self.done && self.cuts > 0 && self.halts == 0 && self.healed == self.cuts && self.keyed
    }
}

#[cfg(all(target_arch = "x86_64", feature = "installdemo", feature = "ahci"))]
fn kelfstage_cuts() -> KelfCuts {
    use crate::install::abimage::{self, kelfsha::{self, Check}};
    use crate::install::abslot;
    let old = kelf_image(0x5a, 8192);
    let new = kelf_image(0xa5, 8192); // another build: the same `kelf=` max and size, another head
    // keyed: the same-max build never aliases; a size change is stale; a tear past the head is a mismatch.
    let rec_old = kelf_row_of(kelfsha::SLOT_A, &old);
    let mut longer = old.clone();
    longer.push(0);
    let mut torn = old.clone();
    torn[6000] ^= 1;
    let keyed = kelfsha::kelf_of(&old) == kelfsha::kelf_of(&new)
        && kelf_chk(Some(rec_old.as_slice()), kelfsha::SLOT_A, &old) == Check::Ok
        && kelf_chk(Some(rec_old.as_slice()), kelfsha::SLOT_A, &new) == Check::Stale
        && kelf_chk(Some(rec_old.as_slice()), kelfsha::SLOT_A, &longer) == Check::Stale
        && kelf_chk(Some(rec_old.as_slice()), kelfsha::SLOT_A, &torn) == Check::Bad;
    // The seed: a trial of `new` that passed — ELF=old, NEW=new, KELF.SHA {A:old, B:new}, STAGE.REC, why=trial.
    let mut r1 = [0u8; kelfsha::MAX];
    let mut r2 = [0u8; kelfsha::MAX];
    let n1 = kelfsha::with_row(&[], kelfsha::SLOT_A, Some(rec_old.as_slice()), &mut r1).unwrap_or(0);
    let n2 = kelfsha::with_row(&r1[..n1], kelfsha::SLOT_B, Some(kelf_row_of(kelfsha::SLOT_B, &new).as_slice()), &mut r2).unwrap_or(0);
    let mut sr = [0u8; abslot::REC_MAX];
    let sn = abslot::stage_record(kelfsha::kelf_of(&new).unwrap_or(0), new.len() as u64, 0, &mut sr);
    let seed = RamEsp {
        files: alloc::vec![
            (alloc::string::String::from(abslot::ELF), old.clone()),
            (alloc::string::String::from(abslot::NEW), new.clone()),
            (alloc::string::String::from(kelfsha::NAME), r2[..n2].to_vec()),
            (alloc::string::String::from(abslot::STAGE), sr[..sn].to_vec()),
            (alloc::string::String::from(abslot::SHUTDOWN), abslot::REC_TRIAL.to_vec()),
        ],
        budget: u32::MAX,
        torn: false,
        dead: false,
        tore: false,
    };
    let mut out = KelfCuts { cuts: 0, halts: 0, healed: 0, keyed, done: false };
    for k in 0..32u32 {
        for tear in [false, true] {
            let mut e = seed.clone();
            e.budget = k;
            e.torn = tear;
            let r = abimage::promote_on(&mut e);
            if !e.dead {
                if tear {
                    continue; // ran to the end: the clean pass already counts it
                }
                out.done = r.is_ok();
            } else if tear && !e.tore {
                continue; // the cut fell on a rename or an unlink: nothing to tear
            }
            out.cuts += 1;
            let v = kelf_loader_reads(&e);
            out.halts += v.is_none() as u32;
            let mut h = e.clone();
            h.budget = u32::MAX;
            h.dead = false;
            h.torn = false;
            let hv = abimage::heal_on(&mut h).ok().and_then(|_| kelf_loader_reads(&h));
            let healed = hv == Some((0, Check::Ok));
            out.healed += healed as u32;
            let say = |v: Option<(u8, Check)>| match v {
                None => alloc::string::String::from("HALT"),
                Some((s, c)) => alloc::format!("{}:{}", if s == 0 { "A" } else { "B" }, c.why_word()),
            };
            serial_println!("[kelfstage] cut writes={} torn={} loader={} healed={} -> {}", k, tear as u8, say(v), say(hv), if v.is_some() && healed { "ok" } else { "WRONG" });
        }
        if out.done {
            break;
        }
    }
    out
}

// HWTAB (rmbp-ledger B737, BP-video-32, BP-bar-25) — TAIL-APPENDED, so no line above moves. `tests hwreport`'s tab
// leg: the Settings Hardware tab (`settings::HW_TAB`) painted offscreen page by page paints one inked line per census
// row (`rows_painted` == the rows `hw::last` holds == the fixture's census), and a press on a line resolves back to
// that row with its note (`No driver yet — tell us: <bus> <id>` for a `none` driver). A build without the Settings
// window (no desktop family) has no tab: the leg reads `tab=- rows_painted=-` and does not fail the census line.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn hwtab_leg(n: usize) -> (bool, alloc::string::String, alloc::string::String) {
    let (tab, listed, painted, press, note) = crate::video::settings::hwtab_probe();
    let (clip, report) = hwtab2_leg(); let ok = tab == crate::video::settings::HW_TAB && listed == n && painted == listed && press && clip == "ok" && !report.starts_with("FAIL"); // HWTAB2 (B759): the clipboard and the report legs (file END). LINE-NEUTRAL fold, code before the comment.
    (ok, format!("tab={} rows_painted={}", tab, painted), format!("listed={} press={} note=\"{}\" clip={} report={}", listed, if press { "ok" } else { "bad" }, note, clip, report))
}

#[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn hwtab_leg(_n: usize) -> (bool, alloc::string::String, alloc::string::String) {
    (true, alloc::string::String::from("tab=- rows_painted=-"), alloc::string::String::from("press=- (no Settings window in this build)"))
}

/// PINCHZOOM (rmbp-ledger B735) — `tests pinchzoom`: synthetic contact streams through the gesture recogniser
/// (`video::zoom::gestures::Recogniser`) and the routes' targets (not the swipe latch, so the device-service pass never
/// acts twice): (1) two contacts spreading 1000 -> 1640 sensor units lock as a pinch, step once, and
/// `zoom::set_factor_at` asks z = 2 (stepped from the user's own z when a lens is on; restored, never stored);
/// (2) two contacts stroking 800 units together lock as a scroll whose deltas sum to the stroke, never claimed;
/// (3) on three fixture windows a left three-finger swipe brings the least-recent row to the front with the focus
/// (`wm::gesture_swipe(+1)`) and a right one brings the row under the top back (`-1`).
/// `:: PINCHZOOM: pinch=ok scroll=ok swipe=ok contacts=<real|synthetic> -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn pinchzoom_selftest() {
    use crate::video::wm;
    use crate::video::zoom::{self, gestures::{self as g, Contact, Out, Recogniser, Sample}};
    static SURF: [u32; 64] = [0; 64];
    const OWNERS: [u64; 3] = [0x735A, 0x735B, 0x735C];
    const _: () = assert!(!wm::is_kernel_owner(0x735A) && !wm::is_kernel_owner(0x735B) && !wm::is_kernel_owner(0x735C));
    let c = |id: u8, x: i32, y: i32| Some(Contact { id, x, y, touch: 400 });
    let lift = |ms: u64| Sample { n: 0, c: [None, None], ms, pressed: false, swipe_on: true };
    // (1) the pinch
    let mut r = Recogniser::new();
    let (mut steps, mut scale, mut claimed, mut scrolled) = (0i32, 0u32, true, false);
    for i in 0..=16i32 {
        let h = 500 + i * 20;
        let s = Sample { n: 2, c: [c(0, -h, 3000), c(1, h, 3000)], ms: i as u64 * 8, pressed: false, swipe_on: true };
        match r.step(&s) {
            Out::Pinch { step, scale_pct, .. } => {
                steps += step as i32;
                scale = scale_pct;
                claimed &= r.claims();
            }
            Out::Scroll { .. } => scrolled = true,
            _ => {}
        }
    }
    let _ = r.step(&lift(200));
    let (z0, f0) = zoom::want();
    let zw = zoom::stepped(z0, steps.clamp(-1, 1) as i8);
    let at = crate::video::panel_info_nonblocking().map_or((320, 240), |i| ((i.width / 2) as u32, (i.height / 2) as u32));
    zoom::set_factor_at(zw, Some(at), false);
    let z = zoom::want().0;
    zoom::set(z0, f0);
    let pinch_ok = steps == 1 && scale == 164 && claimed && !scrolled && z == zw && (z0 != 0 || z == 2);
    // (2) the scroll
    let mut r = Recogniser::new();
    let (mut sum, mut end, mut sc_claim, mut pinched) = ((0i32, 0i32), None, false, false);
    for i in 0..=20i32 {
        let y = 2000 + i * 40;
        let s = Sample { n: 2, c: [c(0, -500, y), c(1, 500, y)], ms: i as u64 * 8, pressed: false, swipe_on: true };
        match r.step(&s) {
            Out::Scroll { dx, dy } => sum = (sum.0 + dx, sum.1 + dy),
            Out::Pinch { .. } => pinched = true,
            _ => {}
        }
        sc_claim |= r.claims();
    }
    if let Out::ScrollEnd { dx, dy, .. } = r.step(&lift(200)) {
        end = Some((dx, dy));
    }
    let scroll_ok = sum == (0, 800) && end == Some((0, 800)) && !sc_claim && !pinched;
    // (3) the swipe
    let stream = |r: &mut Recogniser, dx: i32| -> (i8, u32, bool) {
        let (mut dir, mut fired, mut claim) = (0i8, 0u32, true);
        for i in 0..=16i32 {
            let x = 2000 + i * dx;
            let s = Sample { n: 3, c: [c(0, x, 3000), c(1, x + 900, 3100)], ms: i as u64 * 8, pressed: false, swipe_on: true };
            if let Out::Swipe { dir: d, .. } = r.step(&s) {
                dir = d;
                fired += 1;
            }
            claim &= r.claims();
        }
        let _ = r.step(&lift(300));
        (dir, fired, claim)
    };
    let surf = &raw const SURF as usize;
    let len = core::mem::size_of_val(&SURF);
    let saved = wm::focus_asid();
    let ring = crate::arch::x86_64::syscall::user_input_active();
    let mut ids = [wm::WIN_NONE; 3];
    for (i, o) in OWNERS.iter().enumerate() {
        ids[i] = wm::create(*o, surf, len, 8, 8, 32, b"pz-swipe");
    }
    let swipe = if ids.contains(&wm::WIN_NONE) {
        "skip"
    } else {
        let mut r = Recogniser::new();
        let (d1, n1, c1) = stream(&mut r, -100);
        let mut o0 = alloc::vec::Vec::new();
        let n0 = wm::cycle_order(&mut o0);
        let want1 = if n0 >= 2 { o0[n0 - 1] } else { wm::WIN_NONE };
        let got1 = if d1 != 0 { wm::gesture_swipe(d1) } else { None };
        let mut o1 = alloc::vec::Vec::new();
        let m1 = wm::cycle_order(&mut o1);
        let next_ok = d1 == 1 && n1 == 1 && c1 && got1 == Some(want1) && m1 >= 2 && o1[0] == want1 && wm::wl_focused().map(|f| f.0) == Some(want1);
        let want2 = if m1 >= 2 { o1[1] } else { wm::WIN_NONE };
        let (d2, n2, c2) = stream(&mut r, 100);
        let got2 = if d2 != 0 { wm::gesture_swipe(d2) } else { None };
        let mut o2 = alloc::vec::Vec::new();
        let m2 = wm::cycle_order(&mut o2);
        let prev_ok = d2 == -1 && n2 == 1 && c2 && got2 == Some(want2) && m2 >= 1 && o2[0] == want2 && wm::wl_focused().map(|f| f.0) == Some(want2);
        serial_println!("[input] pinchzoom swipe windows={} next={}->{} prev={}->{}", n0, want1, next_ok as u8, want2, prev_ok as u8);
        if next_ok && prev_ok { "ok" } else { "bad" }
    };
    for o in OWNERS {
        wm::close_owner(o);
    }
    crate::arch::x86_64::syscall::user_input_set_active(ring);
    wm::focus_changed(saved);
    let contacts = if g::real_samples() > 0 { "real" } else { "synthetic" };
    let ok = |b: bool| if b { "ok" } else { "bad" };
    let verdict = if swipe == "skip" { "SKIP" } else if pinch_ok && scroll_ok && swipe == "ok" { "PASS" } else { "FAIL" };
    serial_println!(
        ":: PINCHZOOM: pinch={} scroll={} swipe={} contacts={} -> {} :: z={} steps={} scale={}.{:02} scroll_dy={} end={:?}",
        ok(pinch_ok), ok(scroll_ok), swipe, contacts, verdict, z, steps, scale / 100, scale % 100, sum.1, end
    );
}

// GRANTNAMES (rmbp-ledger B742, BP-security-17, BP-net-12, BP-bar-12) — TAIL-APPENDED. `tests netgrant`'s x86 legs, called
// from the x86 body above (folded, line count kept). `named`: the ask on the glass reads `dst=<name>(<ip:port>)` for the
// ip SYS_RESOLVE answered, and Settings > Privacy's net row reads `<name> (<ip>)`. `revoke_run`: with Always kept AND this
// run's allow row present (the Always leg's answer), Privacy's Revoke (`apptrust::net_revoked`, then `prefs::revoke`, the
// order `settings::privacy_service` runs them) drops the run row, so the same run's next connect ASKS (`-EINPROGRESS`, one
// more ask) instead of passing on the stale row; the ask is answered Never and `net_forget` clears it after.
// `:: NETGRANT: asked=1 denied_pkts=0 allowed=1 persisted=1 named=1 -> PASS :: … revoke_run=1 …`.
#[cfg(all(feature = "netring3", feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
fn netgrant_named(dst: &str, app: &str, name: &str, ip: [u8; 4], port: u16) -> bool {
    let ask = alloc::format!("{}({}.{}.{}.{}:{})", name, ip[0], ip[1], ip[2], ip[3], port);
    let row = alloc::format!("{} ({}.{}.{}.{})", name, ip[0], ip[1], ip[2], ip[3]);
    dst == ask && crate::fs::apptrust::net_row(app).1.as_deref() == Some(row.as_str())
}

#[cfg(all(feature = "netring3", feature = "smolnet", feature = "wc", target_arch = "x86_64"))]
fn netgrant_revoke_run(connect: &dyn Fn(u64) -> i64, key: &str, run: u64) -> bool {
    use crate::video::dialog::{self, Answer};
    let dropped = crate::fs::apptrust::net_revoked(key);
    let revoked = crate::prefs::revoke(key);
    let a = crate::fs::apptrust::net_asks();
    let r = connect(run);
    let asked = crate::fs::apptrust::net_asks() - a;
    dialog::open_now();
    dialog::answer(Answer::Default); // Never; the fixture's `net_forget` revokes it
    serial_println!("[netgrant] fixture revoke_run dropped={} revoked={} r={} asked={}", dropped, revoked as u8, r, asked);
    dropped >= 1 && revoked && r == -115 && asked == 1
}

// ══════════════ HELPSEARCH (rmbp-ledger B729; BP-bar-2/3, research C §8) — TAIL-APPENDED ══════════════
// `tests helpsearch`: the ONE help index the builder packed from `docs/help/` (`HELP.IDX`), read by `help::index`, and
// the three sources the Launcher shows together — 'disk full' finds the page `disk-full` first, 'time zone' finds the
// Settings row that edits `system.locale.tz` first, 'ptp' finds the `ptp` verb first. Then every "Open the setting"
// key any page names is on a Settings row (`unrowed=0`), and the Help window opens `date-time` with its three links
// and closes (`open=skip(no-panel)` headless). Registered beside `undomove` on the desktop block.
// `:: HELPSEARCH: pages=<n> hits=3/3 open=ok -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn helpsearch_selftest() {
    use crate::help::{index, window};
    index::retry();
    let loaded = index::ensure_loaded();
    let (pages, words, bytes) = index::with(|ix| (ix.pages.len(), ix.words.len(), ix.bytes)).unwrap_or((0, 0, 0));
    let page_top = index::with(|ix| ix.search_pages("disk full").first().map(|(p, _)| ix.pages[*p].slug.clone())).flatten().unwrap_or_default();
    let row_top = window::search_rows("time zone").first().map(|(i, _)| crate::video::settings::MAP[*i]);
    let verb_top = index::search_verbs("ptp").first().map(|(i, _)| crate::help::DOCS[*i].name).unwrap_or("");
    let page_hit = page_top == "disk-full";
    let row_hit = row_top.map(|r| r.keys.first().copied() == Some("locale.tz")).unwrap_or(false);
    let verb_hit = verb_top == "ptp";
    let hits = page_hit as u8 + row_hit as u8 + verb_hit as u8;
    let (links, unrowed) = index::with(|ix| {
        let mut n = 0usize;
        let mut bad = 0usize;
        for p in &ix.pages {
            for k in &p.keys {
                n += 1;
                if window::row_label(k).is_none() {
                    bad += 1;
                    serial_println!("[help] fixture unrowed page={} key=system.{}", p.slug, k);
                }
            }
        }
        (n, bad)
    })
    .unwrap_or((0, 0));
    let open = if !loaded {
        "skip(no-index)"
    } else {
        match window::open_page("date-time") {
            Ok(_) => {
                let ok = window::is_open() && window::shown() == "date-time" && window::links_shown() == 3;
                window::close();
                if ok && !window::is_open() { "ok" } else { "FAIL" }
            }
            Err(e) if e == "panel busy" || e == "geometry unavailable" || e == "window below floor" => "skip(no-panel)",
            Err(e) => {
                serial_println!("[help] fixture open refused reason={}", e);
                "FAIL"
            }
        }
    };
    serial_println!(
        "[help] fixture words={} bytes={} disk_full={} time_zone={} ptp={} links={} unrowed={}",
        words, bytes, if page_top.is_empty() { "-" } else { page_top.as_str() }, row_top.map(|r| r.label).unwrap_or("-"), if verb_top.is_empty() { "-" } else { verb_top }, links, unrowed
    );
    let ok = loaded && pages > 0 && hits == 3 && unrowed == 0 && open != "FAIL";
    serial_println!(":: HELPSEARCH: pages={} hits={}/3 open={} -> {} ::", pages, hits, open, if ok { "PASS" } else { "FAIL" });
}

// ══════════════ FILESEARCH (rmbp-ledger B741, BP-video-36, BP-storage-27) — `tests filesearch` — TAIL-APPENDED ══════════════
/// The Launcher's Files engine (`video::launcher::filesearch::answer`, the SAME function the field runs per keystroke)
/// over a RAM UnaFS volume (formatted here, never the user's disk): 10 000 names in 100 folders of 100 (FILESEARCH2
/// B745: a SPARSE RAM device — only the blocks written non-zero are held, so the volume costs its metadata, not its
/// 256 MiB span; was 2000 names on a 32 MiB `MemDevice`), each
/// created by `create_files_batch` with its `una:type` (and a photo's `media:taken`) in the same transaction, so the
/// name index and the facts are the core's own (NAMEINDEXTX B680). Three name queries — `beach` (one exact, two leaf
/// prefixes, two word prefixes, in that order), `img_0` (~250 prefix hits at any index size, cut to 20), `qqq` (none); the recent-first
/// rule through the opener's own `push_recent` (40 pushes keep 32, a recent name leads its rank and never crosses a
/// rank); the filters on the stored facts (`beach kind:photo` = 3, `kind:pdf` alone = 20 PDF rows via the attribute
/// catalog, `img taken:2026` and `taken:2026` alone = 20 rows all taken in 2026). `ms` = the slowest of the three
/// name queries (whole ms; `us` on the detail line); `under=1` = the slowest name query is within the 2 ms budget, a
/// VERDICT leg (FILESEARCH2). The reveal (Shift-Return) is the Launcher's own `pick` on the first file the root's walk
/// finds: `[search] reveal path= folder= -> shown=1 selected=1` (`reveal=ok`; `skip` when Quarry declines or is absent;
/// shown without the selection FAILS).
/// FSEARCHICONS (B760): `icons=` is [`filesearch_icons`] (tail) — the type cache leg.
/// `:: FILESEARCH: index=10000 hits_ok=3 recent=ok kind=ok ms=<n> under=1 icons=ok -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "unafs"))]
fn filesearch_selftest() {
    use crate::video::launcher::filesearch as fsr;
    use ::unafs::storage::Error as DevError;
    use ::unafs::{AttributeValue, BatchFile, BlockDevice, UnaFS, BLOCK_SIZE};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;
    const DIRS: usize = 100;
    const PER: usize = 100;
    const BLOCKS: u64 = 65536;
    /// Blocks the sparse device may hold (64 MiB): past it a write is refused, never an abort.
    const HELD_MAX: usize = 16384;
    /// FILESEARCH2 (B745): a RAM device that keeps only the blocks written non-zero; an unwritten (or re-zeroed) block
    /// reads as zeros, which is what a formatted span reads as.
    struct Sparse {
        map: BTreeMap<u64, Vec<u8>>,
    }
    impl BlockDevice for Sparse {
        fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), DevError> {
            if buf.len() as u64 != BLOCK_SIZE {
                return Err(DevError::BadBlockSize(buf.len(), BLOCK_SIZE));
            }
            if id >= BLOCKS {
                return Err(DevError::OutOfBounds(id));
            }
            match self.map.get(&id) {
                Some(b) => buf.copy_from_slice(b),
                None => buf.fill(0),
            }
            Ok(())
        }
        fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), DevError> {
            if buf.len() as u64 != BLOCK_SIZE {
                return Err(DevError::BadBlockSize(buf.len(), BLOCK_SIZE));
            }
            if id >= BLOCKS {
                return Err(DevError::OutOfBounds(id));
            }
            if buf.iter().all(|&b| b == 0) {
                self.map.remove(&id);
                return Ok(());
            }
            if let Some(b) = self.map.get_mut(&id) {
                b.copy_from_slice(buf);
                return Ok(());
            }
            let mut b: Vec<u8> = Vec::new();
            if self.map.len() >= HELD_MAX || b.try_reserve_exact(BLOCK_SIZE as usize).is_err() {
                return Err(DevError::AllocRefused(BLOCK_SIZE));
            }
            b.extend_from_slice(buf);
            self.map.insert(id, b);
            Ok(())
        }
        fn block_count(&self) -> u64 {
            BLOCKS
        }
        fn flush(&mut self) -> Result<(), DevError> {
            Ok(())
        }
    }
    let fail = |why: &str| serial_println!(":: FILESEARCH: index=0 hits_ok=0 recent=FAIL kind=FAIL ms=0 under=0 icons=FAIL -> FAIL :: reason={} ::", why);
    let mut fs = match UnaFS::format(Sparse { map: BTreeMap::new() }, 0) {
        Ok(f) => f,
        Err(_) => return fail("format"),
    };
    let (y2025, y2026) = (fsr::year_start(2025) + 150 * 86400, fsr::year_start(2026) + 150 * 86400);
    // Name i: the five beach names first, then IMG_/report-/notes_/song by i % 4.
    let spec = |i: usize| -> (String, &'static str, Option<i64>) {
        match i {
            0 => (String::from("Beach"), "text/plain", None),
            1 => (String::from("beach-day.jpg"), "image/jpeg", Some(y2026)),
            2 => (String::from("beachball.png"), "image/png", Some(y2025)),
            3 => (String::from("summer_beach.jpg"), "image/jpeg", Some(y2026)),
            4 => (String::from("old beach.pdf"), "application/pdf", None),
            _ => match i % 4 {
                0 => (format!("IMG_{:04}.jpg", i), "image/jpeg", Some(if i % 8 == 0 { y2026 } else { y2025 })),
                1 => (format!("report-{:04}.pdf", i), "application/pdf", None),
                2 => (format!("notes_{:04}.txt", i), "text/plain", None),
                _ => (format!("song {:04}.mp3", i), "audio/mpeg", None),
            },
        }
    };
    let t_build = crate::arch::ms();
    let root = fs.superblock.root_inode;
    let mut index = 0usize;
    for d in 0..DIRS {
        let Ok(dir) = fs.mkdir(root, format!("d{:02}", d)) else { return fail("mkdir") };
        let batch: Vec<BatchFile> = (d * PER..(d + 1) * PER)
            .map(|i| {
                let (name, mime, taken) = spec(i);
                let mut attributes = BTreeMap::new();
                attributes.insert(String::from(una_abi::attr_keys::TYPE), AttributeValue::String(String::from(mime)));
                if let Some(t) = taken {
                    attributes.insert(String::from(una_abi::attr_keys::MEDIA_TAKEN), AttributeValue::Int(t));
                }
                BatchFile { name, data: Vec::new(), attributes }
            })
            .collect();
        match fs.create_files_batch(dir, batch) {
            Ok(ids) => index += ids.len(),
            Err(_) => return fail("create"),
        }
    }
    let build_ms = crate::arch::ms().saturating_sub(t_build);
    let held_kb = fs.device.map.len() * (BLOCK_SIZE as usize / 1024);
    let ask = |fs: &mut UnaFS<Sparse>, q: &str, recent: &[String]| -> (Vec<fsr::Row>, u64) {
        let t0 = crate::hidstall::now_us();
        let rows = match fsr::answer(fs, &fsr::parse(q), recent) {
            Ok(Some(a)) => a.rows,
            _ => Vec::new(),
        };
        (rows, crate::hidstall::now_us().saturating_sub(t0))
    };
    let leaves = |rows: &[fsr::Row]| rows.iter().map(|r| String::from(r.leaf())).collect::<Vec<String>>();
    // The three name queries.
    let (r1, us1) = ask(&mut fs, "beach", &[]);
    let (r2, us2) = ask(&mut fs, "img_0", &[]);
    let (r3, us3) = ask(&mut fs, "qqq", &[]);
    let tiers1: Vec<u8> = r1.iter().map(|r| r.tier).collect();
    let q1 = tiers1 == [0, 1, 1, 2, 2] && r1[0].leaf() == "Beach" && r1[0].folder() == "/d00";
    let q2 = r2.len() == fsr::ROWS_MAX && r2.iter().all(|r| r.tier == 1 && r.leaf().starts_with("IMG_0") && r.mime == "image/jpeg");
    let q3 = r3.is_empty();
    let hits_ok = q1 as u32 + q2 as u32 + q3 as u32;
    // The recent-first rule, through the opener's own list rule.
    let mut recent: Vec<String> = Vec::new();
    for i in 0..40 {
        fsr::push_recent(&mut recent, &format!("/d19/filler-{}", i));
    }
    fsr::push_recent(&mut recent, "/d00/beachball.png");
    fsr::push_recent(&mut recent, "/d00/summer_beach.jpg");
    let (rr, _) = ask(&mut fs, "beach", &recent);
    let lr = leaves(&rr);
    let recent_ok = recent.len() == fsr::RECENT_MAX
        && recent[0] == "/d00/summer_beach.jpg"
        && lr.len() == 5
        && lr[0] == "Beach"
        && lr[1] == "beachball.png"
        && lr[3] == "summer_beach.jpg";
    // The filters on the stored facts.
    let tk = crate::hidstall::now_us();
    let (k1, _) = ask(&mut fs, "beach kind:photo", &[]);
    let (k2, _) = ask(&mut fs, "kind:pdf", &[]);
    let (k3, _) = ask(&mut fs, "img taken:2026", &[]);
    let (k4, _) = ask(&mut fs, "taken:2026", &[]);
    let kind_us = crate::hidstall::now_us().saturating_sub(tk);
    let n_of = |leaf: &str| leaf.get(4..8).and_then(|n| n.parse::<usize>().ok());
    let l1 = leaves(&k1);
    let kind1 = l1.len() == 3 && ["beach-day.jpg", "beachball.png", "summer_beach.jpg"].iter().all(|w| l1.iter().any(|l| l == w));
    let kind2 = k2.len() == fsr::ROWS_MAX && k2.iter().all(|r| r.mime == "application/pdf" && r.tier == 3);
    let kind3 = k3.len() == fsr::ROWS_MAX && k3.iter().all(|r| r.leaf().starts_with("IMG_") && n_of(r.leaf()).is_some_and(|n| n % 8 == 0));
    let kind4 = k4.len() == fsr::ROWS_MAX && k4.iter().all(|r| r.mime == "image/jpeg" && (r.leaf().ends_with("beach-day.jpg") || r.leaf().ends_with("summer_beach.jpg") || n_of(r.leaf()).is_some_and(|n| n % 8 == 0)));
    let kind_ok = kind1 && kind2 && kind3 && kind4;
    let worst_us = us1.max(us2).max(us3);
    let ms = worst_us / 1000;
    let under = worst_us <= fsr::BUDGET_MS * 1000;
    // The reveal (FILESEARCH2 M1): the Launcher's own pick, Shift latched, on the first file the root's walk finds.
    #[allow(unused_mut)]
    let mut reveal = "skip";
    #[cfg(feature = "quarry")]
    {
        let was_open = crate::video::quarry::is_open();
        let snap = crate::fs::search::snapshot(256);
        if let Some(h) = snap.hits.iter().find(|h| !h.dir) {
            fsr::latch_reveal(true);
            let said = fsr::pick(&h.path, false).unwrap_or_default();
            reveal = if said.contains("shown=1 selected=1") { "ok" } else if said.contains("shown=1") { "FAIL" } else { "skip" };
            if !was_open {
                crate::video::quarry::close();
            }
        }
    }
    let (icons, icons_detail) = filesearch_icons(&mut fs); // FSEARCHICONS (B760)
    serial_println!(
        "[filesearch] detail build_ms={} blocks={} held_kb={} q_us={}/{}/{} kind_us={} budget_ms={} under={} reveal={} q1={} q2={} q3={} kinds={}{}{}{} {} beach={:?}",
        build_ms, BLOCKS, held_kb, us1, us2, us3, kind_us, fsr::BUDGET_MS, under as u8, reveal,
        q1 as u8, q2 as u8, q3 as u8, kind1 as u8, kind2 as u8, kind3 as u8, kind4 as u8, icons_detail, leaves(&r1)
    );
    let ok = index == DIRS * PER && hits_ok == 3 && recent_ok && kind_ok && under && reveal != "FAIL" && icons;
    serial_println!(
        ":: FILESEARCH: index={} hits_ok={} recent={} kind={} ms={} under={} icons={} -> {} ::",
        index, hits_ok, if recent_ok { "ok" } else { "FAIL" }, if kind_ok { "ok" } else { "FAIL" }, ms, under as u8, if icons { "ok" } else { "FAIL" }, if ok { "PASS" } else { "FAIL" }
    );
}

// GEODEBACKUP (rmbp-ledger B740, BP-bar-1/14, BP-storage-28) — TAIL-APPENDED, so no line above moves. `tests geodebackup`:
// the kernel's `.geode` v1 writer and reader (`fs::geodew`) round-trip a home, in RAM. Home `a` (40 files in four
// folders; every file carries attributes — strings, ints, floats every third, a vector every seventh, a spilled 6000-byte
// blob every fifth — and every entry distinct ctime/mtime/atime) is backed up to a RAM stick; the bytes must equal an
// independent whole-record encoding in the host crate's `write_archive` order (`fmt`), and their length the walk's
// measure (`measured`); the stick restores into home `b` of a second RAM volume and every record must be equal (kind,
// bytes, the attribute set, the three times); a second restore into `b` is refused (`taken`) with nothing written; a
// one-byte-damaged archive is refused before any write; a restore into a new folder (`make`) succeeds.
// `:: GEODEBACKUP: files=40 attrs=ok times=ok restore=ok refused_taken=1 -> PASS|FAIL ::`
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
type GbRecord = (alloc::string::String, ::unafs::FileKind, u64, u64, u64, alloc::collections::BTreeMap<alloc::string::String, ::unafs::AttributeValue>, alloc::vec::Vec<u8>);

/// GEODEBACKUP: every record under inode `id` in pre-order, siblings in `ls` order — read whole, independently of the writer.
#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn gb_records<D: ::unafs::BlockDevice>(fs: &mut ::unafs::UnaFS<D>, id: u64, rel: &str, out: &mut alloc::vec::Vec<GbRecord>) -> Result<(), ::unafs::fs::FileSystemError> {
    use ::unafs::FileKind;
    for c in fs.ls(id)? {
        let p = if rel.is_empty() { c.name.clone() } else { alloc::format!("{}/{}", rel, c.name) };
        let ino = fs.read_inode(c.inode_id)?;
        let mut attrs = ino.attributes.clone();
        for k in ino.large_attributes.keys() {
            if let Some(v) = fs.get_attribute(c.inode_id, k)? {
                attrs.insert(k.clone(), v);
            }
        }
        let data = if ino.kind == FileKind::File && ino.size > 0 { fs.read_data(c.inode_id, 0, ino.size)? } else { alloc::vec::Vec::new() };
        out.push((p.clone(), ino.kind, ino.ctime, ino.mtime, ino.atime, attrs, data));
        if ino.kind == FileKind::Directory {
            gb_records(fs, c.inode_id, &p, out)?;
        }
    }
    Ok(())
}

#[cfg(any(target_arch = "aarch64", feature = "unafs"))]
fn geodebackup_selftest() {
    use crate::fs::geodew::{self, Bytes};
    use ::unafs::codec::{Encode, Writer};
    use ::unafs::fs::FileSystemError;
    use ::unafs::{AttributeValue as A, FileKind, MemDevice, UnaFS};
    use alloc::format;
    use alloc::vec::Vec;
    const BLOCKS: u64 = 1024;
    let (Ok(mut a), Ok(mut b)) = (UnaFS::format(MemDevice::with_blocks(BLOCKS), 0), UnaFS::format(MemDevice::with_blocks(BLOCKS), 0)) else {
        serial_println!(":: GEODEBACKUP: files=0 attrs=- times=- restore=- refused_taken=0 -> FAIL reason=format ::");
        return;
    };
    // Home a: four folders, 40 files with attributes, then the times on every entry (after every create).
    let made = (|| -> Result<(), FileSystemError> {
        let root = a.superblock.root_inode;
        let home = a.mkdir(root, "home".into())?;
        let ha = a.mkdir(home, "a".into())?;
        let docs = a.mkdir(ha, "Documents".into())?;
        let pics = a.mkdir(ha, "Pictures".into())?;
        let music = a.mkdir(ha, "Music".into())?;
        let albums = a.mkdir(music, "Albums".into())?;
        a.set_attribute(docs, "geode.view".into(), A::String("list".into()))?;
        let mut ids = alloc::vec![docs, pics, music, albums];
        let parents = [ha, docs, pics, albums];
        for i in 0..40u64 {
            let id = a.create_file(parents[(i % 4) as usize], format!("file-{:02}.dat", i))?;
            let len = if i == 13 { 200_000 } else { (i * 701) as usize };
            let data: Vec<u8> = (0..len).map(|k| (k as u64 * 31 + i) as u8).collect();
            if !data.is_empty() {
                a.write_data(id, 0, &data)?;
            }
            a.set_attribute(id, "geode.note".into(), A::String(format!("note {}", i)))?;
            a.set_attribute(id, "geode.rank".into(), A::Int(i as i64 - 20))?;
            if i % 3 == 0 {
                a.set_attribute(id, "geode.weight".into(), A::Float(i as f64 * 0.5))?;
            }
            if i % 5 == 0 {
                a.set_attribute(id, "geode.thumb".into(), A::Blob((0..6000usize).map(|k| (k + i as usize) as u8).collect()))?;
            }
            if i % 7 == 0 {
                a.set_attribute(id, "geode.vec".into(), A::Vector((0..16).map(|k| k as f32 * 0.25 + i as f32).collect()))?;
            }
            ids.push(id);
        }
        for (k, id) in ids.iter().enumerate() {
            let t = 1_600_000_000 + k as u64 * 10;
            a.set_times(*id, t, t + 1, t + 2)?;
        }
        Ok(())
    })();
    if let Err(e) = made {
        serial_println!(":: GEODEBACKUP: files=0 attrs=- times=- restore=- refused_taken=0 -> FAIL reason=populate({:?}) ::", e);
        return;
    }
    let mut want: Vec<GbRecord> = Vec::new();
    let _ = a.resolve_path("/home/a").and_then(|id| gb_records(&mut a, id, "", &mut want));
    // The reference: the host crate's write_archive over the whole records.
    let reference = {
        let mut w = Writer::new();
        w.raw(b"GEODEAR1");
        w.u32(1);
        w.u64(want.len() as u64);
        for (p, kind, c, m, t, attrs, data) in &want {
            w.str(p);
            kind.encode(&mut w);
            w.u64(*c);
            w.u64(*m);
            w.u64(*t);
            w.map(attrs);
            w.bytes(data);
        }
        let mut out = w.into_bytes();
        let sum = ::unafs::hash::hash_bytes(&out);
        out.extend_from_slice(b"GEODEEND");
        out.extend_from_slice(&sum.to_le_bytes());
        out
    };
    let measured = geodew::plan(&mut a, "/home/a").map(|p| p.archive_bytes).unwrap_or(0);
    let mut stick: Vec<u8> = Vec::new();
    let backed = geodew::backup_to(&mut a, "/home/a", &mut stick);
    let fmt = backed.is_ok() && stick == reference;
    let measured_ok = measured == stick.len() as u64;
    // Restore into home b of the second volume.
    let target = (|| -> Result<u64, FileSystemError> {
        let root = b.superblock.root_inode;
        let h = b.mkdir(root, "home".into())?;
        b.mkdir(h, "b".into())
    })();
    let r1 = if target.is_ok() { geodew::restore(&mut b, &mut Bytes(&stick), "/home/b", false, &mut |_: u32, _: u32| {}) } else { Err("target") };
    let mut got: Vec<GbRecord> = Vec::new();
    let _ = b.resolve_path("/home/b").and_then(|id| gb_records(&mut b, id, "", &mut got));
    let same_len = got.len() == want.len() && !want.is_empty();
    let restore = r1.is_ok() && same_len && got.iter().zip(want.iter()).all(|(g, w)| g.0 == w.0 && g.1 == w.1 && g.6 == w.6);
    let attrs = same_len && got.iter().zip(want.iter()).all(|(g, w)| g.5 == w.5);
    let times = same_len && got.iter().zip(want.iter()).all(|(g, w)| (g.2, g.3, g.4) == (w.2, w.3, w.4));
    let files = got.iter().filter(|g| g.1 == FileKind::File).count();
    // A second restore into b: every top-level name is taken; nothing written.
    let r2 = geodew::restore(&mut b, &mut Bytes(&stick), "/home/b", false, &mut |_: u32, _: u32| {});
    let mut after: Vec<GbRecord> = Vec::new();
    let _ = b.resolve_path("/home/b").and_then(|id| gb_records(&mut b, id, "", &mut after));
    let refused_taken = (r2 == Err("taken") && after.len() == got.len()) as u32;
    // A damaged archive (one data byte flipped) is refused before anything is written (its folder is not made).
    let mut bad = stick.clone();
    let mid = bad.len() / 2;
    if let Some(x) = bad.get_mut(mid) {
        *x ^= 0x40;
    }
    let r3 = geodew::restore(&mut b, &mut Bytes(&bad), "/home/c", true, &mut |_: u32, _: u32| {});
    let corrupt = r3.is_err() && b.resolve_path("/home/c").is_err();
    // A restore into a new folder.
    let r4 = geodew::restore(&mut b, &mut Bytes(&stick), "/home/d", true, &mut |_: u32, _: u32| {});
    let make = r4.is_ok_and(|r| r.files == 40);
    serial_println!(
        "[geodebackup] archive_bytes={} measured={} fmt={} restored={:?} corrupt={} make={} fsck={}",
        stick.len(), if measured_ok { "ok" } else { "FAIL" }, if fmt { "ok" } else { "FAIL" }, r1.map(|r| (r.files, r.dirs, r.attrs, r.bytes)),
        match r3 { Err(w) => w, Ok(_) => "ACCEPTED" }, if make { "ok" } else { "FAIL" },
        if b.fsck(false).is_ok_and(|r| r.is_clean()) { "clean" } else { "FAIL" }
    );
    let pass = files == 40 && attrs && times && restore && refused_taken == 1 && fmt && measured_ok && corrupt && make;
    serial_println!(
        ":: GEODEBACKUP: files={} attrs={} times={} restore={} refused_taken={} -> {} ::",
        files, if attrs { "ok" } else { "FAIL" }, if times { "ok" } else { "FAIL" }, if restore { "ok" } else { "FAIL" }, refused_taken,
        if pass { "PASS" } else { "FAIL" }
    );
}

// MICRING3 (rmbp-ledger B736, BP-audio-12) — TAIL-APPENDED. `tests micring3`: the microphone from ring 3 — the fixture
// program `APPS/MICRING3.ELF` (`unaos/crates/user-micring3`) speaks the three bus verbs (una-abi's MICRING3 block) from
// three runs: (1) `micring3` OPENs — the grant is asked once, the ask answered Allow (its default) — READs 100 ms and
// CLOSEs; (2) `micring3 hold` OPENs (the kept grant: no ask) and READs until Settings > Privacy's Revoke, driven in the
// tab's order (`fs::apptrust::mic_revoked`, then `prefs::revoke`), closes its source and the pipe goes idle on the
// driver's service pass; (3) `micring3 exit` OPENs and exits without CLOSE — the terminate edge closes it.
// `:: MICRING3: asked=1 opened=1 read_bytes=<n> revoked=closed terminate=closed -> PASS|SKIP|FAIL ::` (SKIP with the
// reason when the image stages no fixture, or no microphone has a capture alternate — QEMU's `usb-audio`).
fn micring3_selftest() {
    #[cfg(all(feature = "uac", target_arch = "x86_64", feature = "wc"))]
    {
        use crate::arch::syscall::BgPoll;
        use crate::video::dialog::{self, Answer};
        const PATH: &str = "/apps/MICRING3.ELF";
        const APP: &str = "micring3";
        const ASKED: usize = 0;
        const OPENED: usize = 1;
        const READ: usize = 2;
        const REVOKED: usize = 3;
        const TERM: usize = 4;
        const CLOSED: usize = 5;
        const NODEV: usize = 6;
        let skip = |why: &str| serial_println!(":: MICRING3: asked=- opened=- read_bytes=- revoked=- terminate=- -> SKIP :: reason={} ::", why);
        if dialog::is_up() {
            return skip("dialog-up");
        }
        let mt = crate::shell::vfs_mount_table();
        let full = crate::shell::vfs_path(PATH);
        let Ok(st) = mt.stat(&full) else { return skip("no-fixture (the image stages no APPS/MICRING3.ELF)") };
        let bytes = match mt.read(&full, 0, st.size as usize) {
            Ok(b) if b.len() as u64 == st.size => b,
            _ => return skip("fixture-read"),
        };
        let key = crate::prefs::decision_key(crate::fs::apptrust::MIC_DOMAIN, APP);
        let forget = || {
            if crate::prefs::decisions().iter().any(|d| d.key == key) {
                let _ = crate::prefs::revoke(&key);
            }
        };
        let wait = |ms: u64, f: &dyn Fn() -> bool| -> bool {
            let dl = crate::arch::ticks() + ms;
            while crate::arch::ticks() < dl {
                if f() {
                    return true;
                }
                crate::arch::sched::yield_now();
            }
            f()
        };
        // A run to its end: `Some(status)` when it exited by itself in 8 s (else it is killed); always reaped.
        let finish = |pid: u64, slot: u64| -> Option<i32> {
            wait(8_000, &|| !matches!(crate::arch::syscall::bg_poll(pid, false), BgPoll::Running));
            let st = match crate::arch::syscall::bg_poll(pid, false) {
                BgPoll::Exited(s) => Some(s),
                BgPoll::Running => {
                    let _ = crate::arch::syscall::bg_kill(pid, slot);
                    None
                }
                _ => None,
            };
            let _ = crate::arch::syscall::bg_poll(pid, true);
            st
        };
        let idle = || wait(1_500, &|| crate::drivers::usb::uac::capture_armed() == 0);
        forget();
        let was = dialog::headless(true);
        let c0 = crate::audio::ring3_counts();
        let d = |i: usize| crate::audio::ring3_counts()[i] - c0[i];

        // (1) read: asked once, Allow, 100 ms, CLOSE.
        let r1 = match crate::arch::syscall::spawn_user_image_bg_argv(&bytes, &[APP]) {
            Ok((pid, slot, _)) => {
                if wait(3_000, &|| d(ASKED) > 0 || d(NODEV) > 0 || !matches!(crate::arch::syscall::bg_poll(pid, false), BgPoll::Running)) && d(ASKED) > 0 {
                    dialog::open_now();
                    dialog::answer(Answer::Default); // Allow, the ask's default: kept in Principia
                }
                finish(pid, slot)
            }
            Err(why) => {
                serial_println!("[micring3] run=read refused={}", why);
                None
            }
        };
        let (asked, opened, read_bytes, closed) = (d(ASKED), d(OPENED), d(READ), d(CLOSED));
        let idle1 = idle();
        serial_println!("[micring3] run=read status={:?} asked={} opened={} read_bytes={} closed={} idle={}", r1, asked, opened, read_bytes, closed, idle1 as u8);
        if d(NODEV) > 0 && opened == 0 {
            forget();
            dialog::headless(was);
            serial_println!(":: MICRING3: asked={} opened=0 read_bytes=0 revoked=- terminate=- -> SKIP :: reason=no-capture-device (QEMU's usb-audio has no capture alternate) ::", asked);
            return;
        }

        // (2) hold: the kept grant opens unasked; the Privacy tab's Revoke closes it within one service pass.
        let o2 = d(OPENED);
        let mut revoked_ok = false;
        let mut rv_ms = u64::MAX;
        match crate::arch::syscall::spawn_user_image_bg_argv(&bytes, &[APP, "hold"]) {
            Ok((pid, slot, _)) => {
                if wait(3_000, &|| crate::audio::ring3_open(slot as usize) != 0) {
                    let t0 = crate::arch::ms();
                    crate::fs::apptrust::mic_revoked(&key); // the tab's order: the hook BEFORE the store write
                    let dropped = crate::prefs::revoke(&key);
                    let shut = crate::audio::ring3_open(slot as usize) == 0 && d(REVOKED) == 1;
                    let idle2 = idle();
                    rv_ms = crate::arch::ms().wrapping_sub(t0);
                    let st = finish(pid, slot);
                    let saw = st.is_some_and(|s| s & (1 << 22) != 0);
                    revoked_ok = dropped && shut && idle2 && saw;
                    serial_println!("[micring3] run=hold status={:?} store_revoked={} source_closed={} idle={} program_saw_eperm={} ms={}", st, dropped as u8, shut as u8, idle2 as u8, saw as u8, rv_ms);
                } else {
                    let st = finish(pid, slot);
                    serial_println!("[micring3] run=hold status={:?} opened={} (no source in 3 s)", st, d(OPENED) - o2);
                }
            }
            Err(why) => serial_println!("[micring3] run=hold refused={}", why),
        }

        // (3) exit: granted again (a second Allow), opened, ended without CLOSE — the terminate edge closes it.
        crate::prefs::decide(crate::fs::apptrust::MIC_DOMAIN, APP, "-", "allow");
        let mut term_ok = false;
        match crate::arch::syscall::spawn_user_image_bg_argv(&bytes, &[APP, "exit"]) {
            Ok((pid, slot, _)) => {
                let st = finish(pid, slot);
                let edge = wait(2_000, &|| d(TERM) >= 1);
                let idle3 = idle();
                term_ok = edge && d(TERM) == 1 && crate::audio::ring3_open(slot as usize) == 0 && idle3;
                serial_println!("[micring3] run=exit status={:?} term_closed={} idle={}", st, d(TERM), idle3 as u8);
            }
            Err(why) => serial_println!("[micring3] run=exit refused={}", why),
        }
        forget();
        dialog::headless(was);
        let pass = asked == 1 && opened == 1 && read_bytes > 0 && closed == 1 && idle1 && revoked_ok && term_ok && d(ASKED) == 1;
        serial_println!(
            ":: MICRING3: asked={} opened={} read_bytes={} revoked={} terminate={} -> {} :: revoke_ms={} opens={} ::",
            asked, opened, read_bytes, if revoked_ok { "closed" } else { "open" }, if term_ok { "closed" } else { "open" },
            if pass { "PASS" } else { "FAIL" }, rv_ms, d(OPENED)
        );
    }
    #[cfg(not(all(feature = "uac", target_arch = "x86_64", feature = "wc")))]
    serial_println!(":: MICRING3: asked=- opened=- read_bytes=- revoked=- terminate=- -> SKIP :: reason=no-uac-x86-desktop (the `uac` knob on an x86 `wc` image) ::");
}

// ══════════════ KEYCHARSARM (LEDGER SR153) — `tests keychars` on aarch64, the wake and repeat legs — TAIL-APPENDED ══════════════

/// KEYCHARSARM: the aarch64 ring router as `keychars_selftest`'s `sc`. `user_input_route` is the real seam
/// (`user_input_enqueue`: the Esc / Quarry / Tab doors, then `pack_input`'s `Char` arm into the focused ring). aarch64
/// has no ring read-back (`user_input_take` is x86's), so `user_input_take` here writes nothing and returns how far the
/// fixture slot's ring GREW since the fixture focused it (`user_input_depth`, the CLICK-SWALLOW witness read); the
/// fixture counts the window only when that equals the events the router queued (`window_by=depth`).
#[cfg(all(feature = "witness", target_arch = "aarch64", feature = "desktop_firmware"))]
mod keychars_arm {
    use crate::arch::aarch64::syscall as a;
    use core::sync::atomic::{AtomicU32, Ordering};
    static DEPTH0: AtomicU32 = AtomicU32::new(0);
    pub fn user_input_active() -> u64 { a::user_input_active() }
    pub fn user_input_set_active(asid: u64) {
        a::user_input_set_active(asid);
        if asid as usize == crate::procslot::SLOT_ID_MAX { DEPTH0.store(a::user_input_depth(asid), Ordering::Relaxed); }
    }
    pub fn user_input_route(ev: crate::pal::Event) -> crate::pal::Event {
        if a::user_input_enqueue(ev) { crate::pal::Event::Unknown } else { ev }
    }
    pub fn user_input_take(asid: u64, out: &mut [u64]) -> usize {
        (a::user_input_depth(asid).wrapping_sub(DEPTH0.load(Ordering::Relaxed)) as usize).min(out.len())
    }
}

/// KEYCHARSARM `wake=`: a layout character is ACTIVITY to the idle blank (`dimidle::gate`'s press arm). Waits out the
/// millisecond the last stamp may share (bounded 50 ms), offers `Char('ü')`, and asks that the idle clock moved, that the
/// gate dropped it only if the panel was blank (the waking key is swallowed, BP-bar-20), and that the panel is not blank.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn keychars_wake() -> bool {
    use crate::video::dimidle;
    let before = dimidle::last_activity_ms();
    let t0 = crate::arch::ms();
    while crate::arch::ms() <= t0 && crate::arch::ms().wrapping_sub(t0) < 50 { core::hint::spin_loop(); }
    let blank0 = dimidle::blanked();
    let dropped = dimidle::gate(crate::pal::Event::Char('ü'));
    let after = dimidle::last_activity_ms();
    let ok = after > before && after > t0 && dropped == blank0 && !dimidle::blanked();
    serial_println!("[keychars] wake before={} after={} blank0={} dropped={} -> {}", before, after, blank0 as u8, dropped as u8, if ok { "ok" } else { "bad" });
    ok
}

/// KEYCHARSARM `repeat_byte=` (asked while `de` is live): the key a HELD usage hands the typematic tracker is the
/// layout's — usage 0x1C (US `y`) claims `z`, and `ü` (0x2F) claims 0, so neither EHCI's feed nor the aarch64 xHCI feed
/// forges the US byte (`y`, `[`). CHARREPEAT (LEDGER SR156): the feeds' token (`hid_repeat_key`) is `z` for 0x1C and the
/// `ü` CHARACTER token for 0x2F, and where the tracker is compiled (`keychars_repeat_tracker`) the tracker repeats them.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn keychars_repeat_byte() -> bool {
    use crate::drivers::xhci as hx;
    let bytes = hx::hid_layout_claim(0x1C, 0, false) == Some(b'z') && hx::hid_layout_claim(0x2F, 0, false) == Some(0);
    let (tz, tu) = (hx::hid_repeat_key(0x1C, 0, false, b'y'), hx::hid_repeat_key(0x2F, 0, false, b'['));
    let tokens = tz == b'z' as u32 && tu == crate::pal::typematic_char_key('\u{fc}');
    let tracker = keychars_repeat_tracker(tz, tu);
    let ok = bytes && tokens && tracker.unwrap_or(true);
    KEYCHARS_REPEAT.store(match tracker { Some(_) if ok => 1, None if ok => 2, _ => 0 }, core::sync::atomic::Ordering::Relaxed);
    serial_println!("[keychars] repeat bytes={} tokens={} tracker={} -> {}", bytes as u8, tokens as u8, match tracker { Some(true) => "ok", Some(false) => "bad", None => "absent" }, keychars_repeat_word());
    ok
}

/// CHARREPEAT: the `repeat=` word — `ok` (tokens right AND the tracker repeated them), `bad`, or `no-tracker` (tokens
/// right on an image with no host typematic at all: no feed compiles there for ANY key, an aarch64 image with neither `baremetal` nor `tegra`; ORINREPEAT SR164 gave the Orin its feed).
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
static KEYCHARS_REPEAT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn keychars_repeat_word() -> &'static str {
    match KEYCHARS_REPEAT.load(core::sync::atomic::Ordering::Relaxed) { 1 => "ok", 2 => "no-tracker", _ => "bad" }
}

/// CHARREPEAT: drive the live tracker through its own seams — a held `ü` repeats `Event::Char('ü')`, a held de `z`
/// repeats `Event::Key(b'z')`, a RELEASED `ü` (its token absent from the next report) repeats nothing in 64 due ticks.
/// Each hold is retried up to 8 times: on x86 the service core's `x86_typematic_pump` may take a forced-due repeat
/// between the fixture's force and its tick. The tracker is reset before and after (`typematic_test_reset`).
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc", feature = "ehcihid"), all(target_arch = "aarch64", feature = "desktop_firmware", any(feature = "baremetal", feature = "tegra")))))]
fn keychars_repeat_tracker(tz: u32, tu: u32) -> Option<bool> {
    use crate::pal::{self, Event};
    let held_repeats = |tok: u32, want: Event| -> bool {
        (0..8).any(|_| {
            pal::typematic_test_reset();
            pal::typematic_note_keys(tok, &[tok]);
            pal::typematic_test_force_due();
            pal::typematic_tick() == Some(want)
        })
    };
    let char_repeats = held_repeats(tu, Event::Char('\u{fc}'));
    let z_repeats = held_repeats(tz, Event::Key(b'z'));
    pal::typematic_test_reset();
    pal::typematic_note_keys(tu, &[tu]);
    pal::typematic_note_keys(0, &[]); // the `ü` released
    let released = (0..64).all(|_| {
        pal::typematic_test_force_due();
        pal::typematic_tick().is_none()
    });
    pal::typematic_test_reset();
    serial_println!("[keychars] repeat char={} z={} released={}", char_repeats as u8, z_repeats as u8, released as u8);
    Some(char_repeats && z_repeats && released)
}

#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")), not(any(all(target_arch = "x86_64", feature = "ehcihid"), all(target_arch = "aarch64", any(feature = "baremetal", feature = "tegra"))))))]
fn keychars_repeat_tracker(_tz: u32, _tu: u32) -> Option<bool> {
    None
}

// A2DP (rmbp-ledger B739) — TAIL-APPENDED. `tests a2dp`: the SBC known-answer test (`sbc_core::kat`, the function the
// host `cargo test -p sbc_core` runs), the AVDTP state machine over a synthetic peer in both roles (with the SDP sink
// record and the L2CAP fit below it), and whether a stream runs now. Registered on the `ensure_shellux` line.
// Wire: `[a2dp] fixture <leg> -> PASS|FAIL`, `:: A2DP: sbc_kat=<ok|FAIL> avdtp=<ok|FAIL> frag=<ok|FAIL> stream=<ok|no-device> sdp_server=<ok|FAIL> inbound=<ok|FAIL> avrcp=<ok|FAIL> avrcp_ct=<ok|FAIL> avrcp_mute=<ok|FAIL> -> PASS|SKIP|FAIL ::` (`sdp_server` and `inbound` SDPSERVER B769; `avrcp` AVRCP B774, `drivers/bt/avctp.rs` `legs`; `avrcp_ct` AVRCPCT B780, `legs_ct`; `avrcp_mute` AVRCPMUTE B790, `legs_mute`).
#[cfg(all(target_arch = "x86_64", feature = "a2dp"))]
fn a2dp_selftest() {
    crate::drivers::ehci::bthid::avdtp::fixture();
}

/// ONEINSTANCE (rmbp-ledger B734; BP-ring3-18, BP-video-23, BP-bar-7/18) — `tests oneinstance`: the smallest windowed ELF
/// under `/apps` (APPLAUNCH's pick) launched the way the glass launches it (`origin::Glass`, `bg`'s spawn, the name armed
/// as the dock arms it), its first frame awaited, then:
///  1. the SAME program again by the user's verb, with an argument: no second slot (`Err(ONEINSTANCE_RAISED)`), the
///     running instance raised with the focus given, the argument enqueued in ITS mailbox as one OPEN_ARG frame (rc 0);
///  2. again with `new=1` (`spawn_user_image_bg_argv_new`, the dock's Shift-click / `New Window`): a second slot.
///  3. ONEINSTANCE2 (rmbp-ledger B751): Quarry's program opener on the running program (`openers::program_running`):
///     raised, the file delivered as OPEN_ARG to the same owner (rc 0), nothing launched (`quarry=`); and the mailbox's
///     OPEN_ARG hold across a call in flight over a local mailbox (`syscall::oneinstance_pending_selftest`, `pending=`).
///  4. OPENARGQUEUE (rmbp-ledger B761): four OPEN_ARGs met during one blocking call all drain, in order, after its reply
///     (`syscall::oneinstance_hold_selftest`, local mailbox, `hold=<in order>/4`).
///     OPEN_ARG hold across a call in flight over a local mailbox (`syscall::oneinstance_pending_selftest`, `pending=`); QUARRYLAUNCH (B754): Quarry's double-click body on the running program is raised too (`quarry_launch=`, file tail).
/// Every slot it made is killed and reaped. SKIP with no panel, no windowed program, or that program already running.
/// `:: ONEINSTANCE: slots=1 raised=1 arg=ok new_flag=2 quarry=ok pending=ok quarry_launch=ok hold=4/4 -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn oneinstance_selftest() {
    use crate::arch::syscall as sc;
    use crate::origin::{with, Origin};
    use crate::video::wm;
    const WAIT_MS: u64 = 5_000;
    const ARG: &str = "/home/oneinstance-arg.txt";
    let skip = |why: &str| serial_println!(":: ONEINSTANCE: slots=- raised=- arg=- new_flag=- quarry=- pending=- quarry_launch=- hold=- -> SKIP reason={} ::", why);
    if !crate::video::WRITER.lock().is_ready() {
        return skip("no-panel");
    }
    let mt = crate::shell::vfs_mount_table();
    let dir = crate::shell::vfs_path("/apps");
    let mut cands: alloc::vec::Vec<(u64, alloc::string::String)> = match mt.read_dir(&dir) {
        Ok(v) => v
            .into_iter()
            .filter(|e| matches!(e.kind, crate::fs::vfs::NodeKind::File) && e.size > 0 && e.size as usize <= sc::user_image_cap())
            .filter(|e| e.name.len() > 4 && e.name[e.name.len() - 4..].eq_ignore_ascii_case(".ELF"))
            .map(|e| (e.size, e.name))
            .collect(),
        Err(_) => return skip("no-apps-dir"),
    };
    cands.sort();
    let mut pick: Option<(alloc::string::String, alloc::vec::Vec<u8>)> = None;
    for (size, leaf) in cands.iter() {
        let path = format!("{}/{}", dir.trim_end_matches('/'), leaf);
        if let Ok(b) = mt.read(&path, 0, *size as usize) {
            if b.len() as u64 == *size && crate::arch::elf::app_flags(&b) & una_abi::APP_FLAG_WINDOWED != 0 {
                pick = Some((path, b));
                break;
            }
        }
    }
    let Some((path, bytes)) = pick else { return skip("no-windowed-elf") };
    let launch = |argv: &[&str], new: bool| {
        with(Origin::Glass, || if new { sc::spawn_user_image_bg_argv_new(&bytes, argv) } else { sc::spawn_user_image_bg_argv(&bytes, argv) })
    };
    let framed = |slot: u64| -> bool {
        let t0 = crate::arch::ms();
        while crate::arch::ms().saturating_sub(t0) < WAIT_MS {
            if crate::procslot::launch::frame_ms(slot as usize).is_some() {
                return true;
            }
            crate::arch::sched::sleep_ms(20);
        }
        false
    };
    let mut spawned: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    // The first launch: one slot and its first frame (its window).
    let (pid1, slot1) = match launch(&[path.as_str()], false) {
        Ok((p, s, _)) => (p, s),
        Err(e) if e == sc::ONEINSTANCE_RAISED => return skip("already-running"),
        Err(_) => return skip("first-launch-refused"),
    };
    spawned.push((pid1, slot1));
    let owner1 = wm::owner_of_launch(slot1);
    let _ = wm::app_name_arm_launch(owner1, &path);
    let up = framed(slot1);
    // 1 — the same program again, with an argument.
    let second = launch(&[path.as_str(), ARG], false);
    let raised = matches!(second, Err(e) if e == sc::ONEINSTANCE_RAISED);
    if let Ok((p, s, _)) = second {
        spawned.push((p, s));
    }
    let slots = if raised { 1 } else { 2 };
    let (owner, given, rc) = sc::oneinstance_last();
    let focus_ok = raised && owner == owner1 && given && wm::focus_asid() == owner1;
    let arg_ok = raised && owner == owner1 && rc == 0;
    // 3 — ONEINSTANCE2: Quarry's opener on the running program raises it and hands it the file (no launch).
    #[cfg(feature = "quarry")]
    let quarry = {
        let r = crate::video::quarry::live::openers::program_running(&path, ARG, "text/plain");
        let (qo, _, qrc) = sc::oneinstance_last();
        if r.is_some() && qo == owner1 && qrc == 0 { "ok" } else { "FAIL" }
    };
    #[cfg(not(feature = "quarry"))]
    let quarry = "skip";
    let quarry_launch = oneinstance_quarry_launch_leg(&path, owner1); let pending = if sc::oneinstance_pending_selftest() { "ok" } else { "FAIL" }; // QUARRYLAUNCH (B754): Quarry's double-click on the running program (file tail); LINE-NEUTRAL fold
    // 2 — new=1: another copy.
    let new_flag = match launch(&[path.as_str()], true) {
        Ok((p, s, _)) => {
            spawned.push((p, s));
            if s != slot1 { 2 } else { 1 }
        }
        Err(_) => 1,
    };
    for &(p, s) in spawned.iter() {
        let _ = sc::bg_kill(p, s);
        let _ = sc::bg_poll(p, true);
        wm::app_name_forget(wm::owner_of_launch(s));
    }
    let (held_in_order, held_sent) = sc::oneinstance_hold_selftest(); // 4 — OPENARGQUEUE (B761): four held, four out in order
    let ok = up && raised && slots == 1 && focus_ok && arg_ok && new_flag == 2 && quarry != "FAIL" && pending == "ok" && quarry_launch != "FAIL" && held_in_order == held_sent;
    let arg = if arg_ok { alloc::string::String::from("ok") } else { format!("rc{}", rc) };
    serial_println!(
        ":: ONEINSTANCE: slots={} raised={} arg={} new_flag={} quarry={} pending={} quarry_launch={} hold={}/{} -> {} ::",
        slots, raised as u8, arg, new_flag, quarry, pending, quarry_launch, held_in_order, held_sent, if ok { "PASS" } else { "FAIL" }
    );
    if !ok {
        serial_println!(
            "[app] oneinstance app={} framed={} owner={} raised_owner={} given={} focus={}",
            wm::program_name(&path), up as u8, owner1, owner, given as u8, wm::focus_asid()
        );
    }
}

// LOCKPREFS (rmbp-ledger B748, BP-security-18) — TAIL-APPENDED, so no line above moves. `tests autolock`'s `prefs=` leg,
// folded onto its lines: the lock's policy is Principia's, read over the bus on a PrefChanged (`lock::prefs_pass`). The
// fixture writes `system.lock.after_idle_min` through the bus client (`prefs_client::pref_set`, the path Settings'
// `lock::persist` takes; inline here, the fixture is not the render task) to a value no idle stretch reaches (1440 or
// 1439 minutes: the pass may not idle-lock the fixture), runs ONE `lock::prefs_pass`, and asks that `lock::policy()`
// moved to it (the render task's own pass may have taken the edge first: the value is still asked of `policy()`); the
// operator's value is put back the same way and the lock follows it in one more pass. `prefs=client` = seen and
// restored; `prefs=skip` (the home not writable: the write would be refused, the SETTINGSBUS shape) does not fail the
// line; anything else is `prefs=FAIL`.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn lockprefs_leg() -> (bool, &'static str) {
    use crate::prefs::key;
    use crate::prefs_client::{pref_set, sys_int, PrefValue};
    use crate::video::lock;
    if crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()).is_some() {
        serial_println!("[lock] prefs leg skipped: the home is not writable (a set would be refused)");
        return (true, "skip");
    }
    let _ = lock::prefs_pass(); // the policy is the store's before the write
    let orig = sys_int(key::LOCK_AFTER_IDLE_MIN, 0, 1440).unwrap_or(lock::IDLE_MIN_DEFAULT as i64);
    let want: i64 = if orig == 1440 { 1439 } else { 1440 };
    let before = lock::policy().0;
    let set_ok = pref_set(crate::prefs::NS, key::LOCK_AFTER_IDLE_MIN, PrefValue::Int(want)).is_ok();
    let reread = lock::prefs_pass();
    let seen = lock::policy().0;
    let back_ok = pref_set(crate::prefs::NS, key::LOCK_AFTER_IDLE_MIN, PrefValue::Int(orig)).is_ok();
    let _ = lock::prefs_pass();
    let restored = lock::policy().0 as i64 == orig;
    let ok = set_ok && before as i64 != want && seen as i64 == want && back_ok && restored;
    serial_println!(
        "[lock] prefs leg want={} before={} seen={} reread={} restored={} via={} -> {}",
        want, before, seen, reread as u8, restored as u8, crate::prefs_client::last_via(), if ok { "ok" } else { "bad" }
    );
    (ok, if ok { "client" } else { "FAIL" })
}


// ══════════════ HELPF1 (rmbp-ledger B746, BP-video-35) — `tests helpf1` — TAIL-APPENDED ══════════════
/// F1 and the shell's `help <words>`. The decision on the three table shapes (`fnrow::help_key`: PC bare F1 = help,
/// CRISPY bare F1 = brightness, CRISPY Shift+F1 = help, F2 never); then a synthetic Shift+F1 (Help on every table)
/// through THE router (`fnrow::fn_row_usage` returns `Help`), the latch drained by the Help window's pass, and the Help
/// rows counted on the window table (`wm::front_of_owner(help::window::OWNER)`: 0 -> 1) showing the focused owner's
/// page (`page_for_app`); the window is closed after. `help zoom` is a search (`help ls` and `help` are not) and its
/// top 5 hold the page whose links name `accessibility.zoom` (the Launcher's ranking, `help::top_pages`).
/// `f1=skip(no-panel)` when the window cannot open headless. `:: HELPF1: f1=ok shell=ok -> PASS ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn helpf1_selftest() {
    use crate::drivers::xhci::HID_MOD_SHIFT;
    use crate::help::{index, window};
    use crate::video::{fnrow, theme, wm};
    use crate::video::keymap::{self, Action};
    // HELPVERBDOC (B756): the PC rows NAME F1 as Help now, so the fn-row arm yields to the table there.
    let decide_ok = !fnrow::help_key(theme::PC_BINDINGS, fnrow::USAGE_F1, 0)
        && keymap::resolve(theme::PC_BINDINGS, 0, fnrow::USAGE_F1) == Some(Action::Help)
        && !fnrow::help_key(theme::CRISPY_BINDINGS, fnrow::USAGE_F1, 0)
        && fnrow::help_key(theme::CRISPY_BINDINGS, fnrow::USAGE_F1, HID_MOD_SHIFT)
        && !fnrow::help_key(theme::PC_BINDINGS, 0x3B, 0)
        && !fnrow::help_key(theme::CRISPY_BINDINGS, 0x3B, HID_MOD_SHIFT);
    index::retry();
    let loaded = index::ensure_loaded();
    // F1: a clean table first (no Help row, no latch pending).
    window::service();
    window::close();
    let before = wm::front_of_owner(window::OWNER).is_some() as usize;
    let want = window::page_for_app(window::app_of(wm::focus_asid()));
    // HELPVERBDOC (B756): where the live table names F1 as Help (the Softy tables) the press is the decoder's Action
    // through the route, and the fn-row router declines it; elsewhere Shift+F1 is the fn-row arm.
    let row_f1 = keymap::resolve(keymap::active(), HID_MOD_SHIFT, fnrow::USAGE_F1) == Some(Action::Help);
    let routed = if row_f1 {
        fnrow::fn_row_usage(fnrow::PATH_XHCI, fnrow::USAGE_F1, HID_MOD_SHIFT).is_none()
            && matches!(crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Action(Action::Help)), crate::pal::Event::Unknown)
    } else {
        fnrow::fn_row_usage(fnrow::PATH_XHCI, fnrow::USAGE_F1, HID_MOD_SHIFT) == Some(fnrow::FnAct::Help)
    };
    window::service();
    let t0 = crate::arch::ms();
    while !window::is_open() && crate::arch::ms().wrapping_sub(t0) < 500 {
        core::hint::spin_loop(); // the desktop pass may have drained the latch first
    }
    let front = wm::front_of_owner(window::OWNER);
    let after = front.is_some() as usize;
    let shown = window::shown();
    let f1 = if !routed || !decide_ok {
        "FAIL"
    } else if after == before + 1 && window::is_open() && shown == want {
        "ok"
    } else if !loaded || crate::video::panel_info_nonblocking().is_none() {
        "skip(no-panel)"
    } else {
        "FAIL"
    };
    window::close();
    // The shell: `help zoom` searches; the page whose links name accessibility.zoom is in its top 5.
    let q_ok = crate::help::shell_query("help zoom").as_deref() == Some("zoom")
        && crate::help::shell_query("help ls").is_none()
        && crate::help::shell_query("help").is_none()
        && crate::help::shell_query("help disk full").as_deref() == Some("disk full");
    let zoom_page = index::with(|ix| ix.pages.iter().find(|p| p.keys.iter().any(|k| k == "accessibility.zoom")).map(|p| p.slug.clone())).flatten().unwrap_or_default();
    let top = crate::help::top_pages("zoom", crate::help::SHELL_PAGES).unwrap_or_default();
    let shell_ok = q_ok && !zoom_page.is_empty() && top.len() <= crate::help::SHELL_PAGES && top.iter().any(|(s, _)| *s == zoom_page);
    serial_println!(
        "[help] fixture f1 decide={} routed={} rows={}->{} win={} page={} want={} shell q={} top={} zoom_page={} hits={}",
        decide_ok as u8, routed as u8, before, after, front.map(|w| w as i64).unwrap_or(-1), if shown.is_empty() { "-" } else { shown.as_str() }, want,
        q_ok as u8, top.first().map(|t| t.0.as_str()).unwrap_or("-"), if zoom_page.is_empty() { "-" } else { zoom_page.as_str() }, top.len()
    );
    // HELPVERBDOC (B756): `Action::Help` is a keymap row on all four tables and the route acts on it.
    let gui = crate::drivers::xhci::HID_MOD_GUI;
    let alt = crate::drivers::xhci::HID_MOD_ALT;
    let rows_ok = keymap::resolve(theme::CRISPY_BINDINGS, gui | HID_MOD_SHIFT, 0x38) == Some(Action::Help)
        && keymap::resolve(theme::CRISPY_BINDINGS, gui, 0x38) == Some(Action::ShowShortcuts)
        && keymap::resolve(keymap::softy2_table(true, false), alt | HID_MOD_SHIFT, 0x38) == Some(Action::Help)
        && keymap::resolve(keymap::softy2_table(false, true), 0, fnrow::USAGE_F1) == Some(Action::Help)
        && keymap::resolve(theme::PC_BINDINGS, 0, fnrow::USAGE_F1) == Some(Action::Help)
        && keymap::resolve(theme::CRISPY_BINDINGS, 0, fnrow::USAGE_F1) == Some(Action::BrightnessDown)
        && Action::Help.name() == "help"
        && crate::smallfix3::listed(Action::Help)
        && crate::smallfix3::ACTIONS.contains(&Action::Help);
    window::service();
    window::close();
    let c_before = wm::front_of_owner(window::OWNER).is_some() as usize;
    let c_want = window::page_for_app(window::app_of(wm::focus_asid()));
    let c_routed = matches!(crate::arch::x86_64::syscall::wc_route_event(crate::pal::Event::Action(Action::Help)), crate::pal::Event::Unknown);
    window::service();
    let t1 = crate::arch::ms();
    while !window::is_open() && crate::arch::ms().wrapping_sub(t1) < 500 {
        core::hint::spin_loop();
    }
    let c_after = wm::front_of_owner(window::OWNER).is_some() as usize;
    let c_shown = window::shown();
    let chord = if !rows_ok || !c_routed {
        "FAIL"
    } else if c_after == c_before + 1 && window::is_open() && c_shown == c_want {
        "ok"
    } else if !loaded || crate::video::panel_info_nonblocking().is_none() {
        "skip(no-panel)"
    } else {
        "FAIL"
    };
    window::close();
    serial_println!(
        "[help] fixture chord rows={} routed={} rows={}->{} page={} want={} table={}",
        rows_ok as u8, c_routed as u8, c_before, c_after, if c_shown.is_empty() { "-" } else { c_shown.as_str() }, c_want, keymap::active().name
    );
    let ok = f1 != "FAIL" && shell_ok && chord != "FAIL";
    serial_println!(":: HELPF1: f1={} shell={} chord={} -> {} ::", f1, if shell_ok { "ok" } else { "FAIL" }, chord, if ok { "PASS" } else { "FAIL" });
}

/// TRASHTOAST (rmbp-ledger B747; crawl A 26, BP-video-29): `:: TRASHTOAST: items=3 toasts=1 undone=3 -> PASS ::` — the
/// Trash verb over three items (`undo::batch_begin` / `trash` x3 / `batch_end`, the shape `ops::do_trash` runs over the
/// selection) posts ONE `Undo` toast and pushes ONE journal entry; that toast's own Undo (`undo::from_toast(seq)`, notify
/// `ACT_UNDO`) brings all three back. M2: a batch of one posts one toast (the classic entry) and its undo returns it. The
/// session's journal is set aside for the run and put back; the scratch folder is removed (`leftovers=`).
#[cfg(all(feature = "quarry", feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn trashtoast_selftest() {
    use alloc::{string::String, vec::Vec};
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use crate::video::quarry::live::ops;
    use crate::video::quarry::undo;
    let t = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    if t.stat(&home).is_err() {
        let _ = t.create(&home, NodeKind::Dir, P);
    }
    let base = format!("{}/TrashToastTest", home);
    let _ = ops::op_delete(&base);
    let saved = undo::fixture_take();
    let files: Vec<String> = ["ONE.TXT", "TWO.TXT", "THREE.TXT"].iter().map(|n| format!("{}/{}", base, n)).collect();
    let solo = format!("{}/SOLO.TXT", base);
    let mk = |p: &str| t.create(p, NodeKind::File, P).and_then(|_| t.write(p, 0, p.as_bytes(), P)).is_ok();
    let at = |p: &str| t.stat(p).is_ok();
    let made = ops::op_mkdir(&base).is_ok() && files.iter().all(|p| mk(p)) && mk(&solo);
    // the batch of three: one toast, one entry, all gone
    let t0 = undo::trash_toasts();
    undo::batch_begin();
    let items = files.iter().filter(|p| undo::trash(p).is_ok()).count();
    let seq = undo::batch_end();
    let toasts = undo::trash_toasts() - t0;
    let entries = undo::depth().0;
    let gone = files.iter().all(|p| !at(p));
    // the toast's Undo: the whole batch back
    let pressed = seq.map(|s| undo::from_toast(&s.to_le_bytes())).unwrap_or("no-seq");
    let undone = files.iter().filter(|p| at(p)).count();
    // M2: one item is the classic entry and toast
    let t1 = undo::trash_toasts();
    undo::batch_begin();
    let solo_ok = undo::trash(&solo).is_ok();
    let solo_seq = undo::batch_end();
    let solo_toasts = undo::trash_toasts() - t1;
    let solo_back = solo_ok && solo_seq.map(|s| undo::from_toast(&s.to_le_bytes()) == "quarry-undone").unwrap_or(false) && at(&solo);
    let single = solo_toasts == 1 && solo_back;
    // DNDTRASH (B753): a drag of three onto the dock's Trash tile — `dnd::deliver`, the release's own call — is one
    // entry and one toast, and one Ctrl+Z (`undo::undo`) brings all three back; a drag of one posts one toast (M2)
    let dnd_files: Vec<String> = ["DND1.TXT", "DND2.TXT", "DND3.TXT"].iter().map(|n| format!("{}/{}", base, n)).collect();
    let dnd_made = dnd_files.iter().all(|p| mk(p));
    let pl = |paths: Vec<String>| crate::video::dnd::Payload { kind: crate::video::dnd::Kind::File, paths, dir: false, from: crate::video::wm::WIN_NONE, label: String::from("dnd") };
    let (d0, t2) = (undo::depth().0, undo::trash_toasts());
    let (_, d_ok) = crate::video::dnd::deliver(&pl(dnd_files.clone()), &crate::video::dnd::Target::Trash, false);
    let d_toasts = undo::trash_toasts() - t2;
    let d_entries = undo::depth().0.saturating_sub(d0);
    let d_gone = dnd_files.iter().all(|p| !at(p));
    let d_undone = undo::undo().is_ok() && dnd_files.iter().all(|p| at(p));
    let t3 = undo::trash_toasts();
    let (_, s_ok) = crate::video::dnd::deliver(&pl(alloc::vec![dnd_files[0].clone()]), &crate::video::dnd::Target::Trash, false);
    let d_single = s_ok && undo::trash_toasts() - t3 == 1 && !at(&dnd_files[0]) && undo::undo().is_ok() && at(&dnd_files[0]);
    let dnd = dnd_made && d_ok && d_toasts == 1 && d_entries == 1 && d_gone && d_undone && d_single;
    // clean-up: anything still in the Trash from the scratch folder, then the folder
    for e in crate::fs::trash::entries().into_iter().filter(|e| e.orig.starts_with(&base)) {
        if crate::fs::trash::restore(&e.name).is_ok() {
            let _ = ops::op_delete(&e.orig);
        }
    }
    let _ = ops::op_delete(&base);
    let leftovers = at(&base) as usize + crate::fs::trash::entries().iter().filter(|e| e.orig.starts_with(&base)).count();
    let empty = trashtoast_empty_leg(&base); // TRASHBATCHDOCK (B762): the dock's Empty Trash, at the END (file tail)
    undo::fixture_put(saved);
    serial_println!("[trashtoast] made={} entries={} gone={} pressed={} single={} leftovers={} base={} dnd_toasts={} dnd_entries={} dnd_gone={} dnd_undone={} dnd_single={}", made, entries, gone, pressed, single, leftovers, base, d_toasts, d_entries, d_gone, d_undone, d_single);
    let pass = made && items == 3 && toasts == 1 && entries == 1 && gone && pressed == "quarry-undone" && undone == 3 && single && leftovers == 0 && dnd && empty;
    serial_println!(":: TRASHTOAST: items={} toasts={} undone={} empty={} -> {} :: dnd={}", items, toasts, undone, if empty { "ok" } else { "FAIL" }, if pass { "PASS" } else { "FAIL" }, if dnd { "ok" } else { "FAIL" });
}
// ══════════════ UNDOKEYS (rmbp-ledger B743) — `tests undokeys` — TAIL-APPENDED ══════════════

/// `tests undokeys`: Undo and Redo are KEYMAP rows (BP-input-2/3) and the zoom chord is consumed.
///
/// * `undo=` / `redo=`: a scratch file under the user's home is renamed through Quarry's journal (`undo::rename`, the
///   wrapper the menu and the keys call; the session's journal is set aside for the run), then `⌘Z` (the live table's
///   Command role + usage 0x1D) is resolved through `keymap::resolve_edge` — the decoders' own call — and the action it
///   answers is handed to `quarry::ops::action`, the arm `quarry::key_route` gives an `Event::Action`; the rename must be
///   undone (old name present, new absent). `⌘⇧Z` the same way must redo it.
/// * `zoom_consumed=`: a fixture slot takes the ring focus (as `tests keychars`). Control first: a bare `=` key-up
///   through the REAL x86 route (`wc_route_event`) lands in the slot's ring (1 event) — the path is live. Then `Win+=`
///   down through `resolve_edge`: it must answer `None` (no `Event::Action`), step the magnifier's asked factor
///   (`zoom::want`, the chord's atomics), and the key-up the decoder pushes when `=` lifts
///   (`hid_key_release_ascii`, which ignores GUI) must be consumed by the route with the ring untouched (0 events). The
///   factor, its persist flag and the ring focus are put back.
/// * `codes=`: `clipboard::action_code` for Undo / Redo, the ring-3 `INPUT_EV_ACTION` payloads.
///
/// `:: UNDOKEYS: undo=ok redo=ok zoom_consumed=ok codes=51/52 -> PASS ::`
#[cfg(all(feature = "witness", feature = "quarry", target_arch = "x86_64", feature = "wc"))]
fn undokeys_selftest() {
    use crate::arch::x86_64::syscall as sc;
    use crate::drivers::xhci::{hid_key_release_ascii, HID_MOD_GUI, HID_MOD_SHIFT};
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use crate::pal::Event;
    use crate::video::keymap::{self, Action};
    use crate::video::quarry::live::ops;
    use crate::video::quarry::undo;
    use crate::video::zoom;
    #[cfg(feature = "login")]
    if crate::fs::users::screen_up() {
        serial_println!(":: UNDOKEYS: -> SKIP :: reason=login-screen-up (the screen takes every key-up first)");
        return;
    }
    let table = keymap::active();
    let none = [0u8; 6];
    let press = |u: u8, mods: u8| {
        let mut cur = [0u8; 6];
        cur[0] = u;
        keymap::resolve_edge(table, &cur, &none, mods)
    };
    let yn = |v: bool| if v { "ok" } else { "bad" };
    let (c_undo, c_redo) = (crate::video::clipboard::action_code(Action::Undo), crate::video::clipboard::action_code(Action::Redo));
    let codes_ok = c_undo == 51 && c_redo == 52;
    // M1 — the journal through the table.
    let t = crate::shell::vfs_mount_table();
    let home = crate::fs::trash::home_base();
    if t.stat(&home).is_err() {
        let _ = t.create(&home, NodeKind::Dir, P);
    }
    let base = format!("{}/UndoKeysTest", home);
    let _ = ops::op_delete(&base);
    let saved = undo::fixture_take();
    let (old, new) = (format!("{}/OLD.TXT", base), format!("{}/NEW.TXT", base));
    let at = |p: &str| t.stat(p).is_ok();
    let made = ops::op_mkdir(&base).is_ok() && t.create(&old, NodeKind::File, P).is_ok();
    let renamed = made && undo::rename(&old, &new).is_ok() && at(&new) && !at(&old);
    let z = press(0x1D, table.cmd_role);
    let z_act = z.map(|(a, _)| a);
    let undo_ok = renamed && z_act == Some(Action::Undo) && ops::action(Action::Undo) && at(&old) && !at(&new);
    let sz = press(0x1D, table.cmd_role | HID_MOD_SHIFT);
    let sz_act = sz.map(|(a, _)| a);
    let redo_ok = undo_ok && sz_act == Some(Action::Redo) && ops::action(Action::Redo) && at(&new) && !at(&old);
    let _ = ops::op_delete(&base);
    let leftovers = at(&base) as u32;
    undo::fixture_put(saved);
    // M2 — the zoom chord reaches the magnifier and never the focused window.
    let want0 = zoom::want();
    let persist0 = zoom::take_persist().is_some();
    let slot = crate::procslot::SLOT_ID_MAX as u64;
    let focus0 = sc::user_input_active();
    sc::user_input_set_active(slot);
    let mut ring = [0u64; 16];
    let _ = sc::user_input_take(slot, &mut ring);
    let control_routed = matches!(sc::wc_route_event(Event::KeyUp(b'=')), Event::Unknown);
    let control_n = sc::user_input_take(slot, &mut ring);
    let chord = press(0x2E, HID_MOD_GUI);
    let zoom_step = zoom::want().0 == zoom::stepped(want0.0, 1);
    let up = hid_key_release_ascii(0x2E, HID_MOD_GUI, false);
    let swallowed = matches!(sc::wc_route_event(Event::KeyUp(up)), Event::Unknown);
    let chord_n = sc::user_input_take(slot, &mut ring);
    sc::user_input_set_active(focus0);
    let _ = zoom::take_persist();
    zoom::set_factor_at(want0.0, None, persist0);
    let zoom_ok = control_routed && control_n == 1 && chord.is_none() && zoom_step && swallowed && chord_n == 0;
    let (ring3, r3_ok) = undokeys_ring3(); let pass = undo_ok && redo_ok && zoom_ok && codes_ok && leftovers == 0 && r3_ok;
    serial_println!(
        ":: UNDOKEYS: undo={} redo={} zoom_consumed={} codes={}/{} -> {} :: table={} renamed={} cmd_z={} cmd_shift_z={} control_ring={} chord={} zoom_step={} release=0x{:02x} chord_ring={} leftovers={} ring3={}",
        yn(undo_ok), yn(redo_ok), yn(zoom_ok), c_undo, c_redo, if pass { "PASS" } else { "FAIL" },
        table.name, renamed, z_act.map(Action::name).unwrap_or("none"), sz_act.map(Action::name).unwrap_or("none"),
        control_n, chord.map(|(a, _)| a.name()).unwrap_or("consumed"), zoom_step, up, chord_n, leftovers, ring3
    );
}

// SOFTYMIC (rmbp-ledger B749; BP-video-24, BP-audio-10/12) — TAIL-APPENDED, so no line above moves.
// `tests softymic`: the microphone's in-use light in BOTH looks, and the Privacy row that names who holds it.
// (1) crispy — the Una bar's `micdot` row through the dot's centre, painted offscreen with a synthetic open count of 1
// (the accent dot) and of 0 (none); (2) softy — the taskbar band's row from its live model with the mic light on and
// off (the tray's first item's accent badge), and the live model agrees with the driver's count; (3) privacy — the pure
// note says `in use by <app>` on that app's microphone row while lit, nothing while dark or on another app's row, and
// the live note is silent while the light is out; (4) live — the kernel recorder opens the ONE source: with a capture
// device the pipe arms, `audio::mic_holder` names it, the Una bar paints the dot and the close puts it out; without one
// (QEMU's `usb-audio` has no capture alternate) `live=no-device(<why>)`. The painters are the live ones; the counts
// they are handed in (1)-(2) are the fixture's (the glass and the driver are untouched). One line:
// `:: SOFTYMIC: on=ok off=ok looks=2 privacy=ok -> PASS :: live=<ok|no-device(..)|FAIL> … ::`
#[cfg(all(feature = "uac", feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn softymic_selftest() {
    use crate::video::{look, menubar::micdot, settings::softymic, softy::taskbar, wm};
    let (pw, ph) = {
        let fb = *crate::video::WRITER.lock();
        if !fb.is_ready() {
            serial_println!(":: SOFTYMIC: -> SKIP :: reason=no-panel");
            return;
        }
        (fb.width(), fb.height())
    };
    #[cfg(feature = "login")]
    if !crate::video::desktopbuild::built() {
        serial_println!(":: SOFTYMIC: -> SKIP :: reason=no-desktop");
        return;
    }
    const RGB: u32 = 0x00FF_FFFF;
    let yn = |b: bool| if b { "ok" } else { "FAIL" };
    let say = |on: bool, look: &str| serial_println!("[mic] dot={} by=softymic look={} via=probe", if on { "on" } else { "off" }, look);
    // (1) crispy.
    let crispy = micdot::probe(pw);
    // (2) softy (the look switched in for the band's read, the look found put back).
    let was = look::current();
    let _ = look::set(look::Look::Softy, "tests");
    wm::composite();
    let softy = taskbar::mic_probe(pw, ph);
    let _ = look::set(was, "tests");
    wm::composite();
    let (c_on, c_off) = match crispy { Some((on, off, acc)) => (on & RGB == acc & RGB, off & RGB != acc & RGB), None => (false, false) };
    let (s_on, s_off, s_live) = match softy { Some((on, off, acc, live)) => (on & RGB == acc & RGB, off & RGB != acc & RGB, live), None => (false, false, false) };
    if c_on { say(true, "crispy"); }
    if c_off { say(false, "crispy"); }
    if s_on { say(true, "softy"); }
    if s_off { say(false, "softy"); }
    let looks = (c_on && c_off) as u32 + (s_on && s_off && s_live) as u32;
    // (3) privacy.
    const APP: &str = "softymic";
    let key = crate::prefs::decision_key(crate::fs::apptrust::MIC_DOMAIN, APP);
    let other = crate::prefs::decision_key(crate::fs::apptrust::MIC_DOMAIN, "softymic-other");
    let lit_now = crate::drivers::usb::uac::capture_armed() != 0;
    let privacy = softymic::note(&key, true, APP).as_deref() == Some("in use by softymic")
        && softymic::note(&key, false, APP).is_none()
        && softymic::note(&other, true, APP).is_none()
        && softymic::note(&key, true, "-").is_none()
        && (lit_now || softymic::in_use(&key).is_none());
    // (4) live: the kernel recorder's source, when a microphone has a capture alternate.
    let wait = |ms: u64, f: &dyn Fn() -> bool| -> bool {
        let t0 = crate::arch::ms();
        while !f() {
            if crate::arch::ms().wrapping_sub(t0) >= ms {
                return false;
            }
            crate::arch::sched::sleep_ms(5);
        }
        true
    };
    let live: alloc::string::String = match crate::audio::source(crate::audio::KERNEL_RECORDER) {
        Err(e) => alloc::format!("no-device({})", e),
        Ok(src) => {
            let armed = wait(1_500, &|| crate::drivers::usb::uac::capture_armed() != 0);
            let mut nb = [0u8; 32];
            let (open, n) = crate::audio::mic_holder(&mut nb);
            let named = open >= 1 && &nb[..n] == crate::audio::KERNEL_RECORDER.as_bytes();
            let una = !look::is_softy();
            let lit = !una || wait(2_000, &|| micdot::painted());
            drop(src);
            let closed = wait(1_500, &|| crate::drivers::usb::uac::capture_armed() == 0);
            let dark = !una || wait(2_000, &|| !micdot::painted());
            serial_println!("[softymic] live armed={} named={} open={} lit={} closed={} dark={} look={}", armed as u8, named as u8, open, lit as u8, closed as u8, dark as u8, if una { "crispy" } else { "softy" });
            alloc::string::String::from(if armed && named && lit && closed && dark { "ok" } else { "FAIL" })
        }
    };
    let on = c_on && s_on;
    let off = c_off && s_off;
    let pass = on && off && looks == 2 && privacy && live != "FAIL";
    serial_println!(
        "[softymic] crispy={} softy={} softy_live={} lit_now={}",
        match crispy { Some((a, b, c)) => alloc::format!("{:#08x}/{:#08x}/{:#08x}", a, b, c), None => alloc::string::String::from("none") },
        match softy { Some((a, b, c, _)) => alloc::format!("{:#08x}/{:#08x}/{:#08x}", a, b, c), None => alloc::string::String::from("none") },
        s_live as u8, lit_now as u8
    );
    serial_println!(
        ":: SOFTYMIC: on={} off={} looks={} privacy={} -> {} :: live={} ::",
        yn(on), yn(off), looks, yn(privacy), if pass { "PASS" } else { "FAIL" }, live
    );
}

// LOCKCHANGED (rmbp-ledger B758, BP-security-18) — TAIL-APPENDED, so no line above moves. `tests autolock`'s `gets=` leg,
// folded onto its lines (the field is the line's LAST): the lock re-reads its policy only on a PrefChanged that names one
// of its three keys (`lock::prefs_pass` through `prefs_client::changed_since`). Counted with `lock::gets()` (the GETs
// `lock::refresh` makes): a bus write of `system.lock.after_idle_min` (1440 or 1439: no idle stretch reaches it) and ONE
// pass cost <= 3 (3 whether this pass or the render task's took the edge — the claim is exclusive); `system.settings.tab`
// moved to another tab and back, one pass after each, costs 0; then the delay is put back. `gets=<lock>/<other>`;
// `gets=skip` (the home not writable: a set would be refused) does not fail the line.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn lockchanged_leg() -> (bool, alloc::string::String) {
    use crate::prefs::key;
    use crate::prefs_client::{pref_set, sys_int, PrefValue};
    use crate::video::lock;
    if crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()).is_some() {
        serial_println!("[lock] gets leg skipped: the home is not writable (a set would be refused)");
        return (true, alloc::string::String::from("skip"));
    }
    let set = |k: &str, v: i64| pref_set(crate::prefs::NS, k, PrefValue::Int(v)).is_ok();
    let _ = lock::prefs_pass(); // settle: every earlier change is behind the mark
    let orig = sys_int(key::LOCK_AFTER_IDLE_MIN, 0, 1440).unwrap_or(lock::IDLE_MIN_DEFAULT as i64);
    let want: i64 = if orig == 1440 { 1439 } else { 1440 };
    let g0 = lock::gets();
    let set_ok = set(key::LOCK_AFTER_IDLE_MIN, want);
    let _ = lock::prefs_pass();
    let lock_gets = lock::gets().wrapping_sub(g0);
    let seen = lock::policy().0 as i64 == want;
    let tab = sys_int(key::SETTINGS_TAB, 0, 255).unwrap_or(0);
    let other_tab = if tab == 0 { 1 } else { 0 };
    let g1 = lock::gets();
    let o1 = set(key::SETTINGS_TAB, other_tab);
    let r1 = lock::prefs_pass();
    let o2 = set(key::SETTINGS_TAB, tab);
    let r2 = lock::prefs_pass();
    let other_gets = lock::gets().wrapping_sub(g1);
    let back_ok = set(key::LOCK_AFTER_IDLE_MIN, orig);
    let _ = lock::prefs_pass();
    let restored = lock::policy().0 as i64 == orig;
    let ok = set_ok && seen && lock_gets <= 3 && o1 && o2 && !r1 && !r2 && other_gets == 0 && back_ok && restored;
    serial_println!(
        "[lock] gets leg lock_key={} other_key={} (tab {}->{}->{}) seen={} restored={} last={} via={} -> {}",
        lock_gets, other_gets, tab, other_tab, tab, seen as u8, restored as u8,
        crate::prefs_client::last_changed().map_or(alloc::string::String::from("-"), |(_, a)| a), crate::prefs_client::last_via(), if ok { "ok" } else { "bad" }
    );
    (ok, alloc::format!("{}/{}", lock_gets, other_gets))
}

// PREFSDELETED (rmbp-ledger B755, BP-security-18) — TAIL-APPENDED, so no line above moves. `tests autolock`'s `deleted=`
// leg, folded onto its lines after `prefs=`: a settings domain the user deletes resets its keys to their defaults (R98's
// delete watch) AND posts a PrefChanged per cleared key, so the lock re-reads in ONE `lock::prefs_pass` with no clock
// poll. The fixture keeps the lock domain's file bytes, writes `system.lock.after_idle_min` over the bus client to a
// non-default (1440, or 1439 when the operator already holds 1440; no idle stretch reaches it), lets the lock take it,
// deletes the domain file as Quarry would and runs THE delete watch (`prefs::delete_domain_watched`), runs ONE
// `prefs_pass` and asks `lock::policy()` for the three defaults (10, on, on); then the file is put back byte for byte
// (`prefs::restore_domain`, which posts its keys) and the lock is asked to follow back to the operator's policy.
// `deleted=ok` = the defaults seen in one pass and the operator's policy back; `deleted=skip` (home not writable, or the
// domain not on disk / held) does not fail the line; anything else is `deleted=FAIL`.
#[cfg(all(feature = "login", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn lockdeleted_leg() -> (bool, &'static str) {
    use crate::prefs::key;
    use crate::prefs_client::{pref_set, PrefValue};
    use crate::video::lock;
    if crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()).is_some() {
        serial_println!("[lock] deleted leg skipped: the home is not writable");
        return (true, "skip");
    }
    let d = crate::prefs::domain_of_key(crate::prefs::NS, key::LOCK_AFTER_IDLE_MIN);
    let _ = lock::prefs_pass();
    let orig = lock::policy();
    let Some(bytes) = crate::prefs::domain_bytes(&d) else {
        serial_println!("[lock] deleted leg skipped: settings/{} is not on disk (or held)", d);
        return (true, "skip");
    };
    let want: u32 = if orig.0 == 1440 { 1439 } else { 1440 };
    let set_ok = pref_set(crate::prefs::NS, key::LOCK_AFTER_IDLE_MIN, PrefValue::Int(want as i64)).is_ok();
    let _ = lock::prefs_pass();
    let moved = lock::policy().0 == want;
    let gone = crate::prefs::delete_domain_watched(&d);
    let (dk, ds) = crate::prefs::deleted_counts(); // PREFSUNSET (SR169): every cleared key posted (its default or the unset form)
    let reread = lock::prefs_pass();
    let seen = lock::policy();
    let defaults = seen == (lock::IDLE_MIN_DEFAULT as u32, true, true);
    let back_ok = crate::prefs::restore_domain(&d, &bytes);
    let _ = lock::prefs_pass();
    let restored = lock::policy() == orig;
    let ok = set_ok && moved && gone && defaults && back_ok && restored && dk > 0 && ds == dk;
    serial_println!(
        "[lock] deleted leg domain={} want={} moved={} gone={} reread={} seen={}/{}/{} restored={} keys={} unsent={} -> {}",
        d, want, moved as u8, gone as u8, reread as u8, seen.0, seen.1 as u8, seen.2 as u8, restored as u8, dk, dk.saturating_sub(ds), if ok { "ok" } else { "bad" }
    );
    (ok, if ok { "ok" } else { "FAIL" })
}

// HWTAB2 (rmbp-ledger B759, BP-bar-25, BP-video-32) — TAIL-APPENDED, so no line above moves. `tests hwreport`'s
// HWTAB2 legs, folded onto `hwtab_leg`'s tail: a census line's press puts the line on the clipboard (read back equal,
// the note says copied; the user's clipboard put back), and `Report…`'s save writes `<home>/hardware-report.txt`
// through the VFS (read back: the image line, the features line, every census line; its toast posted). The PASS line
// ends `clip=ok report=ok`; a home that vetoes writes reads `report=skip(home-not-writable:<k>)` and does not fail.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn hwtab2_leg() -> (alloc::string::String, alloc::string::String) {
    crate::video::settings::hwtab2_probe()
}

// QUARRYLAUNCH (rmbp-ledger B754; BP-ring3-18) — TAIL-APPENDED, so no line above moves. `tests oneinstance`'s
// `quarry_launch=` leg, folded onto its lines: Quarry's double-click body (`quarry::live::launch`, through the witness
// wrapper `launch_probe`) on the program the fixture already runs. With the gate armed the launch spawns with argv
// `[path]`, so the reply is `ONEINSTANCE_RAISED` and the last raise names the running owner with rc 0 (no argument).
// `ok`, `FAIL` (a second copy started, or another refusal; the reply is printed), `skip` on a build without `quarry`.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn oneinstance_quarry_launch_leg(path: &str, owner1: u64) -> &'static str {
    #[cfg(feature = "quarry")]
    {
        let reply = crate::video::quarry::live::launch_probe(path);
        let (owner, _, rc) = crate::arch::syscall::oneinstance_last();
        if reply == crate::arch::syscall::ONEINSTANCE_RAISED && owner == owner1 && rc == 0 {
            return "ok";
        }
        serial_println!("[app] oneinstance quarry_launch reply={} raised_owner={} rc={}", reply, owner, rc);
        "FAIL"
    }
    #[cfg(not(feature = "quarry"))]
    {
        let _ = (path, owner1);
        "skip"
    }
}

// FSEARCHICONS (rmbp-ledger B760, BP-video-36, BP-storage-27) — TAIL-APPENDED, so no line above moves. The `icons=` leg of
// `tests filesearch`, over the fixture's own RAM volume (the same `filesearch::answer` the Launcher runs): the type cache
// cleared; `beach kind:text` (a FILTERED query, which reads the stored facts) caches exactly the row it shows,
// `/d00/Beach` = text/plain (`icons_fill=1`, reads > 0); the name-only `beach` reads no attribute (`key_reads=0`); the
// cached `Beach` (no extension) is handed to the painter as `typed_path` (`/d00/Beach.txt`, `typed=1`), the uncached
// `old beach.pdf` as its own path (`plain=1`); with Quarry the pixels agree (`paint=ok`: cached == draw_icon(typed) !=
// draw_icon(path), uncached == draw_icon(path); `skip` without Quarry); 64 inserts, the first touched, one more: 64
// held, the touched kept, the second dropped (`lru=1`); cleared at the end (the live session's cache keeps no test row).
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "unafs"))]
fn filesearch_icons<D: ::unafs::BlockDevice>(fs: &mut ::unafs::UnaFS<D>) -> (bool, alloc::string::String) {
    use crate::video::launcher::filesearch as fsr;
    use alloc::vec::Vec;
    let ask = |fs: &mut ::unafs::UnaFS<D>, q: &str| -> Vec<fsr::Row> {
        match fsr::answer(fs, &fsr::parse(q), &[]) {
            Ok(Some(a)) => a.rows,
            _ => Vec::new(),
        }
    };
    fsr::icon_cache_clear();
    let r0 = fsr::attr_reads();
    let f = ask(fs, "beach kind:text");
    let fill_reads = fsr::attr_reads().wrapping_sub(r0);
    let filled = fill_reads > 0 && f.len() == 1 && f[0].leaf() == "Beach" && fsr::icon_cache_len() == 1 && fsr::cached_type("/d00/Beach").as_deref() == Some("text/plain");
    let r1 = fsr::attr_reads();
    let rows = ask(fs, "beach");
    let key_reads = fsr::attr_reads().wrapping_sub(r1);
    let beach = rows.iter().find(|r| r.leaf() == "Beach");
    let pdf = rows.iter().find(|r| r.leaf() == "old beach.pdf");
    let typed = beach.and_then(|r| fsr::icon_path(&r.path, r.dir));
    let typed_ok = typed.as_deref().is_some_and(|t| t.starts_with("/d00/Beach.") && fsr::typed_path("/d00/Beach", "text/plain").as_deref() == Some(t));
    let plain_ok = pdf.is_some_and(|r| fsr::icon_path(&r.path, r.dir).is_none());
    #[allow(unused_mut)]
    let mut paint = "skip";
    #[cfg(feature = "quarry")]
    if let (Some(b), Some(p), Some(t)) = (beach, pdf, typed.as_deref()) {
        const S: usize = 32;
        let draw = |path: &str| -> Vec<u32> {
            let mut v = alloc::vec![0u32; S * S];
            crate::video::quarry::live::iconview::draw_icon(&mut v, S, S, 0, 0, S, path, false, crate::video::text::Face::Ui);
            v
        };
        let row = |path: &str| -> Vec<u32> {
            let mut v = alloc::vec![0u32; S * S];
            fsr::icon(&mut v, S, S, 0, 0, S, path, false);
            v
        };
        let (cb, cp) = (row(&b.path), row(&p.path));
        paint = if cb == draw(t) && cb != draw(&b.path) && cp == draw(&p.path) { "ok" } else { "FAIL" };
    }
    let paint_reads = fsr::attr_reads().wrapping_sub(r1);
    fsr::icon_cache_clear();
    for i in 0..fsr::TYPE_CACHE_MAX {
        fsr::note_type(&format!("/lru/{}", i), "text/plain");
    }
    let _ = fsr::cached_type("/lru/0");
    fsr::note_type("/lru/new", "text/plain");
    let lru = fsr::icon_cache_len() == fsr::TYPE_CACHE_MAX
        && fsr::cached_type("/lru/0").is_some()
        && fsr::cached_type("/lru/1").is_none()
        && fsr::cached_type("/lru/new").is_some();
    fsr::icon_cache_clear();
    let cleared = fsr::icon_cache_len() == 0;
    let ok = filled && key_reads == 0 && paint_reads == 0 && typed_ok && plain_ok && paint != "FAIL" && lru && cleared;
    let detail = format!(
        "icons_fill={} fill_reads={} key_reads={} typed={} plain={} paint={} lru={} cleared={} typed_path={}",
        filled as u8, fill_reads, key_reads + paint_reads, typed_ok as u8, plain_ok as u8, paint, lru as u8, cleared as u8, typed.as_deref().unwrap_or("-")
    );
    (ok, detail)
}

// RING3UNDO (rmbp-ledger B771) — TAIL-APPENDED, so no line above moves.
/// CHARTER: RING3UNDO (B771): the ring-3 leg of `tests undokeys`, Una's when the image carries it (`unaapp::undo_leg`);
/// `("skip", true)` on a kernel without `una`.
#[cfg(all(feature = "una", feature = "witness", feature = "quarry", target_arch = "x86_64", feature = "wc"))]
fn undokeys_ring3() -> (&'static str, bool) {
    crate::unaapp::undo_leg()
}
#[cfg(all(not(feature = "una"), feature = "witness", feature = "quarry", target_arch = "x86_64", feature = "wc"))]
fn undokeys_ring3() -> (&'static str, bool) {
    ("skip", true)
}

// ── TICKLESS (rmbp-ledger B765, blueprint cores.md BP-cores-18) — `tests tickless` ─────────────────────────────────
//
// FILE-TAIL append under the `tickless` knob (x86). Registered beside `tests fpu`, never run at boot (R80).
// (a) a 2 s window with this task asleep: per online core `[tick] core= wakes_per_s= deadline_us= mode= idle=`, where
//     idle = no dispatch on that core in the window; the verdict is over the idle TICKLESS-eligible cores (core 0 is
//     periodic while the ms clock is its tick, and the HID pump core is periodic). (b) sleep_ms accuracy on a task
//     pinned to an idle eligible core, measured on the system clock (TSC us when the clock is the TSC, else ms).
//     (c) the quantum: two spinners pinned to that core both progress by the midpoint and the core dispatches >= 20
//     times in 300 ms (the periodic tick came back while it is busy). The lid poll and DIMIDLE ride the usb-pump's
//     1-tick sleep (a sleeper deadline), the mechanism leg (b) measures. Detail: the one-shot arm-vs-wake error.
#[cfg(all(target_arch = "x86_64", feature = "tickless"))]
mod tickless_t {
    use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static SLEEP_ERR_US: AtomicU64 = AtomicU64::new(0);
    pub static SLEEP_DONE: AtomicU64 = AtomicU64::new(0);
    pub static SPIN_END_MS: AtomicU64 = AtomicU64::new(0);
    pub static SPIN_CNT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

    /// The system clock in us: the TSC when it IS the ms clock, else the ms clock itself.
    pub fn now_us() -> u64 {
        let hz = crate::arch::apic::tsc_hz();
        if crate::arch::clockcore::tsc_clock() && hz != 0 {
            (crate::arch::now_cycles() as u128 * 1_000_000 / hz as u128) as u64
        } else {
            crate::arch::ms() * 1000
        }
    }

    /// Leg (b), on the pinned core: each requested sleep against the clock; the worst |error| in us.
    pub fn sleep_leg(_: usize) {
        for &n in [1u64, 2, 5, 10, 17, 50, 1, 2, 5, 10, 17, 50].iter() {
            let t0 = now_us();
            crate::arch::sched::sleep_ms(n);
            let el = now_us().saturating_sub(t0);
            SLEEP_ERR_US.fetch_max(el.abs_diff(n * 1000), Relaxed);
            SLEEP_DONE.fetch_add(1, Relaxed);
        }
    }

    /// Leg (c): spin (never yielding) until the end, counting.
    pub fn spin_leg(i: usize) {
        while crate::arch::ms() < SPIN_END_MS.load(Relaxed) {
            SPIN_CNT[i & 1].fetch_add(1, Relaxed);
            core::hint::spin_loop();
        }
    }
}

#[cfg(all(target_arch = "x86_64", feature = "tickless"))]
fn tickless_selftest() {
    use crate::arch::sched::{self, tickless, JoinResult};
    use core::sync::atomic::Ordering::Relaxed;
    use tickless_t::*;
    if crate::arch::apic::apic_timer_hz() == 0 {
        serial_println!(":: TICKLESS: cores=0 idle_wakes_per_s<=- sleep_ms_err<=- quantum=- -> SKIP reason=uncalibrated ::");
        return;
    }
    let mut cores = alloc::vec![0usize];
    for c in crate::arch::smp::online_aps() {
        if c != 0 && !cores.contains(&c) {
            cores.push(c);
        }
    }
    // (a) the idle window.
    let w0: alloc::vec::Vec<(u64, u64)> = cores.iter().map(|&c| (tickless::census(c).0, sched::meter_cpu_ticks(c).0)).collect();
    let t0 = crate::arch::ms();
    sched::sleep_ms(2000);
    let el = (crate::arch::ms() - t0).max(1);
    let here = crate::arch::percpu::this_cpu().cpu_index as usize;
    let mut idle = alloc::vec::Vec::new();
    let mut worst = 0u64;
    for (i, &c) in cores.iter().enumerate() {
        let (wakes, arm_us, _) = tickless::census(c);
        let busy = sched::meter_cpu_ticks(c).0.wrapping_sub(w0[i].1);
        let wps = wakes.wrapping_sub(w0[i].0) * 1000 / el;
        let elig = tickless::eligible(c);
        serial_println!("[tick] core={} wakes_per_s={} deadline_us={} mode={} idle={}", c, wps, arm_us, if elig { "tickless" } else { "periodic" }, (busy == 0) as u8);
        if busy == 0 && elig {
            idle.push(c);
            worst = worst.max(wps);
        }
    }
    let Some(&target) = idle.iter().rev().find(|&&c| c != here) else {
        serial_println!(":: TICKLESS: cores={} idle_wakes_per_s<=- sleep_ms_err<=- quantum=- -> SKIP reason=no-idle-core ::", cores.len());
        return;
    };
    // (b) sleep accuracy on the idle core.
    SLEEP_ERR_US.store(0, Relaxed);
    SLEEP_DONE.store(0, Relaxed);
    let h = sched::spawn_joinable("tickless-sleep", sleep_leg, 0, target, sched::PRIO_NORMAL);
    let sleep_ok = h.join_timeout(crate::arch::ms_to_ticks(3000)) == JoinResult::Completed && SLEEP_DONE.load(Relaxed) == 12;
    let err_us = SLEEP_ERR_US.load(Relaxed);
    let err_ms = (err_us + 500) / 1000;
    // (c) the quantum on the same core.
    SPIN_CNT[0].store(0, Relaxed);
    SPIN_CNT[1].store(0, Relaxed);
    let d0 = sched::meter_cpu_ticks(target).0;
    let k0 = crate::arch::percpu::cpu(target).map_or(0, |p| p.ticks.load(Relaxed));
    SPIN_END_MS.store(crate::arch::ms() + 300, Relaxed);
    let a = sched::spawn_joinable("tickless-spin0", spin_leg, 0, target, sched::PRIO_LOW);
    let b = sched::spawn_joinable("tickless-spin1", spin_leg, 1, target, sched::PRIO_LOW);
    sched::sleep_ms(150);
    let (m0, m1) = (SPIN_CNT[0].load(Relaxed), SPIN_CNT[1].load(Relaxed));
    let ja = a.join_timeout(crate::arch::ms_to_ticks(2000));
    let jb = b.join_timeout(crate::arch::ms_to_ticks(2000));
    let disp = sched::meter_cpu_ticks(target).0.wrapping_sub(d0);
    let ticks = crate::arch::percpu::cpu(target).map_or(0, |p| p.ticks.load(Relaxed)).wrapping_sub(k0);
    let qok = m0 > 0 && m1 > 0 && disp >= 20 && ja == JoinResult::Completed && jb == JoinResult::Completed;
    let (late, early) = tickless::arm_err_us();
    let pass = worst <= 10 && sleep_ok && err_ms <= 1 && qok;
    serial_println!(
        ":: TICKLESS: cores={} idle_wakes_per_s<={} sleep_ms_err<={} quantum={} -> {} :: idle_cores={} target=c{} sleep_err_us={} sleep_legs={} spin_mid={}/{} dispatches={} ticks={} arm_err_us late={} early={} slack_us={} beat_ms={} ::",
        cores.len(), worst, err_ms, if qok { "ok" } else { "FAIL" }, if pass { "PASS" } else { "FAIL" },
        idle.len(), target, err_us, SLEEP_DONE.load(Relaxed), m0, m1, disp, ticks, late, early, tickless::SLACK_US, tickless::IDLE_BEAT_MS
    );
    tally(pass);
}

/// SCROLLVIEW (rmbp-ledger B764) — `tests scrollview`: wm's one scroll model, measured. (1) A 10 000-px extent in a
/// 56 x 300 viewport of a 72-px-stride surface (sentinels around it) scrolled 200 steps of a fixed pseudo-random walk
/// (small moves, moves past a viewport, runs into both ends): each step's offset is the independent clamp, the
/// surface after `wm::scroll_blit` + the strip painted equals the content at the new offset, nothing outside the
/// viewport moved, and the repaint is the strip alone (`repaint_px <= |dy| x w`, `blit + repaint = viewport` on a
/// move). (2) The thumb: exact at both ends, its length the formula's, monotonic, its inverse within a line, and
/// `draw::scrollbar` paints the thumb and track inks where the model says. (3) Quarry's list and (4) Settings'
/// Hardware list: the live strip bodies against a full paint, pixel for pixel.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn scrollview_selftest() {
    use crate::video::wm::{scroll_blit, ScrollView};
    const STRIDE: usize = 72;
    const H: usize = 320;
    const SENT: u32 = 0x00A5_5A5A;
    let (vx, vy, vw, vh) = (8usize, 10usize, 56usize, 300usize);
    let content = |x: usize, cy: usize| -> u32 { ((cy as u32).wrapping_mul(0x9E37_79B1) ^ (x as u32).wrapping_mul(0x85EB_CA77)) & 0x00FF_FFFF };
    let mut px: alloc::vec::Vec<u32> = alloc::vec![SENT; STRIDE * H];
    let mut view = ScrollView { offset: 0, extent: 10_000, viewport: (vx, vy, vw, vh), line: 1 };
    let paint_rows = |px: &mut [u32], off: usize, y0: usize, y1: usize| {
        for y in y0..y1 {
            for x in vx..vx + vw {
                px[y * STRIDE + x] = content(x, off + (y - vy));
            }
        }
    };
    paint_rows(&mut px, 0, vy, vy + vh);
    let max = view.extent - vh;
    let (mut clamp_ok, mut blit_ok) = (true, true);
    let (mut clamps, mut fulls, mut max_rep, mut max_blit) = (0usize, 0usize, 0usize, 0usize);
    let mut seed: u32 = 0x5C01_1E55;
    let mut steps = 0usize;
    for k in 0..200usize {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let r = (seed >> 8) as isize;
        let dy: isize = match k % 10 {
            0 => if (r & 1) == 0 { vh as isize + 41 } else { -(vh as isize) - 77 },
            1 if k < 40 => -2_000,
            2 if k > 160 => 3_000,
            _ => (r % 801) - 400,
        };
        let want = (view.offset as isize + dy).clamp(0, max as isize) as usize;
        let st = view.step(dy);
        if st.off != want || st.off > max {
            clamp_ok = false;
        }
        if st.off as isize != view.offset as isize + dy {
            clamps += 1;
        }
        let blit = scroll_blit(&mut px, STRIDE, &st);
        paint_rows(&mut px, st.off, st.strip.0, st.strip.1);
        let repaint = (st.strip.1 - st.strip.0) * vw;
        let n = st.moved.unsigned_abs();
        if blit != st.blit_px || repaint > n * vw || (n > 0 && blit + repaint != vw * vh) || (n == 0 && repaint + blit != 0) {
            blit_ok = false;
        }
        if n >= vh {
            fulls += 1;
        }
        max_rep = max_rep.max(repaint);
        max_blit = max_blit.max(blit);
        view.offset = st.off;
        for y in 0..H {
            for x in 0..STRIDE {
                let inside = x >= vx && x < vx + vw && y >= vy && y < vy + vh;
                let want = if inside { content(x, st.off + (y - vy)) } else { SENT };
                if px[y * STRIDE + x] != want {
                    blit_ok = false;
                }
            }
        }
        steps += 1;
    }
    let reached_ends = clamps > 0;
    // (2) the thumb.
    let track = vh;
    let tmin = crate::ui::px(24).min(track);
    let th_want = (track * vh / 10_000).max(tmin).min(track);
    let mut thumb_ok = true;
    let mut last = 0usize;
    for off in (0..=max).step_by(97).chain(core::iter::once(max)) {
        let v = ScrollView { offset: off, ..view };
        match v.thumb(track) {
            Some((ty, th)) => {
                if th != th_want || ty + th > track || ty < last || (off == 0 && ty != 0) || (off == max && ty + th != track) {
                    thumb_ok = false;
                }
                let back = v.offset_at_thumb(track, ty);
                let span = track - th;
                if back.abs_diff(off) > max.div_ceil(span.max(1)) + 1 {
                    thumb_ok = false;
                }
                last = ty;
            }
            None => thumb_ok = false,
        }
    }
    if (ScrollView { offset: 0, extent: 200, viewport: (0, 0, 10, 300), line: 1 }).thumb(300).is_some() {
        thumb_ok = false;
    }
    {
        use crate::video::draw;
        let bw = draw::scrollbar_w();
        let (sw, sh) = (bw + 4, 200usize);
        let mut buf: alloc::vec::Vec<u32> = alloc::vec![0; sw * sh];
        let v = ScrollView { offset: 5_000, extent: 10_000, viewport: (0, 0, 10, sh), line: 1 };
        let (ty, th) = v.thumb(sh).unwrap_or((0, 0));
        let mut s = draw::Surf::new(&mut buf, sw);
        draw::scrollbar(&mut s, draw::Rect::new(2, 0, bw as i32, sh as i32), Some((ty as i32, th as i32)), draw::ControlState::NORMAL);
        let cx = 2 + bw / 2;
        let at = |y: usize| buf[y * sw + cx] & 0x00FF_FFFF;
        let thumb_in = at(ty + th / 2) == crate::video::theme::line_strong() & 0x00FF_FFFF;
        let track_in = (ty > bw + 2 && at(ty / 2) == crate::video::theme::surface_sunken() & 0x00FF_FFFF) || (ty + th + bw + 2 < sh && at((ty + th + sh) / 2) == crate::video::theme::surface_sunken() & 0x00FF_FFFF);
        if !(thumb_in && track_in && buf[0] == 0) {
            thumb_ok = false;
        }
    }
    // (3) Quarry's list, (4) the Hardware list.
    #[cfg(feature = "quarry")]
    let quarry = crate::video::quarry::live::scrollview_probe();
    #[cfg(not(feature = "quarry"))]
    let quarry: Result<usize, &'static str> = Err("no-quarry");
    let hw = crate::video::settings::scrollview_probe();
    let word = |r: &Result<usize, &'static str>| -> alloc::string::String { match r { Ok(_) => alloc::string::String::from("ok"), Err(e) => alloc::format!("FAIL({})", e) } };
    serial_println!(
        "[scrollview] extent=10000 viewport={}x{} stride={} steps={} clamps={} full_strips={} max_repaint_px={} max_blit_px={} thumb_min={} quarry_steps={} hw_steps={}",
        vw, vh, STRIDE, steps, clamps, fulls, max_rep, max_blit, tmin, quarry.as_ref().copied().unwrap_or(0), hw.as_ref().copied().unwrap_or(0)
    );
    // (5) RING3SCROLL (B778): a ring-3 window through the bus verb — registered, one wheel step, one INPUT_EV_SCROLL with
    // the clamped offset, the bar in its gutter — each arch's own arm (x86 `wc` `busx_scroll_set`; aarch64 `bus_scroll_set`, SCROLLARM64 SR172); an aarch64 desktop with no EL0 has no ring-3 window: `none(no-el0)`.
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    let ring3 = crate::arch::syscall::scroll_ring3_probe();
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    let ring3: Result<usize, &'static str> = Ok(0);
    let ring3_word = if cfg!(any(target_arch = "x86_64", feature = "aarch64_el0")) { word(&ring3) } else { alloc::string::String::from("none(no-el0)") };
    let (panes, panes_ok) = scrollpanes_legs(); // SCROLLPANES (B777): the eight pane words (tree icons columns tabs types overlay sidebar cols), before the verdict
    let pass = steps == 200 && clamp_ok && reached_ends && blit_ok && thumb_ok && quarry.is_ok() && hw.is_ok() && panes_ok && ring3.is_ok();
    serial_println!(
        ":: SCROLLVIEW: steps={} clamp={} blit={} thumb={} quarry={} hw={} {} -> {} :: ring3={}",
        steps, if clamp_ok && reached_ends { "ok" } else { "FAIL" }, if blit_ok { "ok" } else { "FAIL" }, if thumb_ok { "ok" } else { "FAIL" }, word(&quarry), word(&hw), panes, if pass { "PASS" } else { "FAIL" }, ring3_word
    );
}

/// USBSUSPEND (rmbp-ledger B766, BP-power-7): `tests usbsuspend` (R80: registered, never at boot). Ask both USB drivers for
/// the suspend, wait (≤2 s) for both passes, read each suspended xHCI port's PLS, ask for the resume, wait (≤2 s) for UP, then
/// a keyboard report over EP0 (GET_REPORT) and the interrupt endpoint's state — never re-enumerated; the panel stays lit.
/// EHCIWAKE (B776): QEMU's default usb-kbd rides EHCI, now suspended too: PASS on it with `ehci=<s>/<n>`, s = n (`usbsuspend_ehci_leg`).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))]
fn usbsuspend_selftest() {
    use crate::drivers::xhci::{self, suspend as xs};
    let wait = |ms: u64, f: &dyn Fn() -> bool| -> bool {
        let t0 = crate::arch::ms();
        while !f() {
            if crate::arch::ms().wrapping_sub(t0) >= ms {
                return false;
            }
            crate::arch::sched::sleep_ms(5);
        }
        true
    };
    if matches!(xhci::claim(), Err(xhci::XhciClaimError::NotReady)) {
        serial_println!(":: USBSUSPEND: ports=0 suspended=0 resumed=0 kbd=none -> SKIP reason=no-xhci ::");
        return;
    }
    #[cfg(feature = "ehcihid")] use crate::drivers::ehci::suspend as es; #[cfg(feature = "ehcihid")] let (e_down, e_up): (fn() -> bool, fn() -> bool) = (es::suspended, es::up);
    #[cfg(not(feature = "ehcihid"))] let (e_down, e_up): (fn() -> bool, fn() -> bool) = (|| true, || true);
    xs::request(true); #[cfg(feature = "ehcihid")] es::request(true);
    let in_u3 = wait(2_000, &|| xs::suspended() && e_down());
    let c = xs::census();
    let held = usbsuspend_held_queue(in_u3);
    xs::request(false); #[cfg(feature = "ehcihid")] es::request(false);
    let up = wait(2_000, &|| xs::up() && e_up());
    let held_word = usbsuspend_held_collect(held);
    let r = xs::census();
    let mut probe = None;
    let t0 = crate::arch::ms();
    while probe.is_none() && crate::arch::ms().wrapping_sub(t0) < 1_000 {
        match xhci::claim() { Ok(mut x) => probe = Some(x.kbd_report_probe()), Err(_) => crate::arch::sched::sleep_ms(5) }
    }
    let (kbd, ep) = match probe {
        None => ("busy", 0xFF),
        Some(None) => ("none", 0xFF),
        Some(Some((cc, ep, len))) if matches!(cc, 1 | 13) && ep == 1 && len > 0 => ("ok", ep),
        Some(Some((_, ep, _))) => ("fail", ep),
    };
    let (e, e_kbd) = usbsuspend_ehci_leg(kbd == "none");
    let verdict = if !in_u3 || !up {
        "FAIL reason=phase-timeout"
    } else if c.pls3 != c.suspended || r.resumed < c.suspended || e.resumed < e.suspended || r.ep_running < r.ep_stopped { "FAIL"
    } else if kbd != "none" { if kbd != "ok" || c.suspended == 0 || held_word != "ok" { "FAIL" } else { "PASS" }
    } else if e_kbd == "none" {
        "SKIP reason=no-kbd"
    } else if e_kbd != "ok" || e.ports == 0 || e.suspended != e.ports {
        "FAIL"
    } else {
        "PASS"
    };
    let (kbd_on, kbd_word) = if kbd != "none" { ("xhci", kbd) } else { ("ehci", e_kbd) };
    serial_println!(
        ":: USBSUSPEND: ports={} suspended={} resumed={} kbd={} ep_stopped={} ep_running={} -> {} :: pls3={} kept={} kbd_ep={} remote_wakes={} ehci={}/{} ehci_kept={} ehci_resumed={} wake_armed={} kbd_on={} held_report={} held_rings={}",
        c.ports, c.suspended, r.resumed, kbd_word, r.ep_stopped, r.ep_running, verdict, c.pls3, c.kept, ep, r.remote_wakes, e.suspended, e.ports, e.kept, e.resumed, e.wake_armed, kbd_on, held_word, r.held_rings
    );
}

// ══════════════ TRASHBATCHDOCK (rmbp-ledger B762) — `tests trashtoast`'s `empty=` leg — TAIL-APPENDED ══════════════

/// TRASHBATCHDOCK (B762, BP-video-29): the dock's Empty Trash body (`dock::empty_trash_now`, what the service pass runs
/// after the confirm). A clean Trash (the TRASH fixture's own `empty()` slate), three scratch files trashed as ONE batch
/// (one journal entry), then the empty: the Trash holds 0, the undo stack holds 0 (`undo::drop_trash_entries`), and
/// exactly one `Trash emptied` toast was posted. Runs on `trashtoast_selftest`'s set-aside journal; the scratch folder
/// is removed. `[trashtoast] empty before=3 items=3 bytes=<n> ms=<n> after=0 journal=1->0 toasts=1`.
#[cfg(all(feature = "quarry", feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn trashtoast_empty_leg(base: &str) -> bool {
    use alloc::{string::String, vec::Vec};
    use crate::fs::vfs::{NodeKind, KERNEL_PRINCIPAL as P};
    use crate::video::quarry::live::ops;
    use crate::video::quarry::undo;
    let t = crate::shell::vfs_mount_table();
    let _ = ops::op_delete(base);
    let _ = crate::fs::trash::empty();
    let files: Vec<String> = ["E1.TXT", "E2.TXT", "E3.TXT"].iter().map(|n| format!("{}/{}", base, n)).collect();
    let made = ops::op_mkdir(base).is_ok() && files.iter().all(|p| t.create(p, NodeKind::File, P).and_then(|_| t.write(p, 0, p.as_bytes(), P)).is_ok());
    undo::batch_begin();
    let trashed = files.iter().filter(|p| undo::trash(p).is_ok()).count();
    let _ = undo::batch_end();
    let before = crate::fs::trash::count();
    let j0 = undo::depth().0;
    let t0 = crate::video::dock::emptied_toasts();
    let r = crate::video::dock::empty_trash_now();
    let toasts = crate::video::dock::emptied_toasts() - t0;
    let after = crate::fs::trash::count();
    let j1 = undo::depth().0;
    let _ = ops::op_delete(base);
    let (n, bytes, ms) = r.as_ref().map(|&(n, b, ms, _)| (n, b, ms)).unwrap_or((0, 0, 0));
    serial_println!("[trashtoast] empty before={} items={} bytes={} ms={} after={} journal={}->{} toasts={}", before, n, bytes, ms, after, j0, j1, toasts);
    made && trashed == 3 && before == 3 && r.is_ok() && n == 3 && after == 0 && j0 == 1 && j1 == 0 && toasts == 1 && t.stat(base).is_err()
}

// SINKALL (rmbp-ledger B767; BP-audio-14/15/16) — TAIL-APPENDED. `tests sinkall`: the Player, the video's sound and the
// alert are mixer sources and the chosen sink is the only writer. For each sink (HDA, USB with `UNAOS_UAC=1`, BT when
// an A2DP stream runs) chosen through `audio::sink` (the person's choice restored after): the alert through its real
// latch (`hda::play::request_alert`, the service tick) — the mixer's source count goes 0 -> 1 -> 0 and the alert's
// frames reach that sink's output. On the first live sink, the Player's source fns as `player` and as `video`
// (`hda::play::fixture_source`), and the alert over a running Player (the Player's source survives the alert's close).
// QEMU has no Bluetooth: the BT output's path runs offline (`mixer::bt_synthetic`). direct_writes = HDA stream epochs
// not opened by the mixer. usb_volume = the device's Feature Unit took SET_CUR (ok), or with no device the descriptor
// walk of QEMU's unit (fixture).
// Wire: `[sinkall] leg …`, `[audio] source=<player|video|alert> sink=<hda|usb|bt> frames=<n>`,
// `:: SINKALL: sources=<n> direct_writes=<n> usb_volume=<ok|fixture|mixer|FAIL|not-built> -> PASS|SKIP|FAIL :: hda= usb= bt= overlap= pause_ms=<n> slider_ms=<n> ::` (SINKALLDOCS B781: -1 when not measured; PASS needs <= 250; the leg is `sinkall_pause`, file tail; MIXERPUMP B787: slider_ms <= 20, `sinkall_slider`, file tail).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_selftest() {
    use crate::audio::{self, mixer, Sink};
    use crate::drivers::hda::play;
    if mixer::sources() != 0 || play::busy() || play::alert_sounding() {
        serial_println!(":: SINKALL: sources=- direct_writes=- usb_volume=- -> SKIP reason=output-in-use ::");
        return;
    }
    let was = audio::current();
    let (e0, o0) = (play::epoch(), mixer::hda_opens());
    let hda = sinkall_alert("hda", 0);
    let usb = sinkall_alert("usb", 1);
    let bt_live = sinkall_alert("bt", 2);
    let bt = if bt_live != "absent" {
        bt_live
    } else {
        let (open, after, _, _, ok) = mixer::bt_synthetic(&play::alert_fixture_pcm());
        if open == 1 && after == 0 && ok { "synthetic" } else { "FAIL" }
    };
    let live = if hda == "ok" { Some(("hda", 0usize)) } else if usb == "ok" { Some(("usb", 1usize)) } else { None };
    let (mut sources, mut overlap, mut pause_ms) = (0u32, "-", u64::MAX);
    if let Some((s, k)) = live {
        sources = 1; // the alert reached a sink
        let _ = audio::sink(s);
        sources += sinkall_player("player", s, k) as u32;
        sources += sinkall_player("video", s, k) as u32;
        overlap = if sinkall_overlap(s) { "ok" } else { "FAIL" }; pause_ms = sinkall_pause(s); // SINKALLDOCS (B781): the Player's pause lands within the sink's queue (file tail)
    }
    let usb_volume = sinkall_usb_volume(usb); let slider_ms = sinkall_slider(hda, usb); // MIXERPUMP (B787): a set through `status::set_volume` reaches its carrier within one period + 10 ms (file tail)
    let direct = play::epoch().wrapping_sub(e0) as i64 - mixer::hda_opens().wrapping_sub(o0) as i64;
    let _ = audio::sink(if was == Sink::Usb { "usb" } else if was == Sink::Hda { "hda" } else { "bt" });
    let failed = [hda, usb, bt_live, bt, overlap, usb_volume].iter().any(|v| *v == "FAIL");
    let verdict = if live.is_none() { "SKIP" } else if !failed && sources == 3 && direct == 0 && pause_ms <= SINKALL_PAUSE_MS && slider_ms <= SINKALL_SLIDER_MS { "PASS" } else { "FAIL" };
    serial_println!(
        ":: SINKALL: sources={} direct_writes={} usb_volume={} -> {} :: hda={} usb={} bt={} overlap={} pause_ms={} slider_ms={} ::",
        sources, direct, usb_volume, verdict, hda, usb, bt, overlap, pause_ms as i64, slider_ms as i64
    );
}

/// One sink's alert leg: `ok`, `absent` (the sink cannot be chosen on this image or bench) or `FAIL`.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_alert(sink: &str, k: usize) -> &'static str {
    use crate::audio::mixer;
    use crate::drivers::hda::play;
    if crate::audio::sink(sink).is_err() {
        serial_println!("[sinkall] leg source=alert sink={} -> absent", sink);
        return "absent";
    }
    let d0 = mixer::delivered()[k];
    let n0 = mixer::sources();
    let asked = play::request_alert().is_ok();
    let one = asked && play::pump_until(1_000, || mixer::sources() == 1);
    let zero = one && play::pump_until(4_000, || mixer::sources() == 0 && !play::alert_sounding());
    let frames = mixer::delivered()[k].wrapping_sub(d0);
    let alert = (play::alert_fixture_pcm().len() / 2) as u64; // mono 16-bit, 48 kHz
    let want = if k == 2 { alert * 44_100 / 48_000 } else { alert };
    let ok = n0 == 0 && one && zero && frames + 2 >= want;
    serial_println!("[sinkall] leg source=alert sink={} sources={}->{}->{} frames={} want={} -> {}", sink, n0, one as u8, if zero { 0 } else { 1 }, frames, want, if ok { "ok" } else { "FAIL" });
    if ok { "ok" } else { "FAIL" }
}

/// The Player's source as `name` on `sink`: 200 ms through the Player's fns, played out, closed by the service tick.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_player(name: &'static str, sink: &str, k: usize) -> bool {
    use crate::audio::mixer;
    use crate::drivers::hda::play;
    const N: u64 = 9_600;
    let d0 = mixer::delivered()[k];
    let opened = play::fixture_source(name).is_ok();
    let fed = opened && play::pump_until(2_000, || play::fixture_feed(N) >= N);
    play::fixture_end();
    let done = fed && play::pump_until(4_000, || !play::busy());
    let frames = mixer::delivered()[k].wrapping_sub(d0);
    let ok = done && frames >= N;
    serial_println!("[sinkall] leg source={} sink={} fed={} done={} frames={} -> {}", name, sink, fed as u8, done as u8, frames, if ok { "ok" } else { "FAIL" });
    if !done {
        play::stop();
    }
    ok
}

/// The alert over a running Player on `sink`: both sources mixed, the alert closes, the Player's source is still open
/// (the stop split: the alert's close and any re-arm of the output never end the play); then the Player is stopped.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_overlap(sink: &str) -> bool {
    use crate::audio::mixer;
    use crate::drivers::hda::play;
    const LONG: u64 = 48_000 * 4;
    if play::fixture_source("player").is_err() {
        return false;
    }
    let primed = play::pump_until(1_000, || play::fixture_feed(LONG) >= 9_600);
    let asked = primed && play::request_alert().is_ok();
    let two = asked && play::pump_until(1_000, || play::fixture_feed(LONG) > 0 && mixer::sources() == 2);
    let gone = two && play::pump_until(4_000, || play::fixture_feed(LONG) > 0 && !play::alert_sounding());
    let survived = gone && play::busy() && mixer::sources() == 1;
    play::stop();
    let closed = play::pump_until(500, || mixer::sources() == 0);
    let ok = survived && closed;
    serial_println!("[sinkall] leg overlap sink={} two={} alert_closed={} player_survived={} closed={} -> {}", sink, two as u8, gone as u8, survived as u8, closed as u8, if ok { "ok" } else { "FAIL" });
    ok
}

/// The USB sink's volume: the live device's Feature Unit took SET_CUR (`ok`), has no Volume control (`mixer`: the mixer
/// scales), refused (`FAIL`); with no device, QEMU's unit through the descriptor walk (`fixture`).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_usb_volume(usb: &str) -> &'static str {
    #[cfg(feature = "uac")]
    {
        use crate::drivers::usb::uac;
        if usb != "ok" {
            return if uac::fu_fixture() { "fixture" } else { "FAIL" };
        }
        let took = crate::drivers::hda::play::pump_until(1_500, || uac::volume_state().0 > 0 || uac::volume_state().1);
        let (sets, refused) = uac::volume_state();
        serial_println!("[sinkall] usb volume sets={} refused={} on_device={} waited={}", sets, refused as u8, uac::volume_on_device() as u8, took as u8);
        if refused { "FAIL" } else if sets > 0 { "ok" } else if !uac::volume_on_device() { "mixer" } else { "FAIL" }
    }
    #[cfg(not(feature = "uac"))]
    {
        let _ = usb;
        "not-built"
    }
}

// PREFSRELOAD (rmbp-ledger B763) — TAIL-APPENDED: `tests settingsmap` runs SETTINGSMAP's own leg
// (`video::settings::selftest_map`, unchanged) and then the session-reload leg, whose verdict line ends `reload=ok`.
// The leg: the tree the store holds is kept as the previous session's (`prefs::snapshot`); ONE key
// (`system.locale.first_day`) is written to the store (`prefs::set`, its own PrefChanged); the session is switched
// against that kept tree (`prefs::reload_session` — the diff a login or logout runs, `[prefs] reload gen= changed= ms=`)
// and exactly that key's PrefChanged arrives, once (`changed=1`, and the kernel subscriber's count moves by 1). The key is
// put back afterwards. No session row is touched and SESSION_GEN is not moved (its other readers are not this leg's).
// `:: SETTINGSMAP: reload leg key=system.locale.first_day changed=<n> arrived=<n|na> -> PASS|FAIL :: reload=<ok|bad> ::`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn settingsmap_selftest() {
    use crate::prefs::{key, PrefValue, NS};
    crate::video::settings::selftest_map();
    fn arrived_n() -> Option<u32> {
        #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64", all(target_arch = "aarch64", feature = "desktop_firmware")))]
        {
            crate::prefs_client::subscribe();
            return Some(crate::prefs_client::CHANGED_N.load(Ordering::Acquire));
        }
        #[allow(unreachable_code)]
        None
    }
    crate::prefs::ensure_loaded();
    let k = key::LOCALE_FIRST_DAY;
    let old = crate::prefs::snapshot();
    let was = crate::prefs::text(k);
    let want = if was.as_deref() == Some("sun") { "sat" } else { "sun" };
    let wrote = crate::prefs::set(NS, k, PrefValue::Str(alloc::string::String::from(want)));
    let (changed, arrived) = if wrote.is_ok() {
        let c0 = arrived_n();
        let n = crate::prefs::reload_session(&old);
        (n, c0.and_then(|c| arrived_n().map(|c1| c1.wrapping_sub(c))))
    } else {
        (0, None)
    };
    let _ = crate::prefs::set(NS, k, PrefValue::Str(was.unwrap_or_else(|| alloc::string::String::from("mon"))));
    let ok = wrote.is_ok() && changed == 1 && arrived.map_or(true, |a| a == 1);
    serial_println!(
        ":: SETTINGSMAP: reload leg key={}.{} changed={} arrived={} wrote={} -> {} :: reload={} ::",
        NS, k, changed, arrived.map_or(alloc::string::String::from("na"), |a| format!("{}", a)),
        if wrote.is_ok() { "ok" } else { "refused" }, if ok { "PASS" } else { "FAIL" }, if ok { "ok" } else { "bad" }
    );
    settingsmap_switch_leg(); settingsmap_ring_leg(); // SESSIONGENPOLL (B782): the switch leg (file tail). SUBSCRIBEKEEP (B796): the ring leg (file tail)
}

// SESSIONGENPOLL (rmbp-ledger B782) — TAIL-APPENDED: `tests settingsmap`'s switch leg. Settings, the lock, the clock and the
// look no longer poll `fs::users::SESSION_GEN`: a session switch reaches them only as the reload's PrefChanged
// (PREFSRELOAD B763). The leg: one key Settings' view shows (`system.display.idle_min`) is written to a differing value
// and the session is switched against the tree held before (`prefs::reload_session`). Pass 1 runs the four consumers'
// service halves (`settings::bus_changes_idle` — Settings' view —, `look::service`, `clock::locale::service`,
// `lock::prefs_pass`): the key's frame reaches Settings' drain. Pass 2 runs them again with nothing new pending and
// counts the login-generation reads (`fs::users::session_gen_reads`, the counted gateway `prefs::session_gen` uses):
// a poll would read it every pass, so the poll's absence is `gen_reads=0`. The key is put back afterwards.
// `:: SETTINGSMAP: switch leg key=system.display.idle_min changed=<n> frames=<n> gen_reads=<n> -> PASS|FAIL :: switch=<ok|bad> ::`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn settingsmap_switch_leg() {
    use crate::prefs::{key, PrefValue, NS};
    fn consumers_pass() -> usize {
        let (_, idle, _) = crate::video::settings::bus_changes_idle();
        crate::video::look::service();
        let _ = crate::clock::locale::service();
        #[cfg(feature = "login")]
        let _ = crate::video::lock::prefs_pass();
        idle
    }
    crate::prefs_client::subscribe();
    let _ = consumers_pass(); // the queue and the marks settled before the switch
    let k = key::IDLE_MIN;
    let old = crate::prefs::snapshot();
    let was = crate::prefs::int(k, 0, 1440);
    let want = if was == Some(15) { 30 } else { 15 };
    let wrote = crate::prefs::set(NS, k, PrefValue::Int(want));
    let (changed, frames, reads) = if wrote.is_ok() {
        let _ = crate::video::settings::bus_changes_idle(); // the write's own frame drained: only the switch's is counted
        let n = crate::prefs::reload_session(&old);
        let frames = consumers_pass();
        let r0 = crate::fs::users::session_gen_reads();
        let _ = consumers_pass();
        (n, frames, crate::fs::users::session_gen_reads().wrapping_sub(r0))
    } else {
        (0, 0, 0)
    };
    let _ = crate::prefs::set(NS, k, PrefValue::Int(was.unwrap_or(prefs_core::display::IDLE_MIN_DEFAULT)));
    let ok = wrote.is_ok() && changed == 1 && frames >= 1 && reads == 0;
    serial_println!(
        ":: SETTINGSMAP: switch leg key={}.{} changed={} frames={} gen_reads={} wrote={} -> {} :: switch={} ::",
        NS, k, changed, frames, reads, if wrote.is_ok() { "ok" } else { "refused" }, if ok { "PASS" } else { "FAIL" }, if ok { "ok" } else { "bad" }
    );
}

// PREFSCLIENTKEYS (rmbp-ledger B779) — TAIL-APPENDED, so no line above moves. `tests settingsmap`'s `gets_other=` leg (the
// line's LAST field): a PrefChanged for a key no poller reads costs ZERO bus GETs across every PrefChanged consumer — the
// Settings drain (`settings::bus_changes`: `from_prefs` only on a shown key, the keyboard / accessibility / appearance
// re-reads only on their own prefix), `appearance::service`, `look::service` and `clock::locale::service` (keyed through
// `prefs_client::changed_take`), and `lock::prefs_pass` (LOCKCHANGED's claim). `system.privacy.remember` (read straight
// from the store where it is used, never over the bus by a pass) is flipped over the bus client and put back, every pass
// run once after each write; `gets_total()` (every `pref_get`) must not move, and the ring must have seen both changes
// (`edges>=2`, so a 0 is a measured 0, not a missed change). Home not writable -> `skip` (the SETTINGSBUS shape).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn prefsclientkeys_leg() -> (bool, alloc::string::String) {
    use crate::prefs::key;
    use crate::prefs_client::{changed_mark, gets_total, last_changed, pref_set, PrefValue};
    if crate::fs::vfs::home_unwritable(&crate::shell::vfs_mount_table(), &crate::prefs::path()).is_some() {
        serial_println!("[prefsbus] keys leg skipped: the home is not writable (a set would be refused)");
        return (true, alloc::string::String::from("skip"));
    }
    prefchanged_passes(); // settle: every earlier change is behind each pass's mark
    let was = match crate::prefs_client::pref_get(crate::prefs::NS, key::PRIVACY_REMEMBER) { Ok(v) => v.as_bool().unwrap_or(true), Err(e) if e == una_abi::ENOENT => true, Err(e) => { serial_println!("[prefsbus] keys leg skipped: the key's GET failed errno={} (the fixture never writes a value it did not read)", e); return (true, alloc::string::String::from("skip")); } }; // CRAWLE (LEDGER SR173): unset = the default; a FAILED GET is `skip`, never a flip of the person's key. LINE-NEUTRAL, code first
    let (g0, m0) = (gets_total(), changed_mark());
    let a = pref_set(crate::prefs::NS, key::PRIVACY_REMEMBER, PrefValue::Bool(!was)).is_ok();
    prefchanged_passes();
    let b = pref_set(crate::prefs::NS, key::PRIVACY_REMEMBER, PrefValue::Bool(was)).is_ok();
    prefchanged_passes();
    let other = gets_total().wrapping_sub(g0);
    let edges = changed_mark().wrapping_sub(m0);
    let ok = a && b && edges >= 2 && other == 0;
    serial_println!(
        "[prefsbus] keys leg other_key=system.{} sets={}/{} edges={} gets={} last={} via={} -> {}",
        key::PRIVACY_REMEMBER, a as u8, b as u8, edges, other,
        last_changed().map_or(alloc::string::String::from("-"), |(_, s)| s), crate::prefs_client::last_via(), if ok { "ok" } else { "bad" }
    );
    (ok, alloc::format!("{}", other))
}

/// PREFSCLIENTKEYS (B779): one run of every PrefChanged consumer's pass (the leg above).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn prefchanged_passes() {
    let _ = crate::video::settings::bus_changes();
    crate::video::appearance::service();
    crate::video::look::service();
    let _ = crate::clock::locale::service();
    #[cfg(feature = "login")]
    {
        let _ = crate::video::lock::prefs_pass();
    }
}

// ---- WCPAR2 (rmbp-ledger B770) M3 — `tests wcpar`: the compose fan-out against the serial loop ----------------

/// `tests wcpar`: 60 frames of 8 overlapping windows composed back-to-front into a heap back buffer, once
/// serially (N=1, `FrameBuffer::blit` per row — the CPU blitter's own fallback loop) and once through
/// `wcpar::par_blit_bands` (N = online APs, max 3; the presenter composes alongside the pool and joins).
/// Every frame's two buffers are compared and the last is CRC'd; then one frame with an injected late
/// worker band (`inject_stall_once`, past the 2 ms deadline) must still be pixel-identical. Heap
/// buffers only — the panel is never touched. Wire:
/// `[wc] par bands=<n> compose_us=<n> serial_us=<n> crc_eq=<0|1> frames=60 windows=8 late=<n> busy=<n> stall_late=<n> stall_eq=<0|1> crc=<hex>`
/// `:: WCPAR: bands=<n> crc_eq=<0|1> speedup=<a.bc> bar=1.50 -> PASS|FAIL|SKIP ::`
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc", feature = "facet"))]
fn wcpar_selftest() {
    use crate::video::wcpar;
    const BW: usize = 1280;
    const BH: usize = 800;
    const WW: usize = 480;
    const WH: usize = 360;
    const NWIN: usize = 8;
    const FRAMES: usize = 60;
    const BPP: usize = 4;
    let n = wcpar::workers().min(3);
    if n < 2 {
        serial_println!(":: WCPAR: bands={} crc_eq=0 speedup=0.00 bar=1.50 -> SKIP :: reason=workers={} (no APs to fan out to)", n, wcpar::workers());
        return;
    }
    let heap = |len: usize| -> Option<alloc::vec::Vec<u8>> {
        let mut v = alloc::vec::Vec::new();
        v.try_reserve_exact(len).ok()?;
        v.resize(len, 0);
        Some(v)
    };
    let (Some(b1), Some(b3)) = (heap(BW * BH * BPP), heap(BW * BH * BPP)) else {
        serial_println!(":: WCPAR: bands={} crc_eq=0 speedup=0.00 bar=1.50 -> SKIP :: reason=heap-short", n);
        return;
    };
    let mut wins: alloc::vec::Vec<alloc::vec::Vec<u8>> = alloc::vec::Vec::new();
    for k in 0..NWIN {
        let Some(mut s) = heap(WW * WH * BPP) else {
            serial_println!(":: WCPAR: bands={} crc_eq=0 speedup=0.00 bar=1.50 -> SKIP :: reason=heap-short", n);
            return;
        };
        for y in 0..WH {
            for x in 0..WW {
                let o = (y * WW + x) * BPP;
                s[o] = (x * 3 + k * 29) as u8;
                s[o + 1] = (y * 5 ^ (k * 47)) as u8;
                s[o + 2] = (x ^ y ^ (k * 13)) as u8;
                s[o + 3] = 0xFF;
            }
        }
        wins.push(s);
    }
    let mk = |v: &alloc::vec::Vec<u8>| {
        let mut f = crate::video::FrameBuffer::new();
        f.init(v.as_ptr() as usize, v.len(), unaos_boot_info::FrameBufferInfo { width: BW, height: BH, stride: BW, bytes_per_pixel: BPP, pixel_format: unaos_boot_info::PixelFormat::Bgr });
        f
    };
    let (f1, f3) = (mk(&b1), mk(&b3));
    let (rb, pitch) = (WW * BPP, BW * BPP);
    // Window k's origin on frame `fr`: a cascade that slides a little each frame (always inside the buffer).
    let origin = |k: usize, fr: usize| (40 + k * 96 + (fr % 8) * 4, 30 + k * 52 + (fr % 5) * 3);
    let clear = |f: &crate::video::FrameBuffer, fr: usize| {
        let row = [(fr as u8) ^ 0x5A; BW * BPP];
        for y in 0..BH {
            f.blit(y * pitch, &row);
        }
    };
    // One frame, serial: the CPU blitter's per-row loop.
    let serial = |fr: usize| {
        for (k, s) in wins.iter().enumerate() {
            let (x, y) = origin(k, fr);
            let d0 = (y * BW + x) * BPP;
            for r in 0..WH {
                f1.blit(d0 + r * pitch, &s[r * rb..(r + 1) * rb]);
            }
        }
    };
    // One frame, fanned out: each window's rows in `n` bands; a refused job (pool busy) runs serially.
    let fanned = |fr: usize, late: &mut u32, busy: &mut u32| {
        for (k, s) in wins.iter().enumerate() {
            let (x, y) = origin(k, fr);
            let d0 = (y * BW + x) * BPP;
            match wcpar::par_blit_bands(&f3, s, rb, d0, pitch, WH, n) {
                Some(l) => *late += l,
                None => {
                    *busy += 1;
                    for r in 0..WH {
                        f3.blit(d0 + r * pitch, &s[r * rb..(r + 1) * rb]);
                    }
                }
            }
        }
    };
    let (mut serial_cyc, mut par_cyc, mut late, mut busy) = (0u64, 0u64, 0u32, 0u32);
    let mut eq = true;
    for fr in 0..FRAMES {
        clear(&f1, fr);
        clear(&f3, fr);
        let t0 = crate::arch::now_cycles();
        serial(fr);
        let t1 = crate::arch::now_cycles();
        fanned(fr, &mut late, &mut busy);
        let t2 = crate::arch::now_cycles();
        serial_cyc += t1.saturating_sub(t0);
        par_cyc += t2.saturating_sub(t1);
        eq &= b1 == b3;
    }
    let (c1, c3) = (crate::hash::crc32(&b1), crate::hash::crc32(&b3));
    let crc_eq = eq && c1 == c3;
    // The deadline leg: a worker band stalls 5 ms (past the 2 ms deadline); the presenter takes it late.
    let fr = FRAMES;
    clear(&f1, fr);
    clear(&f3, fr);
    serial(fr);
    wcpar::inject_stall_once(5_000);
    let (mut stall_late, mut stall_busy) = (0u32, 0u32);
    fanned(fr, &mut stall_late, &mut stall_busy);
    wcpar::inject_stall_once(0);
    let stall_eq = b1 == b3 && crate::hash::crc32(&b1) == crate::hash::crc32(&b3);
    let serial_us = crate::video::blitter::cycles_to_us(serial_cyc) / FRAMES as u64;
    let compose_us = crate::video::blitter::cycles_to_us(par_cyc) / FRAMES as u64;
    let x100 = if compose_us == 0 { 0 } else { serial_us * 100 / compose_us };
    serial_println!(
        "[wc] par bands={} compose_us={} serial_us={} crc_eq={} frames={} windows={} late={} busy={} stall_late={} stall_eq={} crc={:#010x}",
        n, compose_us, serial_us, crc_eq as u8, FRAMES, NWIN, late, busy + stall_busy, stall_late, stall_eq as u8, c3
    );
    let pass = crc_eq && stall_eq;
    serial_println!(
        ":: WCPAR: bands={} crc_eq={} speedup={}.{:02} bar=1.50 -> {} ::",
        n, crc_eq as u8, x100 / 100, x100 % 100, if pass { "PASS" } else { "FAIL" }
    );
}
// ══════════════ EHCIWAKE (rmbp-ledger B776) — `tests usbsuspend`'s EHCI half — TAIL-APPENDED ══════════════

/// EHCIWAKE (B776): the EHCI half of `tests usbsuspend` — its census after the resume and, when no xHCI keyboard
/// answered (`probe`), the EHCI keyboard's GET_REPORT (`ok` = a report of bytes and an un-halted interrupt QH).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc", feature = "ehcihid"))]
fn usbsuspend_ehci_leg(probe: bool) -> (crate::drivers::ehci::suspend::Census, &'static str) {
    use crate::drivers::ehci::suspend as es;
    let mut got = None;
    let t0 = crate::arch::ms();
    while probe && got.is_none() && crate::arch::ms().wrapping_sub(t0) < 1_000 {
        match es::kbd_report_probe() { Some(p) => got = Some(p), None => crate::arch::sched::sleep_ms(5) }
    }
    let word = match got {
        None if probe => "busy",
        None => "none",
        Some(None) => "none",
        Some(Some((len, ep_ok))) => if len > 0 && ep_ok { "ok" } else { "fail" },
    };
    (es::census(), word)
}

/// EHCIWAKE (B776): no EHCI driver on this image — the EHCI half is empty.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc", not(feature = "ehcihid")))]
fn usbsuspend_ehci_leg(_probe: bool) -> (UsbsuspendNoEhci, &'static str) {
    (UsbsuspendNoEhci::default(), "none")
}
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc", not(feature = "ehcihid")))]
#[derive(Default)]
struct UsbsuspendNoEhci { ports: u32, suspended: u32, kept: u32, resumed: u32, wake_armed: u32 }
// SINKALLDOCS (rmbp-ledger B781; BP-audio-17) — TAIL-APPENDED: `tests sinkall`'s pause leg. A Player pause holds its
// source out of the mix (`mixer::pause`); what already left the mixer still plays, so the Player's position (the A/V
// clock, `hda::play::position_ms`) keeps moving until the sink's queue drains. pause_ms = the last move of that position
// after `pause(true)` (a position still for 150 ms is stopped). On HDA the queue is `hda::play::HIGH_FRAMES` (224 ms);
// PASS in `sinkall_selftest` needs <= SINKALL_PAUSE_MS. Wire: `[sinkall] leg pause sink= steady= stopped= pause_ms= closed=`.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
const SINKALL_PAUSE_MS: u64 = 250;

/// The Player's pause latency on `sink` in ms (u64::MAX when it could not be measured).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_pause(sink: &str) -> u64 {
    use crate::audio::mixer;
    use crate::drivers::hda::play;
    use core::cell::Cell;
    const LONG: u64 = 48_000 * 6;
    if play::fixture_source("player").is_err() {
        return u64::MAX;
    }
    let steady = play::pump_until(3_000, || {
        let _ = play::fixture_feed(LONG);
        let p = play::position_ms();
        p != u64::MAX && p >= 300
    });
    let mut p0 = play::position_ms();
    for _ in 0..1_000 {
        if p0 != u64::MAX {
            break;
        }
        p0 = play::position_ms();
    }
    let t0 = crate::arch::ms();
    let _ = play::pause(true);
    let last = Cell::new((p0, t0));
    let stopped = steady
        && play::pump_until(2_000, || {
            let now = crate::arch::ms();
            let p = play::position_ms();
            if p != u64::MAX && p != last.get().0 {
                last.set((p, now));
            }
            now.saturating_sub(last.get().1) >= 150
        });
    let pause_ms = if stopped { last.get().1.saturating_sub(t0) } else { u64::MAX };
    play::stop();
    let closed = play::pump_until(500, || mixer::sources() == 0);
    serial_println!(
        "[sinkall] leg pause sink={} steady={} stopped={} pause_ms={} closed={}",
        sink, steady as u8, stopped as u8, pause_ms as i64, closed as u8
    );
    if closed { pause_ms } else { u64::MAX }
}

/// SCROLLPANES (rmbp-ledger B777) — `tests scrollview`'s pane legs, printed at the END of its pass line:
/// `tree=ok icons=ok columns=ok tabs=ok types=ok overlay=ok sidebar=ok cols=ok`. Quarry's tree, Icons grid and Columns focus pane walked
/// through their live step bodies against a full paint; Settings' tab column and File Types rows stepped through
/// their models against an independent clamp; the overlay gate (the context menu or a mark stops the blit); Quarry's
/// sidebar walked through its live step body (SIDEBARWHEEL rmbp-ledger B792); every non-focus Columns column walked through
/// its live step body (MILLERWHEEL rmbp-ledger B798, `cols_steps` = the sum over the columns). `(words, all ok)`.
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn scrollpanes_legs() -> (alloc::string::String, bool) {
    let word = |r: &Result<usize, &'static str>| -> alloc::string::String { match r { Ok(_) => alloc::string::String::from("ok"), Err(e) => alloc::format!("FAIL({})", e) } };
    #[cfg(feature = "quarry")]
    let [tree, icons, columns, overlay, sidebar, cols] = crate::video::quarry::live::scrollpanes_probe();
    #[cfg(not(feature = "quarry"))]
    let [tree, icons, columns, overlay, sidebar, cols]: [Result<usize, &'static str>; 6] = [Err("no-quarry"); 6];
    let [tabs, types] = crate::video::settings::scrollpanes_probe();
    let all = [&tree, &icons, &columns, &tabs, &types, &overlay, &sidebar, &cols];
    serial_println!(
        "[scrollview] panes tree_steps={} icons_steps={} columns_steps={} tabs_steps={} types_steps={} overlay_legs={} sidebar_steps={} cols_steps={}",
        tree.as_ref().copied().unwrap_or(0), icons.as_ref().copied().unwrap_or(0), columns.as_ref().copied().unwrap_or(0), tabs.as_ref().copied().unwrap_or(0), types.as_ref().copied().unwrap_or(0), overlay.as_ref().copied().unwrap_or(0), sidebar.as_ref().copied().unwrap_or(0), cols.as_ref().copied().unwrap_or(0)
    );
    (
        alloc::format!("tree={} icons={} columns={} tabs={} types={} overlay={} sidebar={} cols={}", word(&tree), word(&icons), word(&columns), word(&tabs), word(&types), word(&overlay), word(&sidebar), word(&cols)),
        all.iter().all(|r| r.is_ok()),
    )
}

// MIXERPUMP (rmbp-ledger B787; BP-audio-16) — TAIL-APPENDED: `tests sinkall`'s slider leg. A level set through
// `status::set_volume` (Settings' slider, the status menu, Softy, AVRCP) reaches its carrier by the master stage's one
// hand-off (`mixer::level_changed`): HDA = the codec amp verb `set_volume` issues (`by=amp`); USB = the device's Feature
// Unit SET_CUR (`uac::volume_state`'s count moves, `by=set-cur`); a USB device with no Volume control is the mixer's
// (`by=mixer`, no device write, not measured). slider_ms = ms from the call to the carrier holding the level, with a
// Player source playing on that sink; the worst over the live sinks; PASS in `sinkall_selftest` needs <= SINKALL_SLIDER_MS
// (one 10 ms period + 10 ms). Wire: `[sinkall] leg slider sink= by=<amp|set-cur|mixer> level=<a>-><b> slider_ms= closed=`.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
const SINKALL_SLIDER_MS: u64 = crate::audio::mixer::PERIOD_US as u64 / 1000 + 10;

/// The worst slider latency over the sinks whose alert leg was `ok` (u64::MAX when a carrier did not take the level,
/// a source did not close, or nothing was measured).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_slider(hda: &str, usb: &str) -> u64 {
    let (mut worst, mut measured) = (0u64, false);
    for (s, k, st) in [("hda", 0usize, hda), ("usb", 1usize, usb)] {
        if st != "ok" || crate::audio::sink(s).is_err() {
            continue;
        }
        if let Some(ms) = sinkall_slider_on(s, k) {
            worst = worst.max(ms);
            measured = true;
        }
    }
    if measured { worst } else { u64::MAX }
}

/// (SET_CUR rounds the USB device took, its Feature Unit carries the volume now); (0, false) without `uac`.
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_usb_sets() -> (u32, bool) {
    #[cfg(feature = "uac")]
    return (crate::drivers::usb::uac::volume_state().0, crate::drivers::usb::uac::volume_on_device());
    #[cfg(not(feature = "uac"))]
    (0, false)
}

/// One sink's slider leg: Some(ms) measured (u64::MAX when the carrier did not take it or the source did not close),
/// None when the sink's level is the mixer's own (no device write to measure).
#[cfg(all(target_arch = "x86_64", feature = "hda-tone"))]
fn sinkall_slider_on(sink: &str, k: usize) -> Option<u64> {
    use crate::audio::mixer;
    use crate::drivers::hda::play;
    use crate::video::status;
    const LONG: u64 = 48_000 * 4;
    if play::fixture_source("player").is_err() {
        return Some(u64::MAX);
    }
    let steady = play::pump_until(1_000, || play::fixture_feed(LONG) >= 9_600 && mixer::output() == Some(sink));
    let (l0, m0) = status::volume();
    let lv = if l0 >= 16 { 15 } else { l0 + 1 };
    let by = if k == 0 { "amp" } else if sinkall_usb_sets().1 { "set-cur" } else { "mixer" };
    let s0 = sinkall_usb_sets().0;
    let t0 = crate::arch::ms();
    let amp = status::set_volume(lv, m0);
    let held = steady
        && match by {
            "amp" => amp,
            "set-cur" => play::pump_until(1_000, || {
                let _ = play::fixture_feed(LONG);
                sinkall_usb_sets().0 != s0
            }),
            _ => false,
        };
    let ms = if held { crate::arch::ms().saturating_sub(t0) } else { u64::MAX };
    let _ = status::set_volume(l0, m0);
    play::stop();
    let closed = play::pump_until(500, || mixer::sources() == 0);
    serial_println!(
        "[sinkall] leg slider sink={} by={} level={}->{} slider_ms={} closed={}",
        sink, by, l0, lv, if by == "mixer" { -1 } else { ms as i64 }, closed as u8
    );
    if by == "mixer" {
        return if closed { None } else { Some(u64::MAX) };
    }
    Some(if closed { ms } else { u64::MAX })
}

// ══════════════ XHCIHOLDDCI (rmbp-ledger B795) — `tests usbsuspend`'s held leg — TAIL-APPENDED ══════════════

/// XHCIHOLDDCI (B795, BP-power-7): while the ports are SUSPENDED, queue the xHCI keyboard's GET_REPORT over EP0
/// (`held_report_queue`: its doorbell is held and remembered). `Err` = the leg's word (`phase`, `none`, `unheld`, `busy`).
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))]
fn usbsuspend_held_queue(in_u3: bool) -> Result<u64, &'static str> {
    if !in_u3 {
        return Err("phase");
    }
    let t0 = crate::arch::ms();
    loop {
        match crate::drivers::xhci::claim() {
            Ok(mut x) => return x.held_report_queue(),
            Err(_) if crate::arch::ms().wrapping_sub(t0) < 1_000 => crate::arch::sched::sleep_ms(5),
            Err(_) => return Err("busy"),
        }
    }
}

/// XHCIHOLDDCI (B795): after UP, the held GET_REPORT's completion (`held_report_collect`): `ok` = a report of bytes,
/// `fail` = an error or no bytes, `lost` = its pending was replaced; a queue refusal passes its own word through.
#[cfg(all(target_arch = "x86_64", feature = "lidsleep", feature = "wc"))]
fn usbsuspend_held_collect(queued: Result<u64, &'static str>) -> &'static str {
    let phys = match queued {
        Ok(p) => p,
        Err(w) => return w,
    };
    let t0 = crate::arch::ms();
    loop {
        match crate::drivers::xhci::claim() {
            Ok(mut x) => {
                return match x.held_report_collect(phys) {
                    Some((cc, len)) if matches!(cc, 1 | 13) && len > 0 => "ok",
                    Some(_) => "fail",
                    None => "lost",
                };
            }
            Err(_) if crate::arch::ms().wrapping_sub(t0) < 1_000 => crate::arch::sched::sleep_ms(5),
            Err(_) => return "busy",
        }
    }
}

// SUBSCRIBEKEEP (rmbp-ledger B796, BP-security-18) — TAIL-APPENDED: `tests settingsmap`'s ring leg. The last-changed ring is
// `prefs_client`'s own (on at its first reader, sticky), so a session switch reaches the look and the locale with NO kernel
// subscriber at all. The leg holds every subscriber off (`prefs_client::subscriber_hold`: the lock's, Settings' and this
// fixture's `subscribe()` are no-ops), runs one settle pass of the consumers, and reads `on=` BEFORE the fixture touches the
// ring (a consumer, not the fixture, switched it on). The switch is `prefs::reload_session` against a COPY of the tree with
// `system.look` and `system.locale.first_day` flipped (the store is never written: nothing to put back, no glass switch), so
// exactly those two keys' PrefChanged are posted (`changed=2`). `look=` / `locale=` are the ring's answer for each consumer's
// own keys (`changed_since` from the fixture's mark); with a user session open the consumers' own passes run and their bus
// GETs are counted (look >= 1, locale >= 3 — the re-read the switch caused); no session -> `na` (both read nothing without
// one). The re-read is counted by its GETs and not by its `via=` word: a switch's re-read prints `via=bus` (look.rs:118,
// locale.rs:405); `via=login` is only each consumer's first-ever read (`SEEN_CHANGES == u32::MAX`). `queued=0`: with the
// subscribers held, the Settings queue took no frame (`CHANGED_N` did not move).
// `:: SETTINGSMAP: ring leg subscriber=held changed=<n> queued=<n> on=<0|1> look=<hit|miss> locale=<hit|miss> look_gets=<n|na> locale_gets=<n|na> -> PASS|FAIL :: ring=<owned|lost> ::`
#[cfg(all(feature = "witness", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
fn settingsmap_ring_leg() {
    use crate::prefs::{key, PrefValue, NS};
    use crate::prefs_client as pc;
    use prefs_core::appearance as ap;
    fn settle() {
        let _ = crate::video::settings::bus_changes();
        crate::video::look::service();
        let _ = crate::clock::locale::service();
        #[cfg(feature = "login")]
        let _ = crate::video::lock::prefs_pass();
    }
    let mut wb = [0u8; crate::prefs::WHO_BUF];
    let session = crate::prefs::user_name_in(&mut wb).is_some();
    let _ = pc::subscriber_hold(true);
    settle(); // every earlier change behind each consumer's mark
    let on = pc::ring_on(); // read before the fixture's own ring read below
    let m0 = pc::changed_mark();
    let q0 = pc::CHANGED_N.load(Ordering::Acquire);
    let mut old = crate::prefs::snapshot();
    let lv = if old.get(NS, ap::LOOK_KEY).and_then(|v| v.as_str()) == Some(ap::LOOKS[1]) { ap::LOOKS[0] } else { ap::LOOKS[1] };
    let fv = if old.get(NS, key::LOCALE_FIRST_DAY).and_then(|v| v.as_str()) == Some("sun") { "sat" } else { "sun" };
    let staged = old.set(NS, ap::LOOK_KEY, PrefValue::Str(alloc::string::String::from(lv))).is_ok()
        && old.set(NS, key::LOCALE_FIRST_DAY, PrefValue::Str(alloc::string::String::from(fv))).is_ok();
    let changed = if staged { crate::prefs::reload_session(&old) } else { 0 };
    let (_, look_hit) = pc::changed_since(m0, |ns, k| ns == NS && k == ap::LOOK_KEY);
    let (_, locale_hit) = pc::changed_since(m0, |ns, k| ns == NS && k.starts_with("locale."));
    let (look_gets, locale_gets) = if session {
        let g0 = pc::gets_total();
        crate::video::look::service();
        let g1 = pc::gets_total();
        let _ = crate::clock::locale::service();
        let g2 = pc::gets_total();
        (Some(g1.wrapping_sub(g0)), Some(g2.wrapping_sub(g1)))
    } else {
        (None, None)
    };
    #[cfg(feature = "login")]
    let _ = crate::video::lock::prefs_pass();
    let queued = pc::CHANGED_N.load(Ordering::Acquire).wrapping_sub(q0);
    let _ = pc::subscriber_hold(false);
    let need_on = cfg!(feature = "login") || session;
    let ok = staged && changed == 2 && queued == 0 && look_hit && locale_hit && (on || !need_on)
        && look_gets.map_or(true, |g| g >= 1) && locale_gets.map_or(true, |g| g >= 3);
    let na = |g: Option<u32>| g.map_or(alloc::string::String::from("na"), |g| format!("{}", g));
    serial_println!(
        ":: SETTINGSMAP: ring leg subscriber=held changed={} queued={} on={} look={} locale={} look_gets={} locale_gets={} -> {} :: ring={} ::",
        changed, queued, on as u8, if look_hit { "hit" } else { "miss" }, if locale_hit { "hit" } else { "miss" },
        na(look_gets), na(locale_gets), if ok { "PASS" } else { "FAIL" }, if ok { "owned" } else { "lost" }
    );
}
