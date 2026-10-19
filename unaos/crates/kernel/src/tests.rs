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
    ensure_bootverdicts(); crate::boot::ensure_tests(); crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } register("tzlocale", tzlocale_selftest); register("uac1", uac1_fixture); register("mtpphone", mtpphone_selftest); } ensure_attr(); #[cfg(target_arch = "x86_64")] crate::arch::clockcore::ensure_tests(); #[cfg(all(feature = "midden", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::middenapp::ensure(); #[cfg(all(feature = "aether", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::aether::ensure(); #[cfg(all(feature = "una", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::unaapp::ensure(); #[cfg(feature = "sonde")] crate::sonde::ensure(); #[cfg(all(feature = "uvc", target_arch = "aarch64"))] crate::drivers::uvc::ensure_tests(); crate::pointerfeel::ensure(); #[cfg(feature = "installdemo")] crate::install::uninstall::ensure(); #[cfg(all(feature = "stria", any(target_arch = "x86_64", feature = "aarch64_el0")))] crate::striaapp::ensure(); #[cfg(feature = "uvc")] crate::drivers::xhci::uvc::ensure_tests(); #[cfg(all(target_arch = "aarch64", feature = "v3d"))] crate::arch::aarch64::v3d::ensure_tests(); #[cfg(target_arch = "aarch64")] crate::arch::aarch64::mmu_wxn::ensure(); // PIEMPTYBIN (PI24): `tests v3d`, the [v3d105] emptybin rung (LINE-NEUTRAL append, before this comment). STRIAMETAL (SR139): `tests stria` (LINE-NEUTRAL append, before this comment). ARMSONDE (SR101): `tests sonde` arch-neutral, and `tests uvc` registered eagerly on aarch64 (LINE-NEUTRAL append, before this comment). CLOCKCORE (B397): `tests clock`. HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`. MIDDENTWIN (SR75): `tests midden` (LINE-NEUTRAL append, before this comment). UNAMETAL (SR90): `tests una` (LINE-NEUTRAL append, before this comment). REALSENSE (SR92): `tests sonde` (LINE-NEUTRAL append, before this comment). // CLOCKCORE (B397): `tests clock`. HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`. MIDDENTWIN (SR75): `tests midden` (LINE-NEUTRAL append, before this comment). UNAMETAL (SR90): `tests una` (LINE-NEUTRAL append, before this comment). POINTERFEEL (B535): `tests pointerfeel` (LINE-NEUTRAL append, before this comment).
    crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } } ensure_attr(); ensure_brightfloor(); ensure_gen7(); ensure_wifi(); #[cfg(target_arch = "x86_64")] register("kelfhash", kelfhash_selftest); #[cfg(all(feature = "smolnet", target_arch = "x86_64"))] { static SK: AtomicBool = AtomicBool::new(false); if !SK.swap(true, Ordering::AcqRel) { register("sock", crate::drivers::e1000::sock_selftest); } register("smartline", smartline_selftest); register("notelemetry", notelemetry_selftest); #[cfg(all(feature = "smolnet", target_arch = "x86_64"))] ensure_idlenet(); } /* INPUTSTALL2 M4 (B388, R80): `tests sock` arms the SOCK ladder; it never runs at boot */ // HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`.
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
        if !DONE2.swap(true, Ordering::AcqRel) { #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("dongleboot", crate::video::desktopbuild::dongleboot_selftest); /* DONGLEBOOT (B658): the prefs stat, the AHCI bound, the render task on a held disk, the starter release */ register("settings", crate::video::settings::selftest_all); register("appearance", crate::video::appearance::selftest); /* APPEARANCE (B408) */ register("softy", crate::video::softy::selftest); /* SOFTY (B629): the look switch, the taskbar, Start, the caption buttons */ register("winkey", crate::video::softy::startmenu::winkey_selftest); /* WINKEY (B665): the Windows-key tap toggles Start once (counter path), Esc through the door closes it */ register("trackpad", crate::video::settings::selftest_trackpad); /* TRACKPADPANE (B412) */ register("prefs", crate::prefs::selftest); register("prefscap", crate::prefs::prefscap_selftest); register("appmenu", crate::video::sysmenu::selftest); /* APPMENU2 (B393) */ #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("windowlist", crate::video::winlist::selftest); /* WINDOWLIST (R75) */ register("glasskit", crate::video::draw::selftest); /* GLASSKIT (B628): the drawing kit's invariants and per-op cost (LINE-NEUTRAL append) */ register("desktopcraft", crate::video::wm::desktop::selftest); /* DESKTOPCRAFT (B631): the Una desktop painted offscreen at the panel size, its invariants and the frame cost (LINE-NEUTRAL append) */ register("starfield", crate::video::wm::desktop::starfield_selftest); /* STARFIELD (B661, R128): the generated scene offscreen — gen time, the lens on three stars, the zircon, the beam, the shimmer's cost and occlusion (LINE-NEUTRAL append) */ register("quarryhover", crate::video::quarry::live::hover_selftest); /* QUARRYHOVER (B647): Quarry's hover on the service pass and the wheel into every pane (LINE-NEUTRAL append) */  register("motion", crate::video::draw::motion::selftest); register("a11y", crate::video::settings::selftest_a11y); /* A11YPANE (B655): the Accessibility pane, its four keys and the reduce-motion snap (LINE-NEUTRAL append) */ #[cfg(feature = "facet")] register("startscene", crate::video::wallpaper::startscene_selftest); /* STARTSCENE (B662, R128): one picture from the splash to the desktop — gen_count=1, the splash scene, the hold shimmer, the GOP stars, the login backdrop (LINE-NEUTRAL append) */ register("contrast", crate::video::theme::contrast_selftest); /* CONTRAST (B664): the increase-contrast set on both looks, >= 7:1 text-on-surface, two epoch bumps (LINE-NEUTRAL append) */ register("settingsmap", crate::video::settings::selftest_map); /* SETTINGSMAP (B673): every prefs::key on one Settings row, every row on a key, each label reached by <= 2 keys + Return (LINE-NEUTRAL append) */ register("toastcap", crate::video::notify::toastcap_selftest); /* TOASTCAP (B674): DESIGN-LANGUAGE §3.6/§3.8 — three toasts at most, the rest wait, 5 s paused on hover; one primary per dialog, Return/Escape on every view (LINE-NEUTRAL append) */ #[cfg(all(target_arch = "x86_64", feature = "facet"))] register("shimmeridle", crate::video::wm::desktop::shimmeridle_selftest); /* SHIMMERIDLE (B663): the idle desktop's twinkle from the device-service pass — 4..=7 ticks in 200 ms, each under 800 us, none under reduce motion, no gate steal (LINE-NEUTRAL append) */ register("memline", crate::video::desktopbuild::memline_selftest); /* MEMLINE (B677): the desktop's memory line at `[desktop] built` and the Log Out round trip's high-water (LINE-NEUTRAL append) */ register("reask", crate::fs::apptrust::reask_selftest); /* REASK (B681, R130 law 2): an answered ask is kept in Principia's store — 3 reboots and 90 days ask 0 times, a revoke asks once (LINE-NEUTRAL append) */ #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("a11ytree", crate::video::wm::a11y::selftest); /* A11YTREE (B675, BP-bar-22): the role/name tree from the window table through the verb's own body, unnamed=0, focus = wm's, Settings' ring at every Tab stop (LINE-NEUTRAL append) */ register("keymaps", keymaps_selftest); register("focussteal", focussteal_selftest); register("veilcost", veilcost_selftest); register("aging", aging_selftest); register("a11yfilters", crate::video::settings::selftest_a11yfilters); register("zoom", zoom_selftest); register("perfhud", crate::video::perfhud::selftest); register("undomove", undomove_selftest); #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("raiseverb", raiseverb_selftest); /* RAISEVERB (B722): the raise verb's body twice (unsolicited -> pulse, after the user's verb -> given) and a foreign ask -> EPERM (LINE-NEUTRAL append) */ }
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
        if !DONE.swap(true, Ordering::AcqRel) { register("selfinstall", crate::install::selfinstall::selftest); register("install", crate::install::selfinstall::install_selftest); register("abimage", crate::install::abimage::selftest); #[cfg(feature = "ahciroot")] register("ahciw", crate::install::ahciroot::ahciw_selftest); #[cfg(all(feature = "ahciroot", feature = "instgui"))] register("instgui", crate::video::instgui::install3::selftest); }
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
        if !GROW.swap(true, Ordering::AcqRel) { register("unafsgrow", crate::fs::unafsgrow::selftest); register("diskfull", diskfull_selftest); } // UNAFSGROW (B347) M4: the native module's builds (aarch64 always, x86 under `unafs`)
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
        if !DONE.swap(true, Ordering::AcqRel) { register("lumen", crate::lumen::selftest); register("lumenfast", crate::lumen::lumenfast); register("holocron", crate::keyring::selftest); register("ringlogin", crate::keyring::ringlogin_selftest); #[cfg(all(target_arch = "x86_64", feature = "btc"))] register("btkeyseal", crate::drivers::ehci::bthid::btkeyseal_selftest); #[cfg(any(target_arch = "aarch64", feature = "unafs"))] register("volkey", crate::keyring::volkey_selftest); /* VOLKEY (B684): the volume key slot on a RAM volume (LINE-NEUTRAL append) */ #[cfg(any(target_arch = "aarch64", feature = "unafs"))] register("xts", xts_selftest); /* XTS (B689): the volume cipher on a scratch RAM volume, `tests.rs` tail (LINE-NEUTRAL append) */ register("keyboot", crate::keyring::keyboot_selftest); }
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
        if !DONE.swap(true, Ordering::AcqRel) { register("brightfloor", crate::video::backlight::selftest); register("brightstep", crate::video::backlight::brightstep); register("lidsleep", crate::video::lidsleep::selftest); #[cfg(all(target_arch = "x86_64", feature = "lidsleep"))] register("sleep", crate::video::lidsleep::sleep_selftest); #[cfg(feature = "login")] register("autolock", autolock_selftest); } // AUTOLOCK (B712): `tests autolock` (the file-tail fn; same gate, `login`). SLEEP (B691): `tests sleep` enters and leaves the state (same-line fold, code before the comment)
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
        if !DONE.swap(true, Ordering::AcqRel) { register("fpu", crate::arch::sched::fpu::selftest); register("abielf", abielf_selftest); } // ABIELF (B693): `tests abielf` beside it, same cfg (the file-tail fn). LINE-NEUTRAL fold, code before the comment.
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
/// `:: TZLOCALE: zones=52 dst_edges_ok=8/8 painted_ok=<n>/<n> painters=3 seen=<bits> orphans=0 -> PASS ::`
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
    let ok = zones_ok && dst_ok == 2 * edges.len() && painted_ok == painted.len() && defaults && orphans == 0;
    serial_println!(
        ":: TZLOCALE: zones={} dst_edges_ok={}/{} painted_ok={}/{} painters={} seen={:#05b} orphans={} -> {} :: faults={} defaults={}",
        tz::ZONES.len(), dst_ok, 2 * edges.len(), painted_ok, painted.len(), locale::PAINTERS, locale::painted(), orphans, if ok { "PASS" } else { "FAIL" }, faults, defaults as u8
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
        use crate::fs::apptrust::{net_asks, net_forget, net_gate_for, NET_EPERM};
        use crate::video::dialog::{self, Answer};
        const APP: &str = "NGPROBE";
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
        let was = dialog::headless(true);
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
        let r1 = connect(RUN1);
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
        let persisted = (kept && r6.is_ok() && net_asks() == a2) as u32;

        crate::smolnet::stack_close(sid);
        net_forget(APP);
        dialog::headless(was);
        let pending = -115;
        let ok = asked == 1 && up && r1 == pending && r2 == NET_EPERM && r3 == NET_EPERM && never_kept && denied_pkts == 0
            && revoked && r4 == pending && reasks_always == 1 && r5 != NET_EPERM && allowed == 1 && persisted == 1;
        serial_println!(
            ":: NETGRANT: asked={} denied_pkts={} allowed={} persisted={} -> {} :: r={}/{}/{}/{}/{} never_kept={} revoked={} reask_after_revoke={} flushed={} keys={} tx_frames_in_never_leg={} key={}",
            asked, denied_pkts, allowed, persisted, if ok { "PASS" } else { "FAIL" }, r1, r2, r3, r4, r5, never_kept as u8, revoked as u8, reasks_always, flushed, keys, tx_frames, key
        );
    }
    #[cfg(not(all(feature = "smolnet", feature = "wc", target_arch = "x86_64")))]
    serial_println!(":: NETGRANT: -> SKIP reason=x86-smolnet-wc-only ::");
}

/// UAC1 (rmbp-ledger B706): `tests uac1` — the USB Audio Class 1 descriptor walk on its fixture, then 440 Hz for 1 s
/// through the audio seam to the USB sink (`:: UAC1: enumerated= stream= frames= underruns= -> PASS|SKIP|FAIL ::`).
#[cfg(feature = "uvc")]
fn uac1_fixture() {
    crate::drivers::usb::uac::fixture();
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

/// FOCUSSTEAL (rmbp-ledger B698; BP-video-20, BP-bar-18) — `tests focussteal`: three windows with
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
    let refused = entered && lock::active() && wrong == 1;
    if entered { feed(PW); let _ = crate::fs::users::screen_key(b'\n'); settle(); }
    let reopened = entered && !lock::active() && !login::is_open() && is_una(&mut nb);
    let after = lock::fingerprint();
    let same = before == after;
    let ms = lock::last_open_ms();
    let pass = entered && leaked == 0 && refused && reopened && same && ms.is_some();
    serial_println!(
        ":: AUTOLOCK: entered={} leaked_keys={} reopened={} table_same={} ms={} -> {} :: chord={} wrong={} windows_hidden={}",
        entered as u8, leaked, if reopened { "ok" } else { "FAIL" }, same as u8, ms.map_or(alloc::string::String::from("-"), |m| alloc::format!("{}", m)),
        if pass { "PASS" } else { "FAIL" }, token, wrong, hidden
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
// `:: MTPPHONE: enumerated=1 storages=2 objects=40 read=ok write=ok delete=ok ms=<n> -> PASS ::`
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
    let counts = miss("counts", nst == 2 && nobj == fixture::OBJECTS);
    let ms = crate::arch::ms().saturating_sub(t0);
    serial_println!("[mtp] fixture txns={} walk_txns={} fast_rows={} walk_rows={} whole={} written={}", r.txns, w.txns, fast_rows.len(), walk_rows.len(), whole.len(), data.len());
    let pass = enumerated && counts && read_ok && write_ok && delete_ok && why.is_none();
    let okf = |b: bool| if b { "ok" } else { "fail" };
    if pass {
        serial_println!(":: MTPPHONE: enumerated=1 storages={} objects={} read=ok write=ok delete=ok ms={} -> PASS ::", nst, nobj, ms);
    } else {
        serial_println!(
            ":: MTPPHONE: enumerated={} storages={} objects={} read={} write={} delete={} ms={} -> FAIL why={} ::",
            enumerated as u8, nst, nobj, okf(read_ok), okf(write_ok), okf(delete_ok), ms, why.unwrap_or("-")
        );
    }
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
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
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
