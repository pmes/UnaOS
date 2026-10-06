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
const CAP: usize = 128; // QUIETBOOT2: 80 -> 128, the ~20 boot fixtures B325 moved here. // QUIETBOOT: 48 -> 80, the boot witnesses R80 moved here (flight 19 registered 45).

static TABLE: spin::Mutex<[Option<(&'static str, fn())>; CAP]> = spin::Mutex::new([None; CAP]);
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
pub fn tally(pass: bool) {
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
static CUR: spin::Mutex<&'static str> = spin::Mutex::new("");
static FAILED: spin::Mutex<[Option<&'static str>; 16]> = spin::Mutex::new([None; 16]);

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
        let mut t = TABLE.lock();
        match t.iter_mut().find(|s| s.is_none()) {
            Some(slot) => {
                *slot = Some((name, f));
                DEFERRED.fetch_add(1, Ordering::Relaxed);
            }
            None => serial_println!(":: TESTS: table full (cap={}) — `{}` NOT registered -> FAIL ::", CAP, name),
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
    ensure_shellux();
    if RUNNING.swap(true, Ordering::AcqRel) {
        serial_println!(":: TESTS: already running — refused ::");
        return 0;
    }
    let (p0, f0) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    *FAILED.lock() = [None; 16]; skip_reset(); RESULTS.lock().clear(); // QUIETBOOT3 (B352): same-line fold.
    let mut ran = 0usize;
    let mut i = 0usize;
    loop {
        // Copy the entry out so the table lock is NOT held across the fixture (fixtures print, spin, and may register nothing).
        let ent = { let t = TABLE.lock(); if i >= CAP { None } else { t[i] } };
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
    serial_println!(":: TESTS: ran={} pass={} fail={} failed=[{}] skipped=[{}] ::", ran, p, f, names, skipped_names());
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
    ensure_ring3win(); ensure_ring3abi(); ensure_elfbss(); // RING3WIN, RING3ABI2 (merge12 fold)
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); crate::fs::filetype::ensure_tests(); // FILETYPE (B307): `tests filetype`.
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); ensure_usbnet(); ensure_kvblank8();
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] crate::video::shotmask::ensure_tests(); crate::video::blitter::ensure_tests(); crate::prof::ensure_tests(); crate::video::text::ensure_tests(); crate::video::metrics::ensure_tests(); // GLASSEYES (B343): `tests shot`. KCOMP (B321): `tests blitter`. PROFILE (B331): `tests prof`.
    ensure_shellux(); ensure_selfinstall(); ensure_unafsx86(); ensure_usbnet(); #[cfg(target_arch = "x86_64")] crate::execname::ensure(); #[cfg(any(feature = "aarch64_el0", target_arch = "x86_64"))] crate::prefs_client::ensure_tests(); #[cfg(feature = "selfdiag")] crate::selfdiag::ensure(); // EXECNAME (B322): `tests exec`. SETTINGSBUS (B337): `tests settingsbus`. SELFDIAG (B324): `tests selfdiag`. (merge12 fold: one line)
    if args.first().copied() == Some("list") {
        let t = TABLE.lock();
        for e in t.iter().flatten() { console.println(e.0); }
        console.println(&format!("{} deferred, {} ran at boot", deferred_count(), AT_BOOT.load(Ordering::Relaxed)));
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
    crate::boot::ensure_tests(); crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } } ensure_attr(); #[cfg(target_arch = "x86_64")] crate::arch::clockcore::ensure_tests(); // CLOCKCORE (B397): `tests clock`. HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`.
    crate::help::ensure(); #[cfg(all(feature = "busreg", any(feature = "aarch64_el0", target_arch = "x86_64")))] { static B3: AtomicBool = AtomicBool::new(false); if !B3.swap(true, Ordering::AcqRel) { register("bandy3", crate::arch::syscall::bandy3_selftest); } } ensure_attr(); ensure_brightfloor(); ensure_gen7(); ensure_wifi(); // HELPVERB: `tests helpdoc` ATTRSURF: `tests attr`.
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
        if !DONE2.swap(true, Ordering::AcqRel) { register("settings", crate::video::settings::selftest_all); register("prefs", crate::prefs::selftest); #[cfg(all(target_arch = "x86_64", feature = "wc"))] register("windowlist", crate::video::winlist::selftest); /* WINDOWLIST (R75) */ }
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
        if !DONE.swap(true, Ordering::AcqRel) { register("selfinstall", crate::install::selfinstall::selftest); register("install", crate::install::selfinstall::install_selftest); #[cfg(feature = "ahciroot")] register("ahciw", crate::install::ahciroot::ahciw_selftest); #[cfg(all(feature = "ahciroot", feature = "instgui"))] register("instgui", crate::video::instgui::install3::selftest); }
    }
}

/// UNAFSX86 M4: register `unafs` (the root-volume seam fixture) exactly once, on any build carrying the
/// native module under the `unafs` feature; on a FAT root the fixture prints SKIP, never a pin.
fn ensure_unafsx86() {
    #[cfg(feature = "unafs")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("unafs", crate::fs::unafs::unafsx86_selftest); register("boot80", crate::fs::bootstep::boot80_selftest); }
    }
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        static GROW: AtomicBool = AtomicBool::new(false);
        if !GROW.swap(true, Ordering::AcqRel) { register("unafsgrow", crate::fs::unafsgrow::selftest); } // UNAFSGROW (B347) M4: the native module's builds (aarch64 always, x86 under `unafs`)
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
        if !DONE.swap(true, Ordering::AcqRel) { register("lumen", crate::lumen::selftest); register("holocron", crate::keyring::selftest); }
    }
}

/// NETRING3 (B306): register `tests net` (M1 entropy + M2 resolve) exactly once under `netring3`.
fn ensure_netring3() {
    #[cfg(feature = "netring3")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("net", crate::netring3::selftest); register("nethang", crate::netring3::nethang_selftest); } // NETHANG: `tests nethang` (code first)
    }
}

/// RING3WIN (B316): register `tests ring3win` exactly once on x86 (the ELF window is x86's this arc; the
/// fixture SKIPs when the volume carries no `/apps/BIG.ELF`).
fn ensure_ring3win() {
    #[cfg(target_arch = "x86_64")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("ring3win", crate::arch::syscall::ring3win_selftest); register("window", crate::window2::selftest); } // WINDOW2 (B361): `tests window`
    }
}

/// QUIETBOOT M3 (R80) — the one-statement shape for a boot-time WITNESS: `if crate::tests::defer("name", f) { return; }`
/// as the first statement of the fixture `f` itself. Returns `false` (run the body now, as before) under
/// `tests-at-boot` — so a QEMU lane sees the old line in the old order — and while `tests` is executing a
/// fixture (so the registered `f` re-entering here runs its body). Otherwise registers `f` under `name`
/// ONCE and returns `true`: the boot prints nothing and `tests <name>` fires it.
pub fn defer(name: &'static str, f: fn()) -> bool {
    if cfg!(feature = "tests-at-boot") || RUNNING.load(Ordering::Acquire) {
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
        if !DONE.swap(true, Ordering::AcqRel) { register("brightfloor", crate::video::backlight::selftest); register("brightstep", crate::video::backlight::brightstep); }
    }
}

/// USBNET6 M4: register `tests usbnet` (the dongle receives a frame within 5 s) exactly once, on any build
/// carrying the USB Ethernet driver; no dongle or no link prints SKIP, never a pin.
fn ensure_usbnet() {
    #[cfg(feature = "usbnet")]
    {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, Ordering::AcqRel) { register("usbnet", crate::drivers::xhci::usbnet::selftest); register("usbnet7", crate::drivers::xhci::usbnet::usbnet7_selftest); } // NETFRAME (B368): `tests usbnet7`
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
static SKIPPED: spin::Mutex<[Option<&'static str>; CAP]> = spin::Mutex::new([None; CAP]);

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
    if cfg!(feature = "tests-at-boot") || RUNNING.load(Ordering::Acquire) {
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
static RESULTS: spin::Mutex<alloc::vec::Vec<(&'static str, alloc::string::String)>> = spin::Mutex::new(alloc::vec::Vec::new());

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
static ARG: spin::Mutex<Option<alloc::string::String>> = spin::Mutex::new(None);
pub fn arg() -> Option<alloc::string::String> { ARG.lock().clone() }

/// VOLUMES (rmbp-ledger B366) — TAIL-APPENDED: register `tests volumes` (the Volumes layout: boot = EFI only, the
/// UnaFS root shown, no home on the FAT) and `tests testf` (system/test-f staged vs claimed) exactly once, every build;
/// the fixtures SKIP or FAIL with their reason from the mounted tree.
fn ensure_volumes() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) { register("volumes", crate::fs::volumes::selftest); register("testf", crate::fs::volumes::testf_selftest); }
}
